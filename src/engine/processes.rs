//! Terra owns signed helper selection, IPC descriptor delegation and reaping.
//! WebKit supplies a process kind and a borrowed socket, never a pathname.
use super::abi::ProcessRequest;
use appcore::process::{Child, Command, Executable};
use std::{
    collections::BTreeMap,
    ffi::c_void,
    io,
    os::fd::{AsFd, FromRawFd, OwnedFd},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering},
    },
};
const MAX_LIVE: usize = 64;
const MAX_IDENTIFIERS: usize = 1024;
const IPC_FD: i32 = 3;
trait Process {
    fn id(&self) -> u32;
    fn exited(&mut self) -> io::Result<bool>;
    fn terminate(&mut self) -> io::Result<()>;
    fn wait(&mut self) -> io::Result<()>;
}
impl Process for Child {
    fn id(&self) -> u32 {
        Child::id(self)
    }
    fn exited(&mut self) -> io::Result<bool> {
        self.try_wait().map(|s| s.is_some())
    }
    fn terminate(&mut self) -> io::Result<()> {
        Child::terminate(self)
    }
    fn wait(&mut self) -> io::Result<()> {
        Child::wait(self).map(|_| ())
    }
}
struct Entry<P> {
    child: P,
    exited: bool,
}
struct Registry<P> {
    entries: BTreeMap<i32, Entry<P>>,
    quarantine: Vec<Entry<P>>,
}
impl<P: Process> Registry<P> {
    fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            quarantine: Vec::new(),
        }
    }
    fn reap(&mut self) -> io::Result<()> {
        for entry in self
            .entries
            .values_mut()
            .chain(self.quarantine.iter_mut())
            .filter(|e| !e.exited)
        {
            entry.exited = entry.child.exited()?;
        }
        Ok(())
    }
    fn available(&self) -> bool {
        self.quarantine.is_empty()
            && self.entries.len() < MAX_IDENTIFIERS
            && self.entries.values().filter(|e| !e.exited).count() < MAX_LIVE
    }
    fn insert(&mut self, mut child: P) -> io::Result<i32> {
        let pid = i32::try_from(child.id()).ok().filter(|p| *p > 0);
        if !self.available() || pid.is_none_or(|p| self.entries.contains_key(&p)) {
            // Only this newly owned process is stopped. Never look up a PID.
            if child.terminate().and_then(|()| child.wait()).is_err() {
                self.quarantine.push(Entry {
                    child,
                    exited: false,
                });
            }
            return Err(io::Error::other(
                "helper limit or reused process identifier",
            ));
        }
        let pid = pid.unwrap();
        self.entries.insert(
            pid,
            Entry {
                child,
                exited: false,
            },
        );
        Ok(pid)
    }
    fn terminate(&mut self, pid: i32) -> io::Result<()> {
        let e = self
            .entries
            .get_mut(&pid)
            .ok_or_else(|| io::Error::other("unknown helper reference"))?;
        if !e.exited {
            e.child.terminate()?;
        }
        Ok(())
    }
    fn shutdown(&mut self) -> io::Result<()> {
        let mut error = None;
        for e in self
            .entries
            .values_mut()
            .chain(self.quarantine.iter_mut())
            .filter(|e| !e.exited)
        {
            if let Err(e) = e.child.terminate().and_then(|()| e.child.wait()) {
                error.get_or_insert(e);
            } else {
                e.exited = true;
            }
        }
        error.map_or(Ok(()), Err)
    }
}
#[derive(Default)]
pub(super) struct Diagnostics {
    #[cfg(feature = "engine-probe")]
    pub phase: AtomicU32,
    #[cfg(feature = "engine-probe")]
    pub process_exit_reported: AtomicBool,
    pub web: AtomicU32,
    pub network: AtomicU32,
    pub live: AtomicU32,
    pub stopped: AtomicBool,
    pub kill: AtomicI32,
    pub web_pid: AtomicI32,
}
impl Diagnostics {
    pub(super) fn observe(&self, phase: u32) {
        #[cfg(feature = "engine-probe")]
        self.phase.store(phase, Ordering::Release);
        #[cfg(not(feature = "engine-probe"))]
        let _ = phase;
    }
}
pub(super) struct Helpers {
    registry: Registry<Child>,
    error: Option<String>,
    stopping: bool,
    pub diagnostics: Arc<Diagnostics>,
}
impl Helpers {
    pub(super) fn new() -> Self {
        Self {
            registry: Registry::new(),
            error: None,
            stopping: false,
            diagnostics: Arc::new(Diagnostics::default()),
        }
    }
    pub(super) fn reap(&mut self) {
        #[cfg(feature = "engine-probe")]
        {
            let pid = self.diagnostics.kill.swap(0, Ordering::AcqRel);
            if pid > 0 {
                if let Err(e) = self.registry.terminate(pid) {
                    self.error = Some(format!("Owned helper termination failed: {e}"));
                }
            }
        }
        if let Err(e) = self.registry.reap() {
            self.error = Some(format!("Could not reap a WebKit helper: {e}"));
        }
        self.update_live();
    }
    fn update_live(&self) {
        self.diagnostics.live.store(
            self.registry.entries.values().filter(|e| !e.exited).count() as u32,
            Ordering::Release,
        );
    }
    pub(super) fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }
    pub(super) fn shutdown(&mut self) -> bool {
        self.stopping = true;
        if let Err(e) = self.registry.shutdown() {
            self.error = Some(format!("Could not stop a WebKit helper: {e}"));
            return false;
        }
        self.update_live();
        self.diagnostics.stopped.store(true, Ordering::Release);
        true
    }
    fn launch(&mut self, request: &ProcessRequest) -> io::Result<i32> {
        if self.stopping {
            return Err(io::Error::other("WebKit session is stopping"));
        }
        if request.size < size_of::<ProcessRequest>() as u32
            || request.reserved != 0
            || request.identifier == 0
            || request.socket < 0
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid WebKit process request",
            ));
        }
        let alias = match request.kind {
            1 => "webkit-web-process",
            2 => "webkit-network-process",
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "unsupported WebKit process kind",
                ));
            }
        };
        self.registry.reap()?;
        if !self.registry.available() {
            return Err(io::Error::other("WebKit helper limit reached"));
        }
        // Validate and pin the borrowed descriptor atomically. Invalid labels
        // return EBADF; they are never constructed as an invalid BorrowedFd.
        let fd = unsafe { libc::fcntl(request.socket, libc::F_DUPFD_CLOEXEC, IPC_FD + 1) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let socket = unsafe { OwnedFd::from_raw_fd(fd) };
        self.diagnostics.observe(10 + request.kind);
        let executable = Executable::dependency(alias)?;
        let mut command = Command::new(executable);
        command
            .arg(request.identifier.to_string())
            .arg(IPC_FD.to_string())
            .env_clear()
            .env("LANG", "C.UTF-8")
            .env("FONTCONFIG_FILE", "/system/libraries/webkit/fonts.conf")
            .env("FONTCONFIG_PATH", "/system/libraries/webkit")
            .env("GIO_USE_TLS", "mochios")
            .env("WEBKIT_SKIA_ENABLE_CPU_RENDERING", "1")
            .env(
                "LD_LIBRARY_PATH",
                "/system/libraries:/system/libraries/webkit",
            );
        command.inherit_fd(socket.as_fd(), IPC_FD)?;
        self.diagnostics.observe(20 + request.kind);
        let pid = self.registry.insert(command.spawn()?)?;
        self.diagnostics.observe(30 + request.kind);
        if request.kind == 1 {
            self.diagnostics.web.fetch_add(1, Ordering::AcqRel);
            self.diagnostics.web_pid.store(pid, Ordering::Release);
        } else {
            self.diagnostics.network.fetch_add(1, Ordering::AcqRel);
        }
        self.update_live();
        Ok(pid)
    }
}
/// C callbacks run on the engine worker. Returned PIDs name only owned Childs;
/// tombstones prevent a stale identifier from targeting a reused native PID.
pub(super) unsafe extern "C" fn launch(
    user: *mut c_void,
    request: *const ProcessRequest,
    output: *mut i32,
) -> u32 {
    if output.is_null() {
        return 1;
    }
    unsafe {
        *output = 0;
    }
    if user.is_null() || request.is_null() {
        return 1;
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        unsafe { &mut *user.cast::<Helpers>() }.launch(unsafe { &*request })
    }));
    match result {
        Ok(Ok(pid)) => {
            unsafe {
                *output = pid;
            }
            0
        }
        other => {
            unsafe { &mut *user.cast::<Helpers>() }.error =
                Some(format!("Could not start a signed WebKit helper: {other:?}"));
            6
        }
    }
}
pub(super) unsafe extern "C" fn terminate(user: *mut c_void, pid: i32) -> u32 {
    if user.is_null() || pid <= 0 {
        return 1;
    }
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        unsafe { &mut *user.cast::<Helpers>() }
            .registry
            .terminate(pid)
    })) {
        Ok(Ok(())) => 0,
        _ => 6,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc};
    struct Fake {
        pid: u32,
        done: bool,
        operations: Rc<RefCell<Vec<&'static str>>>,
    }
    impl Process for Fake {
        fn id(&self) -> u32 {
            self.pid
        }
        fn exited(&mut self) -> io::Result<bool> {
            self.operations.borrow_mut().push("poll");
            Ok(self.done)
        }
        fn terminate(&mut self) -> io::Result<()> {
            self.operations.borrow_mut().push("terminate");
            Ok(())
        }
        fn wait(&mut self) -> io::Result<()> {
            self.operations.borrow_mut().push("wait");
            self.done = true;
            Ok(())
        }
    }
    #[test]
    fn failed_cleanup_keeps_the_owned_child_and_stops_more_launches() {
        struct Failing(Rc<RefCell<usize>>);
        impl Process for Failing {
            fn id(&self) -> u32 {
                0
            }
            fn exited(&mut self) -> io::Result<bool> {
                Ok(false)
            }
            fn terminate(&mut self) -> io::Result<()> {
                Err(io::Error::other("fixture"))
            }
            fn wait(&mut self) -> io::Result<()> {
                unreachable!()
            }
        }
        impl Drop for Failing {
            fn drop(&mut self) {
                *self.0.borrow_mut() += 1;
            }
        }
        let drops = Rc::new(RefCell::new(0));
        let mut registry = Registry::new();
        assert!(registry.insert(Failing(drops.clone())).is_err());
        assert_eq!(*drops.borrow(), 0);
        assert!(!registry.available());
        assert!(registry.shutdown().is_err());
        assert_eq!(*drops.borrow(), 0);
    }
    #[test]
    fn reaping_retains_identity_and_never_redirects_termination_after_pid_reuse() {
        let ops = Rc::new(RefCell::new(Vec::new()));
        let mut r = Registry::new();
        r.insert(Fake {
            pid: 42,
            done: true,
            operations: ops.clone(),
        })
        .unwrap();
        r.reap().unwrap();
        r.terminate(42).unwrap();
        assert_eq!(&*ops.borrow(), &["poll"]);
        assert!(
            r.insert(Fake {
                pid: 42,
                done: false,
                operations: ops.clone()
            })
            .is_err()
        );
        assert_eq!(&*ops.borrow(), &["poll", "terminate", "wait"]);
        assert!(r.terminate(43).is_err());
        assert_eq!(r.entries.len(), 1);
    }
    #[test]
    fn shutdown_terminates_and_reaps_only_the_owned_children() {
        let ops = Rc::new(RefCell::new(Vec::new()));
        let mut r = Registry::new();
        r.insert(Fake {
            pid: 12,
            done: false,
            operations: ops.clone(),
        })
        .unwrap();
        r.shutdown().unwrap();
        r.shutdown().unwrap();
        assert_eq!(&*ops.borrow(), &["terminate", "wait"]);
    }
    #[test]
    fn invalid_kind_and_invalid_socket_fail_without_path_or_host_fallback() {
        let mut h = Helpers::new();
        let mut q = ProcessRequest {
            size: size_of::<ProcessRequest>() as u32,
            kind: 3,
            identifier: 1,
            socket: 999999,
            reserved: 0,
        };
        assert_eq!(h.launch(&q).unwrap_err().kind(), io::ErrorKind::Unsupported);
        q.kind = 1;
        assert!(h.launch(&q).is_err());
        assert!(h.registry.entries.is_empty());
        let mut pid = 17;
        assert_eq!(unsafe { launch(std::ptr::null_mut(), &q, &mut pid) }, 1);
        assert_eq!(pid, 0);
    }
}

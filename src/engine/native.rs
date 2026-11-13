//! Terra's worker-backed client for the independently installed WebKit library.
//! Painting only reads retained frames. All C API calls stay on one worker.
use super::abi::*;
use super::{Command, Engine, Event, Features, NavigationId, Security, TabId};
use appcore::prelude::*;
use appcore::viewkit::{
    event::{EventContext, EventResult, ViewEvent},
    platform::PointerButton,
    view::{Constraints, MeasureContext, PaintContext},
};
use std::{
    cell::Cell,
    collections::{BTreeMap, VecDeque},
    ffi::c_void,
    ptr,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError},
    },
    thread,
    time::{Duration, Instant},
};
const TICK: Duration = Duration::from_millis(16);
const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;
#[derive(Default)]
struct PageState {
    size: AtomicU64,
    navigation: AtomicU64,
    image: Mutex<Option<ImageData>>,
}
#[derive(Clone)]
struct InputData {
    kind: u32,
    flags: u32,
    modifiers: u32,
    x: f64,
    y: f64,
    dx: f64,
    dy: f64,
    key: u32,
    button: u32,
    text: String,
}
impl Default for InputData {
    fn default() -> Self {
        Self {
            kind: 0,
            flags: 0,
            modifiers: 0,
            x: 0.,
            y: 0.,
            dx: 0.,
            dy: 0.,
            key: 0,
            button: 0,
            text: String::new(),
        }
    }
}
enum Request {
    Browser(Command),
    Create(TabId, Arc<PageState>),
    Input(TabId, InputData),
}
struct Page {
    id: TabId,
    state: Arc<PageState>,
    send: SyncSender<Request>,
    focused: Cell<bool>,
    pressed: Cell<bool>,
    modifiers: Cell<u32>,
}
pub(super) struct Native {
    send: SyncSender<Request>,
    events: Receiver<Event>,
    pages: BTreeMap<TabId, Rc<Page>>,
    alive: Arc<AtomicBool>,
    zoom: bool,
    stop: Arc<AtomicBool>,
    diagnostics: Arc<super::processes::Diagnostics>,
}
impl Native {
    /// Starts a session using a validated library-owned table that outlives the process.
    /// No engine object or borrowed callback pointer crosses the worker boundary.
    pub(super) fn start(api: &'static Api) -> Result<Self, String> {
        if !api.compatible() || api.features & 8 == 0 {
            return Err("The installed WebKit library has an incompatible ABI.".into());
        }
        let (send, requests) = mpsc::sync_channel(256);
        let (output, events) = mpsc::sync_channel(256);
        let alive = Arc::new(AtomicBool::new(true));
        let running = alive.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let diagnostics = Arc::new(super::processes::Diagnostics::default());
        let observations = diagnostics.clone();
        // Only the immutable, startup-linked function table crosses threads.
        let address = api as *const Api as usize;
        thread::Builder::new()
            .name("Terra WebKit".into())
            .spawn(move || {
                // SAFETY: table is validated and remains startup-linked for process lifetime.
                unsafe {
                    worker(
                        &*(address as *const Api),
                        requests,
                        output,
                        stopped,
                        observations,
                    );
                }
                running.store(false, Ordering::Release);
            })
            .map_err(|e| format!("Could not start WebKit: {e}"))?;
        Ok(Self {
            send,
            events,
            pages: BTreeMap::new(),
            alive,
            zoom: api.features & 2 != 0,
            stop,
            diagnostics,
        })
    }
    fn enqueue(&self, request: Request) -> Result<(), String> {
        self.send
            .try_send(request)
            .map_err(|_| "WebKit is busy or has stopped. Please try again.".into())
    }
}
impl Engine for Native {
    fn available(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }
    fn features(&self) -> Features {
        Features {
            zoom: self.zoom,
            ..Features::default()
        }
    }
    fn send(&mut self, command: Command) -> Result<(), String> {
        match command {
            Command::Create(id) => {
                if id == 0 || self.pages.contains_key(&id) {
                    return Err("Invalid page identity.".into());
                }
                let state = Arc::new(PageState::default());
                self.enqueue(Request::Create(id, state.clone()))?;
                self.pages.insert(
                    id,
                    Rc::new(Page {
                        id,
                        state,
                        send: self.send.clone(),
                        focused: Cell::new(false),
                        pressed: Cell::new(false),
                        modifiers: Cell::new(0),
                    }),
                );
                Ok(())
            }
            Command::Close(id) => {
                self.enqueue(Request::Browser(Command::Close(id)))?;
                self.pages.remove(&id);
                Ok(())
            }
            other => {
                if !matches!(other, Command::Activate(_)) && operation(&other).is_none() {
                    return Err("This WebKit feature is not supported yet.".into());
                }
                let generation = operation(&other).map(|(id, op)| (id, op.navigation));
                self.enqueue(Request::Browser(other))?;
                if let Some((id, nav)) = generation {
                    if let Some(p) = self.pages.get(&id) {
                        p.state.navigation.store(nav, Ordering::Release);
                    }
                }
                Ok(())
            }
        }
    }
    fn poll(&mut self) -> Option<Event> {
        if let Ok(event) = self.events.try_recv() {
            return Some(event);
        }
        if !self.available() {
            for (&id, p) in &self.pages {
                let nav = p.state.navigation.swap(0, Ordering::AcqRel);
                if nav != 0 {
                    return Some(failure(id, nav, "The WebKit engine stopped.".into()));
                }
            }
        }
        None
    }
    fn page(&self, tab: TabId) -> Option<Rc<dyn View>> {
        self.pages.get(&tab).map(|v| v.clone() as Rc<dyn View>)
    }
    fn next_poll_at(&self) -> Option<Instant> {
        (!self.pages.is_empty() && self.available()).then(|| Instant::now() + TICK)
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}
impl View for Page {
    fn measure(&self, c: Constraints, _: &mut MeasureContext<'_>) -> Size {
        c.constrain(c.maximum)
    }
    fn paint(&self, b: Rect, c: &mut PaintContext<'_>) {
        let w = b.size.width.ceil().clamp(1., 8192.) as u32;
        let h = b.size.height.ceil().clamp(1., 8192.) as u32;
        self.state
            .size
            .store((u64::from(w) << 32) | u64::from(h), Ordering::Release);
        if let Ok(image) = self.state.image.try_lock() {
            if let Some(image) = image.as_ref() {
                Image::new(image.clone())
                    .content_mode(ImageContentMode::Stretch)
                    .paint(b, c);
            }
        }
        // Page is a focusable native viewport, not a set of host HTML controls.
        let mut node = AccessibilityNode::new(AccessibilityRole::Group, b);
        node.focusable = true;
        node.focused = self.focused.get();
        c.record_accessibility(node);
    }
    fn handle_event(&self, b: Rect, e: &ViewEvent, c: &mut EventContext<'_>) -> EventResult {
        let mut input = InputData {
            modifiers: self.modifiers.get(),
            ..InputData::default()
        };
        if let Some(p) = e.position() {
            input.x = f64::from(p.x - b.origin.x);
            input.y = f64::from(p.y - b.origin.y);
        }
        match e {
            ViewEvent::ModifiersChanged { modifiers } => {
                self.modifiers.set(u32::from(modifiers.bits()));
                return EventResult::Ignored;
            }
            ViewEvent::PointerFocusRequested { position } => {
                let focus = b.contains(*position);
                self.focused.set(focus);
                input.kind = 10;
                input.flags = u32::from(focus);
            }
            ViewEvent::KeyboardFocusRequested { bounds } => {
                let focus = bounds.is_some_and(|target| target == b);
                self.focused.set(focus);
                input.kind = 10;
                input.flags = u32::from(focus);
            }
            ViewEvent::FocusChanged { focused: false } => {
                self.focused.set(false);
                self.pressed.set(false);
                input.kind = 10;
            }
            ViewEvent::PointerPressed { position, button }
            | ViewEvent::PointerReleased { position, button }
                if b.contains(*position)
                    || (self.pressed.get() && matches!(e, ViewEvent::PointerReleased { .. })) =>
            {
                self.pressed
                    .set(matches!(e, ViewEvent::PointerPressed { .. }));
                if matches!(e, ViewEvent::PointerPressed { .. }) {
                    c.request_keyboard_focus(b);
                }
                input.kind = if matches!(e, ViewEvent::PointerPressed { .. }) {
                    2
                } else {
                    3
                };
                input.button = match button {
                    PointerButton::Primary => 1,
                    PointerButton::Middle => 2,
                    PointerButton::Secondary => 3,
                    _ => return EventResult::Ignored,
                };
            }
            ViewEvent::PointerMoved { position } if b.contains(*position) || self.pressed.get() => {
                input.kind = 1
            }
            ViewEvent::Scroll {
                position,
                delta_x,
                delta_y,
            } if b.contains(*position) => {
                input.kind = 4;
                input.dx = f64::from(*delta_x);
                input.dy = f64::from(*delta_y);
            }
            ViewEvent::TextInput { text } if self.focused.get() => {
                input.kind = 7;
                input.text = text.clone();
            }
            ViewEvent::KeyPressed { key, modifiers }
            | ViewEvent::KeyReleased { key, modifiers }
                if self.focused.get() =>
            {
                input.kind = if matches!(e, ViewEvent::KeyPressed { .. }) {
                    5
                } else {
                    6
                };
                input.modifiers = u32::from(modifiers.bits());
                input.key = match key {
                    Key::Enter => 40,
                    Key::Escape => 41,
                    Key::Backspace => 42,
                    Key::Tab => 43,
                    Key::Delete => 76,
                    Key::Home => 74,
                    Key::End => 77,
                    Key::ArrowRight => 79,
                    Key::ArrowLeft => 80,
                    Key::ArrowDown => 81,
                    Key::ArrowUp => 82,
                    Key::PageUp => 75,
                    Key::PageDown => 78,
                    _ => return EventResult::Ignored,
                };
            }
            _ => return EventResult::Ignored,
        }
        match self.send.try_send(Request::Input(self.id, input)) {
            Ok(()) => EventResult::Consumed,
            Err(_) => EventResult::Ignored,
        }
    }
}
struct Callback {
    queue: VecDeque<Event>,
    local: BTreeMap<TabId, NavigationId>,
    overflow: bool,
}
unsafe extern "C" fn notification(user: *mut c_void, event: *const Notification) {
    if user.is_null() || event.is_null() {
        return;
    }
    // SAFETY: callback/user are live only during worker dispatch; copy borrowed UTF-8.
    if unsafe { *event.cast::<u32>() } < size_of::<Notification>() as u32 {
        return;
    }
    let e = unsafe { &*event };
    let cb = unsafe { &mut *(user as *mut Callback) };
    if cb.overflow || cb.queue.len() >= 512 {
        cb.overflow = true;
        return;
    }
    if e.text.len > 32 * 1024 || (e.text.len != 0 && e.text.data.is_null()) {
        return;
    }
    let text = if e.text.len == 0 {
        String::new()
    } else {
        String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(e.text.data, e.text.len) })
            .into_owned()
    };
    let text =
        if e.kind == 2 && (e.history & 4 != 0 || cb.local.get(&e.view) == Some(&e.navigation)) {
            "terra:///engine-check".into()
        } else {
            text
        };
    if e.kind == 2 {
        if e.history & 4 != 0 {
            cb.local.insert(e.view, e.navigation);
        } else if cb.local.get(&e.view) != Some(&e.navigation) {
            cb.local.remove(&e.view);
        }
    }
    let event = match e.kind {
        2 => Event::Committed {
            tab: e.view,
            navigation: e.navigation,
            url: text,
            title: String::new(),
            security: Security::Unknown,
            can_back: e.history & 1 != 0,
            can_forward: e.history & 2 != 0,
        },
        3 => Event::Finished {
            tab: e.view,
            navigation: e.navigation,
        },
        4 | 7 => Event::Failed {
            tab: e.view,
            navigation: e.navigation,
            message: text,
        },
        6 => Event::TitleChanged {
            tab: e.view,
            document: e.navigation,
            title: text,
        },
        _ => return,
    };
    cb.queue.push_back(event);
}
struct WorkerPage {
    raw: *mut c_void,
    state: Arc<PageState>,
    navigation: NavigationId,
    size: u64,
    serial: u64,
}
fn failure(tab: TabId, navigation: NavigationId, message: String) -> Event {
    Event::Failed {
        tab,
        navigation,
        message,
    }
}
fn operation(command: &Command) -> Option<(TabId, Operation)> {
    let (tab, id, kind, text, value) = match command {
        Command::Navigate {
            tab,
            navigation,
            url,
        } if url == "terra:///engine-check" => (
            *tab,
            *navigation,
            7,
            include_str!("../../resources/engine-check.html"),
            0.,
        ),
        Command::Navigate {
            tab,
            navigation,
            url,
        } => (*tab, *navigation, 1, url.as_str(), 0.),
        Command::Back { tab, navigation } => (*tab, *navigation, 2, "", 0.),
        Command::Forward { tab, navigation } => (*tab, *navigation, 3, "", 0.),
        Command::Reload { tab, navigation } => (*tab, *navigation, 4, "", 0.),
        Command::Stop { tab, navigation } => (*tab, *navigation, 5, "", 0.),
        Command::SetZoom {
            tab,
            document,
            percent,
        } => (*tab, *document, 6, "", f64::from(*percent) / 100.),
        _ => return None,
    };
    Some((
        tab,
        Operation {
            size: size_of::<Operation>() as u32,
            kind,
            navigation: id,
            text: Bytes::new(text),
            value,
        },
    ))
}
/// Copies a checked leased frame; row padding is excluded from ViewKit pixels.
unsafe fn copy_frame(frame: &Frame) -> Result<ImageData, String> {
    let row = (frame.width as usize)
        .checked_mul(4)
        .ok_or("Invalid frame width")?;
    let total = row
        .checked_mul(frame.height as usize)
        .ok_or("Invalid frame height")?;
    let storage = frame
        .stride
        .checked_mul(frame.height as usize)
        .ok_or("Invalid frame stride")?;
    if frame.size < size_of::<Frame>() as u32
        || frame.format != 1
        || frame.width == 0
        || frame.height == 0
        || frame.reference.is_null()
        || frame.pixels.is_null()
        || frame.stride < row
        || frame.len < storage
        || total > MAX_FRAME_BYTES
        || storage > MAX_FRAME_BYTES
        || !frame.scale.is_finite()
        || frame.scale <= 0.
    {
        return Err("WebKit returned an invalid frame.".into());
    }
    let bytes = unsafe { std::slice::from_raw_parts(frame.pixels, storage) };
    let mut pixels = Vec::with_capacity(total);
    for line in bytes.chunks_exact(frame.stride) {
        pixels.extend_from_slice(&line[..row]);
    }
    ImageData::from_premultiplied_rgba8(frame.width, frame.height, pixels)
        .map_err(|e| e.to_string())
}
unsafe fn worker(
    api: &Api,
    requests: Receiver<Request>,
    output: SyncSender<Event>,
    stop: Arc<AtomicBool>,
    diagnostics: Arc<super::processes::Diagnostics>,
) {
    let mut callback = Box::new(Callback {
        queue: VecDeque::new(),
        local: BTreeMap::new(),
        overflow: false,
    });
    let mut helpers = Box::new(super::processes::Helpers::new());
    helpers.diagnostics = diagnostics;
    let options = Options {
        size: size_of::<Options>() as u32,
        reserved: 0,
        user: (&mut *callback as *mut Callback).cast(),
        wake: None,
        event: Some(notification),
        process_user: (&mut *helpers as *mut super::processes::Helpers).cast(),
        launch: Some(super::processes::launch),
        terminate: Some(super::processes::terminate),
    };
    let mut context = ptr::null_mut();
    // SAFETY: the complete table was validated; every operation below stays on this thread.
    helpers.diagnostics.observe(1);
    let status = unsafe { api.create.unwrap()(&options, &mut context) };
    helpers.diagnostics.observe(2);
    let mut pages = BTreeMap::<TabId, WorkerPage>::new();
    let mut shutdown = false;
    while !shutdown && !stop.load(Ordering::Acquire) {
        for _ in 0..64 {
            match requests.try_recv() {
                Err(TryRecvError::Disconnected) => {
                    shutdown = true;
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Ok(Request::Create(id, state)) => {
                    let mut raw = ptr::null_mut();
                    let result = if status == 0 && !context.is_null() {
                        helpers.diagnostics.observe(3);
                        let result = unsafe { api.view_create.unwrap()(context, id, &mut raw) };
                        helpers.diagnostics.observe(4);
                        result
                    } else {
                        status.max(3)
                    };
                    if result == 0 && !raw.is_null() {
                        pages.insert(
                            id,
                            WorkerPage {
                                raw,
                                state,
                                navigation: 0,
                                size: 0,
                                serial: 0,
                            },
                        );
                    }
                }
                Ok(Request::Browser(Command::Close(id))) => {
                    if let Some(p) = pages.remove(&id) {
                        unsafe { api.view_destroy.unwrap()(p.raw) };
                    }
                }
                Ok(Request::Browser(Command::Activate(_))) => {}
                Ok(Request::Browser(cmd)) => {
                    if let Some((id, mut op)) = operation(&cmd) {
                        if op.kind == 4 && callback.local.contains_key(&id) {
                            op.kind = 7;
                            op.text = Bytes::new(include_str!("../../resources/engine-check.html"));
                        }
                        if op.kind == 7 {
                            callback.local.insert(id, op.navigation);
                        } else if (1..=4).contains(&op.kind) {
                            callback.local.remove(&id);
                        }
                        let result = if let Some(p) = pages.get_mut(&id) {
                            p.navigation = op.navigation;
                            helpers.diagnostics.observe(5);
                            let result = unsafe { api.command.unwrap()(p.raw, &op) };
                            helpers.diagnostics.observe(6);
                            result
                        } else {
                            3
                        };
                        if result != 0 {
                            callback.queue.push_back(failure(id,op.navigation,format!("WebKit rejected this request (status {result}). Check that the engine package and its helpers are installed.")));
                        }
                    }
                }
                Ok(Request::Input(id, event)) => {
                    if let Some(p) = pages.get(&id) {
                        let input = Input {
                            size: size_of::<Input>() as u32,
                            kind: event.kind,
                            flags: event.flags,
                            modifiers: event.modifiers,
                            x: event.x,
                            y: event.y,
                            dx: event.dx,
                            dy: event.dy,
                            key: event.key,
                            button: event.button,
                            text: Bytes::new(&event.text),
                            start: 0,
                            end: 0,
                        };
                        unsafe { api.input.unwrap()(p.raw, &input) };
                    }
                }
            }
        }
        while let Some(event) = callback.queue.pop_front() {
            match output.try_send(event) {
                Ok(()) => {}
                Err(mpsc::TrySendError::Full(e)) => {
                    callback.queue.push_front(e);
                    break;
                }
                Err(_) => {
                    shutdown = true;
                    break;
                }
            }
        }
        if !context.is_null() && callback.queue.len() < 256 {
            for (&id, p) in pages.iter_mut() {
                let size = p.state.size.load(Ordering::Acquire);
                if size != 0 && size != p.size {
                    let result =
                        unsafe { api.resize.unwrap()(p.raw, (size >> 32) as u32, size as u32, 1.) };
                    if result == 0 {
                        p.size = size;
                    } else {
                        callback.queue.push_back(failure(
                            id,
                            p.navigation,
                            format!("WebKit resize failed ({result})."),
                        ));
                    }
                }
            }
            helpers.diagnostics.observe(7);
            let result = unsafe { api.dispatch.unwrap()(context, 32) };
            helpers.diagnostics.observe(8);
            helpers.reap();
            let helper_error = helpers.take_error();
            if result != 0 || callback.overflow || helper_error.is_some() {
                for (&id, p) in &pages {
                    callback.queue.push_back(failure(
                        id,
                        p.navigation,
                        if let Some(error) = &helper_error {
                            error.clone()
                        } else if callback.overflow {
                            "WebKit produced too many events; the session was stopped.".into()
                        } else {
                            format!("WebKit dispatch failed ({result}).")
                        },
                    ));
                }
                shutdown = true;
            }
            for (&id, p) in pages.iter_mut() {
                let mut frame: Frame = unsafe { std::mem::zeroed() };
                frame.size = size_of::<Frame>() as u32;
                if unsafe { api.acquire.unwrap()(p.raw, &mut frame) } == 0 {
                    if frame.serial != p.serial || p.serial == 0 {
                        if let Ok(image) = unsafe { copy_frame(&frame) } {
                            if let Ok(mut target) = p.state.image.lock() {
                                *target = Some(image);
                                p.serial = frame.serial;
                            }
                        } else {
                            callback.queue.push_back(failure(
                                id,
                                p.navigation,
                                "WebKit returned an invalid frame.".into(),
                            ));
                            shutdown = true;
                        }
                    }
                    unsafe { api.release.unwrap()(context, frame.reference) };
                }
            }
        }
        thread::sleep(TICK);
    }
    for event in callback.queue.drain(..) {
        let _ = output.try_send(event);
    }
    for p in pages.values() {
        unsafe { api.view_destroy.unwrap()(p.raw) };
    }
    let helpers_stopped = helpers.shutdown();
    if !context.is_null() {
        // Drain cancellation callbacks before releasing their user data.
        for _ in 0..64 {
            unsafe { api.dispatch.unwrap()(context, 32) };
            if unsafe { api.destroy.unwrap()(context) } == 0 {
                if !helpers_stopped {
                    Box::leak(helpers);
                }
                return;
            }
            thread::sleep(TICK);
        }
        // Keep callback memory valid if a broken engine refuses to quiesce.
        eprintln!("Terra: WebKit did not finish shutdown; retaining callback storage.");
        Box::leak(callback);
        Box::leak(helpers);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frame_validation_excludes_padding_and_rejects_overflow() {
        let pixels = [1, 2, 3, 255, 9, 9, 9, 9, 4, 5, 6, 255, 9, 9, 9, 9];
        let mut f = Frame {
            size: size_of::<Frame>() as u32,
            format: 1,
            width: 1,
            height: 2,
            stride: 8,
            len: 16,
            pixels: pixels.as_ptr(),
            scale: 1.,
            serial: 1,
            reference: 1usize as *mut c_void,
        };
        let image = unsafe { copy_frame(&f) }.unwrap();
        assert_eq!(image.width(), 1);
        assert_eq!(image.height(), 2);
        assert_eq!(image.premultiplied_rgba8(), &[1, 2, 3, 255, 4, 5, 6, 255]);
        f.stride = usize::MAX;
        assert!(unsafe { copy_frame(&f) }.is_err());
        f.stride = 8;
        f.scale = f64::NAN;
        assert!(unsafe { copy_frame(&f) }.is_err());
    }
    #[test]
    fn navigation_preserves_original_id_and_copies_no_security_claim() {
        let cmd = Command::Navigate {
            tab: 7,
            navigation: 91,
            url: "https://example.test/".into(),
        };
        let (id, op) = operation(&cmd).unwrap();
        assert_eq!((id, op.navigation, op.kind), (7, 91, 1));
        let mut cb = Callback {
            queue: VecDeque::new(),
            local: BTreeMap::new(),
            overflow: false,
        };
        let text = "https://example.test/";
        let event = Notification {
            size: size_of::<Notification>() as u32,
            kind: 2,
            view: 7,
            navigation: 91,
            status: 0,
            history: 1,
            text: Bytes::new(text),
        };
        unsafe { notification((&mut cb as *mut Callback).cast(), &event) };
        assert!(matches!(
            cb.queue.pop_front(),
            Some(Event::Committed {
                tab: 7,
                navigation: 91,
                security: Security::Unknown,
                can_back: true,
                ..
            })
        ));
    }
}

#[cfg(test)]
mod worker_tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    static LIVE: AtomicUsize = AtomicUsize::new(0);
    static LEASES: AtomicUsize = AtomicUsize::new(0);
    static RELEASED: AtomicUsize = AtomicUsize::new(0);
    struct Context {
        options: Options,
        owner: thread::ThreadId,
    }
    struct ViewHandle {
        context: *mut Context,
        id: u64,
        navigation: u64,
    }
    unsafe extern "C" fn create(options: *const Options, out: *mut *mut c_void) -> u32 {
        LIVE.fetch_add(1, Ordering::SeqCst);
        unsafe {
            *out = Box::into_raw(Box::new(Context {
                options: ptr::read(options),
                owner: thread::current().id(),
            }))
            .cast();
        }
        0
    }
    unsafe extern "C" fn destroy(context: *mut c_void) -> u32 {
        let ctx = unsafe { Box::from_raw(context.cast::<Context>()) };
        assert_eq!(ctx.owner, thread::current().id());
        LIVE.fetch_sub(1, Ordering::SeqCst);
        0
    }
    unsafe extern "C" fn dispatch(context: *mut c_void, _: u32) -> u32 {
        assert_eq!(
            unsafe { &*context.cast::<Context>() }.owner,
            thread::current().id()
        );
        0
    }
    unsafe extern "C" fn view_create(context: *mut c_void, id: u64, out: *mut *mut c_void) -> u32 {
        unsafe {
            *out = Box::into_raw(Box::new(ViewHandle {
                context: context.cast(),
                id,
                navigation: 0,
            }))
            .cast();
        }
        0
    }
    unsafe extern "C" fn view_destroy(view: *mut c_void) -> u32 {
        drop(unsafe { Box::from_raw(view.cast::<ViewHandle>()) });
        0
    }
    unsafe extern "C" fn command(view: *mut c_void, op: *const Operation) -> u32 {
        let view = unsafe { &mut *view.cast::<ViewHandle>() };
        let op = unsafe { &*op };
        view.navigation = op.navigation;
        let ctx = unsafe { &*view.context };
        assert_eq!(ctx.owner, thread::current().id());
        let uri = "https://example.test/";
        for kind in [2, 3] {
            let e = Notification {
                size: size_of::<Notification>() as u32,
                kind,
                view: view.id,
                navigation: op.navigation,
                status: 0,
                history: 0,
                text: Bytes::new(uri),
            };
            unsafe { ctx.options.event.unwrap()(ctx.options.user, &e) };
        }
        0
    }
    unsafe extern "C" fn resize(_: *mut c_void, _: u32, _: u32, _: f64) -> u32 {
        0
    }
    unsafe extern "C" fn input(_: *mut c_void, _: *const Input) -> u32 {
        0
    }
    unsafe extern "C" fn acquire(view: *mut c_void, out: *mut Frame) -> u32 {
        if unsafe { &*view.cast::<ViewHandle>() }.navigation == 0 {
            return 3;
        }
        static PIXELS: [u8; 4] = [2, 3, 4, 255];
        LEASES.fetch_add(1, Ordering::SeqCst);
        unsafe {
            *out = Frame {
                size: size_of::<Frame>() as u32,
                format: 1,
                width: 1,
                height: 1,
                stride: 4,
                len: 4,
                pixels: PIXELS.as_ptr(),
                scale: 1.,
                serial: 1,
                reference: 1usize as *mut c_void,
            };
        }
        0
    }
    unsafe extern "C" fn release(_: *mut c_void, _: *mut c_void) -> u32 {
        LEASES.fetch_sub(1, Ordering::SeqCst);
        RELEASED.fetch_add(1, Ordering::SeqCst);
        0
    }
    static API: Api = Api {
        size: size_of::<Api>() as u32,
        major: 1,
        features: 9,
        create: Some(create),
        destroy: Some(destroy),
        dispatch: Some(dispatch),
        view_create: Some(view_create),
        view_destroy: Some(view_destroy),
        command: Some(command),
        resize: Some(resize),
        input: Some(input),
        acquire: Some(acquire),
        release: Some(release),
    };
    #[test]
    fn worker_preserves_ids_copies_frames_releases_leases_and_quiesces() {
        let mut engine = Native::start(&API).unwrap();
        engine.send(Command::Create(42)).unwrap();
        engine
            .send(Command::Navigate {
                tab: 42,
                navigation: 8,
                url: "https://example.test/".into(),
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut committed = false;
        let mut finished = false;
        while Instant::now() < deadline {
            while let Some(e) = engine.poll() {
                match e {
                    Event::Committed {
                        tab: 42,
                        navigation: 8,
                        ..
                    } => committed = true,
                    Event::Finished {
                        tab: 42,
                        navigation: 8,
                    } => finished = true,
                    _ => {}
                }
            }
            if committed && finished && engine.pages[&42].state.image.lock().unwrap().is_some() {
                break;
            }
            thread::sleep(TICK);
        }
        assert!(committed && finished);
        assert!(engine.pages[&42].state.image.lock().unwrap().is_some());
        assert!(RELEASED.load(Ordering::SeqCst) > 0);
        // Even a retained page and a saturated input queue must not retain the worker.
        let retained_page = engine.page(42).unwrap();
        for _ in 0..512 {
            let _ = engine
                .send
                .try_send(Request::Input(42, InputData::default()));
        }
        drop(engine);
        while LIVE.load(Ordering::SeqCst) != 0 && Instant::now() < deadline {
            thread::sleep(TICK);
        }
        assert_eq!(LIVE.load(Ordering::SeqCst), 0);
        assert_eq!(LEASES.load(Ordering::SeqCst), 0);
        drop(retained_page);
    }
    #[test]
    fn c_layouts_match_the_major_one_x86_64_contract() {
        assert_eq!(size_of::<Api>(), 96);
        assert_eq!(size_of::<Options>(), 56);
        assert_eq!(size_of::<Notification>(), 48);
        assert_eq!(size_of::<Operation>(), 40);
        assert_eq!(size_of::<Input>(), 80);
        assert_eq!(size_of::<Frame>(), 64);
        let mut broken = Api {
            create: None,
            ..API
        };
        assert!(!broken.compatible());
        broken.create = Some(create);
        broken.major = 2;
        assert!(!broken.compatible());
    }
}

#[cfg(feature = "engine-probe")]
pub(super) fn run_probe(api: &'static Api) -> Result<(), String> {
    use std::io::Write;
    let mut log = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open("/tmp/terra-engine-results.log")
        .map_err(|e| e.to_string())?;
    writeln!(log, "TERRA-ENGINE BEGIN").map_err(|e| e.to_string())?;
    use std::os::fd::AsRawFd;
    let stderr = std::fs::OpenOptions::new()
        .write(true).create_new(true).open("/tmp/terra-engine-stderr.log")
        .map_err(|e| e.to_string())?;
    // Only the disposable native fixture redirects stdio; normal Terra keeps
    // the launching session's descriptors. Signed children inherit this log.
    if unsafe { libc::dup2(stderr.as_raw_fd(), 2) } < 0
        || unsafe { libc::dup2(stderr.as_raw_fd(), 1) } < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let mut engine = match Native::start(api) {
        Ok(engine) => engine,
        Err(error) => {
            writeln!(log, "TERRA-ENGINE FAIL {error}").map_err(|e| e.to_string())?;
            return Err(error);
        }
    };
    let diagnostics = engine.diagnostics.clone();
    let outcome = (|| -> Result<(), String> {
        engine.send(Command::Create(1))?;
        engine.pages[&1]
            .state
            .size
            .store((800u64 << 32) | 600, Ordering::Release);
        engine.send(Command::Navigate {
            tab: 1,
            navigation: 1,
            url: "terra:///engine-check".into(),
        })?;
        let deadline = Instant::now() + Duration::from_secs(1200);
        let mut finished = false;
        let mut next_status = Instant::now();
        while Instant::now() < deadline {
            if Instant::now() >= next_status {
                writeln!(log, "STATE phase={} web={} network={} live={}", diagnostics.phase.load(Ordering::Acquire), diagnostics.web.load(Ordering::Acquire), diagnostics.network.load(Ordering::Acquire), diagnostics.live.load(Ordering::Acquire)).map_err(|e| e.to_string())?;
                next_status = Instant::now() + Duration::from_secs(10);
            }
            while let Some(event) = engine.poll() {
                writeln!(log, "{event:?}").map_err(|e| e.to_string())?;
                match event {
                    Event::Finished { navigation: 1, .. } => finished = true,
                    Event::Failed { message, .. } => return Err(message),
                    _ => {}
                }
            }
            let frame = engine.pages[&1]
                .state
                .image
                .lock()
                .map_err(|_| "frame lock")?
                .is_some();
            if finished
                && frame
                && diagnostics.web.load(Ordering::Acquire) > 0
                && diagnostics.network.load(Ordering::Acquire) > 0
            {
                writeln!(log, "HELPERS + IPC + FRAME PASS").map_err(|e| e.to_string())?;
                diagnostics.kill.store(
                    diagnostics.web_pid.load(Ordering::Acquire),
                    Ordering::Release,
                );
                let deadline = Instant::now() + Duration::from_secs(15);
                while Instant::now() < deadline {
                    if let Some(Event::Failed { message, .. }) = engine.poll() {
                        writeln!(log, "ABNORMAL EXIT REPORTED: {message}")
                            .map_err(|e| e.to_string())?;
                        return Ok(());
                    }
                    thread::sleep(TICK);
                }
                return Err("Web process termination was not reported".into());
            }
            thread::sleep(TICK);
        }
        Err("Real engine did not finish the native IPC/frame probe".into())
    })();
    drop(engine);
    let deadline = Instant::now() + Duration::from_secs(15);
    while !diagnostics.stopped.load(Ordering::Acquire) && Instant::now() < deadline {
        thread::sleep(TICK);
    }
    let outcome = outcome.and_then(|()| {
        if diagnostics.stopped.load(Ordering::Acquire)
            && diagnostics.live.load(Ordering::Acquire) == 0
        {
            Ok(())
        } else {
            Err("Owned WebKit helpers did not quiesce".into())
        }
    });
    match &outcome {
        Ok(()) => writeln!(log, "TERRA-ENGINE PASS"),
        Err(e) => writeln!(log, "TERRA-ENGINE FAIL {e}"),
    }
    .map_err(|e| e.to_string())?;
    outcome
}

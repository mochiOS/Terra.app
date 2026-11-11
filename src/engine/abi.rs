//! Exact provisional major-1 C layouts from mochiOS/libs/mochios/webkit.h.
//! Pointers are used only on the engine worker; borrowed data is copied there.
use std::ffi::c_void;
#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct Bytes {
    pub data: *const u8,
    pub len: usize,
}
impl Bytes {
    pub fn new(s: &str) -> Self {
        Self {
            data: s.as_ptr(),
            len: s.len(),
        }
    }
}
#[repr(C)]
pub(super) struct Notification {
    pub size: u32,
    pub kind: u32,
    pub view: u64,
    pub navigation: u64,
    pub status: u32,
    pub history: u32,
    pub text: Bytes,
}
#[repr(C)]
pub(super) struct Options {
    pub size: u32,
    pub reserved: u32,
    pub user: *mut c_void,
    pub wake: Option<unsafe extern "C" fn(*mut c_void)>,
    pub event: Option<unsafe extern "C" fn(*mut c_void, *const Notification)>,
}
#[repr(C)]
pub(super) struct Operation {
    pub size: u32,
    pub kind: u32,
    pub navigation: u64,
    pub text: Bytes,
    pub value: f64,
}
#[repr(C)]
pub(super) struct Input {
    pub size: u32,
    pub kind: u32,
    pub flags: u32,
    pub modifiers: u32,
    pub x: f64,
    pub y: f64,
    pub dx: f64,
    pub dy: f64,
    pub key: u32,
    pub button: u32,
    pub text: Bytes,
    pub start: u32,
    pub end: u32,
}
#[repr(C)]
pub(super) struct Frame {
    pub size: u32,
    pub format: u32,
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub len: usize,
    pub pixels: *const u8,
    pub scale: f64,
    pub serial: u64,
    pub reference: *mut c_void,
}
#[repr(C)]
pub(super) struct Api {
    pub size: u32,
    pub major: u32,
    pub features: u64,
    pub create: Option<unsafe extern "C" fn(*const Options, *mut *mut c_void) -> u32>,
    pub destroy: Option<unsafe extern "C" fn(*mut c_void) -> u32>,
    pub dispatch: Option<unsafe extern "C" fn(*mut c_void, u32) -> u32>,
    pub view_create: Option<unsafe extern "C" fn(*mut c_void, u64, *mut *mut c_void) -> u32>,
    pub view_destroy: Option<unsafe extern "C" fn(*mut c_void) -> u32>,
    pub command: Option<unsafe extern "C" fn(*mut c_void, *const Operation) -> u32>,
    pub resize: Option<unsafe extern "C" fn(*mut c_void, u32, u32, f64) -> u32>,
    pub input: Option<unsafe extern "C" fn(*mut c_void, *const Input) -> u32>,
    pub acquire: Option<unsafe extern "C" fn(*mut c_void, *mut Frame) -> u32>,
    pub release: Option<unsafe extern "C" fn(*mut c_void, *mut c_void) -> u32>,
}
impl Api {
    pub fn compatible(&self) -> bool {
        self.size as usize >= size_of::<Self>()
            && self.major == 1
            && self.features & 1 != 0
            && self.create.is_some()
            && self.destroy.is_some()
            && self.dispatch.is_some()
            && self.view_create.is_some()
            && self.view_destroy.is_some()
            && self.command.is_some()
            && self.resize.is_some()
            && self.input.is_some()
            && self.acquire.is_some()
            && self.release.is_some()
    }
}

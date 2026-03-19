use std::ffi::{c_char, c_void, CStr, CString};
use std::os::fd::RawFd;
use std::sync::{Arc, Condvar, Mutex, OnceLock};

#[allow(dead_code)]
extern "C" {
    fn mako_vm_create(
        cpu_count: i32,
        memory_bytes: u64,
        kernel_path: *const c_char,
        initrd_path: *const c_char,
        rootfs_path: *const c_char,
        rosetta: bool,
        vsock_control_port: u32,
        vsock_docker_port: u32,
    ) -> *mut c_void;

    fn mako_vm_add_share(
        handle: *mut c_void,
        tag: *const c_char,
        host_path: *const c_char,
        read_only: bool,
    );
    fn mako_vm_configure(handle: *mut c_void) -> i32;
    fn mako_vm_create_instance(handle: *mut c_void) -> i32;
    fn mako_vm_start(handle: *mut c_void, callback: extern "C" fn(bool, *const c_char));
    fn mako_vm_stop(handle: *mut c_void, callback: extern "C" fn(bool, *const c_char));
    fn mako_vm_force_stop(handle: *mut c_void, callback: extern "C" fn(bool, *const c_char));
    fn mako_vm_pause(handle: *mut c_void, callback: extern "C" fn(bool, *const c_char));
    fn mako_vm_resume(handle: *mut c_void, callback: extern "C" fn(bool, *const c_char));
    fn mako_vm_save_state(
        handle: *mut c_void,
        path: *const c_char,
        callback: extern "C" fn(bool, *const c_char),
    );
    fn mako_vm_restore_state(
        handle: *mut c_void,
        path: *const c_char,
        callback: extern "C" fn(bool, *const c_char),
    );
    fn mako_vm_supports_save_restore() -> bool;
    fn mako_vm_is_running(handle: *mut c_void) -> bool;
    fn mako_vm_get_state(handle: *mut c_void) -> *const c_char;
    fn mako_vm_get_error(handle: *mut c_void) -> *const c_char;
    fn mako_vm_get_serial_read_fd(handle: *mut c_void) -> i32;
    fn mako_vm_vsock_connect(
        handle: *mut c_void,
        port: u32,
        out_read_fd: *mut i32,
        out_write_fd: *mut i32,
    ) -> i32;
    fn mako_vm_vsock_listen(handle: *mut c_void, port: u32) -> i32;
    fn mako_vm_vsock_accept(handle: *mut c_void, port: u32, out_fd: *mut i32) -> i32;
    fn mako_vm_destroy(handle: *mut c_void);
}

pub struct VmFfiConfig {
    pub cpu_count: u32,
    pub memory_bytes: u64,
    pub kernel_path: String,
    pub initrd_path: Option<String>,
    pub rootfs_path: String,
    pub rosetta: bool,
    pub vsock_control_port: u32,
    pub vsock_docker_port: u32,
    pub shared_directories: Vec<SharedDirConfig>,
}

pub struct SharedDirConfig {
    pub tag: String,
    pub host_path: String,
    pub read_only: bool,
}

pub struct VmHandle {
    ptr: *mut c_void,
}

unsafe impl Send for VmHandle {}
unsafe impl Sync for VmHandle {}

// Global storage for the callback result. Swift callbacks execute on the main
// thread (DispatchQueue.main) while the caller waits on a blocking thread,
// so we need cross-thread communication rather than a thread-local.
static PENDING_RESULT: OnceLock<Mutex<Option<Arc<CallbackResult>>>> = OnceLock::new();

fn pending_result() -> &'static Mutex<Option<Arc<CallbackResult>>> {
    PENDING_RESULT.get_or_init(|| Mutex::new(None))
}

#[allow(dead_code)]
impl VmHandle {
    pub fn create(config: &VmFfiConfig) -> anyhow::Result<Self> {
        let kernel = CString::new(config.kernel_path.as_str())?;
        let initrd_cstr = config
            .initrd_path
            .as_ref()
            .map(|p| CString::new(p.as_str()).unwrap());
        let rootfs = CString::new(config.rootfs_path.as_str())?;

        let initrd_ptr = initrd_cstr
            .as_ref()
            .map_or(std::ptr::null(), |c| c.as_ptr());

        let ptr = unsafe {
            mako_vm_create(
                config.cpu_count as i32,
                config.memory_bytes,
                kernel.as_ptr(),
                initrd_ptr,
                rootfs.as_ptr(),
                config.rosetta,
                config.vsock_control_port,
                config.vsock_docker_port,
            )
        };

        if ptr.is_null() {
            anyhow::bail!("mako_vm_create returned null");
        }

        for share in &config.shared_directories {
            let tag = CString::new(share.tag.as_str())?;
            let host_path = CString::new(share.host_path.as_str())?;
            unsafe {
                mako_vm_add_share(ptr, tag.as_ptr(), host_path.as_ptr(), share.read_only);
            }
        }

        Ok(Self { ptr })
    }

    pub fn configure(&self) -> anyhow::Result<()> {
        let ret = unsafe { mako_vm_configure(self.ptr) };
        if ret != 0 {
            let err = self.get_error().unwrap_or_else(|| "unknown error".into());
            anyhow::bail!("VM configuration failed: {err}");
        }
        Ok(())
    }

    pub fn create_instance(&self) -> anyhow::Result<()> {
        let ret = unsafe { mako_vm_create_instance(self.ptr) };
        if ret != 0 {
            let err = self.get_error().unwrap_or_else(|| "unknown error".into());
            anyhow::bail!("VM instance creation failed: {err}");
        }
        Ok(())
    }

    pub fn start(&self) -> anyhow::Result<()> {
        call_with_callback(|cb| unsafe { mako_vm_start(self.ptr, cb) })
    }

    pub fn stop(&self) -> anyhow::Result<()> {
        call_with_callback(|cb| unsafe { mako_vm_stop(self.ptr, cb) })
    }

    pub fn force_stop(&self) -> anyhow::Result<()> {
        call_with_callback(|cb| unsafe { mako_vm_force_stop(self.ptr, cb) })
    }

    pub fn pause(&self) -> anyhow::Result<()> {
        call_with_callback(|cb| unsafe { mako_vm_pause(self.ptr, cb) })
    }

    pub fn resume(&self) -> anyhow::Result<()> {
        call_with_callback(|cb| unsafe { mako_vm_resume(self.ptr, cb) })
    }

    pub fn save_state(&self, path: &str) -> anyhow::Result<()> {
        let c_path = CString::new(path)?;
        call_with_callback(|cb| unsafe { mako_vm_save_state(self.ptr, c_path.as_ptr(), cb) })
    }

    pub fn restore_state(&self, path: &str) -> anyhow::Result<()> {
        let c_path = CString::new(path)?;
        call_with_callback(|cb| unsafe { mako_vm_restore_state(self.ptr, c_path.as_ptr(), cb) })
    }

    pub fn supports_save_restore() -> bool {
        unsafe { mako_vm_supports_save_restore() }
    }

    pub fn is_running(&self) -> bool {
        unsafe { mako_vm_is_running(self.ptr) }
    }

    pub fn get_state(&self) -> String {
        let ptr = unsafe { mako_vm_get_state(self.ptr) };
        if ptr.is_null() {
            return "unknown".into();
        }
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    }

    pub fn serial_read_fd(&self) -> RawFd {
        unsafe { mako_vm_get_serial_read_fd(self.ptr) }
    }

    /// Connect to a vsock port in the guest. Returns (read_fd, write_fd) on success.
    pub fn vsock_connect(&self, port: u32) -> anyhow::Result<(RawFd, RawFd)> {
        let mut read_fd: i32 = -1;
        let mut write_fd: i32 = -1;
        let ret = unsafe { mako_vm_vsock_connect(self.ptr, port, &mut read_fd, &mut write_fd) };
        if ret != 0 {
            anyhow::bail!("vsock connect to port {port} failed");
        }
        Ok((read_fd as RawFd, write_fd as RawFd))
    }

    /// Start listening for guest-initiated vsock connections on the given port.
    pub fn vsock_listen(&self, port: u32) -> anyhow::Result<()> {
        let ret = unsafe { mako_vm_vsock_listen(self.ptr, port) };
        if ret != 0 {
            anyhow::bail!("vsock listen on port {port} failed");
        }
        Ok(())
    }

    /// Accept the next guest-initiated vsock connection (blocks until available).
    pub fn vsock_accept(&self, port: u32) -> anyhow::Result<RawFd> {
        let mut fd: i32 = -1;
        let ret = unsafe { mako_vm_vsock_accept(self.ptr, port, &mut fd) };
        if ret != 0 {
            anyhow::bail!("vsock accept on port {port} failed");
        }
        Ok(fd as RawFd)
    }

    fn get_error(&self) -> Option<String> {
        let ptr = unsafe { mako_vm_get_error(self.ptr) };
        if ptr.is_null() {
            return None;
        }
        Some(
            unsafe { CStr::from_ptr(ptr) }
                .to_string_lossy()
                .into_owned(),
        )
    }
}

impl Drop for VmHandle {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe { mako_vm_destroy(self.ptr) };
        }
    }
}

/// Calls a Swift FFI function that takes a C callback, and blocks until
/// the callback fires. The callback may run on any thread (typically the
/// main thread via DispatchQueue.main).
fn call_with_callback(f: impl FnOnce(extern "C" fn(bool, *const c_char))) -> anyhow::Result<()> {
    let result = CallbackResult::new();
    *pending_result().lock().unwrap() = Some(result.clone());

    extern "C" fn on_complete(success: bool, error_msg: *const c_char) {
        let result = pending_result()
            .lock()
            .unwrap()
            .take()
            .expect("no pending callback result");
        let err = if error_msg.is_null() {
            None
        } else {
            Some(
                unsafe { CStr::from_ptr(error_msg) }
                    .to_string_lossy()
                    .into_owned(),
            )
        };
        result.complete(success, err);
    }

    f(on_complete);

    let (success, err) = result.wait();
    if success {
        Ok(())
    } else {
        anyhow::bail!("{}", err.unwrap_or_else(|| "unknown error".into()))
    }
}

struct CallbackResult {
    inner: Mutex<Option<(bool, Option<String>)>>,
    condvar: Condvar,
}

impl CallbackResult {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(None),
            condvar: Condvar::new(),
        })
    }

    fn complete(&self, success: bool, error: Option<String>) {
        *self.inner.lock().unwrap() = Some((success, error));
        self.condvar.notify_all();
    }

    fn wait(&self) -> (bool, Option<String>) {
        let mut guard = self.inner.lock().unwrap();
        while guard.is_none() {
            guard = self.condvar.wait(guard).unwrap();
        }
        guard.take().unwrap()
    }
}

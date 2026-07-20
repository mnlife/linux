// SPDX-License-Identifier: GPL-2.0
//! Rust userspace reference implementation for rt_ipc.
//!
//! `rt_ipc` is the real-time IPC subsystem built on the migrating-thread
//! model (see `Documentation/rt_ipc/`).  All of its operations go through
//! `ioctl(2)` on file descriptors, so this crate is a thin, safe wrapper over
//! that uAPI: it mirrors the versioned request structures from
//! `include/uapi/linux/rt_ipc.h` and exposes ergonomic [`Device`],
//! [`Endpoint`] and [`Connection`] handles.
//!
//! The crate is intentionally dependency-free.  It reaches libc through the
//! symbols the Rust standard library already links, so it builds and runs
//! offline and can be dropped straight into `tools/testing/selftests`.
//!
//! # Example
//!
//! ```no_run
//! use rt_ipc::{Device, EndpointConfig};
//!
//! let dev = Device::open()?;
//! assert_eq!(dev.abi_version()?, rt_ipc::ABI_VERSION);
//!
//! // A server registers a migrating-thread entry ...
//! let mut stack = vec![0u8; 64 * 1024];
//! let cfg = EndpointConfig::new(server_entry as *const () as usize, &mut stack).max_concurrency(4);
//! let endpoint = dev.create_endpoint(&cfg)?;
//!
//! // ... a client connects and performs a synchronous RPC.
//! let conn = endpoint.connect()?;
//! let mut reply = [0u8; 64];
//! let n = conn.call(b"ping", &mut reply)?;
//! # extern "C" fn server_entry() {}
//! # Ok::<(), rt_ipc::Error>(())
//! ```

use std::ffi::c_void;
use std::io;
use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::Path;

mod ffi {
    use std::ffi::{c_char, c_int, c_uint, c_ulong, c_void};

    extern "C" {
        pub fn open(path: *const c_char, oflag: c_int, ...) -> c_int;
        pub fn ioctl(fd: c_int, request: c_ulong, arg: *mut c_void) -> c_int;
    }

    pub const O_RDWR: c_int = 0o2;
    pub const O_CLOEXEC: c_int = 0o2000000;

    // asm-generic ioctl encoding, shared by x86, arm, arm64 and riscv.
    const NRBITS: c_uint = 8;
    const TYPEBITS: c_uint = 8;
    const SIZEBITS: c_uint = 14;
    const NRSHIFT: c_uint = 0;
    const TYPESHIFT: c_uint = NRSHIFT + NRBITS;
    const SIZESHIFT: c_uint = TYPESHIFT + TYPEBITS;
    const DIRSHIFT: c_uint = SIZESHIFT + SIZEBITS;

    #[allow(dead_code)]
    pub const DIR_NONE: c_uint = 0;
    pub const DIR_WRITE: c_uint = 1;
    pub const DIR_READ: c_uint = 2;

    /// Reproduces the kernel `_IOC()` macro used by `_IO*` in the uAPI header.
    pub const fn ioc(dir: c_uint, ty: c_uint, nr: c_uint, size: c_uint) -> c_ulong {
        ((dir << DIRSHIFT) | (ty << TYPESHIFT) | (nr << NRSHIFT) | (size << SIZESHIFT)) as c_ulong
    }
}

/// The control device registered by the kernel module.
pub const DEVICE_PATH: &str = "/dev/rt_ipc";

/// ABI version reported by [`Device::abi_version`] (`RT_IPC_ABI_VERSION`).
pub const ABI_VERSION: u32 = 1;

/// ioctl magic number reserved for rt_ipc (`RT_IPC_IOC`).
const IOC_MAGIC: u32 = b'9' as u32;

// Endpoint creation flags (`RT_IPC_EP_*`).
/// Allow nested RPCs originating from within the server entry.
pub const EP_ALLOW_NESTED: u32 = 1 << 0;
/// Server entry must run with the server's credentials (default policy).
pub const EP_SERVER_CREDS: u32 = 1 << 1;

// Call flags (`RT_IPC_CALL_*`).
/// Do not allow the call to be interrupted by non-fatal signals.
pub const CALL_UNINTERRUPTIBLE: u32 = 1 << 0;

/// `struct rt_ipc_endpoint_create` from the uAPI header.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
struct EndpointCreate {
    size: u32,
    flags: u32,
    entry: u64,
    stack_top: u64,
    stack_size: u64,
    max_concurrency: u32,
    reserved: u32,
}

/// `struct rt_ipc_connect` from the uAPI header.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
struct Connect {
    size: u32,
    flags: u32,
    endpoint_fd: i32,
    reserved: u32,
}

/// `struct rt_ipc_call` from the uAPI header.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
struct Call {
    size: u32,
    flags: u32,
    send_buf: u64,
    send_len: u64,
    recv_buf: u64,
    recv_len: u64,
    out_recv_len: u64,
    timeout_ms: i32,
    reserved: u32,
}

fn ioc_r<T>(nr: u32) -> std::ffi::c_ulong {
    ffi::ioc(
        ffi::DIR_READ,
        IOC_MAGIC,
        nr,
        std::mem::size_of::<T>() as u32,
    )
}

fn ioc_w<T>(nr: u32) -> std::ffi::c_ulong {
    ffi::ioc(
        ffi::DIR_WRITE,
        IOC_MAGIC,
        nr,
        std::mem::size_of::<T>() as u32,
    )
}

fn ioc_wr<T>(nr: u32) -> std::ffi::c_ulong {
    ffi::ioc(
        ffi::DIR_WRITE | ffi::DIR_READ,
        IOC_MAGIC,
        nr,
        std::mem::size_of::<T>() as u32,
    )
}

// Command numbers matching `RT_IPC_*` in include/uapi/linux/rt_ipc.h.
fn cmd_get_version() -> std::ffi::c_ulong {
    ioc_r::<u32>(0x00)
}
fn cmd_endpoint_create() -> std::ffi::c_ulong {
    ioc_w::<EndpointCreate>(0x01)
}
fn cmd_endpoint_connect() -> std::ffi::c_ulong {
    ioc_w::<Connect>(0x02)
}
fn cmd_call() -> std::ffi::c_ulong {
    ioc_wr::<Call>(0x03)
}

/// Errors returned by this crate.
#[derive(Debug)]
pub enum Error {
    /// The request payload exceeds the per-call kernel bound (`EMSGSIZE`).
    PayloadTooLarge,
    /// A reply was truncated because the supplied buffer was too small.
    ReplyTruncated {
        /// Bytes the server produced.
        produced: usize,
        /// Bytes that fit in the caller's buffer.
        capacity: usize,
    },
    /// An underlying system call failed.
    Io(io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::PayloadTooLarge => write!(f, "rt_ipc payload exceeds the per-call bound"),
            Error::ReplyTruncated { produced, capacity } => write!(
                f,
                "rt_ipc reply truncated: server produced {produced} bytes, buffer holds {capacity}"
            ),
            Error::Io(e) => write!(f, "rt_ipc syscall failed: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, Error>;

/// Largest request/reply payload the kernel currently accepts (matches
/// `RT_IPC_MAX_PAYLOAD` in `ipc/rt_ipc/migrate.c`).
pub const MAX_PAYLOAD: usize = 64 * 1024;

/// Perform an `ioctl` that returns 0 on success, mapping errno to [`Error`].
fn ioctl_ok(fd: RawFd, request: std::ffi::c_ulong, arg: *mut c_void) -> Result<i32> {
    // SAFETY: `fd` is a valid descriptor for the lifetime of the call and
    // `arg` points to a correctly sized, initialised structure.
    let ret = unsafe { ffi::ioctl(fd, request, arg) };
    if ret < 0 {
        Err(Error::Io(io::Error::last_os_error()))
    } else {
        Ok(ret)
    }
}

/// Description of a migrating-thread service entry to register.
#[derive(Debug, Clone, Copy)]
pub struct EndpointConfig {
    entry: u64,
    stack_top: u64,
    stack_size: u64,
    max_concurrency: u32,
    flags: u32,
}

impl EndpointConfig {
    /// Build a config from a server entry address and its per-call stack
    /// region.  The stack slice must outlive every call served on the
    /// endpoint; `stack_top` is derived from its end.
    pub fn new(entry: usize, stack: &mut [u8]) -> Self {
        let base = stack.as_mut_ptr() as u64;
        EndpointConfig {
            entry: entry as u64,
            stack_top: base + stack.len() as u64,
            stack_size: stack.len() as u64,
            max_concurrency: 1,
            flags: EP_SERVER_CREDS,
        }
    }

    /// Build a config from raw values (useful for negative tests).
    pub fn from_raw(entry: u64, stack_top: u64, stack_size: u64) -> Self {
        EndpointConfig {
            entry,
            stack_top,
            stack_size,
            max_concurrency: 1,
            flags: EP_SERVER_CREDS,
        }
    }

    /// Set the maximum number of concurrent in-flight calls.
    pub fn max_concurrency(mut self, n: u32) -> Self {
        self.max_concurrency = n;
        self
    }

    /// Replace the endpoint flags (`EP_*`).
    pub fn flags(mut self, flags: u32) -> Self {
        self.flags = flags;
        self
    }

    fn to_req(self) -> EndpointCreate {
        EndpointCreate {
            size: std::mem::size_of::<EndpointCreate>() as u32,
            flags: self.flags,
            entry: self.entry,
            stack_top: self.stack_top,
            stack_size: self.stack_size,
            max_concurrency: self.max_concurrency,
            reserved: 0,
        }
    }
}

/// The rt_ipc control device (`/dev/rt_ipc`).
#[derive(Debug)]
pub struct Device {
    fd: OwnedFd,
}

impl Device {
    /// Open the default control device.
    pub fn open() -> Result<Self> {
        Self::open_path(DEVICE_PATH)
    }

    /// Open a control device at a custom path.
    pub fn open_path<P: AsRef<Path>>(path: P) -> Result<Self> {
        let cpath = path_to_cstring(path.as_ref())?;
        // SAFETY: `cpath` is a valid NUL-terminated string.
        // (The tree-wide clippy rule forbidding `CStr::as_ptr` targets kernel
        // code; this is a userspace std crate where `as_ptr` is correct.)
        #[allow(clippy::disallowed_methods)]
        let raw = unsafe { ffi::open(cpath.as_ptr(), ffi::O_RDWR | ffi::O_CLOEXEC) };
        if raw < 0 {
            return Err(Error::Io(io::Error::last_os_error()));
        }
        // SAFETY: `open` returned a fresh, owned descriptor.
        Ok(Device {
            fd: unsafe { OwnedFd::from_raw_fd(raw) },
        })
    }

    /// Query the kernel ABI version (`RT_IPC_GET_VERSION`).
    pub fn abi_version(&self) -> Result<u32> {
        let mut version: u32 = 0;
        ioctl_ok(
            self.fd.as_raw_fd(),
            cmd_get_version(),
            &mut version as *mut u32 as *mut c_void,
        )?;
        Ok(version)
    }

    /// Register a migrating-thread service and return its endpoint handle
    /// (`RT_IPC_ENDPOINT_CREATE`).
    pub fn create_endpoint(&self, cfg: &EndpointConfig) -> Result<Endpoint> {
        let mut req = cfg.to_req();
        let fd = ioctl_ok(
            self.fd.as_raw_fd(),
            cmd_endpoint_create(),
            &mut req as *mut EndpointCreate as *mut c_void,
        )?;
        // SAFETY: the ioctl returns a fresh O_CLOEXEC endpoint fd on success.
        Ok(Endpoint {
            fd: unsafe { OwnedFd::from_raw_fd(fd) },
        })
    }
}

impl AsRawFd for Device {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

/// A registered migrating-thread endpoint.
#[derive(Debug)]
pub struct Endpoint {
    fd: OwnedFd,
}

impl Endpoint {
    /// Open a connection to this endpoint (`RT_IPC_ENDPOINT_CONNECT`).
    pub fn connect(&self) -> Result<Connection> {
        let mut req = Connect {
            size: std::mem::size_of::<Connect>() as u32,
            flags: 0,
            endpoint_fd: self.fd.as_raw_fd(),
            reserved: 0,
        };
        let fd = ioctl_ok(
            self.fd.as_raw_fd(),
            cmd_endpoint_connect(),
            &mut req as *mut Connect as *mut c_void,
        )?;
        // SAFETY: the ioctl returns a fresh O_CLOEXEC connection fd on success.
        Ok(Connection {
            fd: unsafe { OwnedFd::from_raw_fd(fd) },
        })
    }

    /// Adopt an endpoint fd received over `SCM_RIGHTS`.
    ///
    /// # Safety
    ///
    /// `fd` must be a valid rt_ipc endpoint descriptor that ownership is being
    /// transferred for.
    pub unsafe fn from_raw_fd(fd: RawFd) -> Self {
        Endpoint {
            fd: OwnedFd::from_raw_fd(fd),
        }
    }
}

impl AsRawFd for Endpoint {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

/// A client's connection to an endpoint.
#[derive(Debug)]
pub struct Connection {
    fd: OwnedFd,
}

impl Connection {
    /// Perform one synchronous migrating-thread RPC (`RT_IPC_CALL`).
    ///
    /// Returns the number of reply bytes written into `recv`.
    pub fn call(&self, send: &[u8], recv: &mut [u8]) -> Result<usize> {
        self.call_with(send, recv, -1, 0)
    }

    /// Perform an RPC with an explicit timeout (milliseconds, `-1` = forever)
    /// and call flags (`CALL_*`).
    pub fn call_with(
        &self,
        send: &[u8],
        recv: &mut [u8],
        timeout_ms: i32,
        flags: u32,
    ) -> Result<usize> {
        if send.len() > MAX_PAYLOAD || recv.len() > MAX_PAYLOAD {
            return Err(Error::PayloadTooLarge);
        }

        let mut req = Call {
            size: std::mem::size_of::<Call>() as u32,
            flags,
            send_buf: send.as_ptr() as u64,
            send_len: send.len() as u64,
            recv_buf: recv.as_mut_ptr() as u64,
            recv_len: recv.len() as u64,
            out_recv_len: 0,
            timeout_ms,
            reserved: 0,
        };

        ioctl_ok(
            self.fd.as_raw_fd(),
            cmd_call(),
            &mut req as *mut Call as *mut c_void,
        )?;

        Ok(req.out_recv_len as usize)
    }
}

impl AsRawFd for Connection {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

/// Low-level helpers that issue rt_ipc ioctls with caller-chosen, possibly
/// malformed, request fields.  These bypass the safe wrappers' local checks so
/// tests can exercise the kernel's own argument validation (bad `size`,
/// unknown `flags`, oversized payloads, ...) exactly like the C selftest.
pub mod raw {
    use super::*;

    /// Issue `RT_IPC_ENDPOINT_CREATE` with explicit fields.  Returns the new
    /// endpoint fd, or the raw `io::Error` (inspect `raw_os_error()` for the
    /// errno) on failure.
    #[allow(clippy::too_many_arguments)]
    pub fn endpoint_create(
        dev: &Device,
        size: u32,
        flags: u32,
        entry: u64,
        stack_top: u64,
        stack_size: u64,
        max_concurrency: u32,
    ) -> io::Result<OwnedFd> {
        let mut req = EndpointCreate {
            size,
            flags,
            entry,
            stack_top,
            stack_size,
            max_concurrency,
            reserved: 0,
        };
        // SAFETY: req is a correctly typed structure for this ioctl.
        let fd = unsafe {
            ffi::ioctl(
                dev.as_raw_fd(),
                cmd_endpoint_create(),
                &mut req as *mut EndpointCreate as *mut c_void,
            )
        };
        if fd < 0 {
            Err(io::Error::last_os_error())
        } else {
            // SAFETY: the ioctl returned a fresh owned descriptor.
            Ok(unsafe { OwnedFd::from_raw_fd(fd) })
        }
    }

    /// Issue `RT_IPC_CALL` with explicit fields.  Returns `out_recv_len` on
    /// success or the raw `io::Error` on failure.
    #[allow(clippy::too_many_arguments)]
    pub fn call(
        conn: &Connection,
        size: u32,
        flags: u32,
        send_buf: u64,
        send_len: u64,
        recv_buf: u64,
        recv_len: u64,
        timeout_ms: i32,
    ) -> io::Result<u64> {
        let mut req = Call {
            size,
            flags,
            send_buf,
            send_len,
            recv_buf,
            recv_len,
            out_recv_len: 0,
            timeout_ms,
            reserved: 0,
        };
        // SAFETY: req is a correctly typed structure for this ioctl.
        let ret = unsafe {
            ffi::ioctl(
                conn.as_raw_fd(),
                cmd_call(),
                &mut req as *mut Call as *mut c_void,
            )
        };
        if ret < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(req.out_recv_len)
        }
    }

    /// Size of `struct rt_ipc_endpoint_create`, for building well-formed
    /// requests in tests.
    pub const ENDPOINT_CREATE_SIZE: u32 = std::mem::size_of::<EndpointCreate>() as u32;
    /// Size of `struct rt_ipc_call`.
    pub const CALL_SIZE: u32 = std::mem::size_of::<Call>() as u32;
}

fn path_to_cstring(path: &Path) -> Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| Error::Io(io::Error::from(io::ErrorKind::InvalidInput)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ioctl_numbers_match_uapi() {
        // Encodings computed from include/uapi/linux/rt_ipc.h with magic '9'.
        assert_eq!(cmd_get_version(), 0x8004_3900);
        assert_eq!(cmd_endpoint_create(), 0x4028_3901);
        assert_eq!(cmd_endpoint_connect(), 0x4010_3902);
        assert_eq!(cmd_call(), 0xc038_3903);
    }

    #[test]
    fn struct_sizes_match_uapi() {
        assert_eq!(std::mem::size_of::<EndpointCreate>(), 40);
        assert_eq!(std::mem::size_of::<Connect>(), 16);
        assert_eq!(std::mem::size_of::<Call>(), 56);
    }
}

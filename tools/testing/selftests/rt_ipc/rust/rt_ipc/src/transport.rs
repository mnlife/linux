// SPDX-License-Identifier: GPL-2.0

//! Transport backends for rt_ipc.
//!
//! Two backends implement one logical request/reply contract:
//!
//! * [`Backend::Kernel`] issues the real `rt_ipc_*` syscalls and benefits from
//!   the migrating-thread fast path.
//! * [`Backend::Socket`] is a portable reference/emulation built on
//!   `AF_UNIX` stream sockets.  It is used (a) as the automatic fallback when
//!   the kernel lacks rt_ipc support, and (b) as the *baseline* in the
//!   benchmark, mirroring the "Linux local sockets" comparison from the paper.
//!
//! [`Backend::Auto`] probes for kernel support once and selects accordingly.

use crate::abi::{RT_IPC_MSG_MAX, RT_IPC_NAME_MAX};
use crate::error::{Error, Result};
use crate::{sys, util};
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Selects which transport implementation to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// Probe for kernel rt_ipc support, fall back to sockets otherwise.
    Auto,
    /// Force the kernel syscalls (fails with [`Error::NotSupported`] if absent).
    Kernel,
    /// Force the `AF_UNIX` reference transport.
    Socket,
}

impl Backend {
    /// Resolve [`Backend::Auto`] into a concrete backend by probing the kernel.
    pub fn resolve(self) -> Backend {
        match self {
            Backend::Auto => {
                if kernel_supported() {
                    Backend::Kernel
                } else {
                    Backend::Socket
                }
            }
            other => other,
        }
    }
}

/// Returns `true` if the running kernel exposes the rt_ipc syscalls.
///
/// Detection is **opt-in**: the syscall numbers used by rt_ipc may be assigned
/// to unrelated syscalls on a stock kernel, and blindly issuing them could have
/// side effects.  Therefore the probe only runs when the caller explicitly
/// enables the kernel backend, via `RT_IPC_ENABLE_KERNEL=1` in the environment
/// or by requesting [`Backend::Kernel`].  Otherwise this returns `false` and
/// callers use the portable reference transport.
///
/// When probing is enabled, a deliberately invalid registration (zero-length
/// name) is issued: a supporting kernel answers `-EINVAL`, a stock kernel
/// answers `-ENOSYS`.  The result is cached for the lifetime of the process.
pub fn kernel_supported() -> bool {
    use std::sync::OnceLock;
    static SUPPORTED: OnceLock<bool> = OnceLock::new();
    *SUPPORTED.get_init(|| {
        if !kernel_probe_allowed() {
            return false;
        }
        let ret = unsafe { sys::rt_ipc_register(core::ptr::null(), 0) };
        Error::from_neg_errno(ret) != Error::NotSupported
    })
}

/// Whether we are permitted to actually issue the rt_ipc syscalls to probe for
/// support.  Gated so hermetic test environments never touch unknown syscalls.
fn kernel_probe_allowed() -> bool {
    match std::env::var("RT_IPC_ENABLE_KERNEL").as_deref() {
        Ok("1") | Ok("yes") | Ok("true") => true,
        _ => matches!(std::env::var("RT_IPC_BACKEND").as_deref(), Ok("kernel")),
    }
}

// `OnceLock::get_or_init` is stable, but keep a tiny shim so the intent reads
// clearly and the call site above stays terse.
trait OnceLockExt<T> {
    fn get_init(&self, f: impl FnOnce() -> T) -> &T;
}
impl<T> OnceLockExt<T> for std::sync::OnceLock<T> {
    fn get_init(&self, f: impl FnOnce() -> T) -> &T {
        self.get_or_init(f)
    }
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > RT_IPC_NAME_MAX || name.contains('\0') {
        return Err(Error::InvalidName);
    }
    Ok(())
}

fn validate_len(len: usize) -> Result<()> {
    if len > RT_IPC_MSG_MAX {
        return Err(Error::MessageTooLarge);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// AF_UNIX reference transport helpers
// ---------------------------------------------------------------------------

/// Runtime directory holding the reference-transport rendezvous sockets.
fn runtime_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("RT_IPC_RUNTIME_DIR") {
        return PathBuf::from(dir);
    }
    if let Ok(dir) = std::env::var("TMPDIR") {
        return PathBuf::from(dir);
    }
    PathBuf::from("/tmp")
}

/// Rendezvous socket path for an endpoint name.
fn socket_path(name: &str) -> PathBuf {
    // The endpoint id makes the path collision-free even if two names sanitise
    // to the same string.
    let id = util::endpoint_id(name);
    runtime_dir().join(format!("rt_ipc-{id:016x}.sock"))
}

/// Write a length-prefixed frame.
fn write_frame(w: &mut impl Write, payload: &[u8]) -> Result<()> {
    let len = payload.len() as u32;
    w.write_all(&len.to_le_bytes())?;
    w.write_all(payload)?;
    w.flush()?;
    Ok(())
}

/// Read a length-prefixed frame, returning `Ok(None)` on a clean EOF.
fn read_frame(r: &mut impl Read) -> Result<Option<Vec<u8>>> {
    let mut hdr = [0u8; 4];
    match r.read_exact(&mut hdr) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_le_bytes(hdr) as usize;
    if len > RT_IPC_MSG_MAX {
        return Err(Error::MessageTooLarge);
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Ok(Some(buf))
}

/// Outcome of an interruptible frame read on the server side.
enum FrameRead {
    /// A complete request frame.
    Frame(Vec<u8>),
    /// The peer disconnected at a frame boundary.
    Eof,
    /// A graceful shutdown was requested while idle.
    Stopped,
}

/// Fill `buf` completely, tolerating read timeouts so the serve loop can react
/// to the stop flag between requests.
///
/// A timeout that occurs at a frame boundary (`nothing read yet`) is where we
/// honour a shutdown request; a timeout mid-frame simply keeps waiting for the
/// rest of the bytes (frames are sent atomically by the reference transport).
fn read_exact_interruptible(
    r: &mut impl Read,
    buf: &mut [u8],
    stop: &StopFlag,
) -> Result<Option<()>> {
    let mut filled = 0;
    while filled < buf.len() {
        match r.read(&mut buf[filled..]) {
            Ok(0) => return Ok(None), // clean EOF
            Ok(n) => filled += n,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if filled == 0 && stop.load(Ordering::Relaxed) {
                    return Ok(None);
                }
                // Otherwise keep waiting for the rest of this frame / next one.
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(Some(()))
}

/// Read one request frame from an accepted connection, honouring `stop`.
fn read_request(r: &mut impl Read, stop: &StopFlag) -> Result<FrameRead> {
    let mut hdr = [0u8; 4];
    match read_exact_interruptible(r, &mut hdr, stop)? {
        Some(()) => {}
        None => {
            return Ok(if stop.load(Ordering::Relaxed) {
                FrameRead::Stopped
            } else {
                FrameRead::Eof
            });
        }
    }
    let len = u32::from_le_bytes(hdr) as usize;
    if len > RT_IPC_MSG_MAX {
        return Err(Error::MessageTooLarge);
    }
    let mut buf = vec![0u8; len];
    match read_exact_interruptible(r, &mut buf, stop)? {
        Some(()) => Ok(FrameRead::Frame(buf)),
        None => Ok(FrameRead::Eof),
    }
}

// ---------------------------------------------------------------------------
// Client-side handle
// ---------------------------------------------------------------------------

pub(crate) enum ClientInner {
    Kernel { endpoint: u64 },
    Socket { stream: UnixStream },
}

pub(crate) fn client_connect(name: &str, backend: Backend) -> Result<ClientInner> {
    validate_name(name)?;
    match backend.resolve() {
        Backend::Kernel => Ok(ClientInner::Kernel {
            endpoint: util::endpoint_id(name),
        }),
        Backend::Socket => {
            let stream =
                UnixStream::connect(socket_path(name)).map_err(|_| Error::NoSuchEndpoint)?;
            Ok(ClientInner::Socket { stream })
        }
        Backend::Auto => unreachable!("resolve() removes Auto"),
    }
}

pub(crate) fn client_invoke(inner: &mut ClientInner, req: &[u8], resp: &mut [u8]) -> Result<usize> {
    validate_len(req.len())?;
    match inner {
        ClientInner::Kernel { endpoint } => {
            let ret = unsafe {
                sys::rt_ipc_invoke(
                    *endpoint,
                    req.as_ptr(),
                    req.len(),
                    resp.as_mut_ptr(),
                    resp.len(),
                )
            };
            if ret < 0 {
                Err(Error::from_neg_errno(ret))
            } else {
                Ok(ret as usize)
            }
        }
        ClientInner::Socket { stream } => {
            write_frame(stream, req)?;
            let reply = read_frame(stream)?.ok_or(Error::NoSuchEndpoint)?;
            if reply.len() > resp.len() {
                return Err(Error::MessageTooLarge);
            }
            resp[..reply.len()].copy_from_slice(&reply);
            Ok(reply.len())
        }
    }
}

// ---------------------------------------------------------------------------
// Server-side serve loops
// ---------------------------------------------------------------------------

/// Shared stop flag used to request a graceful shutdown of a serve loop.
pub(crate) type StopFlag = Arc<AtomicBool>;

/// A request handler.
///
/// In the migrating-thread model many client threads execute the server's
/// handler *concurrently* in the server address space, so the handler contract
/// is `Fn + Send + Sync`.  The reference transport mirrors this by serving each
/// connection on its own thread.
pub(crate) type Handler = dyn Fn(&[u8]) -> Vec<u8> + Send + Sync;

/// A registered endpoint that is ready to serve requests.
///
/// Splitting registration ([`serve_prepare`]) from the serving loop
/// ([`serve_run`]) lets [`crate::Server::spawn`] signal readiness to the
/// caller the instant the endpoint exists, closing the connect/register race.
pub(crate) enum Armed {
    Kernel {
        endpoint: u64,
    },
    Socket {
        listener: UnixListener,
        path: PathBuf,
    },
}

pub(crate) fn serve_prepare(name: &str, backend: Backend) -> Result<Armed> {
    validate_name(name)?;
    match backend.resolve() {
        Backend::Kernel => {
            let ret = unsafe { sys::rt_ipc_register(name.as_ptr(), name.len()) };
            if ret < 0 {
                return Err(Error::from_neg_errno(ret));
            }
            Ok(Armed::Kernel {
                endpoint: ret as u64,
            })
        }
        Backend::Socket => {
            let path = socket_path(name);
            // A stale socket file from a crashed server would make bind() fail.
            let _ = std::fs::remove_file(&path);
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let listener = UnixListener::bind(&path).map_err(|e| match e.kind() {
                std::io::ErrorKind::AddrInUse => Error::EndpointExists,
                _ => Error::Io(e.to_string()),
            })?;
            // Non-blocking accept so the loop can observe the stop flag promptly.
            listener.set_nonblocking(true)?;
            Ok(Armed::Socket { listener, path })
        }
        Backend::Auto => unreachable!("resolve() removes Auto"),
    }
}

pub(crate) fn serve_run(armed: Armed, handler: Arc<Handler>, stop: &StopFlag) -> Result<()> {
    match armed {
        Armed::Kernel { endpoint } => serve_kernel(endpoint, &*handler, stop),
        Armed::Socket { listener, path } => {
            let result = serve_socket(listener, handler, stop);
            let _ = std::fs::remove_file(&path);
            result
        }
    }
}

/// Register and serve in the current thread (used by the blocking API).
pub(crate) fn serve_loop(
    name: &str,
    backend: Backend,
    handler: Arc<Handler>,
    stop: &StopFlag,
) -> Result<()> {
    let armed = serve_prepare(name, backend)?;
    serve_run(armed, handler, stop)
}

fn serve_kernel(endpoint: u64, handler: &Handler, stop: &StopFlag) -> Result<()> {
    let mut reqbuf = vec![0u8; RT_IPC_MSG_MAX];
    // First iteration has no reply to publish yet.
    let mut reply: Vec<u8> = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        let n = unsafe {
            sys::rt_ipc_return(
                endpoint,
                reply.as_ptr(),
                reply.len(),
                reqbuf.as_mut_ptr(),
                reqbuf.len(),
            )
        };
        if n < 0 {
            let err = Error::from_neg_errno(n);
            if err == Error::Interrupted {
                continue;
            }
            return Err(err);
        }
        reply = handler(&reqbuf[..n as usize]);
        validate_len(reply.len())?;
    }
    Ok(())
}

/// Accept connections and serve each on its own thread so that an idle
/// keep-alive connection can never block progress for other clients — matching
/// the concurrent execution semantics of the migrating-thread model.
fn serve_socket(listener: UnixListener, handler: Arc<Handler>, stop: &StopFlag) -> Result<()> {
    let mut conns: Vec<std::thread::JoinHandle<()>> = Vec::new();
    let result = (|| -> Result<()> {
        while !stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((stream, _addr)) => {
                    let handler = Arc::clone(&handler);
                    let stop = Arc::clone(stop);
                    conns.push(std::thread::spawn(move || {
                        let _ = serve_connection(stream, &*handler, &stop);
                    }));
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    // Poll the stop flag between accepts.  A modest interval
                    // keeps idle CPU wakeups negligible while still shutting
                    // down promptly; connections are long-lived (keep-alive),
                    // so the one-off accept latency is immaterial for a test
                    // reference transport and does not warrant an epoll loop.
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    })();
    // Signal and wait for all connection threads to finish.
    stop.store(true, Ordering::Relaxed);
    for c in conns {
        let _ = c.join();
    }
    result
}

/// Serve every request on a single accepted connection until the peer
/// disconnects or a shutdown is requested.
fn serve_connection(mut stream: UnixStream, handler: &Handler, stop: &StopFlag) -> Result<()> {
    stream.set_nonblocking(false)?;
    // A read timeout lets the loop poll the stop flag between requests instead
    // of blocking forever on an idle keep-alive connection.
    stream.set_read_timeout(Some(std::time::Duration::from_millis(50)))?;
    // Serve requests until the peer disconnects or a shutdown is requested.
    while let FrameRead::Frame(req) = read_request(&mut stream, stop)? {
        let reply = handler(&req);
        validate_len(reply.len())?;
        if write_frame(&mut stream, &reply).is_err() {
            break;
        }
    }
    Ok(())
}

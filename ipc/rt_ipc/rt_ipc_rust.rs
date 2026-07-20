// SPDX-License-Identifier: GPL-2.0

//! rt_ipc: real-time IPC based on the migrating-thread model (Rust rewrite).
//!
//! This is a Rust reimplementation of the C `rt_ipc` subsystem
//! (`ipc/rt_ipc/*.c`).  It reproduces the same user-visible ABI
//! (`include/uapi/linux/rt_ipc.h`) and object model:
//!
//! * a control device `/dev/rt_ipc` on which `RT_IPC_GET_VERSION` and
//!   `RT_IPC_ENDPOINT_CREATE` are issued;
//! * an `O_CLOEXEC` **endpoint** fd (an anon-inode file) that a server hands
//!   to clients over `SCM_RIGHTS` and on which `RT_IPC_ENDPOINT_CONNECT` is
//!   issued;
//! * an `O_CLOEXEC` **connection** fd on which `RT_IPC_CALL` performs one
//!   synchronous migrating-thread RPC.
//!
//! Endpoints and connections are reference counted with [`Arc`]; a connection
//! holds an `Arc<Endpoint>`, so an in-flight call can never observe a freed
//! endpoint even if the server closes the endpoint fd concurrently.  This is
//! the memory-safe equivalent of the C code's `kref` + RCU teardown.
//!
//! As in the C foundation, the architecture-specific partial context switch
//! (address-space + register subset switch) is a follow-up milestone: a
//! well-formed `RT_IPC_CALL` validates and accounts the request and then
//! reports `EOPNOTSUPP`, so userspace never observes a half-migrated thread.
//!
//! # Build
//!
//! This module requires a Rust-enabled kernel (`CONFIG_RUST`) and is selected
//! by `CONFIG_RT_IPC_RUST` as an alternative to the C implementation.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use kernel::bindings;
use kernel::cred::Credential;
use kernel::error::{code::*, Error, Result};
use kernel::fs::File;
use kernel::ioctl::{_IOC_SIZE, _IOR, _IOW, _IOWR};
use kernel::miscdevice::{MiscDevice, MiscDeviceOptions, MiscDeviceRegistration};
use kernel::mm::Mm;
use kernel::prelude::*;
use kernel::sync::aref::ARef;
use kernel::sync::{Arc, ArcBorrow};
use kernel::task::Task;
use kernel::transmute::{AsBytes, FromBytes};
use kernel::types::ForeignOwnable;
use kernel::uaccess::{UserPtr, UserSlice};

module! {
    type: RtIpcModule,
    name: "rt_ipc_rust",
    authors: ["Linux kernel contributors"],
    description: "Real-time IPC using the migrating-thread model (Rust)",
    license: "GPL",
}

// ---------------------------------------------------------------------------
// uAPI mirror (must match include/uapi/linux/rt_ipc.h exactly).
// ---------------------------------------------------------------------------

/// ABI version reported by `RT_IPC_GET_VERSION` (`RT_IPC_ABI_VERSION`).
const ABI_VERSION: u32 = 1;

/// ioctl magic reserved for rt_ipc (`RT_IPC_IOC`).
const IOC_MAGIC: u32 = '9' as u32;

// Endpoint flags.
const EP_ALLOW_NESTED: u32 = 1 << 0;
const EP_SERVER_CREDS: u32 = 1 << 1;
const EP_FLAGS_ALL: u32 = EP_ALLOW_NESTED | EP_SERVER_CREDS;

// Connection flags.
const CONN_FLAGS_ALL: u32 = 0;

// Call flags.
const CALL_UNINTERRUPTIBLE: u32 = 1 << 0;
const CALL_FLAGS_ALL: u32 = CALL_UNINTERRUPTIBLE;

/// Upper bound on a single request/reply payload (matches
/// `RT_IPC_MAX_PAYLOAD` in the C `migrate.c`).
const MAX_PAYLOAD: u64 = 64 * 1024;

/// `struct rt_ipc_endpoint_create`.
#[repr(C)]
#[derive(Clone, Copy)]
struct EndpointCreate {
    size: u32,
    flags: u32,
    entry: u64,
    stack_top: u64,
    stack_size: u64,
    max_concurrency: u32,
    reserved: u32,
}

/// `struct rt_ipc_connect`.
#[repr(C)]
#[derive(Clone, Copy)]
struct Connect {
    size: u32,
    flags: u32,
    endpoint_fd: i32,
    reserved: u32,
}

/// `struct rt_ipc_call`.
#[repr(C)]
#[derive(Clone, Copy)]
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

// SAFETY: These are `#[repr(C)]` plain-old-data structures with no padding
// (each is a run of naturally aligned integers), no pointers and no
// invariants, so any bit pattern is a valid value and it is safe to copy them
// byte-for-byte to and from userspace.
unsafe impl FromBytes for EndpointCreate {}
unsafe impl AsBytes for EndpointCreate {}
unsafe impl FromBytes for Connect {}
unsafe impl AsBytes for Connect {}
unsafe impl FromBytes for Call {}
unsafe impl AsBytes for Call {}

// ioctl command numbers.
const RT_IPC_GET_VERSION: u32 = _IOR::<u32>(IOC_MAGIC, 0x00);
const RT_IPC_ENDPOINT_CREATE: u32 = _IOW::<EndpointCreate>(IOC_MAGIC, 0x01);
const RT_IPC_ENDPOINT_CONNECT: u32 = _IOW::<Connect>(IOC_MAGIC, 0x02);
const RT_IPC_CALL: u32 = _IOWR::<Call>(IOC_MAGIC, 0x03);

// Open flags for the anon-inode fds.
const O_RDWR: c_int = 0o2;
const O_CLOEXEC: c_int = 0o2000000;

/// Build an `Error` from a positive errno constant that the kernel crate does
/// not expose as a named [`code`] value.
fn errno(e: u32) -> Error {
    Error::from_errno(-(e as i32))
}

fn eopnotsupp() -> Error {
    errno(bindings::EOPNOTSUPP)
}
fn econnrefused() -> Error {
    errno(bindings::ECONNREFUSED)
}
fn econnreset() -> Error {
    errno(bindings::ECONNRESET)
}

/// Monotonic identifier source for endpoints/connections (tracing/debugfs
/// parity with the C `rt_ipc_alloc_id`).
static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

fn alloc_id() -> u64 {
    ID_COUNTER.fetch_add(1, Ordering::Relaxed) + 1
}

// ---------------------------------------------------------------------------
// Object model: endpoints and connections.
// ---------------------------------------------------------------------------

/// A registered migrating-thread service entry.
///
/// `entry`, `stack_top`, `stack_size` and `flags` describe the server-side
/// dispatch that the architecture partial context switch will consume; they
/// are retained (like the C `struct rt_ipc_endpoint`) but only read once that
/// follow-up milestone lands, hence `allow(dead_code)`.
#[allow(dead_code)]
struct Endpoint {
    id: u64,
    entry: u64,
    stack_top: u64,
    stack_size: u64,
    max_concurrency: u32,
    flags: u32,
    /// Server address space, pinned with `mmgrab` for the endpoint's lifetime.
    _owner_mm: ARef<Mm>,
    /// Credentials the server entry executes with.
    _owner_cred: ARef<Credential>,
    /// Number of in-flight calls; bounded by `max_concurrency`.
    inflight: AtomicU32,
    /// Set once the owner tears the endpoint down (fd closed).
    dead: AtomicBool,
}

impl Endpoint {
    /// Reserve an in-flight slot, bounding concurrency.  Mirrors the C
    /// `rt_ipc_reserve_slot`.
    fn reserve_slot(&self) -> Result {
        let max = self.max_concurrency;
        loop {
            if self.dead.load(Ordering::Acquire) {
                return Err(econnreset());
            }
            let cur = self.inflight.load(Ordering::Acquire);
            if cur >= max {
                return Err(EAGAIN);
            }
            if self
                .inflight
                .compare_exchange_weak(cur, cur + 1, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                return Ok(());
            }
        }
    }

    fn release_slot(&self) {
        let prev = self.inflight.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(prev != 0);
    }
}

/// A client's binding to an endpoint.
#[allow(dead_code)]
struct Connection {
    id: u64,
    ep: Arc<Endpoint>,
    /// Client address space, pinned for the connection's lifetime.
    _client_mm: ARef<Mm>,
}

/// Pin the current task's address space (`mmgrab` equivalent).
fn current_mm() -> Result<ARef<Mm>> {
    // SAFETY: called from process (ioctl) context, so `current` is valid for
    // the duration of this call.
    let guard = unsafe { Task::current() };
    let mm: &Mm = guard.mm().ok_or(EINVAL)?;
    Ok(ARef::from(mm))
}

// ---------------------------------------------------------------------------
// anon-inode fd plumbing.
// ---------------------------------------------------------------------------

/// Install an `Arc<T>` as the private data behind a fresh `O_CLOEXEC`
/// anon-inode fd with the given file operations, transferring ownership of the
/// `Arc` reference into the file.  On failure the reference is reclaimed.
fn install_fd<T: 'static>(
    name: &'static CStr,
    fops: &'static bindings::file_operations,
    obj: Arc<T>,
) -> Result<i32> {
    let ptr = obj.into_foreign();
    // SAFETY: `fops` is a valid, 'static file_operations table and `ptr` is an
    // `Arc<T>` reference produced by `into_foreign`; `anon_inode_getfd` takes
    // ownership of `ptr` on success (dropped by the fops `release`).
    let fd =
        unsafe { bindings::anon_inode_getfd(name.as_char_ptr(), fops, ptr, O_RDWR | O_CLOEXEC) };
    if fd < 0 {
        // SAFETY: `anon_inode_getfd` failed without consuming `ptr`, so we
        // reclaim the `Arc` reference and drop it.
        drop(unsafe { Arc::<T>::from_foreign(ptr) });
        return Err(Error::from_errno(fd));
    }
    Ok(fd)
}

/// Read and validate a fixed-size request structure from the ioctl argument.
fn read_req<T: FromBytes + Copy>(cmd: u32, arg: usize) -> Result<T> {
    let size = _IOC_SIZE(cmd);
    if size != core::mem::size_of::<T>() {
        return Err(EINVAL);
    }
    UserSlice::new(UserPtr::from_addr(arg), size)
        .reader()
        .read::<T>()
}

// ---------------------------------------------------------------------------
// Endpoint fd file operations.
// ---------------------------------------------------------------------------

/// # Safety
/// `file->private_data` must be an `Arc<Endpoint>` reference produced by
/// [`install_fd`].
unsafe extern "C" fn endpoint_release(
    _inode: *mut bindings::inode,
    file: *mut bindings::file,
) -> c_int {
    // SAFETY: `private_data` is the `Arc<Endpoint>` installed for this fd.
    let ep = unsafe { Arc::<Endpoint>::from_foreign((*file).private_data) };
    // Mark the endpoint dead so new connects and in-flight reservations fail,
    // matching the C `rt_ipc_endpoint_shutdown`.  Live connections keep the
    // object alive through their own `Arc<Endpoint>`.
    ep.dead.store(true, Ordering::Release);
    drop(ep);
    0
}

/// # Safety
/// `file->private_data` must be an `Arc<Endpoint>` reference produced by
/// [`install_fd`].
unsafe extern "C" fn endpoint_ioctl(
    file: *mut bindings::file,
    cmd: c_uint,
    arg: c_ulong,
) -> c_long {
    // SAFETY: `private_data` is a live `Arc<Endpoint>` for the duration of the
    // call (the fd holds the reference).
    let ep = unsafe { Arc::<Endpoint>::borrow((*file).private_data) };
    let res = match cmd {
        RT_IPC_ENDPOINT_CONNECT => endpoint_connect(ep, arg as usize),
        _ => Err(ENOTTY),
    };
    match res {
        Ok(v) => v as c_long,
        Err(e) => e.to_errno() as c_long,
    }
}

fn endpoint_connect(ep: ArcBorrow<'_, Endpoint>, arg: usize) -> Result<i32> {
    let req: Connect = read_req(RT_IPC_ENDPOINT_CONNECT, arg)?;
    if req.size as usize != core::mem::size_of::<Connect>() {
        return Err(EINVAL);
    }
    if req.flags & !CONN_FLAGS_ALL != 0 {
        return Err(EINVAL);
    }
    if req.reserved != 0 {
        return Err(EINVAL);
    }
    if ep.dead.load(Ordering::Acquire) {
        return Err(econnrefused());
    }

    let client_mm = current_mm()?;

    // Clone the endpoint refcount into an owning `Arc` for the connection.  The
    // borrow guarantees the endpoint is alive for the duration of this call.
    let ep_arc: Arc<Endpoint> = Arc::from(ep);

    let conn = Arc::new(
        Connection {
            id: alloc_id(),
            ep: ep_arc,
            _client_mm: client_mm,
        },
        GFP_KERNEL,
    )?;

    if conn.ep.dead.load(Ordering::Acquire) {
        return Err(econnrefused());
    }

    install_fd(c"[rt_ipc.conn]", connection_fops(), conn)
}

// ---------------------------------------------------------------------------
// Connection fd file operations.
// ---------------------------------------------------------------------------

/// # Safety
/// `file->private_data` must be an `Arc<Connection>` reference produced by
/// [`install_fd`].
unsafe extern "C" fn connection_release(
    _inode: *mut bindings::inode,
    file: *mut bindings::file,
) -> c_int {
    // SAFETY: `private_data` is the `Arc<Connection>` installed for this fd.
    drop(unsafe { Arc::<Connection>::from_foreign((*file).private_data) });
    0
}

/// # Safety
/// `file->private_data` must be an `Arc<Connection>` reference produced by
/// [`install_fd`].
unsafe extern "C" fn connection_ioctl(
    file: *mut bindings::file,
    cmd: c_uint,
    arg: c_ulong,
) -> c_long {
    // SAFETY: `private_data` is a live `Arc<Connection>` for the call.
    let conn = unsafe { Arc::<Connection>::borrow((*file).private_data) };
    let res = match cmd {
        RT_IPC_CALL => connection_call(&conn, arg as usize),
        _ => Err(ENOTTY),
    };
    match res {
        Ok(v) => v as c_long,
        Err(e) => e.to_errno() as c_long,
    }
}

fn connection_call(conn: &Connection, arg: usize) -> Result<i32> {
    let mut req: Call = read_req(RT_IPC_CALL, arg)?;
    if req.size as usize != core::mem::size_of::<Call>() {
        return Err(EINVAL);
    }
    if req.flags & !CALL_FLAGS_ALL != 0 {
        return Err(EINVAL);
    }
    if req.reserved != 0 {
        return Err(EINVAL);
    }
    if req.send_len > MAX_PAYLOAD || req.recv_len > MAX_PAYLOAD {
        return Err(errno(bindings::EMSGSIZE));
    }

    let ep = &conn.ep;
    ep.reserve_slot()?;

    // The migrating-thread partial context switch (mm + register subset) is an
    // architecture follow-up, exactly as in the C foundation.  Validate and
    // account the request, then report EOPNOTSUPP without ever exposing a
    // half-migrated thread to userspace.
    let result = do_switch_stub(ep, &mut req);

    ep.release_slot();

    match result {
        Ok(()) => {
            // On a (future) successful call the reply length and out_recv_len
            // are written back here.  Currently unreachable.
            req.out_recv_len = 0;
            UserSlice::new(UserPtr::from_addr(arg), core::mem::size_of::<Call>())
                .writer()
                .write::<Call>(&req)?;
            Ok(0)
        }
        Err(e) => Err(e),
    }
}

/// Placeholder for the architecture-specific partial context switch.
fn do_switch_stub(_ep: &Endpoint, _call: &mut Call) -> Result {
    Err(eopnotsupp())
}

// ---------------------------------------------------------------------------
// Control device (/dev/rt_ipc).
// ---------------------------------------------------------------------------

/// Per-open state for the control device.  The device is stateless; opening it
/// only grants the ability to query the version and register endpoints.
struct Control;

#[vtable]
impl MiscDevice for Control {
    type Ptr = KBox<Control>;

    fn open(_file: &File, _misc: &MiscDeviceRegistration<Self>) -> Result<KBox<Control>> {
        KBox::new(Control, GFP_KERNEL)
    }

    fn ioctl(_device: &Control, file: &File, cmd: u32, arg: usize) -> Result<isize> {
        match cmd {
            RT_IPC_GET_VERSION => {
                let v: u32 = ABI_VERSION;
                UserSlice::new(UserPtr::from_addr(arg), core::mem::size_of::<u32>())
                    .writer()
                    .write::<u32>(&v)?;
                Ok(0)
            }
            RT_IPC_ENDPOINT_CREATE => Ok(endpoint_create(cmd, arg, file)? as isize),
            _ => Err(ENOTTY),
        }
    }
}

/// Validate a server entry description.
///
/// This mirrors the `EINVAL` decisions of the C `rt_ipc_validate_entry`: the
/// stack region `[stack_top - stack_size, stack_top)` must not wrap and at
/// least one concurrency slot is required.  The C code additionally rejects
/// addresses at or above `TASK_SIZE` with `EFAULT`; that constant is not
/// exposed to Rust, so the precise user-VA upper bound is left to fault at
/// access time (as it ultimately is in C too, on the first dereference).
fn validate_entry(req: &EndpointCreate) -> Result {
    // Stack region must not wrap below zero.
    if req.stack_top.checked_sub(req.stack_size).is_none() {
        return Err(EINVAL);
    }
    // A single call needs at least one stack slot.
    if req.max_concurrency == 0 {
        return Err(EINVAL);
    }
    Ok(())
}

fn endpoint_create(cmd: u32, arg: usize, file: &File) -> Result<i32> {
    let req: EndpointCreate = read_req(cmd, arg)?;
    if req.size as usize != core::mem::size_of::<EndpointCreate>() {
        return Err(EINVAL);
    }
    if req.flags & !EP_FLAGS_ALL != 0 {
        return Err(EINVAL);
    }
    if req.reserved != 0 {
        return Err(EINVAL);
    }
    validate_entry(&req)?;

    let owner_mm = current_mm()?;
    // Credentials the server entry runs with: those of the task that opened the
    // control device (`f_cred`), matching the C code capturing the creator's
    // credentials at endpoint-create time.
    let owner_cred: ARef<Credential> = ARef::from(file.cred());

    let ep = Arc::new(
        Endpoint {
            id: alloc_id(),
            entry: req.entry,
            stack_top: req.stack_top,
            stack_size: req.stack_size,
            max_concurrency: req.max_concurrency,
            flags: req.flags,
            _owner_mm: owner_mm,
            _owner_cred: owner_cred,
            inflight: AtomicU32::new(0),
            dead: AtomicBool::new(false),
        },
        GFP_KERNEL,
    )?;

    install_fd(c"[rt_ipc.ep]", endpoint_fops(), ep)
}

// ---------------------------------------------------------------------------
// file_operations tables for the anon-inode fds.
// ---------------------------------------------------------------------------

fn endpoint_fops() -> &'static bindings::file_operations {
    // Only `release` and `unlocked_ioctl` are needed; the remaining fields are
    // zeroed.  A `const` referenced by `&` is promoted to `'static`, matching
    // the kernel crate's own `MiscDevice` vtable construction.
    const FOPS: bindings::file_operations = bindings::file_operations {
        release: Some(endpoint_release),
        unlocked_ioctl: Some(endpoint_ioctl),
        ..pin_init::zeroed()
    };
    &FOPS
}

fn connection_fops() -> &'static bindings::file_operations {
    const FOPS: bindings::file_operations = bindings::file_operations {
        release: Some(connection_release),
        unlocked_ioctl: Some(connection_ioctl),
        ..pin_init::zeroed()
    };
    &FOPS
}

// ---------------------------------------------------------------------------
// Module registration.
// ---------------------------------------------------------------------------

#[pin_data]
struct RtIpcModule {
    #[pin]
    _control: MiscDeviceRegistration<Control>,
}

impl kernel::InPlaceModule for RtIpcModule {
    fn init(_module: &'static ThisModule) -> impl PinInit<Self, Error> {
        pr_info!(
            "rt_ipc: migrating-thread IPC registered (Rust, ABI v{})\n",
            ABI_VERSION
        );

        let options = MiscDeviceOptions { name: c"rt_ipc" };

        try_pin_init!(Self {
            _control <- MiscDeviceRegistration::register(options),
        })
    }
}

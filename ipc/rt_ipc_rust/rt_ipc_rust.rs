// SPDX-License-Identifier: GPL-2.0

//! rt_ipc_rust: a Rust reimplementation of the migrating-thread real-time IPC.
//!
//! This module is a Rust counterpart to the C `rt_ipc` subsystem (see
//! `ipc/rt_ipc/` and `Documentation/rt_ipc/`).  It reproduces the same object
//! model -- reference-counted *endpoints* registered by servers, client
//! *connections* bound to an endpoint, and a synchronous migrating-thread
//! *call* path -- using the kernel's Rust abstractions (`miscdevice`, `Arc`,
//! the global-lock helpers and the user-copy helpers).
//!
//! Faithful pieces
//! ---------------
//! * Endpoints capture the owner's credentials at registration and are
//!   reference counted via [`Arc`]; an in-flight call keeps an [`Arc`] clone of
//!   the connection (which pins the endpoint), so neither object can be freed
//!   under a running RPC even if userspace closes its handle concurrently.
//! * The server entry / stack region is bounds-checked at registration time.
//! * Concurrency is bounded per endpoint by `max_concurrency`.
//! * The request payload is copied out of the client address space into a
//!   bounded kernel bounce buffer *before* the (would-be) address-space switch,
//!   so raw client pointers are never handed to the server.
//!
//! Deliberate adaptations
//! ----------------------
//! * The C version exposes each endpoint and connection as an `O_CLOEXEC`
//!   anonymous-inode fd that can be transferred over `SCM_RIGHTS`.  The Rust
//!   VFS layer does not yet provide anon-inode / fd-passing abstractions, so
//!   this port identifies objects with integer *handles* managed in a global
//!   table and drives every operation through the single `/dev/rt_ipc_rust`
//!   control device.  Handles created by an open file are released when that
//!   file is closed, mirroring "last fd close tears the object down".
//! * As with the C architecture hooks (which currently return `-EOPNOTSUPP`
//!   until the low-level entry trampoline lands), the actual partial context
//!   switch into the server is not wired up here: `RT_IPC_RUST_CALL` performs
//!   the full setup and then reports [`EOPNOTSUPP`] without ever exposing a
//!   half-migrated thread to userspace.
//! * Per-task nested-call depth tracking in the C version uses a `task_struct`
//!   field; that field is not available to a Rust module, so nesting is not
//!   tracked here (only per-endpoint concurrency is bounded).
//!
//! C headers: [`include/uapi/linux/rt_ipc_rust.h`](srctree/include/uapi/linux/rt_ipc_rust.h).

use kernel::{
    cred::Credential,
    fs::File,
    ioctl::{
        _IOC_SIZE,
        _IOR,
        _IOW,
        _IOWR, //
    },
    miscdevice::{
        MiscDevice,
        MiscDeviceOptions,
        MiscDeviceRegistration, //
    },
    new_spinlock,
    prelude::*,
    sync::{
        aref::ARef,
        lock::spinlock::SpinLock,
        Arc, //
    },
    transmute::{
        AsBytes,
        FromBytes, //
    },
    uaccess::{
        UserPtr,
        UserSlice, //
    },
};

module! {
    type: RtIpcRustModule,
    name: "rt_ipc_rust",
    authors: ["Copilot"],
    description: "Real-time IPC (migrating-thread model), Rust reimplementation",
    license: "GPL",
}

// --- uAPI mirror ----------------------------------------------------------
//
// These `repr(C)` structures mirror the layout of the corresponding
// definitions in `include/uapi/linux/rt_ipc_rust.h`.  They are read from and
// written back to userspace through the user-copy helpers.

const RT_IPC_RUST_ABI_VERSION: u32 = 1;

const RT_IPC_RUST_EP_ALLOW_NESTED: u32 = 1 << 0;
const RT_IPC_RUST_EP_SERVER_CREDS: u32 = 1 << 1;
const RT_IPC_RUST_EP_FLAGS_ALL: u32 = RT_IPC_RUST_EP_ALLOW_NESTED | RT_IPC_RUST_EP_SERVER_CREDS;

const RT_IPC_RUST_CONN_FLAGS_ALL: u32 = 0;

const RT_IPC_RUST_CALL_UNINTERRUPTIBLE: u32 = 1 << 0;
const RT_IPC_RUST_CALL_FLAGS_ALL: u32 = RT_IPC_RUST_CALL_UNINTERRUPTIBLE;

/// Upper bound on a single request/reply payload, matching the C foundation.
const RT_IPC_RUST_MAX_PAYLOAD: u64 = 64 * 1024;

const RT_IPC_RUST_IOC: u32 = '9' as u32;

// ioctl numbers, kept in sync with the uAPI header.
const RT_IPC_RUST_GET_VERSION: u32 = _IOR::<u32>(RT_IPC_RUST_IOC, 0x10);
const RT_IPC_RUST_ENDPOINT_CREATE: u32 = _IOW::<EndpointCreate>(RT_IPC_RUST_IOC, 0x11);
const RT_IPC_RUST_ENDPOINT_CONNECT: u32 = _IOW::<Connect>(RT_IPC_RUST_IOC, 0x12);
const RT_IPC_RUST_CALL: u32 = _IOWR::<Call>(RT_IPC_RUST_IOC, 0x13);

/// Mirror of `struct rt_ipc_rust_endpoint_create`.
#[repr(C)]
#[derive(Copy, Clone)]
struct EndpointCreate {
    size: u32,
    flags: u32,
    entry: u64,
    stack_top: u64,
    stack_size: u64,
    max_concurrency: u32,
    reserved: u32,
}

// SAFETY: `EndpointCreate` is `repr(C)`, contains only integer fields and has
// no padding, so any byte pattern is a valid value.
unsafe impl FromBytes for EndpointCreate {}

/// Mirror of `struct rt_ipc_rust_connect`.
#[repr(C)]
#[derive(Copy, Clone)]
struct Connect {
    size: u32,
    flags: u32,
    endpoint: u64,
    reserved: u32,
    pad: u32,
}

// SAFETY: `Connect` is `repr(C)`, contains only integer fields and has no
// padding, so any byte pattern is a valid value.
unsafe impl FromBytes for Connect {}

/// Mirror of `struct rt_ipc_rust_call`.
#[repr(C)]
#[derive(Copy, Clone)]
struct Call {
    size: u32,
    flags: u32,
    connection: u64,
    send_buf: u64,
    send_len: u64,
    recv_buf: u64,
    recv_len: u64,
    out_recv_len: u64,
    timeout_ms: i32,
    reserved: u32,
}

// SAFETY: `Call` is `repr(C)`, contains only integer fields and has no
// padding, so any byte pattern is a valid value.
unsafe impl FromBytes for Call {}
// SAFETY: `Call` is `repr(C)` with no padding, so it has no uninitialised
// bytes and can be safely viewed as a byte slice for copy-back to userspace.
unsafe impl AsBytes for Call {}

// --- Object model ---------------------------------------------------------

/// A registered migrating-thread service entry.
///
/// All fields are immutable after construction; the mutable per-endpoint state
/// (`inflight`, `dead`) lives in the global [`Tables`] slot so that reserving
/// and releasing a call slot is a single locked operation.
///
/// Several fields (the id, owner credentials, entry and stack description) are
/// captured now but are only consumed once the server-dispatch milestone wires
/// up the real partial context switch, so they are currently unread.
#[allow(dead_code)]
struct Endpoint {
    id: u64,
    /// Credentials the server entry would execute with.
    owner_cred: ARef<Credential>,
    entry: u64,
    stack_top: u64,
    stack_size: u64,
    max_concurrency: u32,
    flags: u32,
}

/// A client's binding to an endpoint.
///
/// `id` and `endpoint` are retained for tracing parity and to keep the target
/// endpoint alive for the connection's lifetime; they are not otherwise read
/// until server dispatch is implemented.
#[allow(dead_code)]
struct Connection {
    id: u64,
    /// Handle of the target endpoint, used to locate its concurrency slot.
    endpoint_handle: u64,
    /// Reference to the endpoint; keeps it alive for the connection's lifetime.
    endpoint: Arc<Endpoint>,
}

/// Global-table slot for an endpoint: the object plus its mutable counters.
struct EndpointSlot {
    endpoint: Arc<Endpoint>,
    inflight: u32,
    dead: bool,
}

/// Global-table slot for a connection.
struct ConnectionSlot {
    connection: Arc<Connection>,
}

/// The global object tables.
///
/// A single mutex protects the whole thing.  The C version uses per-endpoint
/// raw spinlocks plus RCU; this port trades that for a coarser lock, which is
/// adequate for the control-plane operations (create/connect/close) and for
/// the brief slot reservation on the call path.  User copies never happen
/// while the lock is held.
struct Tables {
    next_id: u64,
    endpoints: KVec<Option<EndpointSlot>>,
    connections: KVec<Option<ConnectionSlot>>,
}

impl Tables {
    const fn new() -> Self {
        Tables {
            next_id: 1,
            endpoints: KVec::new(),
            connections: KVec::new(),
        }
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        id
    }

    /// Insert `value` into `table`, reusing a free slot if one exists.
    ///
    /// Returns the 1-based handle (0 is never a valid handle).
    fn insert<T>(table: &mut KVec<Option<T>>, value: T) -> Result<u64> {
        for (idx, entry) in table.iter_mut().enumerate() {
            if entry.is_none() {
                *entry = Some(value);
                return Ok((idx as u64) + 1);
            }
        }
        table.push(Some(value), GFP_KERNEL)?;
        Ok(table.len() as u64)
    }

    fn endpoint_slot_mut(&mut self, handle: u64) -> Option<&mut EndpointSlot> {
        let idx = usize::try_from(handle.checked_sub(1)?).ok()?;
        self.endpoints.get_mut(idx)?.as_mut()
    }

    fn connection_slot(&self, handle: u64) -> Option<&ConnectionSlot> {
        let idx = usize::try_from(handle.checked_sub(1)?).ok()?;
        self.connections.get(idx)?.as_ref()
    }
}

kernel::sync::global_lock! {
    // SAFETY: Initialized in the module initializer before first use.
    unsafe(uninit) static TABLES: Mutex<Tables> = Tables::new();
}

/// Clear the table entry for `handle`, if any.  Used for cleanup/rollback.
fn clear_slot<T>(table: &mut KVec<Option<T>>, handle: u64) {
    if let Some(idx) = handle.checked_sub(1).and_then(|i| usize::try_from(i).ok()) {
        if let Some(entry) = table.get_mut(idx) {
            *entry = None;
        }
    }
}

// --- Device / file plumbing ----------------------------------------------

/// Per-open-file state.
///
/// Tracks the handles created through this file so they can be torn down when
/// the file is released, mirroring the C behaviour where closing an endpoint
/// or connection fd drops its reference.
#[pin_data(PinnedDrop)]
struct RtIpcFile {
    #[pin]
    owned: SpinLock<OwnedHandles>,
}

struct OwnedHandles {
    endpoints: KVec<u64>,
    connections: KVec<u64>,
}

#[pinned_drop]
impl PinnedDrop for RtIpcFile {
    fn drop(self: Pin<&mut Self>) {
        let (endpoints, connections) = {
            let mut owned = self.owned.lock();
            (
                core::mem::replace(&mut owned.endpoints, KVec::new()),
                core::mem::replace(&mut owned.connections, KVec::new()),
            )
        };

        let mut tables = TABLES.lock();
        for handle in connections {
            clear_slot(&mut tables.connections, handle);
        }
        for handle in endpoints {
            clear_slot(&mut tables.endpoints, handle);
        }
    }
}

/// The `/dev/rt_ipc_rust` control device.
struct RtIpcDevice;

#[vtable]
impl MiscDevice for RtIpcDevice {
    type Ptr = Pin<KBox<RtIpcFile>>;

    fn open(_file: &File, _misc: &MiscDeviceRegistration<Self>) -> Result<Pin<KBox<RtIpcFile>>> {
        KBox::try_pin_init(
            try_pin_init! {
                RtIpcFile {
                    owned <- new_spinlock!(OwnedHandles {
                        endpoints: KVec::new(),
                        connections: KVec::new(),
                    }),
                }
            },
            GFP_KERNEL,
        )
    }

    fn ioctl(me: Pin<&RtIpcFile>, file: &File, cmd: u32, arg: usize) -> Result<isize> {
        let uarg = UserPtr::from_addr(arg);
        let size = _IOC_SIZE(cmd);

        match cmd {
            RT_IPC_RUST_GET_VERSION => {
                let mut writer = UserSlice::new(uarg, size).writer();
                writer.write::<u32>(&RT_IPC_RUST_ABI_VERSION)?;
                Ok(0)
            }
            RT_IPC_RUST_ENDPOINT_CREATE => me.endpoint_create(file, uarg, size),
            RT_IPC_RUST_ENDPOINT_CONNECT => me.endpoint_connect(uarg, size),
            RT_IPC_RUST_CALL => me.call(uarg, size),
            _ => Err(ENOTTY),
        }
    }
}

impl RtIpcFile {
    fn record_endpoint(&self, handle: u64) -> Result {
        self.owned.lock().endpoints.push(handle, GFP_KERNEL)?;
        Ok(())
    }

    fn record_connection(&self, handle: u64) -> Result {
        self.owned.lock().connections.push(handle, GFP_KERNEL)?;
        Ok(())
    }

    /// Handle `RT_IPC_RUST_ENDPOINT_CREATE`.
    fn endpoint_create(&self, file: &File, arg: UserPtr, size: usize) -> Result<isize> {
        let mut reader = UserSlice::new(arg, size).reader();
        let req = reader.read::<EndpointCreate>()?;

        if req.size as usize != core::mem::size_of::<EndpointCreate>() {
            return Err(EINVAL);
        }
        if req.flags & !RT_IPC_RUST_EP_FLAGS_ALL != 0 {
            return Err(EINVAL);
        }
        if req.reserved != 0 {
            return Err(EINVAL);
        }
        validate_entry(&req)?;

        // Capture the owner's credentials from the opening task, mirroring
        // `get_current_cred()` at registration time in the C version.
        let owner_cred = ARef::from(file.cred());

        // Allocate the stable tracing id up front so the object is immutable.
        let id = TABLES.lock().alloc_id();

        let endpoint = Arc::new(
            Endpoint {
                id,
                owner_cred,
                entry: req.entry,
                stack_top: req.stack_top,
                stack_size: req.stack_size,
                max_concurrency: req.max_concurrency,
                flags: req.flags,
            },
            GFP_KERNEL,
        )?;

        let handle = {
            let mut tables = TABLES.lock();
            let slot = EndpointSlot {
                endpoint,
                inflight: 0,
                dead: false,
            };
            Tables::insert(&mut tables.endpoints, slot)?
        };

        if let Err(e) = self.record_endpoint(handle) {
            // Roll back the table insertion on bookkeeping failure.
            clear_slot(&mut TABLES.lock().endpoints, handle);
            return Err(e);
        }

        Ok(handle as isize)
    }

    /// Handle `RT_IPC_RUST_ENDPOINT_CONNECT`.
    fn endpoint_connect(&self, arg: UserPtr, size: usize) -> Result<isize> {
        let mut reader = UserSlice::new(arg, size).reader();
        let req = reader.read::<Connect>()?;

        if req.size as usize != core::mem::size_of::<Connect>() {
            return Err(EINVAL);
        }
        if req.flags & !RT_IPC_RUST_CONN_FLAGS_ALL != 0 {
            return Err(EINVAL);
        }
        if req.reserved != 0 || req.pad != 0 {
            return Err(EINVAL);
        }

        // Look up the endpoint and clone its `Arc` while holding the lock, then
        // build the connection object.
        let (endpoint, id) = {
            let mut tables = TABLES.lock();
            let slot = tables.endpoint_slot_mut(req.endpoint).ok_or(EINVAL)?;
            if slot.dead {
                return Err(ECONNREFUSED);
            }
            let endpoint = slot.endpoint.clone();
            let id = tables.alloc_id();
            (endpoint, id)
        };

        let connection = Arc::new(
            Connection {
                id,
                endpoint_handle: req.endpoint,
                endpoint,
            },
            GFP_KERNEL,
        )?;

        let handle = {
            let mut tables = TABLES.lock();
            let conn_slot = ConnectionSlot { connection };
            Tables::insert(&mut tables.connections, conn_slot)?
        };

        if let Err(e) = self.record_connection(handle) {
            clear_slot(&mut TABLES.lock().connections, handle);
            return Err(e);
        }

        Ok(handle as isize)
    }

    /// Handle `RT_IPC_RUST_CALL`: perform one synchronous migrating-thread RPC.
    fn call(&self, arg: UserPtr, size: usize) -> Result<isize> {
        let (mut reader, mut writer) = UserSlice::new(arg, size).reader_writer();
        let mut req = reader.read::<Call>()?;

        if req.size as usize != core::mem::size_of::<Call>() {
            return Err(EINVAL);
        }
        if req.flags & !RT_IPC_RUST_CALL_FLAGS_ALL != 0 {
            return Err(EINVAL);
        }
        if req.reserved != 0 {
            return Err(EINVAL);
        }
        if req.send_len > RT_IPC_RUST_MAX_PAYLOAD || req.recv_len > RT_IPC_RUST_MAX_PAYLOAD {
            return Err(EMSGSIZE);
        }

        // Reserve an in-flight slot on the target endpoint, taking an `Arc`
        // clone of the connection so the objects stay alive for the whole call
        // even if userspace closes their handles concurrently.
        let (conn, endpoint_handle) = {
            let tables = TABLES.lock();
            let slot = tables.connection_slot(req.connection).ok_or(EINVAL)?;
            (slot.connection.clone(), slot.connection.endpoint_handle)
        };

        {
            let mut tables = TABLES.lock();
            let slot = tables
                .endpoint_slot_mut(endpoint_handle)
                .ok_or(ECONNRESET)?;
            if slot.dead {
                return Err(ECONNRESET);
            }
            if slot.inflight >= slot.endpoint.max_concurrency {
                return Err(EAGAIN);
            }
            slot.inflight += 1;
        }

        // From here on we must release the slot on every exit path.
        let ret = self.run_call(&conn, &mut req);

        {
            let mut tables = TABLES.lock();
            if let Some(slot) = tables.endpoint_slot_mut(endpoint_handle) {
                if slot.inflight > 0 {
                    slot.inflight -= 1;
                }
            }
        }

        let out = ret?;

        // Copy the (possibly updated) request structure back so userspace can
        // read `out_recv_len`.
        writer.write::<Call>(&req)?;
        Ok(out)
    }

    /// The part of the call that runs with the slot reserved but no lock held.
    ///
    /// Copies the request out of the client address space into a bounded kernel
    /// bounce buffer, then -- because the architecture-specific partial context
    /// switch into the server is not implemented (parity with the C arch hooks
    /// returning `-EOPNOTSUPP`) -- reports `EOPNOTSUPP` without exposing any
    /// half-migrated state.  `_conn` carries the endpoint credentials that the
    /// server dispatch will run under once it is wired up.
    fn run_call(&self, _conn: &Arc<Connection>, req: &mut Call) -> Result<isize> {
        if req.send_len != 0 {
            let mut bounce: KVec<u8> = KVec::new();
            let len = usize::try_from(req.send_len).map_err(|_| EMSGSIZE)?;
            let send_ptr = UserPtr::from_addr(usize::try_from(req.send_buf).map_err(|_| EFAULT)?);
            UserSlice::new(send_ptr, len).read_all(&mut bounce, GFP_KERNEL)?;
            // The bounce buffer now holds the request; the server dispatch that
            // consumes it is a follow-up milestone.
        }

        req.out_recv_len = 0;

        // No partial context switch is performed: refuse cleanly.
        Err(EOPNOTSUPP)
    }
}

/// Validate a user-provided server entry description.
///
/// The entry and the whole stack region must be non-zero user addresses and
/// the stack region must not underflow.  Unlike the C version we do not have
/// `TASK_SIZE` available to a module, so the upper-bound user/kernel split is
/// not enforced here; it is documented as a limitation of the port.
fn validate_entry(req: &EndpointCreate) -> Result {
    if req.entry == 0 || req.stack_top == 0 || req.stack_size == 0 {
        return Err(EINVAL);
    }
    if req.max_concurrency == 0 {
        return Err(EINVAL);
    }
    // Stack region [stack_top - stack_size, stack_top) must not underflow.
    req.stack_top.checked_sub(req.stack_size).ok_or(EINVAL)?;
    Ok(())
}

// --- Module ---------------------------------------------------------------

#[pin_data]
struct RtIpcRustModule {
    #[pin]
    _miscdev: MiscDeviceRegistration<RtIpcDevice>,
}

impl kernel::InPlaceModule for RtIpcRustModule {
    fn init(_module: &'static ThisModule) -> impl PinInit<Self, Error> {
        // SAFETY: Called exactly once, before any use of the global table.
        unsafe { TABLES.init() };

        pr_info!(
            "rt_ipc_rust: migrating-thread IPC registered (ABI v{})\n",
            RT_IPC_RUST_ABI_VERSION
        );

        let options = MiscDeviceOptions {
            name: c"rt_ipc_rust",
        };

        try_pin_init!(Self {
            _miscdev <- MiscDeviceRegistration::register(options),
        })
    }
}

// SPDX-License-Identifier: GPL-2.0

//! rt_ipc — real-time IPC based on the migrating-thread model.
//!
//! This is the kernel adaptation layer.  The portable, unit-tested policy —
//! the endpoint registry, argument validation and invocation-depth guard —
//! lives in [`rt_ipc_core`].  This file wires that core to the kernel:
//!
//! * it owns the single global [`rt_ipc_core::Registry`], protected by a
//!   spinlock, and
//! * it exposes C-ABI entry points that the architecture syscall stubs
//!   (`ipc/rt_ipc/rt_ipc_syscall.c`) call.
//!
//! ## Why a thin C/asm boundary remains
//!
//! Adding syscall-table entries and performing the *partial context switch*
//! that defines thread migration — switching the address space (CR3) and a
//! subset of CPU state (user stack pointer, instruction pointer) without
//! switching threads, priorities, or invoking the scheduler — is inherently
//! architecture specific and cannot be expressed in safe Rust.  That code is
//! isolated in `rt_ipc_syscall.c` and `arch/x86/kernel/rt_ipc_switch.S`.
//! Everything with non-trivial *logic* is in Rust and is testable.
//!
//! ## Data path (rt_ipc_invoke)
//!
//! 1. C stub copies the request into a per-task bounce buffer and calls
//!    [`rt_ipc_rs_lookup_owner`] to resolve the endpoint's server task.
//! 2. The arch layer saves the caller's user context, switches to the
//!    server's address space and installs the server's entry point and stack.
//! 3. The server handler runs *on the client's thread*, so it inherits the
//!    client's priority and scheduling attributes — no scheduler involvement,
//!    no priority inversion.
//! 4. `rt_ipc_return` copies the reply back and the arch layer restores the
//!    caller's context.

// The verified policy core, shared verbatim with the standalone unit tests.
#[path = "rt_ipc_core.rs"]
mod rt_ipc_core;

use core::ptr::NonNull;
use core::sync::atomic::{AtomicPtr, Ordering};
use kernel::prelude::*;
use kernel::sync::{new_spinlock, Arc, SpinLock};

use rt_ipc_core::{CoreError, Registry};

module! {
    type: RtIpcModule,
    name: "rt_ipc",
    authors: ["XingxingQiao <mnlife@126.com>"],
    description: "Real-time IPC (migrating-thread model)",
    license: "GPL",
}

/// The one global registry, shared between the module object (which owns it)
/// and the C-ABI entry points (which reach it through [`REGISTRY`]).
type SharedRegistry = Arc<SpinLock<Registry>>;

/// Raw, non-owning handle used by the C-ABI entry points.  Set once during
/// [`RtIpcModule::init`] and cleared on drop; only ever dereferenced while the
/// module is loaded.
static REGISTRY: AtomicPtr<SpinLock<Registry>> = AtomicPtr::new(core::ptr::null_mut());

struct RtIpcModule {
    _registry: SharedRegistry,
}

impl kernel::Module for RtIpcModule {
    fn init(_module: &'static ThisModule) -> Result<Self> {
        pr_info!("rt_ipc: migrating-thread IPC initialising\n");

        let registry: SharedRegistry = Arc::pin_init(
            new_spinlock!(Registry::new(), "rt_ipc::registry"),
            GFP_KERNEL,
        )?;

        // Publish the registry pointer for the C entry points.  The `Arc` in
        // `_registry` keeps it alive for the module's lifetime.
        REGISTRY.store(registry.as_ptr_mut(), Ordering::Release);

        Ok(RtIpcModule {
            _registry: registry,
        })
    }
}

impl Drop for RtIpcModule {
    fn drop(&mut self) {
        REGISTRY.store(core::ptr::null_mut(), Ordering::Release);
        pr_info!("rt_ipc: unloaded\n");
    }
}

/// Run `f` with the locked global registry, or return `-ENODEV` (as a negative
/// errno) if the module is not loaded.
fn with_registry<F, R>(f: F) -> core::result::Result<R, i32>
where
    F: FnOnce(&mut Registry) -> core::result::Result<R, CoreError>,
{
    let ptr = REGISTRY.load(Ordering::Acquire);
    let reg = NonNull::new(ptr).ok_or(19 /* ENODEV */)?;
    // SAFETY: while `ptr` is non-null the owning `Arc` in the live module keeps
    // the `SpinLock<Registry>` allocated; the spinlock provides the required
    // synchronisation for concurrent syscalls.
    let lock = unsafe { reg.as_ref() };
    let mut guard = lock.lock();
    f(&mut guard).map_err(CoreError::to_errno)
}

// ---------------------------------------------------------------------------
// C-ABI entry points, called from ipc/rt_ipc/rt_ipc_syscall.c
// ---------------------------------------------------------------------------

/// Register endpoint `name` (already copied into a kernel buffer of `name_len`
/// bytes) as owned by `owner`.  Returns the endpoint id, or a negative errno.
///
/// # Safety
/// `name` must point to `name_len` initialised, readable bytes.
#[no_mangle]
pub unsafe extern "C" fn rt_ipc_rs_register(name: *const u8, name_len: usize, owner: u64) -> i64 {
    if name.is_null() && name_len != 0 {
        return -(14i64); // EFAULT
    }
    // SAFETY: the caller guarantees `name`/`name_len` describe a valid slice.
    let bytes = unsafe { core::slice::from_raw_parts(name, name_len) };
    match with_registry(|reg| reg.register(bytes, owner)) {
        Ok(id) => id as i64,
        Err(errno) => -(errno as i64),
    }
}

/// Resolve endpoint `id` to its owning server task token, or a negative errno.
#[no_mangle]
pub extern "C" fn rt_ipc_rs_lookup_owner(id: u64) -> i64 {
    match with_registry(|reg| reg.lookup(id)) {
        Ok(ep) => ep.owner as i64,
        Err(errno) => -(errno as i64),
    }
}

/// Unregister endpoint `id` if owned by `owner`.  Returns 0 or a negative errno.
#[no_mangle]
pub extern "C" fn rt_ipc_rs_unregister(id: u64, owner: u64) -> i64 {
    match with_registry(|reg| reg.unregister(id, owner)) {
        Ok(()) => 0,
        Err(errno) => -(errno as i64),
    }
}

/// Reclaim every endpoint owned by `owner`.  Called from the task-exit hook.
/// Returns the number of endpoints reclaimed (never fails).
#[no_mangle]
pub extern "C" fn rt_ipc_rs_reclaim_owner(owner: u64) -> i64 {
    match with_registry(|reg| Ok::<usize, CoreError>(reg.reclaim_owner(owner))) {
        Ok(n) => n as i64,
        Err(_) => 0,
    }
}

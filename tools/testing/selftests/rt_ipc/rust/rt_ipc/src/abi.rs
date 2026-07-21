// SPDX-License-Identifier: GPL-2.0

//! Wire-level ABI shared between the rt_ipc kernel module and userspace.
//!
//! The constants here mirror `ipc/rt_ipc/abi.rs` on the kernel side.  Keeping
//! them in one small, well-documented module makes it trivial to audit the
//! contract between client, server and kernel.

/// Syscall numbers, matching `arch/x86/entry/syscalls/syscall_64.tbl`.
///
/// The migrating-thread model needs three primitives:
///
/// * [`SYS_RT_IPC_REGISTER`] — a server publishes a named endpoint.
/// * [`SYS_RT_IPC_INVOKE`] — a client performs an RPC, migrating into the
///   server address space to run the handler with the *client's* scheduling
///   attributes (this is what avoids the priority inversion that plagues the
///   static two-thread model and proxy-execution designs).
/// * [`SYS_RT_IPC_RETURN`] — a server publishes its reply and blocks to
///   receive the next request (the classic "reply-and-receive" fast path).
pub const SYS_RT_IPC_REGISTER: i64 = 472;
pub const SYS_RT_IPC_INVOKE: i64 = 473;
pub const SYS_RT_IPC_RETURN: i64 = 474;

/// Maximum length, in bytes, of an endpoint name (excluding the NUL byte).
pub const RT_IPC_NAME_MAX: usize = 63;

/// Maximum payload carried in a single request or reply.
///
/// The migrating-thread fast path copies the payload through a small per-task
/// bounce buffer, so the bound is deliberately modest and matches the kernel
/// side.  Larger transfers are expected to use shared memory referenced from
/// within the payload.
pub const RT_IPC_MSG_MAX: usize = 4096;

/// Invalid / unset endpoint identifier.
pub const RT_IPC_ENDPOINT_INVALID: u64 = u64::MAX;

/// Negated `errno` values returned by the syscalls, exposed as positive
/// constants for convenience.  The kernel returns `-errno`; the raw syscall
/// wrappers normalise this into [`crate::Error`].
pub mod errno {
    pub const EPERM: i32 = 1;
    pub const ESRCH: i32 = 3;
    pub const EINTR: i32 = 4;
    pub const EAGAIN: i32 = 11;
    pub const EFAULT: i32 = 14;
    pub const EBUSY: i32 = 16;
    pub const EEXIST: i32 = 17;
    pub const EINVAL: i32 = 22;
    pub const ENOSPC: i32 = 28;
    pub const ENOSYS: i32 = 38;
    pub const ENAMETOOLONG: i32 = 36;
    pub const EMSGSIZE: i32 = 90;
    pub const ECONNREFUSED: i32 = 111;
}

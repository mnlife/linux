// SPDX-License-Identifier: GPL-2.0

//! Thin, `unsafe` wrappers over the three rt_ipc syscalls.
//!
//! These do nothing but marshal arguments into registers and issue the
//! `syscall` instruction.  All policy (validation, framing, fallback) lives in
//! higher layers.  On architectures other than x86_64 the wrappers return
//! `-ENOSYS`, which the transport layer treats as "kernel support absent" and
//! transparently falls back to the socket reference transport.

use crate::abi;

/// Issue a raw 5-argument Linux syscall on x86_64.
///
/// # Safety
///
/// The caller must ensure the pointer arguments are valid for the access the
/// specific syscall performs, for the whole duration of the call.
#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn syscall5(nr: i64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64) -> i64 {
    let ret: i64;
    // rax = nr; args in rdi, rsi, rdx, r10, r8.  rcx and r11 are clobbered by
    // the `syscall` instruction itself.
    core::arch::asm!(
        "syscall",
        inlateout("rax") nr => ret,
        in("rdi") a1,
        in("rsi") a2,
        in("rdx") a3,
        in("r10") a4,
        in("r8") a5,
        lateout("rcx") _,
        lateout("r11") _,
        options(nostack),
    );
    ret
}

#[cfg(not(target_arch = "x86_64"))]
#[inline]
unsafe fn syscall5(_nr: i64, _a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64) -> i64 {
    -(abi::errno::ENOSYS as i64)
}

/// `rt_ipc_register(name, name_len)` — publish a named server endpoint.
///
/// Returns the endpoint id (`>= 0`) or `-errno`.
///
/// # Safety
/// `name` must point to at least `name_len` readable bytes.
pub unsafe fn rt_ipc_register(name: *const u8, name_len: usize) -> i64 {
    syscall5(
        abi::SYS_RT_IPC_REGISTER,
        name as u64,
        name_len as u64,
        0,
        0,
        0,
    )
}

/// `rt_ipc_invoke(endpoint, req, req_len, resp, resp_cap)` — client RPC.
///
/// Returns the number of reply bytes written to `resp` (`>= 0`) or `-errno`.
///
/// # Safety
/// `req` must be readable for `req_len` bytes and `resp` writable for
/// `resp_cap` bytes.
pub unsafe fn rt_ipc_invoke(
    endpoint: u64,
    req: *const u8,
    req_len: usize,
    resp: *mut u8,
    resp_cap: usize,
) -> i64 {
    syscall5(
        abi::SYS_RT_IPC_INVOKE,
        endpoint,
        req as u64,
        req_len as u64,
        resp as u64,
        resp_cap as u64,
    )
}

/// `rt_ipc_return(endpoint, resp, resp_len, req, req_cap)` —
/// reply to the current client (if any) and block to receive the next request.
///
/// Returns the number of request bytes written to `req` (`>= 0`) or `-errno`.
///
/// # Safety
/// `resp` must be readable for `resp_len` bytes and `req` writable for
/// `req_cap` bytes.
pub unsafe fn rt_ipc_return(
    endpoint: u64,
    resp: *const u8,
    resp_len: usize,
    req: *mut u8,
    req_cap: usize,
) -> i64 {
    syscall5(
        abi::SYS_RT_IPC_RETURN,
        endpoint,
        resp as u64,
        resp_len as u64,
        req as u64,
        req_cap as u64,
    )
}

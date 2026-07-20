// SPDX-License-Identifier: GPL-2.0
//! Selftests for rt_ipc_rust (Rust reimplementation of the migrating-thread IPC).
//!
//! This is the Rust counterpart of the subsystem it exercises, so the test is
//! written in Rust too.  It drives the `/dev/rt_ipc_rust` control device and
//! its handle-based ioctl uAPI: version query, endpoint creation and argument
//! validation, connection setup and the synchronous call path.
//!
//! The user-mode server dispatch is a follow-up milestone; until it lands a
//! well-formed RT_IPC_RUST_CALL is expected to be refused with EOPNOTSUPP.
//!
//! Output follows the kselftest KTAP format so the program can be run directly
//! or under the kselftest runner.  When `/dev/rt_ipc_rust` is unavailable every
//! test reports as skipped, mirroring the C harness `SKIP` behaviour.
#![allow(non_camel_case_types)]
#![allow(non_upper_case_globals)]

use std::mem::size_of;
use std::process::ExitCode;
use std::ptr::addr_of;

// Minimal libc surface.  std already links against the system C library, so we
// only need to declare the few calls used to talk to the control device.  The
// C integer widths are taken from core::ffi so the ioctl request argument
// matches the platform's `unsigned long` (e.g. 32-bit on 32-bit arm).
use core::ffi::{c_char, c_int, c_ulong, c_void};

extern "C" {
    fn open(path: *const c_char, flags: c_int) -> c_int;
    fn close(fd: c_int) -> c_int;
    fn ioctl(fd: c_int, request: c_ulong, arg: *mut c_void) -> c_int;
}

const O_RDWR: c_int = 0o2;
const O_CLOEXEC: c_int = 0o2000000;

const EINVAL: i32 = 22;
const EMSGSIZE: i32 = 90;
const EOPNOTSUPP: i32 = 95;

const RT_IPC_RUST_DEV: &[u8] = b"/dev/rt_ipc_rust\0";

// --- ioctl encoding (asm-generic, shared by x86/arm/arm64/riscv) -----------

const _IOC_NRBITS: u32 = 8;
const _IOC_TYPEBITS: u32 = 8;
const _IOC_SIZEBITS: u32 = 14;
const _IOC_NRSHIFT: u32 = 0;
const _IOC_TYPESHIFT: u32 = _IOC_NRSHIFT + _IOC_NRBITS;
const _IOC_SIZESHIFT: u32 = _IOC_TYPESHIFT + _IOC_TYPEBITS;
const _IOC_DIRSHIFT: u32 = _IOC_SIZESHIFT + _IOC_SIZEBITS;

const _IOC_WRITE: u32 = 1;
const _IOC_READ: u32 = 2;

const fn ioc(dir: u32, ty: u32, nr: u32, size: usize) -> c_ulong {
    ((dir << _IOC_DIRSHIFT)
        | (ty << _IOC_TYPESHIFT)
        | (nr << _IOC_NRSHIFT)
        | ((size as u32) << _IOC_SIZESHIFT)) as c_ulong
}

// --- uAPI mirror of include/uapi/linux/rt_ipc_rust.h -----------------------

const RT_IPC_RUST_IOC: u32 = b'9' as u32;
const RT_IPC_RUST_ABI_VERSION: u32 = 1;

const RT_IPC_RUST_EP_SERVER_CREDS: u32 = 1 << 1;

#[repr(C)]
#[derive(Default)]
struct EndpointCreate {
    size: u32,
    flags: u32,
    entry: u64,
    stack_top: u64,
    stack_size: u64,
    max_concurrency: u32,
    __reserved: u32,
}

#[repr(C)]
#[derive(Default)]
struct Connect {
    size: u32,
    flags: u32,
    endpoint: u64,
    __reserved: u32,
    __pad: u32,
}

#[repr(C)]
#[derive(Default)]
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
    __reserved: u32,
}

fn get_version_ioc() -> c_ulong {
    ioc(_IOC_READ, RT_IPC_RUST_IOC, 0x10, size_of::<u32>())
}
fn endpoint_create_ioc() -> c_ulong {
    ioc(
        _IOC_WRITE,
        RT_IPC_RUST_IOC,
        0x11,
        size_of::<EndpointCreate>(),
    )
}
fn endpoint_connect_ioc() -> c_ulong {
    ioc(_IOC_WRITE, RT_IPC_RUST_IOC, 0x12, size_of::<Connect>())
}
fn call_ioc() -> c_ulong {
    ioc(
        _IOC_READ | _IOC_WRITE,
        RT_IPC_RUST_IOC,
        0x13,
        size_of::<Call>(),
    )
}

// A dummy server entry address used only to supply a valid function pointer to
// the ENDPOINT_CREATE ioctl. It is intentionally empty and never actually
// invoked, because the call path returns -EOPNOTSUPP in the foundation (the
// partial context switch that would jump here is not yet wired up).
extern "C" fn server_entry() {}

// Backing store for the per-call server stack region handed to the endpoint.
// Only its address and size are passed to the kernel; the bytes are never
// touched here (the call path returns EOPNOTSUPP before any switch), so an
// immutable static is sufficient and avoids `static mut`.
static SERVER_STACK: [u8; 64 * 1024] = [0; 64 * 1024];

/// Outcome of a single test, mirroring the kselftest PASS/FAIL/SKIP states.
enum Outcome {
    Pass,
    Fail(String),
    Skip(String),
}

fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// Open the control device, or return a Skip outcome when it is unavailable.
fn open_dev() -> Result<c_int, Outcome> {
    let fd = unsafe {
        open(
            RT_IPC_RUST_DEV.as_ptr() as *const c_char,
            O_RDWR | O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(Outcome::Skip(format!(
            "/dev/rt_ipc_rust unavailable (errno={})",
            errno()
        )));
    }
    Ok(fd)
}

fn close_dev(fd: c_int) {
    unsafe {
        close(fd);
    }
}

fn fill_ep_req() -> EndpointCreate {
    let base = addr_of!(SERVER_STACK) as u64;
    let len = size_of::<[u8; 64 * 1024]>() as u64;
    EndpointCreate {
        size: size_of::<EndpointCreate>() as u32,
        flags: RT_IPC_RUST_EP_SERVER_CREDS,
        entry: server_entry as *const () as u64,
        stack_top: base + len,
        stack_size: len,
        max_concurrency: 4,
        ..Default::default()
    }
}

/// Create an endpoint and return its handle (> 0) or the raw ioctl return.
fn create_endpoint(dev: c_int) -> i64 {
    let mut req = fill_ep_req();
    unsafe {
        ioctl(
            dev,
            endpoint_create_ioc(),
            &mut req as *mut _ as *mut c_void,
        ) as i64
    }
}

/// Connect to an endpoint handle and return the connection handle (> 0).
fn connect_endpoint(dev: c_int, endpoint: i64) -> i64 {
    let mut req = Connect {
        size: size_of::<Connect>() as u32,
        endpoint: endpoint as u64,
        ..Default::default()
    };
    unsafe {
        ioctl(
            dev,
            endpoint_connect_ioc(),
            &mut req as *mut _ as *mut c_void,
        ) as i64
    }
}

// --- Assertion helpers -----------------------------------------------------

macro_rules! expect_eq {
    ($want:expr, $got:expr) => {{
        let (want, got) = ($want, $got);
        if want != got {
            return Outcome::Fail(format!(
                "{}:{}: expected {:?} == {:?}",
                file!(),
                line!(),
                want,
                got
            ));
        }
    }};
}

macro_rules! expect_gt {
    ($got:expr, $bound:expr) => {{
        let (got, bound) = ($got, $bound);
        if !(got > bound) {
            return Outcome::Fail(format!(
                "{}:{}: expected {:?} > {:?}",
                file!(),
                line!(),
                got,
                bound
            ));
        }
    }};
}

macro_rules! test_dev {
    ($name:ident, |$dev:ident| $body:block) => {
        fn $name() -> Outcome {
            let $dev = match open_dev() {
                Ok(fd) => fd,
                Err(skip) => return skip,
            };
            let outcome = (|| $body)();
            close_dev($dev);
            outcome
        }
    };
}

// --- Test cases (parity with rt_ipc_rust_test.c) ---------------------------

test_dev!(version, |dev| {
    let mut version: u32 = 0;
    let ret = unsafe {
        ioctl(
            dev,
            get_version_ioc(),
            &mut version as *mut _ as *mut c_void,
        )
    };
    expect_eq!(0, ret);
    expect_eq!(RT_IPC_RUST_ABI_VERSION, version);
    Outcome::Pass
});

test_dev!(endpoint_create_ok, |dev| {
    let handle = create_endpoint(dev);
    expect_gt!(handle, 0);
    Outcome::Pass
});

test_dev!(endpoint_create_rejects_bad_size, |dev| {
    let mut req = fill_ep_req();
    req.size = 1;
    let ret = unsafe {
        ioctl(
            dev,
            endpoint_create_ioc(),
            &mut req as *mut _ as *mut c_void,
        )
    };
    expect_eq!(-1, ret);
    expect_eq!(EINVAL, errno());
    Outcome::Pass
});

test_dev!(endpoint_create_rejects_bad_flags, |dev| {
    let mut req = fill_ep_req();
    req.flags = 0xffffffff;
    let ret = unsafe {
        ioctl(
            dev,
            endpoint_create_ioc(),
            &mut req as *mut _ as *mut c_void,
        )
    };
    expect_eq!(-1, ret);
    expect_eq!(EINVAL, errno());
    Outcome::Pass
});

test_dev!(endpoint_create_rejects_zero_entry, |dev| {
    let mut req = fill_ep_req();
    req.entry = 0;
    let ret = unsafe {
        ioctl(
            dev,
            endpoint_create_ioc(),
            &mut req as *mut _ as *mut c_void,
        )
    };
    expect_eq!(-1, ret);
    expect_eq!(EINVAL, errno());
    Outcome::Pass
});

test_dev!(connect_ok, |dev| {
    let endpoint = create_endpoint(dev);
    expect_gt!(endpoint, 0);
    let conn = connect_endpoint(dev, endpoint);
    expect_gt!(conn, 0);
    Outcome::Pass
});

test_dev!(connect_rejects_bad_handle, |dev| {
    expect_eq!(-1, connect_endpoint(dev, 999999));
    expect_eq!(EINVAL, errno());
    Outcome::Pass
});

test_dev!(call_reports_unsupported, |dev| {
    let endpoint = create_endpoint(dev);
    expect_gt!(endpoint, 0);
    let conn = connect_endpoint(dev, endpoint);
    expect_gt!(conn, 0);

    let mut call = Call {
        size: size_of::<Call>() as u32,
        connection: conn as u64,
        timeout_ms: -1,
        ..Default::default()
    };
    // The partial context switch into the server is not wired up yet, so a
    // well-formed call is refused cleanly with EOPNOTSUPP.
    let ret = unsafe { ioctl(dev, call_ioc(), &mut call as *mut _ as *mut c_void) };
    expect_eq!(-1, ret);
    expect_eq!(EOPNOTSUPP, errno());
    Outcome::Pass
});

test_dev!(call_rejects_oversized_payload, |dev| {
    let endpoint = create_endpoint(dev);
    expect_gt!(endpoint, 0);
    let conn = connect_endpoint(dev, endpoint);
    expect_gt!(conn, 0);

    let mut call = Call {
        size: size_of::<Call>() as u32,
        connection: conn as u64,
        send_len: 1024 * 1024, // exceeds the 64 KiB bound
        ..Default::default()
    };
    let ret = unsafe { ioctl(dev, call_ioc(), &mut call as *mut _ as *mut c_void) };
    expect_eq!(-1, ret);
    expect_eq!(EMSGSIZE, errno());
    Outcome::Pass
});

fn main() -> ExitCode {
    let tests: &[(&str, fn() -> Outcome)] = &[
        ("version", version),
        ("endpoint_create_ok", endpoint_create_ok),
        (
            "endpoint_create_rejects_bad_size",
            endpoint_create_rejects_bad_size,
        ),
        (
            "endpoint_create_rejects_bad_flags",
            endpoint_create_rejects_bad_flags,
        ),
        (
            "endpoint_create_rejects_zero_entry",
            endpoint_create_rejects_zero_entry,
        ),
        ("connect_ok", connect_ok),
        ("connect_rejects_bad_handle", connect_rejects_bad_handle),
        ("call_reports_unsupported", call_reports_unsupported),
        (
            "call_rejects_oversized_payload",
            call_rejects_oversized_payload,
        ),
    ];

    println!("TAP version 13");
    println!("1..{}", tests.len());

    let mut failed = 0;
    for (i, (name, func)) in tests.iter().enumerate() {
        let n = i + 1;
        match func() {
            Outcome::Pass => println!("ok {n} {name}"),
            Outcome::Skip(reason) => println!("ok {n} {name} # SKIP {reason}"),
            Outcome::Fail(reason) => {
                failed += 1;
                println!("not ok {n} {name}");
                for line in reason.lines() {
                    println!("# {line}");
                }
            }
        }
    }

    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

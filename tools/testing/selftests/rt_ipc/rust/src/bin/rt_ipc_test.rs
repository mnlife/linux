// SPDX-License-Identifier: GPL-2.0
//! Rust selftest runner for rt_ipc, mirroring
//! `tools/testing/selftests/rt_ipc/rt_ipc_test.c`.
//!
//! Emits KTAP/TAP-13 output so it slots into the kselftest framework, and
//! SKIPs cleanly when `/dev/rt_ipc` is unavailable (module not loaded or
//! insufficient privilege).  As with the C harness, the migrating-thread user
//! dispatch is an architecture follow-up, so a well-formed call is accepted
//! either as a clean zero-reply completion or an `EOPNOTSUPP` refusal.

use std::os::unix::io::AsRawFd;
use std::process::ExitCode;

use rt_ipc::{raw, Device, EndpointConfig, ABI_VERSION};

const EINVAL: i32 = 22;
const EMSGSIZE: i32 = 90;
const EOPNOTSUPP: i32 = 95;

extern "C" fn server_entry() {}

/// One test: returns Ok(()) on pass, Err(msg) on failure, or a skip reason.
type TestResult = std::result::Result<Outcome, String>;

enum Outcome {
    Pass,
    Skip(String),
}

fn open_dev() -> std::result::Result<Device, Outcome> {
    Device::open().map_err(|e| Outcome::Skip(format!("{} unavailable: {e}", rt_ipc::DEVICE_PATH)))
}

fn make_endpoint(dev: &Device) -> std::result::Result<(rt_ipc::Endpoint, Vec<u8>), String> {
    let mut stack = vec![0u8; 64 * 1024];
    let cfg =
        EndpointConfig::new(server_entry as *const () as usize, &mut stack).max_concurrency(4);
    let ep = dev
        .create_endpoint(&cfg)
        .map_err(|e| format!("endpoint create failed: {e}"))?;
    Ok((ep, stack))
}

fn errno_of(e: &std::io::Error) -> i32 {
    e.raw_os_error().unwrap_or(0)
}

fn test_version() -> TestResult {
    let dev = match open_dev() {
        Ok(d) => d,
        Err(o) => return Ok(o),
    };
    let v = dev.abi_version().map_err(|e| format!("{e}"))?;
    if v != ABI_VERSION {
        return Err(format!("ABI version {v} != {ABI_VERSION}"));
    }
    Ok(Outcome::Pass)
}

fn test_endpoint_bad_size() -> TestResult {
    let dev = match open_dev() {
        Ok(d) => d,
        Err(o) => return Ok(o),
    };
    let mut stack = vec![0u8; 64 * 1024];
    let top = stack.as_mut_ptr() as u64 + stack.len() as u64;
    // size = 1 => EINVAL
    match raw::endpoint_create(
        &dev,
        1,
        rt_ipc::EP_SERVER_CREDS,
        server_entry as *const () as u64,
        top,
        stack.len() as u64,
        4,
    ) {
        Ok(_) => Err("bad size unexpectedly accepted".into()),
        Err(e) if errno_of(&e) == EINVAL => Ok(Outcome::Pass),
        Err(e) => Err(format!("expected EINVAL, got {}", errno_of(&e))),
    }
}

fn test_endpoint_bad_flags() -> TestResult {
    let dev = match open_dev() {
        Ok(d) => d,
        Err(o) => return Ok(o),
    };
    let mut stack = vec![0u8; 64 * 1024];
    let top = stack.as_mut_ptr() as u64 + stack.len() as u64;
    match raw::endpoint_create(
        &dev,
        raw::ENDPOINT_CREATE_SIZE,
        0x8000,
        server_entry as *const () as u64,
        top,
        stack.len() as u64,
        4,
    ) {
        Ok(_) => Err("bad flags unexpectedly accepted".into()),
        Err(e) if errno_of(&e) == EINVAL => Ok(Outcome::Pass),
        Err(e) => Err(format!("expected EINVAL, got {}", errno_of(&e))),
    }
}

fn test_endpoint_zero_concurrency() -> TestResult {
    let dev = match open_dev() {
        Ok(d) => d,
        Err(o) => return Ok(o),
    };
    let mut stack = vec![0u8; 64 * 1024];
    let top = stack.as_mut_ptr() as u64 + stack.len() as u64;
    match raw::endpoint_create(
        &dev,
        raw::ENDPOINT_CREATE_SIZE,
        rt_ipc::EP_SERVER_CREDS,
        server_entry as *const () as u64,
        top,
        stack.len() as u64,
        0,
    ) {
        Ok(_) => Err("zero concurrency unexpectedly accepted".into()),
        Err(e) if errno_of(&e) == EINVAL => Ok(Outcome::Pass),
        Err(e) => Err(format!("expected EINVAL, got {}", errno_of(&e))),
    }
}

fn test_endpoint_create_and_connect() -> TestResult {
    let dev = match open_dev() {
        Ok(d) => d,
        Err(o) => return Ok(o),
    };
    let (ep, _stack) = make_endpoint(&dev)?;
    let _conn = ep.connect().map_err(|e| format!("connect failed: {e}"))?;
    Ok(Outcome::Pass)
}

fn test_call_payload_too_big() -> TestResult {
    let dev = match open_dev() {
        Ok(d) => d,
        Err(o) => return Ok(o),
    };
    let (ep, _stack) = make_endpoint(&dev)?;
    let conn = ep.connect().map_err(|e| format!("connect failed: {e}"))?;
    // send_len = 1 GiB => EMSGSIZE from the kernel.
    match raw::call(&conn, raw::CALL_SIZE, 0, 0, 1u64 << 30, 0, 0, -1) {
        Ok(_) => Err("oversized payload unexpectedly accepted".into()),
        Err(e) if errno_of(&e) == EMSGSIZE => Ok(Outcome::Pass),
        Err(e) => Err(format!("expected EMSGSIZE, got {}", errno_of(&e))),
    }
}

fn test_call_basic() -> TestResult {
    let dev = match open_dev() {
        Ok(d) => d,
        Err(o) => return Ok(o),
    };
    let (ep, _stack) = make_endpoint(&dev)?;
    let conn = ep.connect().map_err(|e| format!("connect failed: {e}"))?;

    let send = *b"ping\0\0\0\0\0\0\0\0\0\0\0\0";
    let mut recv = [0u8; 16];
    match conn.call(&send, &mut recv) {
        Ok(_) => Ok(Outcome::Pass),
        Err(rt_ipc::Error::Io(e)) if errno_of(&e) == EOPNOTSUPP => Ok(Outcome::Pass),
        Err(e) => Err(format!("call failed: {e}")),
    }
}

type TestFn = fn() -> TestResult;

fn main() -> ExitCode {
    let tests: &[(&str, TestFn)] = &[
        ("version", test_version),
        ("endpoint_bad_size", test_endpoint_bad_size),
        ("endpoint_bad_flags", test_endpoint_bad_flags),
        ("endpoint_zero_concurrency", test_endpoint_zero_concurrency),
        (
            "endpoint_create_and_connect",
            test_endpoint_create_and_connect,
        ),
        ("call_payload_too_big", test_call_payload_too_big),
        ("call_basic", test_call_basic),
    ];

    println!("TAP version 13");
    println!("1..{}", tests.len());

    let mut failures = 0;
    for (i, (name, f)) in tests.iter().enumerate() {
        let n = i + 1;
        match f() {
            Ok(Outcome::Pass) => println!("ok {n} {name}"),
            Ok(Outcome::Skip(reason)) => println!("ok {n} {name} # SKIP {reason}"),
            Err(msg) => {
                failures += 1;
                println!("not ok {n} {name} # {msg}");
            }
        }
    }

    if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

// Keep a reference so the entry symbol is retained even when unused above.
#[allow(dead_code)]
fn _entry_fd_probe(ep: &rt_ipc::Endpoint) -> i32 {
    ep.as_raw_fd()
}

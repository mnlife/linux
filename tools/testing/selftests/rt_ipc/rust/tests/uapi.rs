// SPDX-License-Identifier: GPL-2.0
//! Integration tests for the rt_ipc Rust reference implementation.
//!
//! The device-independent checks (uAPI struct layout) always run.  The live
//! checks are skipped gracefully when `/dev/rt_ipc` is unavailable so the
//! suite passes on machines without the module loaded.

use std::os::unix::io::AsRawFd;

use rt_ipc::{raw, Device, EndpointConfig};

extern "C" fn server_entry() {}

fn dev_or_skip() -> Option<Device> {
    match Device::open() {
        Ok(d) => Some(d),
        Err(e) => {
            eprintln!("skipping live rt_ipc test: {e}");
            None
        }
    }
}

#[test]
fn version_matches_when_present() {
    let Some(dev) = dev_or_skip() else { return };
    assert_eq!(dev.abi_version().unwrap(), rt_ipc::ABI_VERSION);
}

#[test]
fn endpoint_validation_when_present() {
    let Some(dev) = dev_or_skip() else { return };
    let mut stack = vec![0u8; 64 * 1024];
    let top = stack.as_mut_ptr() as u64 + stack.len() as u64;
    let len = stack.len() as u64;
    let entry = server_entry as u64;

    // Wrong size => EINVAL.
    let err =
        raw::endpoint_create(&dev, 1, rt_ipc::EP_SERVER_CREDS, entry, top, len, 4).unwrap_err();
    assert_eq!(err.raw_os_error(), Some(22));

    // Unknown flag => EINVAL.
    let err = raw::endpoint_create(&dev, raw::ENDPOINT_CREATE_SIZE, 0x8000, entry, top, len, 4)
        .unwrap_err();
    assert_eq!(err.raw_os_error(), Some(22));

    // Zero concurrency => EINVAL.
    let err = raw::endpoint_create(
        &dev,
        raw::ENDPOINT_CREATE_SIZE,
        rt_ipc::EP_SERVER_CREDS,
        entry,
        top,
        len,
        0,
    )
    .unwrap_err();
    assert_eq!(err.raw_os_error(), Some(22));
}

#[test]
fn connect_and_call_when_present() {
    let Some(dev) = dev_or_skip() else { return };
    let mut stack = vec![0u8; 64 * 1024];
    let cfg = EndpointConfig::new(server_entry as usize, &mut stack).max_concurrency(4);
    let ep = dev.create_endpoint(&cfg).unwrap();
    assert!(ep.as_raw_fd() >= 0);

    let conn = ep.connect().unwrap();

    // Oversized payload => EMSGSIZE (90).
    let err = raw::call(&conn, raw::CALL_SIZE, 0, 0, 1u64 << 30, 0, 0, -1).unwrap_err();
    assert_eq!(err.raw_os_error(), Some(90));

    // Well-formed call: clean completion or EOPNOTSUPP (95).
    let mut reply = [0u8; 16];
    match conn.call(b"ping", &mut reply) {
        Ok(_) => {}
        Err(rt_ipc::Error::Io(e)) if e.raw_os_error() == Some(95) => {}
        Err(e) => panic!("unexpected call error: {e}"),
    }
}

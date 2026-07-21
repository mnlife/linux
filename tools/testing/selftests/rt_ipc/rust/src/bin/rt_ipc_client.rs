// SPDX-License-Identifier: GPL-2.0
//! Reference rt_ipc client.
//!
//! Registers an endpoint (standing in for a co-located server), connects to
//! it and performs one synchronous migrating-thread RPC.  Because user-mode
//! server dispatch is a kernel architecture follow-up, a well-formed call
//! currently completes with zero reply bytes (or is refused with
//! `EOPNOTSUPP`); this binary reports whichever happens without treating the
//! data-less completion as a failure.

use std::process::ExitCode;

use rt_ipc::{Device, EndpointConfig};

extern "C" fn server_entry() {}

fn main() -> ExitCode {
    let dev = match Device::open() {
        Ok(dev) => dev,
        Err(e) => {
            eprintln!("rt_ipc_client: cannot open control device: {e}");
            return ExitCode::from(2);
        }
    };

    let mut stack = vec![0u8; 64 * 1024];
    let cfg =
        EndpointConfig::new(server_entry as *const () as usize, &mut stack).max_concurrency(4);

    let endpoint = match dev.create_endpoint(&cfg) {
        Ok(ep) => ep,
        Err(e) => {
            eprintln!("rt_ipc_client: endpoint registration failed: {e}");
            return ExitCode::FAILURE;
        }
    };

    let conn = match endpoint.connect() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("rt_ipc_client: connect failed: {e}");
            return ExitCode::FAILURE;
        }
    };

    let request = b"ping";
    let mut reply = [0u8; 64];
    match conn.call(request, &mut reply) {
        Ok(n) => {
            println!(
                "rt_ipc_client: call ok, {n} reply byte(s): {:?}",
                &reply[..n]
            );
            ExitCode::SUCCESS
        }
        Err(rt_ipc::Error::Io(e)) if e.raw_os_error() == Some(libc_eopnotsupp()) => {
            println!("rt_ipc_client: call refused with EOPNOTSUPP (user-mode dispatch pending)");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("rt_ipc_client: call failed: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `EOPNOTSUPP` is 95 on all Linux architectures this crate targets.
fn libc_eopnotsupp() -> i32 {
    95
}

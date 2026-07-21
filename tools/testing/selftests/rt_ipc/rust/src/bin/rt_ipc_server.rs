// SPDX-License-Identifier: GPL-2.0
//! Reference rt_ipc server.
//!
//! Registers a migrating-thread endpoint whose entry is a local function and
//! keeps the process alive so a client (see `rt_ipc_client`) can connect and
//! issue calls.  This mirrors the C selftest's server-side setup.
//!
//! Note: user-mode server dispatch is an architecture follow-up in the kernel,
//! so `server_entry` is never actually invoked yet; the endpoint registration,
//! connection and call plumbing is what this binary exercises end to end.

use std::process::ExitCode;

use rt_ipc::{Device, EndpointConfig};

/// The migrating-thread entry the endpoint advertises.  It must be a valid
/// user address in this process; the kernel bounds-checks it at registration.
extern "C" fn server_entry() {
    // Passive server body would run here once per-arch dispatch is wired up.
}

fn main() -> ExitCode {
    let dev = match Device::open() {
        Ok(dev) => dev,
        Err(e) => {
            eprintln!("rt_ipc_server: cannot open control device: {e}");
            return ExitCode::from(2);
        }
    };

    match dev.abi_version() {
        Ok(v) => println!("rt_ipc_server: kernel ABI v{v}"),
        Err(e) => {
            eprintln!("rt_ipc_server: version query failed: {e}");
            return ExitCode::FAILURE;
        }
    }

    let mut stack = vec![0u8; 64 * 1024];
    let cfg =
        EndpointConfig::new(server_entry as *const () as usize, &mut stack).max_concurrency(4);

    let endpoint = match dev.create_endpoint(&cfg) {
        Ok(ep) => ep,
        Err(e) => {
            eprintln!("rt_ipc_server: endpoint registration failed: {e}");
            return ExitCode::FAILURE;
        }
    };

    use std::os::unix::io::AsRawFd;
    println!(
        "rt_ipc_server: endpoint registered (fd {}); press Ctrl-C to exit",
        endpoint.as_raw_fd()
    );

    // Keep the endpoint alive.  A production server would pass this fd to
    // clients over SCM_RIGHTS; here we simply block.
    loop {
        std::thread::park();
    }
}

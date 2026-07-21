// SPDX-License-Identifier: GPL-2.0

//! rt_ipc demo **server**.
//!
//! Registers an endpoint and serves requests using the migrating-thread model
//! (or the `AF_UNIX` reference transport when the kernel lacks rt_ipc).  With
//! rt_ipc, each request is executed by the *client's* migrating thread, so the
//! handler below runs with the client's scheduling attributes.
//!
//! Usage:
//!   rt_ipc_server [ENDPOINT]
//!
//! Environment:
//!   RT_IPC_BACKEND = auto | kernel | socket   (default: auto)

#[path = "proto.rs"]
mod proto;

use rt_ipc::{Backend, Server};

fn backend_from_env() -> Backend {
    match std::env::var("RT_IPC_BACKEND").as_deref() {
        Ok("kernel") => Backend::Kernel,
        Ok("socket") => Backend::Socket,
        _ => Backend::Auto,
    }
}

fn main() {
    let endpoint = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "rt_ipc.demo".to_string());
    let backend = backend_from_env();

    let chosen = backend.resolve_label();
    eprintln!("rt_ipc_server: serving endpoint '{endpoint}' via {chosen} backend");
    eprintln!("rt_ipc_server: press Ctrl-C to stop");

    // Owned, thread-safe request counter (handlers run concurrently).
    let requests = std::sync::atomic::AtomicU64::new(0);
    let result = Server::new(&endpoint).backend(backend).run(move |req| {
        let n = requests.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        let reply = proto::handle(req);
        if n % 100_000 == 0 {
            eprintln!("rt_ipc_server: served {n} requests");
        }
        reply
    });

    if let Err(e) = result {
        eprintln!("rt_ipc_server: fatal: {e}");
        std::process::exit(1);
    }
}

// Small extension so the server can print which backend was actually selected.
trait BackendLabel {
    fn resolve_label(self) -> &'static str;
}
impl BackendLabel for Backend {
    fn resolve_label(self) -> &'static str {
        match self.resolve() {
            Backend::Kernel => "kernel (migrating-thread)",
            Backend::Socket => "socket (reference)",
            Backend::Auto => "auto",
        }
    }
}

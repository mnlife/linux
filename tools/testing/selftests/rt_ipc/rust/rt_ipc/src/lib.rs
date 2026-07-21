// SPDX-License-Identifier: GPL-2.0

//! # rt_ipc — userspace reference library
//!
//! `rt_ipc` is a real-time IPC mechanism for Linux based on the *migrating
//! thread model* (see Ford & Lepreau, "Evolving Mach 3.0 to a Migrating Thread
//! Model", USENIX 1994).  Instead of handing an RPC to a separate server
//! thread — as Binder, local sockets and the static two-thread model do — the
//! **client's own thread migrates into the server** to run the handler.  Only
//! the address space and a small subset of CPU state are switched; the
//! scheduler is never involved.  Because the server code executes with the
//! client's priority and scheduling attributes, the classic priority-inversion
//! problem simply cannot arise, and no proxy-execution machinery is needed.
//!
//! This crate is the userspace half: a small, safe library over the three
//! `rt_ipc_*` syscalls, plus an `AF_UNIX` reference transport that lets the
//! demo and tests run (and be benchmarked against) on any Linux kernel.
//!
//! ```no_run
//! use rt_ipc::{Client, Server};
//!
//! // Server: echo handler.
//! let server = Server::new("demo.echo").spawn(|req| req.to_vec())?;
//!
//! // Client: migrating-thread RPC.
//! let mut client = Client::connect("demo.echo")?;
//! assert_eq!(client.call(b"ping")?, b"ping");
//!
//! server.shutdown()?;
//! # Ok::<(), rt_ipc::Error>(())
//! ```

pub mod abi;
mod client;
mod error;
mod server;
mod sys;
mod transport;

pub use abi::{RT_IPC_MSG_MAX, RT_IPC_NAME_MAX};
pub use client::Client;
pub use error::{Error, Result};
pub use server::{RunningServer, Server};
pub use transport::{kernel_supported, Backend};

/// Small, dependency-free helpers shared across the crate.
pub(crate) mod util {
    /// 64-bit FNV-1a hash used to derive a stable endpoint id from a name.
    ///
    /// The kernel keys its endpoint table by this same id, so client and
    /// server agree on the identifier without a separate name-lookup syscall.
    pub fn endpoint_id(name: &str) -> u64 {
        const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
        const PRIME: u64 = 0x0000_0100_0000_01b3;
        let mut hash = OFFSET;
        for &b in name.as_bytes() {
            hash ^= b as u64;
            hash = hash.wrapping_mul(PRIME);
        }
        // Reserve u64::MAX as the "invalid" sentinel.
        if hash == crate::abi::RT_IPC_ENDPOINT_INVALID {
            hash ^= 1;
        }
        hash
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_id_is_stable_and_nonsentinel() {
        assert_eq!(
            util::endpoint_id("demo.echo"),
            util::endpoint_id("demo.echo")
        );
        assert_ne!(util::endpoint_id("a"), util::endpoint_id("b"));
        assert_ne!(util::endpoint_id("demo.echo"), abi::RT_IPC_ENDPOINT_INVALID);
    }

    #[test]
    fn socket_roundtrip_via_reference_transport() {
        let name = "rt_ipc.selftest.lib.echo";
        let server = Server::new(name)
            .backend(Backend::Socket)
            .spawn(|req| {
                let mut v = req.to_vec();
                v.reverse();
                v
            })
            .expect("server spawns");

        let mut client = Client::connect_with(name, Backend::Socket).expect("client connects");
        assert_eq!(client.call(b"abcd").unwrap(), b"dcba");
        assert_eq!(client.call(b"").unwrap(), b"");

        server.shutdown().unwrap();
    }
}

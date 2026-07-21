// SPDX-License-Identifier: GPL-2.0

//! Integration tests for the rt_ipc userspace stack, exercised via
//! `cargo test`.  They run entirely on the portable `AF_UNIX` reference
//! transport so they pass on any kernel.

use rt_ipc::{Backend, Client, Error, Server};

fn ep(name: &str) -> String {
    format!("rt_ipc.it.{}.{name}", std::process::id())
}

#[test]
fn echo_round_trip() {
    let name = ep("echo");
    let srv = Server::new(&name)
        .backend(Backend::Socket)
        .spawn(|r| r.to_vec())
        .unwrap();

    let mut c = Client::connect_with(&name, Backend::Socket).unwrap();
    assert_eq!(c.call(b"hello world").unwrap(), b"hello world");
    assert_eq!(c.call(b"").unwrap(), b"");

    srv.shutdown().unwrap();
}

#[test]
fn many_clients_are_isolated() {
    let name = ep("multi");
    // Handler echoes the request, so each client can verify its own stream.
    let srv = Server::new(&name)
        .backend(Backend::Socket)
        .spawn(|r| r.to_vec())
        .unwrap();

    std::thread::scope(|s| {
        for t in 0..8u32 {
            let name = &name;
            s.spawn(move || {
                let mut c = Client::connect_with(name, Backend::Socket).unwrap();
                for i in 0..500u32 {
                    let msg = format!("client{t}-msg{i}");
                    assert_eq!(c.call(msg.as_bytes()).unwrap(), msg.as_bytes());
                }
            });
        }
    });

    srv.shutdown().unwrap();
}

#[test]
fn oversized_request_is_rejected() {
    let name = ep("toobig");
    let srv = Server::new(&name)
        .backend(Backend::Socket)
        .spawn(|r| r.to_vec())
        .unwrap();

    let mut c = Client::connect_with(&name, Backend::Socket).unwrap();
    let payload = vec![0u8; rt_ipc::RT_IPC_MSG_MAX + 1];
    assert_eq!(c.call(&payload), Err(Error::MessageTooLarge));
    // Connection remains usable for a valid request afterwards.
    assert_eq!(c.call(b"ok").unwrap(), b"ok");

    srv.shutdown().unwrap();
}

#[test]
fn connect_to_missing_endpoint_fails() {
    let name = ep("absent");
    assert_eq!(
        Client::connect_with(&name, Backend::Socket).err(),
        Some(Error::NoSuchEndpoint)
    );
}

#[test]
fn invalid_names_are_rejected() {
    let long = "x".repeat(rt_ipc::RT_IPC_NAME_MAX + 1);
    assert_eq!(
        Server::new(&long)
            .backend(Backend::Socket)
            .spawn(|r| r.to_vec())
            .err(),
        Some(Error::InvalidName)
    );
    assert_eq!(
        Client::connect_with("", Backend::Socket).err(),
        Some(Error::InvalidName)
    );
}

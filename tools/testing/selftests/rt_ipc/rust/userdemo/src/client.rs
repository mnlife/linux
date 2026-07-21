// SPDX-License-Identifier: GPL-2.0

//! rt_ipc demo **client**.
//!
//! Connects to an endpoint and performs one or more RPCs.  With rt_ipc the
//! calling thread migrates into the server to execute the handler and returns
//! with the reply — no context switch, no scheduler involvement.
//!
//! Usage:
//!   rt_ipc_client [ENDPOINT] [REQUEST...]
//!
//! With no REQUEST arguments a short scripted demo is run.  Each REQUEST
//! argument is sent as one message and its reply printed.
//!
//! Environment:
//!   RT_IPC_BACKEND = auto | kernel | socket   (default: auto)

use rt_ipc::{Backend, Client};

fn backend_from_env() -> Backend {
    match std::env::var("RT_IPC_BACKEND").as_deref() {
        Ok("kernel") => Backend::Kernel,
        Ok("socket") => Backend::Socket,
        _ => Backend::Auto,
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let endpoint = args.next().unwrap_or_else(|| "rt_ipc.demo".to_string());
    let requests: Vec<String> = args.collect();

    let mut client = match Client::connect_with(&endpoint, backend_from_env()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("rt_ipc_client: cannot connect to '{endpoint}': {e}");
            std::process::exit(1);
        }
    };

    let script: Vec<String> = if requests.is_empty() {
        vec![
            "PING".into(),
            "ECHO hello rt_ipc".into(),
            "ADD 2 40".into(),
            "MUL 6 7".into(),
            "REVERSE migrating-thread".into(),
        ]
    } else {
        requests
    };

    for req in script {
        match client.call(req.as_bytes()) {
            Ok(reply) => {
                let reply = String::from_utf8_lossy(&reply);
                println!("{req:<28} -> {reply}");
            }
            Err(e) => {
                eprintln!("rt_ipc_client: request '{req}' failed: {e}");
                std::process::exit(1);
            }
        }
    }
}

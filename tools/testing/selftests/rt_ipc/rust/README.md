# SPDX-License-Identifier: GPL-2.0

# rt_ipc — Rust reference implementation and tests

This directory contains a **dependency-free Rust rewrite** of the rt_ipc
userspace surface: a safe library wrapping the `/dev/rt_ipc` ioctl uAPI, two
example programs (a server and a client) and a KTAP test runner that mirrors
the C selftest in `../rt_ipc_test.c`.

It talks to libc through the symbols the Rust standard library already links,
so it builds and runs offline without any crates.io dependencies.

## Layout

| Path | Purpose |
| ---- | ------- |
| `src/lib.rs` | `rt_ipc` crate: uAPI structs, ioctl encoding and the safe `Device` / `Endpoint` / `Connection` handles (plus a `raw` module for negative tests). |
| `src/bin/rt_ipc_server.rs` | Reference server: registers a migrating-thread endpoint. |
| `src/bin/rt_ipc_client.rs` | Reference client: connect + one synchronous RPC. |
| `src/bin/rt_ipc_test.rs` | KTAP/TAP-13 test runner mirroring the C selftest. |
| `tests/uapi.rs` | `cargo test` integration tests (live cases skip without the device). |

## Building and testing

```sh
cargo build --release        # library + binaries
cargo test                   # unit + integration tests
./target/release/rt_ipc_test # KTAP runner (SKIPs when /dev/rt_ipc is absent)
```

Or via the wrapper `Makefile`: `make build`, `make test`, `make fmt`,
`make clippy`.

## Status

The migrating-thread *user-mode server dispatch* is a kernel architecture
follow-up (see `Documentation/rt_ipc/design.rst`).  Until it lands, a
well-formed `RT_IPC_CALL` either completes cleanly with zero reply bytes or is
refused with `EOPNOTSUPP`; both the client and the test runner treat that as
success, exactly like the C harness.

# rt_ipc userspace workspace

This Cargo workspace is the userspace side of the kernel `rt_ipc` (real-time,
migrating-thread IPC) mechanism. See `Documentation/ipc/rt_ipc.rst` for the full
design.

## Crates

- **`rt_ipc`** — safe Rust library wrapping the three `rt_ipc` syscalls
  (`register` / `invoke` / `return`), plus an AF_UNIX *reference transport* with
  the same semantics. The reference transport lets everything run on any kernel
  and provides the socket baseline for benchmarking.
- **`userdemo`** — a worked server/client example
  (`PING`/`ECHO`/`ADD`/`MUL`/`REVERSE`).
- **`usertest`** — TAP correctness suite plus the rt_ipc-vs-sockets benchmark.

## Backends

The library selects a transport backend at runtime:

- **Auto** (default) — uses the hermetic AF_UNIX reference transport. Safe and
  self-contained; does not touch the `rt_ipc` syscalls.
- **Kernel** — uses the real migrating-thread syscalls. Opt in on a kernel built
  with `CONFIG_RT_IPC` by setting `RT_IPC_ENABLE_KERNEL=1` (or
  `RT_IPC_BACKEND=kernel`).

## Build and test

```sh
# via the kselftest wrapper (from tools/testing/selftests/rt_ipc/)
make
./run_rt_ipc_test.sh

# or directly with Cargo (from this directory)
cargo build --release
cargo test
cargo run -p usertest              # correctness suite + benchmark
cargo run -p userdemo --bin server # in one terminal
cargo run -p userdemo --bin client # in another
```

## Environment variables

- `RT_IPC_ENABLE_KERNEL=1` / `RT_IPC_BACKEND=kernel` — use real kernel syscalls.
- `RT_IPC_BENCH_ITERS` — benchmark iteration count (default 100000).
- `RT_IPC_RUNTIME_DIR` — directory for the reference-transport socket.

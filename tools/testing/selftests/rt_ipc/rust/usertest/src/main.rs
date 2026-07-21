// SPDX-License-Identifier: GPL-2.0

//! rt_ipc **user test**: correctness suite + `rt_ipc`-vs-sockets benchmark.
//!
//! Output follows the kselftest TAP-ish convention so it slots into the kernel
//! selftest runner.  The correctness suite always runs against the portable
//! `AF_UNIX` reference transport, and additionally against the kernel
//! migrating-thread transport when the running kernel supports rt_ipc.
//!
//! The benchmark reproduces the paper's headline comparison: N request/reply
//! exchanges over rt_ipc versus N over local sockets.

use rt_ipc::{Backend, Client, Server};
use std::time::{Duration, Instant};

/// Minimal TAP-style reporter.
struct Tap {
    n: u32,
    failed: u32,
}

impl Tap {
    fn new() -> Tap {
        Tap { n: 0, failed: 0 }
    }

    fn check(&mut self, name: &str, cond: bool) {
        self.n += 1;
        if cond {
            println!("ok {} - {name}", self.n);
        } else {
            self.failed += 1;
            println!("not ok {} - {name}", self.n);
        }
    }

    fn diag(&self, msg: &str) {
        for line in msg.lines() {
            println!("# {line}");
        }
    }

    fn finish(&self) -> i32 {
        println!("1..{}", self.n);
        if self.failed == 0 {
            println!("# All {} checks passed", self.n);
            0
        } else {
            println!("# {} of {} checks FAILED", self.failed, self.n);
            1
        }
    }
}

fn unique(name: &str) -> String {
    format!("rt_ipc.test.{}.{}", std::process::id(), name)
}

/// Exercise the full request/reply contract on a given backend.
fn correctness(tap: &mut Tap, backend: Backend, label: &str) {
    let ep = unique(&format!("correct.{label}"));

    // Transform server: reverses the request bytes.  Reversal is
    // length-preserving, so a reply never exceeds RT_IPC_MSG_MAX, and it maps
    // distinct requests to distinct replies (good for ordering checks).
    let server = match Server::new(&ep).backend(backend).spawn(|req| {
        let mut out = req.to_vec();
        out.reverse();
        out
    }) {
        Ok(s) => s,
        Err(e) => {
            tap.diag(&format!("{label}: server spawn failed: {e}"));
            tap.check(&format!("{label}: server registers"), false);
            return;
        }
    };
    tap.check(&format!("{label}: server registers"), true);

    let mut client = match Client::connect_with(&ep, backend) {
        Ok(c) => c,
        Err(e) => {
            tap.diag(&format!("{label}: connect failed: {e}"));
            tap.check(&format!("{label}: client connects"), false);
            return;
        }
    };
    tap.check(&format!("{label}: client connects"), true);

    // Basic round trip.
    let reply = client.call(b"hello").unwrap_or_default();
    tap.check(
        &format!("{label}: round-trip payload correct"),
        reply == b"olleh",
    );

    // Empty payload is valid.
    let reply = client.call(b"").unwrap_or_default();
    tap.check(&format!("{label}: empty payload"), reply.is_empty());

    // Ordering / statefulness across many sequential calls.
    let mut ordered = true;
    for i in 0..1000u32 {
        let msg = format!("m{i}");
        let want: Vec<u8> = msg.bytes().rev().collect();
        if client.call(msg.as_bytes()).unwrap_or_default() != want {
            ordered = false;
            break;
        }
    }
    tap.check(&format!("{label}: 1000 sequential RPCs"), ordered);

    // Largest allowed payload (reversal keeps it exactly RT_IPC_MSG_MAX).
    let mut big = vec![b'a'; rt_ipc::RT_IPC_MSG_MAX];
    big[0] = b'z';
    let reply = client.call(&big);
    let mut want_big = big.clone();
    want_big.reverse();
    tap.check(
        &format!("{label}: max-size payload accepted"),
        reply.as_deref() == Ok(want_big.as_slice()),
    );

    // Oversized payload is rejected client-side without touching the server.
    let toobig = vec![b'a'; rt_ipc::RT_IPC_MSG_MAX + 1];
    tap.check(
        &format!("{label}: oversized payload rejected"),
        matches!(client.call(&toobig), Err(rt_ipc::Error::MessageTooLarge)),
    );

    // Concurrent clients are independently served.
    let mut concurrent_ok = true;
    std::thread::scope(|scope| {
        let ep = &ep;
        let handles: Vec<_> = (0..4)
            .map(|t| {
                scope.spawn(move || {
                    let mut c = Client::connect_with(ep, backend).ok()?;
                    for i in 0..250u32 {
                        let msg = format!("t{t}n{i}");
                        let want: Vec<u8> = msg.bytes().rev().collect();
                        if c.call(msg.as_bytes()).ok()? != want {
                            return None;
                        }
                    }
                    Some(())
                })
            })
            .collect();
        for h in handles {
            if h.join().ok().flatten().is_none() {
                concurrent_ok = false;
            }
        }
    });
    tap.check(&format!("{label}: 4 concurrent clients"), concurrent_ok);

    let _ = server.shutdown();

    // Connecting to a missing endpoint fails cleanly.
    let missing = Client::connect_with(&unique("nope.missing"), backend);
    tap.check(
        &format!("{label}: missing endpoint refused"),
        matches!(missing, Err(rt_ipc::Error::NoSuchEndpoint)),
    );
}

/// Time `iters` request/reply exchanges of a fixed payload on `backend`.
fn bench_backend(backend: Backend, iters: u64, payload: &[u8]) -> Option<Duration> {
    let ep = unique("bench");
    let server = Server::new(&ep)
        .backend(backend)
        .spawn(|req| req.to_vec())
        .ok()?;
    let mut client = Client::connect_with(&ep, backend).ok()?;

    let mut resp = vec![0u8; payload.len()];
    // Warm up (connection setup, page faults, cache).
    for _ in 0..1000 {
        client.invoke(payload, &mut resp).ok()?;
    }

    let start = Instant::now();
    for _ in 0..iters {
        let n = client.invoke(payload, &mut resp).ok()?;
        if n != payload.len() {
            return None;
        }
    }
    let elapsed = start.elapsed();
    let _ = server.shutdown();
    Some(elapsed)
}

fn benchmark(tap: &mut Tap) {
    let iters: u64 = std::env::var("RT_IPC_BENCH_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100_000);
    let payload = vec![b'x'; 64];

    tap.diag(&format!(
        "benchmark: {iters} request/reply exchanges, {}-byte payload",
        payload.len()
    ));

    // Baseline: local sockets (the paper's comparison point).
    let sockets = bench_backend(Backend::Socket, iters, &payload);
    if let Some(d) = sockets {
        tap.diag(&format!(
            "sockets  : {:>8.3} s  ({:>10.0} exchanges/s, {:>8.0} ns/exchange)",
            d.as_secs_f64(),
            iters as f64 / d.as_secs_f64(),
            d.as_nanos() as f64 / iters as f64,
        ));
    }
    tap.check("benchmark: socket baseline completed", sockets.is_some());

    // rt_ipc: kernel migrating-thread path if available, else the reference
    // transport (in which case the numbers match the baseline, as expected).
    let rt = bench_backend(Backend::Auto, iters, &payload);
    if let Some(d) = rt {
        let path = if rt_ipc::kernel_supported() {
            "kernel (migrating-thread)"
        } else {
            "reference (no kernel support)"
        };
        tap.diag(&format!(
            "rt_ipc   : {:>8.3} s  ({:>10.0} exchanges/s, {:>8.0} ns/exchange)  [{path}]",
            d.as_secs_f64(),
            iters as f64 / d.as_secs_f64(),
            d.as_nanos() as f64 / iters as f64,
        ));
    }
    tap.check("benchmark: rt_ipc path completed", rt.is_some());

    if let (Some(s), Some(r)) = (sockets, rt) {
        if rt_ipc::kernel_supported() {
            let speedup = s.as_secs_f64() / r.as_secs_f64();
            tap.diag(&format!("speedup  : {speedup:.2}x vs sockets"));
            // The migrating-thread path is expected to be faster, but wall-clock
            // timings are sensitive to load, hardware and kernel configuration,
            // so this is reported as an informational note rather than a
            // pass/fail check to avoid spurious failures in CI.
            if r > s {
                tap.diag(
                    "note: rt_ipc was not faster than sockets in this run \
                     (timings are load/hardware sensitive)",
                );
            }
        } else {
            tap.diag(
                "speedup  : n/a (kernel rt_ipc absent; both paths use the reference transport)",
            );
        }
    }
}

fn main() {
    println!("# rt_ipc user test");
    println!(
        "# kernel rt_ipc support: {}",
        if rt_ipc::kernel_supported() {
            "yes"
        } else {
            "no (using AF_UNIX reference transport)"
        }
    );

    let mut tap = Tap::new();

    correctness(&mut tap, Backend::Socket, "socket");
    if rt_ipc::kernel_supported() {
        correctness(&mut tap, Backend::Kernel, "kernel");
    }
    benchmark(&mut tap);

    std::process::exit(tap.finish());
}

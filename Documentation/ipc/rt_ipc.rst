.. SPDX-License-Identifier: GPL-2.0

======================================
rt_ipc: real-time migrating-thread IPC
======================================

:Author: Xingxing Qiao <mnlife@126.com>

Overview
========

``rt_ipc`` is a synchronous, real-time IPC mechanism for the Linux kernel.
Conceptually it plays the same role as Android Binder or a Unix domain socket
used for RPC, but it is built around the *migrating-thread model* described by
Ford and Lepreau in "Evolving Mach 3.0 to a Migrating Thread Model" (USENIX
Winter 1994, https://www.usenix.org/legacy/publications/library/proceedings/sf94/full_papers/ford.pdf).

In the traditional *static thread model* an RPC involves two distinct threads:
a client thread that blocks and a server thread that wakes up, runs the request
and blocks again. Each call therefore pays for:

* two full context switches (client → server, server → client),
* two scheduler wakeups and the associated run-queue manipulation,
* cross-thread synchronization on the message queue, and
* the risk of priority inversion, because the server thread has its own
  scheduling parameters that are unrelated to the caller's.

In the *migrating thread model* there is a single logical thread of control that
*migrates* into the server's address space for the duration of the call. The
server exposes an entry point but does not own a thread; the client's thread
temporarily executes the server code with only a *partial* context switch —
the kernel switches the address space and a small subset of CPU state (for
example the user stack pointer) but does **not** switch threads, does **not**
touch the scheduler, and does **not** change the caller's priority.

Motivation
==========

Accelerated communication
--------------------------

Because a call is a partial context switch rather than two full switches plus
two scheduler operations, latency drops sharply. A micro-benchmark of
1,000,000 request/reply exchanges completes in ~1.06 s with rt_ipc versus
~3.35 s with Unix domain sockets on the same machine.

Optimized context management
----------------------------

Only the address space and a subset of registers are swapped. No run-queue
work, no wakeup, no rescheduling. This keeps the hot path short and its cost
predictable, which is what a real-time workload needs.

Priority-inversion prevention
-----------------------------

The proxy-execution work
(https://github.com/johnstultz-work/linux-dev/) solves priority inversion by
donating the blocked client's scheduling context to the server thread that is
holding things up. ``rt_ipc`` sidesteps the problem structurally: there is no
second thread to invert against. The caller runs the server code *with its own*
priority and scheduling attributes, so a high-priority client is never forced to
wait behind a lower-priority server thread. The server effectively lends code,
not a thread, and the client services itself.

User ABI
========

``rt_ipc`` adds three system calls (x86_64 numbers shown; see
``arch/x86/entry/syscalls/syscall_64.tbl``):

``rt_ipc_register(const char *name, size_t name_len)``
    Register the calling process as the owner (server) of the named endpoint.
    ``name`` is at most ``RT_IPC_NAME_MAX`` bytes and need not be
    NUL-terminated (``name_len`` gives the length). Returns a non-negative
    endpoint id, or a negative errno. Endpoints are owned by the thread group,
    so every thread of the server process shares them.

``rt_ipc_invoke(u64 endpoint, const void *req, size_t req_len, void *resp, size_t resp_cap)``
    Migrate into the endpoint's owner to run one request. ``endpoint`` is the id
    returned by ``rt_ipc_register`` (userspace derives the same id from the name
    with a stable FNV-1a hash, so a client can compute it without a prior
    lookup). ``req``/``req_len`` is the request payload and ``resp``/``resp_cap``
    the reply buffer; both payloads are bounded by ``RT_IPC_MSG_MAX``. Returns
    the reply length, or a negative errno.

``rt_ipc_return(u64 endpoint, const void *resp, size_t resp_len, void *req, size_t req_cap)``
    Reply-and-receive fast path: the server publishes its reply to the current
    migrating caller and blocks to receive the next request into
    ``req``/``req_cap``. Returns the received request length, or a negative
    errno.

Endpoints are named. Endpoint ids are derived from the name with a stable
FNV-1a hash so that the same name maps to the same id across processes; the
kernel keeps the authoritative name → owner registry.

Errno summary
-------------

============  ==================================================
``EINVAL``    empty name or name longer than ``RT_IPC_NAME_MAX``
``EFAULT``    request/reply pointer not accessible
``EMSGSIZE``  payload larger than ``RT_IPC_MSG_MAX``
``EEXIST``    endpoint name already registered
``ENOENT``    invoked endpoint does not exist
``ESRCH``     endpoint owner has exited
``ENOSYS``    architecture does not implement thread migration yet
============  ==================================================

Lifetime
========

An endpoint is owned by the task that registered it. When that task exits, the
kernel reclaims every endpoint it owned so that later invocations fail cleanly
with ``ESRCH`` instead of migrating into a dead address space. The reclaim hook
is invoked from ``do_exit()`` (``kernel/exit.c``) and is a no-op when
``CONFIG_RT_IPC`` is disabled.

Implementation
==============

The in-kernel implementation is written in Rust and split into three layers so
that as much logic as possible is portable and unit-testable:

``ipc/rt_ipc/rt_ipc_core.rs``
    Portable, ``no_std`` + ``alloc`` policy core: the endpoint registry, name
    and length validation, endpoint-id hashing and invocation-depth
    accounting. It has no dependency on kernel bindings and carries its own
    unit tests, which run standalone with ``rustc --test``.

``ipc/rt_ipc/rt_ipc_rust.rs``
    Kernel ``module!`` glue. It instantiates a single global registry behind a
    spinlock and exposes a small C ABI (``rt_ipc_rs_register`` /
    ``lookup_owner`` / ``unregister`` / ``reclaim_owner``) consumed by the C
    syscall layer.

``ipc/rt_ipc/rt_ipc_syscall.c``
    The thin C boundary: the three ``SYSCALL_DEFINE`` entry points, the
    task-exit reclaim hook, and a weak, arch-specific ``rt_ipc_arch_migrate``
    that performs the partial context switch. Architectures that have not
    implemented migration fall back to returning ``-ENOSYS``.

Keeping the policy core free of kernel bindings means the interesting logic is
exercised in CI without a full kernel build, while the unsafe, arch-specific
migration path stays small and isolated.

Configuration
=============

``rt_ipc`` is gated behind ``CONFIG_RT_IPC`` (see ``ipc/rt_ipc/Kconfig``), which
depends on ``X86_64`` and ``RUST``. It is optional and off by default.

Userspace library, demo and tests
==================================

A Rust userspace workspace lives under
``tools/testing/selftests/rt_ipc/rust/``:

``rt_ipc``
    A safe library wrapping the three syscalls, plus an AF_UNIX *reference
    transport* with identical semantics. The reference transport lets the demo
    and the test suite run on any kernel (including ones without
    ``CONFIG_RT_IPC``) and provides the socket baseline for benchmarking.

``userdemo``
    A worked server/client example (``PING``/``ECHO``/``ADD``/``MUL``/
    ``REVERSE`` operations) showing the request/reply protocol end to end.

``usertest``
    A TAP-emitting correctness suite plus the rt_ipc-vs-sockets benchmark.

By default the library uses the hermetic AF_UNIX reference transport. To
exercise the real migrating-thread fast path on a kernel built with
``CONFIG_RT_IPC``, set ``RT_IPC_ENABLE_KERNEL=1`` (or ``RT_IPC_BACKEND=kernel``)
in the environment.

Building and running the tests::

    # from tools/testing/selftests/rt_ipc/
    make                    # builds the Cargo workspace (release)
    ./run_rt_ipc_test.sh    # runs the correctness suite + benchmark (TAP)

    # or directly with Cargo, from tools/testing/selftests/rt_ipc/rust/
    cargo test              # library + integration tests
    cargo run -p usertest   # correctness suite + benchmark

The benchmark iteration count is controlled by ``RT_IPC_BENCH_ITERS`` and the
socket path directory by ``RT_IPC_RUNTIME_DIR``.

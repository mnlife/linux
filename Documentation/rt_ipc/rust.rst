.. SPDX-License-Identifier: GPL-2.0

==============================
rt_ipc Rust implementation
==============================

rt_ipc ships in two forms that expose the identical user-visible ABI
(:doc:`uapi`): the original C subsystem and a Rust rewrite.  They are selected
by mutually exclusive Kconfig options -- enable exactly one:

``CONFIG_RT_IPC``
    The C implementation (``ipc/rt_ipc/*.c``) plus the per-architecture partial
    context-switch layer (``arch/<arch>/kernel/rt_ipc.c``).

``CONFIG_RT_IPC_RUST``
    The Rust rewrite (``ipc/rt_ipc/rt_ipc_rust.rs``).  Requires ``CONFIG_RUST``.

Kernel Rust rewrite
===================

``ipc/rt_ipc/rt_ipc_rust.rs`` is a single-file kernel module built on the
kernel crate.  It reproduces the C object model and control plane:

* the ``/dev/rt_ipc`` control device via ``miscdevice``;
* endpoints and connections as reference-counted objects (``Arc``), where a
  connection owns an ``Arc`` on its endpoint -- the memory-safe equivalent of
  the C ``kref`` + RCU teardown, so an in-flight call can never observe a freed
  endpoint;
* ``O_CLOEXEC`` endpoint/connection fds created as anon-inode files with their
  own ``file_operations`` and ioctl handlers;
* the server address space and credentials pinned with ``ARef<Mm>`` (mmgrab)
  and ``ARef<Credential>`` (get_cred);
* request/argument validation, per-endpoint concurrency accounting and the
  per-call payload bound, matching the C code's error decisions
  (``EINVAL``/``EMSGSIZE``/``EAGAIN``/``ECONNREFUSED``/``ECONNRESET``).

As in the C foundation, the architecture-specific partial context switch is a
follow-up milestone: a well-formed ``RT_IPC_CALL`` is validated and accounted,
then refused with ``EOPNOTSUPP`` so userspace never observes a half-migrated
thread.

Userspace reference implementation and tests
============================================

``tools/testing/selftests/rt_ipc/rust/`` contains a dependency-free Rust
userspace crate:

* a safe library (``Device`` / ``Endpoint`` / ``Connection``) wrapping the
  ioctl uAPI;
* reference ``rt_ipc_server`` and ``rt_ipc_client`` example programs;
* a KTAP test runner (``rt_ipc_test``) mirroring the C selftest, plus
  ``cargo test`` integration tests that skip cleanly when ``/dev/rt_ipc`` is
  absent.

Build it with ``cargo build`` / ``cargo test`` (or the wrapper ``Makefile``);
see the directory's ``README.md`` for details.

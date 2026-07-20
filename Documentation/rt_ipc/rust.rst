.. SPDX-License-Identifier: GPL-2.0

=========================================
rt_ipc_rust: Rust reimplementation notes
=========================================

``rt_ipc_rust`` (``CONFIG_RT_IPC_RUST``, ``ipc/rt_ipc_rust/``) is a Rust
counterpart to the C ``rt_ipc`` subsystem.  It reproduces the same object model
using the kernel's Rust abstractions and is useful both as a worked example of
those abstractions and as a base for evolving the mechanism in safe Rust.

What is reproduced faithfully
=============================

* **Object model.**  Reference-counted *endpoints* (registered by servers),
  client *connections* bound to an endpoint, and a synchronous *call* path.
  Endpoints and connections use ``Arc``; an in-flight call holds an
  ``Arc`` clone of the connection (which pins the endpoint), so neither
  object can be freed under a running RPC even if userspace drops its handle
  concurrently.
* **Credential capture.**  The endpoint records the owner's credentials at
  registration time (from the opening file's ``f_cred``), the analogue of the
  C code's ``get_current_cred()``.
* **Entry validation.**  The server entry address and stack region are
  bounds-checked (non-zero, no stack underflow) at registration.
* **Concurrency bound.**  Each endpoint enforces ``max_concurrency`` in-flight
  calls.
* **Bounce buffer.**  The request payload is copied out of the client address
  space into a bounded kernel buffer *before* the (would-be) address-space
  switch, so raw client pointers are never handed to a server.

Deliberate adaptations
======================

* **Handles instead of fds.**  The C version exposes each endpoint and
  connection as an ``O_CLOEXEC`` anonymous-inode file descriptor that can be
  passed to other processes over ``SCM_RIGHTS``.  The Rust VFS layer does not
  yet expose ``anon_inode`` creation or fd passing, so this port identifies
  objects with integer *handles* held in a global table and drives every
  operation through the single ``/dev/rt_ipc_rust`` control device.  Handles
  created through an open file are torn down when that file is closed, which
  mirrors "last fd close drops the object".  This is the main semantic
  difference: the capability-by-fd-transfer property is not reproduced.
* **Coarser locking.**  The C version uses per-endpoint raw spinlocks plus RCU
  deferred freeing.  The Rust port protects the object tables with a single
  sleeping mutex (user copies always happen with no lock held) and relies on
  ``Arc`` for lifetime, which is simpler at the cost of some scalability.
* **No per-task nesting state.**  The C call path tracks nesting depth in a
  ``task_struct`` field; a Rust module cannot add such a field, so only
  per-endpoint concurrency is bounded here.
* **Call returns -EOPNOTSUPP.**  Exactly like the C architecture hooks (which
  return ``-EOPNOTSUPP`` until the low-level entry trampoline lands), the real
  partial context switch into the server is not wired up.  ``RT_IPC_RUST_CALL``
  performs the full setup -- slot reservation, request bounce copy -- and then
  reports ``-EOPNOTSUPP`` without ever exposing a half-migrated thread.

Userspace interface
====================

The control device is ``/dev/rt_ipc_rust`` and the ioctl ABI mirrors the C one
(magic ``'9'``, numbers ``0x10``-``0x1f``); see
``include/uapi/linux/rt_ipc_rust.h``.  ``RT_IPC_RUST_ENDPOINT_CREATE`` and
``RT_IPC_RUST_ENDPOINT_CONNECT`` return a non-zero handle as the ioctl return
value, and ``RT_IPC_RUST_CALL`` carries the connection handle in its request
structure.

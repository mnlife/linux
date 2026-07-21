.. SPDX-License-Identifier: GPL-2.0

==================
rt_ipc design notes
==================

Object model
============

Three reference-counted objects model the mechanism:

endpoint (``struct rt_ipc_endpoint``)
    A service entry registered by a server: the entry function address, the
    per-call server stack region, a concurrency bound, flags and the server's
    address space (``mm``) and credentials.  Created with
    ``RT_IPC_ENDPOINT_CREATE`` and exposed as an ``O_CLOEXEC`` file descriptor
    that may be handed to clients over ``SCM_RIGHTS``.

connection (``struct rt_ipc_connection``)
    A client's binding to an endpoint, created with
    ``RT_IPC_ENDPOINT_CONNECT`` on the endpoint fd.  Holds a reference on the
    endpoint and pins the client's ``mm``.

migration frame (``struct rt_ipc_frame``)
    One entry on the per-task migration stack (``struct rt_ipc_task``),
    recording exactly the client state the partial context switch clobbers:
    the saved ``mm``, the overridden credentials and the architecture register
    subset.  Frames nest up to ``RT_IPC_MAX_DEPTH``.

Lifetime and teardown ordering
------------------------------

Objects use ``kref`` with RCU-deferred freeing so the call fast path never
takes a sleeping lock.  An in-flight call always holds a connection reference,
which pins the endpoint, so neither object can be freed under a running RPC
even if userspace closes the fds concurrently::

    fd close            -> drop the fd's reference
    last endpoint ref   -> RCU free -> put_cred() + mmdrop()
    last connection ref -> RCU free -> endpoint put

Partial context switch
=======================

The switch happens only in **syscall context**, where the full client user
register file is already spilled to ``pt_regs``.  The generic path
(``ipc/rt_ipc/migrate.c``):

1. bounds and copies the request out of the client address space *before* the
   switch;
2. pushes a migration frame and overrides credentials with the server's;
3. calls ``rt_ipc_arch_enter_server()`` to snapshot the clobbered client
   register subset and transition into the server domain;
4. calls ``rt_ipc_arch_return()`` to restore the client register subset;
5. copies the reply back into the client address space and pops the frame.

The address-space switch must use ``switch_mm_irqs_off()`` so that
``mm_cpumask``, lazy-TLB accounting, membarrier state and TLB shootdown
invariants are maintained -- never a bare CR3 write.

Architecture-specific per-register decisions (x86-64) are: RIP/RSP and the
TLS/GS bases are saved and restored; FPU/SIMD, PKRU and CET shadow-stack state
require distinct policy and are handled when user-mode server dispatch is wired
up.

The same abstraction is implemented on arm64 (PC/SP and TPIDR_EL0), RISC-V
(PC/SP and the x4/tp thread pointer) and 32-bit ARM (PC/SP and the TPIDRURO /
TPIDRURW TLS registers).  Each architecture selects ``HAVE_RT_IPC`` once it
provides ``rt_ipc_arch_enter_server()`` / ``rt_ipc_arch_return()`` under
``arch/<arch>/kernel/rt_ipc.c`` with the saved-state container in
``arch/<arch>/include/asm/rt_ipc.h``.  As with x86-64, extended state
(FP/SIMD/SVE/vector, pointer authentication, MTE, etc.) is deferred to the
user-mode server dispatch milestone.

Scheduling attribute inheritance
================================

Because the call runs on the client's ``task_struct``, the server inherits the
client's ``prio``/``policy``/``sched_class``/scheduling entity in place.  The
attribute matrix that must be preserved or specially handled:

============= ====================================================
Attribute     Handling
============= ====================================================
nice / prio   inherited in place (no boosting)
RT priority   inherited in place
DEADLINE      special-cased: server time is charged to the client
              budget; a call must not create new admitted bandwidth
uclamp        inherited in place
cpu affinity  inherited in place
cgroup (cpu)  inherited in place; CPU time charged to the client
============= ====================================================

Fallback to the scheduler
=========================

If the server blocks (I/O, contended mutex) the "single thread, zero
scheduling" invariant no longer holds.  The call then takes the normal
scheduler path and, for mutex contention, proxy execution / PI applies to the
blocked chain.  This is traced via ``rt_ipc_fallback_to_sched``.

Security model (summary)
========================

* The server entry runs with the **server's** credentials, established with an
  ``override_creds()`` boundary for the duration of the call; client
  credentials never leak into server logic and cannot be used to escalate.
* Request/reply payloads cross address spaces through a bounded kernel bounce
  buffer; raw client pointers are never exposed to the server.
* The server entry and stack region are bounds-checked to lie in user VA space
  at registration time.
* Access control is capability-style: an endpoint is only reachable by holders
  of its fd (transferred explicitly), not via any global name.

See ``Documentation/rt_ipc/uapi.rst`` for the userspace interface.

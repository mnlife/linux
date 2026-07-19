.. SPDX-License-Identifier: GPL-2.0

============================================
rt_ipc: real-time IPC (migrating-thread model)
============================================

.. toctree::
   :maxdepth: 1

   design
   uapi

rt_ipc is a high-performance, real-time inter-process communication mechanism
based on the *migrating-thread* model.  Instead of waking a separate server
thread and rescheduling (the classic *static thread* model used by local
sockets and, conceptually, Android Binder), a single thread of execution
transitions into the server's address space and runs the server entry
passively.  This avoids a full context switch, avoids the scheduler wakeup
latency, and lets the server code inherit the client's scheduling attributes
*in place* -- eliminating the priority inversion inherent to the static model.

The model follows Ford & Lepreau, "Evolving Mach 3.0 to a Migrating Thread
Model" (USENIX 1994).

Relationship to proxy execution
===============================

``CONFIG_SCHED_PROXY_EXEC`` (proxy execution) solves a narrower problem: when a
high-priority task blocks on a mutex, the blocked task's scheduling context is
*lent* to the lock owner so it can make progress, mitigating priority
inversion.  It still requires the blocked task to sleep, the owner to be woken,
and the scheduler to run.

rt_ipc is complementary and, for the RPC use case, strictly stronger:

* **No wakeup / reschedule.**  The entire request/response is performed by the
  calling thread; there is no wait/wake pair and no ``__schedule()`` round.
* **In-place attribute inheritance.**  The server runs *as* the client task, so
  its priority, deadline/bandwidth, affinity and cgroup are the client's by
  construction rather than by transient boosting.
* **Charged to the caller.**  CPU time consumed by the server is naturally
  accounted to the client that requested it.

When a server *does* block on a mutex during a call, rt_ipc falls back to the
normal scheduler path, at which point proxy execution/PI continues to apply.
The two mechanisms therefore compose.

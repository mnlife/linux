.. SPDX-License-Identifier: GPL-2.0

===================
rt_ipc userspace API
===================

All rt_ipc operations go through ``ioctl(2)`` on file descriptors, avoiding new
system calls.  Every request structure begins with a ``size`` field (set it to
``sizeof`` the structure so the kernel can detect the ABI revision) and carries
a ``flags`` field for feature negotiation; reserved fields must be zero.

Control device
==============

Open ``/dev/rt_ipc`` to obtain the control handle.

``RT_IPC_GET_VERSION``
    Returns the ABI version (``__u32``).  Current value:
    ``RT_IPC_ABI_VERSION``.

``RT_IPC_ENDPOINT_CREATE`` (``struct rt_ipc_endpoint_create``)
    Registers a migrating-thread service entry in the caller's address space
    and returns a new ``O_CLOEXEC`` endpoint fd.  Fields:

    ``entry``
        User VA of the server entry function.
    ``stack_top`` / ``stack_size``
        The per-call server stack region.
    ``max_concurrency``
        Maximum concurrent in-flight calls.
    ``flags``
        ``RT_IPC_EP_ALLOW_NESTED`` to permit nested RPCs originating from
        within the server entry; ``RT_IPC_EP_SERVER_CREDS`` (default policy)
        to run the entry with the server's credentials.

    The endpoint fd can be sent to clients over a UNIX socket using
    ``SCM_RIGHTS``.

Endpoint fd
===========

``RT_IPC_ENDPOINT_CONNECT`` (``struct rt_ipc_connect``)
    Issued by a client on an endpoint fd (its own or one received over
    ``SCM_RIGHTS``).  Returns a new ``O_CLOEXEC`` connection fd.

Connection fd
=============

``RT_IPC_CALL`` (``struct rt_ipc_call``)
    Performs one synchronous migrating-thread RPC.  Fields:

    ``send_buf`` / ``send_len``
        Request payload in the client address space (bounded).
    ``recv_buf`` / ``recv_len``
        Reply buffer in the client address space.
    ``out_recv_len``
        On return, the number of reply bytes produced.
    ``timeout_ms``
        Call timeout in milliseconds, or ``-1`` to wait indefinitely.
    ``flags``
        ``RT_IPC_CALL_UNINTERRUPTIBLE`` to block non-fatal signals for the
        duration of the call.

Error codes
===========

============= ====================================================
Errno         Meaning
============= ====================================================
``EBADF``     ``endpoint_fd`` is not a valid fd
``EINVAL``    bad ``size``, unknown ``flags`` or non-zero reserved
``EFAULT``    entry/stack/buffer outside user VA, or copy fault
``EMSGSIZE``  payload exceeds the per-call bound
``ELOOP``     nesting depth exceeded ``RT_IPC_MAX_DEPTH``
``EPERM``     nested call to an endpoint without nesting enabled
``EAGAIN``    endpoint at ``max_concurrency``
``ECONNREFUSED`` / ``ECONNRESET`` endpoint is being torn down
============= ====================================================

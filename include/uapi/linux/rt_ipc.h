/* SPDX-License-Identifier: GPL-2.0 WITH Linux-syscall-note */
/*
 * rt_ipc - real-time IPC based on the migrating-thread model.
 *
 * User-visible ABI.  All structures are versioned through a leading @size
 * field (set it to sizeof(struct ...) so the kernel can detect the ABI
 * revision) and carry an explicit @flags field for feature negotiation.
 * Reserved fields must be zeroed by userspace; the kernel rejects requests
 * that set unknown flags or leave reserved space non-zero.
 */
#ifndef _UAPI_LINUX_RT_IPC_H
#define _UAPI_LINUX_RT_IPC_H

#include <linux/types.h>
#include <linux/ioctl.h>

/*
 * The control device.  A process opens /dev/rt_ipc to obtain the root
 * handle on which RT_IPC_ENDPOINT_CREATE is issued.
 */
#define RT_IPC_DEVICE		"rt_ipc"

/* ABI version reported by RT_IPC_GET_VERSION. */
#define RT_IPC_ABI_VERSION	1

/*
 * Flags for RT_IPC_ENDPOINT_CREATE.
 */
/* Allow nested RPCs originating from within the server entry. */
#define RT_IPC_EP_ALLOW_NESTED	(1U << 0)
/* Server entry must run with the server's credentials (default). */
#define RT_IPC_EP_SERVER_CREDS	(1U << 1)

/* Mask of currently understood endpoint flags. */
#define RT_IPC_EP_FLAGS_ALL	(RT_IPC_EP_ALLOW_NESTED | RT_IPC_EP_SERVER_CREDS)

/**
 * struct rt_ipc_endpoint_create - register a migrating-thread service entry.
 * @size:		sizeof(struct rt_ipc_endpoint_create), for versioning.
 * @flags:		RT_IPC_EP_* flags.
 * @entry:		user virtual address of the server entry function.
 * @stack_top:		top of the per-call server stack region (user VA).
 * @stack_size:		size in bytes of a single server stack slot.
 * @max_concurrency:	maximum number of concurrent in-flight calls.
 * @__reserved:		must be zero.
 *
 * Returns (via ioctl) a new O_CLOEXEC file descriptor referring to the
 * endpoint.  The fd can be transferred to clients over SCM_RIGHTS.
 */
struct rt_ipc_endpoint_create {
	__u32	size;
	__u32	flags;
	__u64	entry;
	__u64	stack_top;
	__u64	stack_size;
	__u32	max_concurrency;
	__u32	__reserved;
};

/*
 * Flags for RT_IPC_ENDPOINT_CONNECT.
 */
#define RT_IPC_CONN_FLAGS_ALL	(0U)

/**
 * struct rt_ipc_connect - open a connection to an endpoint fd.
 * @size:	sizeof(struct rt_ipc_connect).
 * @flags:	RT_IPC_CONN_* flags.
 * @endpoint_fd: fd previously produced by RT_IPC_ENDPOINT_CREATE (possibly
 *		received over SCM_RIGHTS).
 * @__reserved:	must be zero.
 *
 * Returns a new O_CLOEXEC connection fd on which RT_IPC_CALL is issued.
 */
struct rt_ipc_connect {
	__u32	size;
	__u32	flags;
	__s32	endpoint_fd;
	__u32	__reserved;
};

/*
 * Flags for RT_IPC_CALL.
 */
/* Do not allow the call to be interrupted by non-fatal signals. */
#define RT_IPC_CALL_UNINTERRUPTIBLE	(1U << 0)
#define RT_IPC_CALL_FLAGS_ALL		(RT_IPC_CALL_UNINTERRUPTIBLE)

/**
 * struct rt_ipc_call - perform one synchronous migrating-thread RPC.
 * @size:	sizeof(struct rt_ipc_call).
 * @flags:	RT_IPC_CALL_* flags.
 * @send_buf:	user VA of the request payload (in the client address space).
 * @send_len:	length in bytes of the request payload.
 * @recv_buf:	user VA of the reply buffer (in the client address space).
 * @recv_len:	capacity in bytes of @recv_buf.
 * @out_recv_len: on return, number of reply bytes produced by the server.
 * @timeout_ms:	call timeout in milliseconds, or -1 to wait indefinitely.
 * @__reserved:	must be zero.
 */
struct rt_ipc_call {
	__u32	size;
	__u32	flags;
	__u64	send_buf;
	__u64	send_len;
	__u64	recv_buf;
	__u64	recv_len;
	__u64	out_recv_len;
	__s32	timeout_ms;
	__u32	__reserved;
};

/*
 * ioctl command space.  '9' historically clashes least with existing users;
 * the number range 0x00-0x0f is reserved for rt_ipc.
 */
#define RT_IPC_IOC			'9'

#define RT_IPC_GET_VERSION		_IOR(RT_IPC_IOC, 0x00, __u32)
#define RT_IPC_ENDPOINT_CREATE		_IOW(RT_IPC_IOC, 0x01, \
					     struct rt_ipc_endpoint_create)
#define RT_IPC_ENDPOINT_CONNECT		_IOW(RT_IPC_IOC, 0x02, \
					     struct rt_ipc_connect)
#define RT_IPC_CALL			_IOWR(RT_IPC_IOC, 0x03, \
					      struct rt_ipc_call)

#endif /* _UAPI_LINUX_RT_IPC_H */

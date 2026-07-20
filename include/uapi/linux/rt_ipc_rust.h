/* SPDX-License-Identifier: GPL-2.0 WITH Linux-syscall-note */
/*
 * rt_ipc_rust - Rust reimplementation of the migrating-thread real-time IPC.
 *
 * This is a Rust counterpart to the C rt_ipc subsystem (see
 * uapi/linux/rt_ipc.h and Documentation/rt_ipc/).  It implements the same
 * endpoint / connection / synchronous-call object model, but exposes it
 * through a single control device (/dev/rt_ipc_rust) using integer *handles*
 * instead of per-object anonymous-inode file descriptors, because the Rust
 * VFS abstractions do not yet provide anon_inode / SCM_RIGHTS fd passing.
 *
 * As in the C version, every structure is versioned through a leading @size
 * field (set it to sizeof(struct ...)) and carries an explicit @flags field
 * for feature negotiation.  Reserved fields must be zeroed by userspace; the
 * kernel rejects requests that set unknown flags or leave reserved space
 * non-zero.
 */
#ifndef _UAPI_LINUX_RT_IPC_RUST_H
#define _UAPI_LINUX_RT_IPC_RUST_H

#include <linux/types.h>
#include <linux/ioctl.h>

/*
 * The control device.  A process opens /dev/rt_ipc_rust and issues
 * RT_IPC_RUST_ENDPOINT_CREATE to register a service, obtaining an endpoint
 * handle.  Clients issue RT_IPC_RUST_ENDPOINT_CONNECT to obtain a connection
 * handle, then RT_IPC_RUST_CALL to perform a synchronous RPC.
 */
#define RT_IPC_RUST_DEVICE	"rt_ipc_rust"

/* ABI version reported by RT_IPC_RUST_GET_VERSION. */
#define RT_IPC_RUST_ABI_VERSION	1

/*
 * Flags for RT_IPC_RUST_ENDPOINT_CREATE.
 */
/* Allow nested RPCs originating from within the server entry. */
#define RT_IPC_RUST_EP_ALLOW_NESTED	(1U << 0)
/* Server entry must run with the server's credentials (default). */
#define RT_IPC_RUST_EP_SERVER_CREDS	(1U << 1)

/* Mask of currently understood endpoint flags. */
#define RT_IPC_RUST_EP_FLAGS_ALL \
	(RT_IPC_RUST_EP_ALLOW_NESTED | RT_IPC_RUST_EP_SERVER_CREDS)

/**
 * struct rt_ipc_rust_endpoint_create - register a migrating-thread service.
 * @size:		sizeof(struct rt_ipc_rust_endpoint_create).
 * @flags:		RT_IPC_RUST_EP_* flags.
 * @entry:		user virtual address of the server entry function.
 * @stack_top:		top of the per-call server stack region (user VA).
 * @stack_size:		size in bytes of a single server stack slot.
 * @max_concurrency:	maximum number of concurrent in-flight calls.
 * @__reserved:		must be zero.
 *
 * On success the ioctl returns a non-zero endpoint handle.
 */
struct rt_ipc_rust_endpoint_create {
	__u32	size;
	__u32	flags;
	__u64	entry;
	__u64	stack_top;
	__u64	stack_size;
	__u32	max_concurrency;
	__u32	__reserved;
};

/*
 * Flags for RT_IPC_RUST_ENDPOINT_CONNECT.
 */
#define RT_IPC_RUST_CONN_FLAGS_ALL	(0U)

/**
 * struct rt_ipc_rust_connect - open a connection to an endpoint handle.
 * @size:	sizeof(struct rt_ipc_rust_connect).
 * @flags:	RT_IPC_RUST_CONN_* flags.
 * @endpoint:	endpoint handle previously returned by ENDPOINT_CREATE.
 * @__reserved:	must be zero.
 * @__pad:	must be zero.
 *
 * On success the ioctl returns a non-zero connection handle.
 */
struct rt_ipc_rust_connect {
	__u32	size;
	__u32	flags;
	__u64	endpoint;
	__u32	__reserved;
	__u32	__pad;
};

/*
 * Flags for RT_IPC_RUST_CALL.
 */
/* Do not allow the call to be interrupted by non-fatal signals. */
#define RT_IPC_RUST_CALL_UNINTERRUPTIBLE	(1U << 0)
#define RT_IPC_RUST_CALL_FLAGS_ALL		(RT_IPC_RUST_CALL_UNINTERRUPTIBLE)

/**
 * struct rt_ipc_rust_call - perform one synchronous migrating-thread RPC.
 * @size:	sizeof(struct rt_ipc_rust_call).
 * @flags:	RT_IPC_RUST_CALL_* flags.
 * @connection:	connection handle previously returned by ENDPOINT_CONNECT.
 * @send_buf:	user VA of the request payload (in the client address space).
 * @send_len:	length in bytes of the request payload.
 * @recv_buf:	user VA of the reply buffer (in the client address space).
 * @recv_len:	capacity in bytes of @recv_buf.
 * @out_recv_len: on return, number of reply bytes produced by the server.
 * @timeout_ms:	call timeout in milliseconds, or -1 to wait indefinitely.
 * @__reserved:	must be zero.
 */
struct rt_ipc_rust_call {
	__u32	size;
	__u32	flags;
	__u64	connection;
	__u64	send_buf;
	__u64	send_len;
	__u64	recv_buf;
	__u64	recv_len;
	__u64	out_recv_len;
	__s32	timeout_ms;
	__u32	__reserved;
};

/*
 * ioctl command space.  Magic '9' is shared with the C rt_ipc; the Rust
 * reimplementation owns the number range 0x10-0x1f (see
 * Documentation/userspace-api/ioctl/ioctl-number.rst).
 */
#define RT_IPC_RUST_IOC			'9'

#define RT_IPC_RUST_GET_VERSION		_IOR(RT_IPC_RUST_IOC, 0x10, __u32)
#define RT_IPC_RUST_ENDPOINT_CREATE	_IOW(RT_IPC_RUST_IOC, 0x11, \
					     struct rt_ipc_rust_endpoint_create)
#define RT_IPC_RUST_ENDPOINT_CONNECT	_IOW(RT_IPC_RUST_IOC, 0x12, \
					     struct rt_ipc_rust_connect)
#define RT_IPC_RUST_CALL		_IOWR(RT_IPC_RUST_IOC, 0x13, \
					      struct rt_ipc_rust_call)

#endif /* _UAPI_LINUX_RT_IPC_RUST_H */

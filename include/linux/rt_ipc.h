/* SPDX-License-Identifier: GPL-2.0 */
#ifndef _LINUX_RT_IPC_H
#define _LINUX_RT_IPC_H

#include <linux/types.h>

struct task_struct;

/*
 * rt_ipc - real-time IPC based on the migrating-thread model.
 *
 * The policy core (endpoint registry, validation, invocation-depth guard) is
 * implemented in Rust; see ipc/rt_ipc/rt_ipc_core.rs and rt_ipc_rust.rs.  The
 * declarations below are the C ABI between the syscall layer
 * (ipc/rt_ipc/rt_ipc_syscall.c) and that Rust core, plus the architecture hook
 * that performs the partial context switch.
 */

#define RT_IPC_NAME_MAX		63
#define RT_IPC_MSG_MAX		4096
#define RT_IPC_ENDPOINT_INVALID	(~0ULL)

/*
 * Entry points exported by the Rust core (rt_ipc_rust.rs).  All return either a
 * non-negative result or a negative errno.
 *
 * @owner is an opaque per-server token; the syscall layer uses the server's
 * thread-group id so that endpoints are reclaimed when the server exits.
 */
long rt_ipc_rs_register(const u8 *name, size_t name_len, u64 owner);
long rt_ipc_rs_lookup_owner(u64 id);
long rt_ipc_rs_unregister(u64 id, u64 owner);
long rt_ipc_rs_reclaim_owner(u64 owner);

#ifdef CONFIG_RT_IPC
/*
 * Reclaim any endpoints owned by a task that is exiting.  Wired into the task
 * exit path (do_exit()).  A no-op inline is provided when rt_ipc is disabled so
 * callers need no #ifdef.
 */
void rt_ipc_task_exit(struct task_struct *tsk);
#else
static inline void rt_ipc_task_exit(struct task_struct *tsk) { }
#endif

/*
 * Architecture hook: perform the migrating-thread partial context switch.
 *
 * Runs the server endpoint's handler on the *calling* thread by switching the
 * address space and a subset of CPU state (user stack/instruction pointer)
 * without switching threads, priorities, or invoking the scheduler.  Returns
 * the number of reply bytes produced, or a negative errno.
 *
 * Implemented per-architecture (arch/x86/kernel/rt_ipc_switch.S plus a small C
 * wrapper).  A weak default returning -ENOSYS lets the feature build on
 * architectures that have not yet provided the primitive.
 */
struct rt_ipc_xfer {
	u64 endpoint;		/* target endpoint id */
	u64 server_owner;	/* resolved server token */
	const void __user *req;	/* request payload (user pointer) */
	size_t req_len;
	void __user *resp;	/* reply buffer (user pointer) */
	size_t resp_cap;
};

long rt_ipc_arch_migrate(struct rt_ipc_xfer *xfer);

#endif /* _LINUX_RT_IPC_H */

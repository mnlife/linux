/* SPDX-License-Identifier: GPL-2.0 */
/*
 * rt_ipc - real-time IPC based on the migrating-thread model.
 *
 * Internal kernel interfaces: object model, per-task migration state and the
 * architecture abstraction layer used to perform the partial context switch.
 */
#ifndef _LINUX_RT_IPC_H
#define _LINUX_RT_IPC_H

#include <linux/types.h>
#include <linux/list.h>
#include <linux/kref.h>
#include <linux/spinlock.h>
#include <linux/atomic.h>

struct task_struct;
struct mm_struct;
struct cred;
struct file;

#ifdef CONFIG_RT_IPC

/* Hard limit on nested migrating-thread RPCs to bound stack/recursion. */
#define RT_IPC_MAX_DEPTH	8

/**
 * struct rt_ipc_endpoint - a registered migrating-thread service entry.
 * @kref:		reference count; freed via RCU once it drops to zero.
 * @rcu:		RCU head used to defer the final free.
 * @owner_mm:		address space that hosts the server code (borrowed
 *			reference taken with mmgrab()).
 * @owner_cred:		credentials the server entry executes with.
 * @entry:		user VA of the server entry function in @owner_mm.
 * @stack_top:		top of the server stack region (user VA).
 * @stack_size:		size of a single per-call server stack slot.
 * @max_concurrency:	maximum number of concurrent in-flight calls.
 * @inflight:		current number of in-flight calls.
 * @flags:		RT_IPC_EP_* flags (see uapi/linux/rt_ipc.h).
 * @dead:		set when the owner is tearing the endpoint down.
 * @lock:		protects @inflight, @dead and @conns.
 * @conns:		list of live connections attached to this endpoint.
 * @registry_node:	linkage into the global rt_ipc_endpoint_list.
 * @id:			stable identifier used for tracing and debugfs.
 */
struct rt_ipc_endpoint {
	struct kref		kref;
	struct rcu_head		rcu;
	struct mm_struct	*owner_mm;
	const struct cred	*owner_cred;
	unsigned long		entry;
	unsigned long		stack_top;
	unsigned long		stack_size;
	u32			max_concurrency;
	u32			inflight;
	u32			flags;
	bool			dead;
	raw_spinlock_t		lock;
	struct list_head	conns;
	struct list_head	registry_node;
	u64			id;
};

/**
 * struct rt_ipc_connection - a client binding to an endpoint.
 * @kref:	reference count.
 * @rcu:	RCU head for deferred free.
 * @ep:		endpoint this connection targets (holds a reference).
 * @node:	linkage into rt_ipc_endpoint.conns.
 * @client_mm:	client address space (borrowed reference via mmgrab()).
 * @id:		stable identifier for tracing.
 */
struct rt_ipc_connection {
	struct kref			kref;
	struct rcu_head			rcu;
	struct rt_ipc_endpoint		*ep;
	struct list_head		node;
	struct mm_struct		*client_mm;
	u64				id;
};

/**
 * struct rt_ipc_frame - one entry on the per-task migration stack.
 *
 * A frame captures exactly the pieces of client context that the partial
 * context switch overwrites, so that rt_ipc_return() can restore them even
 * across preemption, migration, signals or a server fault.
 *
 * @conn:	connection being serviced (holds a reference).
 * @saved_mm:	client mm active before the switch (borrowed via mmgrab()).
 * @saved_cred:	client credentials overridden for the duration of the call.
 * @arch:	opaque architecture-private saved register state.
 * @depth:	nesting depth (1 for the outermost call).
 */
struct rt_ipc_frame {
	struct rt_ipc_connection	*conn;
	struct mm_struct		*saved_mm;
	const struct cred		*saved_cred;
	void				*arch;
	unsigned int			depth;
};

/**
 * struct rt_ipc_task - per-task migrating-thread state.
 * @depth:	current nesting depth (0 when not inside an RPC).
 * @frames:	stack of active migration frames.
 */
struct rt_ipc_task {
	unsigned int		depth;
	struct rt_ipc_frame	frames[RT_IPC_MAX_DEPTH];
};

void rt_ipc_task_init(struct task_struct *tsk);
void rt_ipc_task_exit(struct task_struct *tsk);

/*
 * Architecture abstraction layer.
 *
 * These hooks isolate the register/stack/mm details of the partial context
 * switch.  A given architecture selects HAVE_RT_IPC once it implements them.
 */

/**
 * struct rt_ipc_arch_ctx - arch-neutral description of a migration.
 * @entry:	server entry VA.
 * @stack_ptr:	server user stack pointer to install.
 * @arg0:	first argument handed to the server entry.
 * @arg1:	second argument handed to the server entry.
 */
struct rt_ipc_arch_ctx {
	unsigned long	entry;
	unsigned long	stack_ptr;
	unsigned long	arg0;
	unsigned long	arg1;
};

/*
 * rt_ipc_arch_enter_server() performs the architecture-specific portion of
 * the partial context switch: it saves the subset of client registers that
 * will be clobbered into @frame->arch and installs the server entry/stack.
 *
 * It must be called with preemption enabled but from a well-defined syscall
 * context (user registers already spilled to pt_regs).  It returns 0 on a
 * clean server return, or a negative errno if the transition could not be
 * completed.
 */
int rt_ipc_arch_enter_server(struct rt_ipc_frame *frame,
			     const struct rt_ipc_arch_ctx *ctx);

/*
 * rt_ipc_arch_return() restores the client register state previously saved
 * into @frame->arch.  It is idempotent with respect to a partially completed
 * enter (safe to call on the fault/cancel path).
 */
void rt_ipc_arch_return(struct rt_ipc_frame *frame);

#else /* !CONFIG_RT_IPC */

static inline void rt_ipc_task_init(struct task_struct *tsk) { }
static inline void rt_ipc_task_exit(struct task_struct *tsk) { }

#endif /* CONFIG_RT_IPC */

#endif /* _LINUX_RT_IPC_H */

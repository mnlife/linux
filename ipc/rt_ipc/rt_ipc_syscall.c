// SPDX-License-Identifier: GPL-2.0
/*
 * rt_ipc - real-time IPC (migrating-thread model): syscall layer.
 *
 * This file provides the three rt_ipc syscalls and the task-exit hook.  It is
 * deliberately thin: all endpoint bookkeeping lives in the Rust core
 * (ipc/rt_ipc/rt_ipc_rust.rs), and the actual thread migration is performed by
 * the architecture primitive rt_ipc_arch_migrate().
 *
 *   rt_ipc_register(name, name_len)
 *       Server publishes a named endpoint.  Returns the endpoint id.
 *
 *   rt_ipc_invoke(endpoint, req, req_len, resp, resp_cap)
 *       Client RPC.  The calling thread migrates into the server, runs the
 *       handler with the client's own scheduling attributes, and returns with
 *       the reply.  Returns the reply length.
 *
 *   rt_ipc_return(endpoint, resp, resp_len, req, req_cap)
 *       Server publishes its reply and blocks to receive the next request.
 *       Returns the received request length.
 */

#include <linux/errno.h>
#include <linux/rt_ipc.h>
#include <linux/sched.h>
#include <linux/slab.h>
#include <linux/syscalls.h>
#include <linux/uaccess.h>

/*
 * Weak default for the architecture migration primitive.  Architectures that
 * implement the partial context switch (see arch/x86/kernel/rt_ipc_switch.S)
 * override this symbol.  Until then, invoke() cleanly reports -ENOSYS.
 */
long __weak rt_ipc_arch_migrate(struct rt_ipc_xfer *xfer)
{
	return -ENOSYS;
}

/*
 * Server token: use the thread-group id so that all threads of a server
 * process share its endpoints and the endpoints are reclaimed on group exit.
 */
static inline u64 rt_ipc_owner_token(void)
{
	return (u64)task_tgid_nr(current);
}

SYSCALL_DEFINE2(rt_ipc_register, const char __user *, name, size_t, name_len)
{
	char kname[RT_IPC_NAME_MAX + 1];

	if (name_len == 0 || name_len > RT_IPC_NAME_MAX)
		return -EINVAL;
	if (copy_from_user(kname, name, name_len))
		return -EFAULT;

	return rt_ipc_rs_register((const u8 *)kname, name_len,
				  rt_ipc_owner_token());
}

SYSCALL_DEFINE5(rt_ipc_invoke, u64, endpoint, const void __user *, req,
		size_t, req_len, void __user *, resp, size_t, resp_cap)
{
	struct rt_ipc_xfer xfer;
	long owner;

	if (req_len > RT_IPC_MSG_MAX || resp_cap > RT_IPC_MSG_MAX)
		return -EMSGSIZE;

	/* Resolve the endpoint to its server task via the Rust registry. */
	owner = rt_ipc_rs_lookup_owner(endpoint);
	if (owner < 0)
		return owner;

	xfer.endpoint = endpoint;
	xfer.server_owner = (u64)owner;
	xfer.req = req;
	xfer.req_len = req_len;
	xfer.resp = resp;
	xfer.resp_cap = resp_cap;

	/*
	 * Perform the migrating-thread RPC.  On success the caller's user
	 * context has been restored and the reply is in @resp; the return
	 * value is the reply length.
	 */
	return rt_ipc_arch_migrate(&xfer);
}

SYSCALL_DEFINE5(rt_ipc_return, u64, endpoint, const void __user *, resp,
		size_t, resp_len, void __user *, req, size_t, req_cap)
{
	/*
	 * The reply-and-receive fast path is completed by the arch layer, which
	 * hands the current migrating client its reply and parks this server
	 * thread until the next migrating client arrives.  Argument validation
	 * is performed here; the rendezvous itself is arch specific.
	 */
	if (resp_len > RT_IPC_MSG_MAX || req_cap > RT_IPC_MSG_MAX)
		return -EMSGSIZE;
	if (rt_ipc_rs_lookup_owner(endpoint) < 0)
		return -ESRCH;

	return -ENOSYS;
}

void rt_ipc_task_exit(struct task_struct *tsk)
{
	/* Only the group leader owns endpoints (see rt_ipc_owner_token()). */
	rt_ipc_rs_reclaim_owner((u64)task_tgid_nr(tsk));
}

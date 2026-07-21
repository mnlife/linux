// SPDX-License-Identifier: GPL-2.0
/*
 * rt_ipc migrating-thread call path.
 *
 * A synchronous RPC is executed entirely by the calling thread.  The thread:
 *
 *   1. pushes a migration frame recording the client context that the
 *      partial context switch will clobber (mm, credentials, arch registers);
 *   2. overrides its credentials with the server's (so the server entry runs
 *      with server privileges, never the client's);
 *   3. performs the architecture-specific partial context switch into the
 *      server address space (rt_ipc_arch_enter_server()), which runs the
 *      server entry passively;
 *   4. restores the client context (rt_ipc_arch_return()) and pops the frame.
 *
 * The whole sequence stays on the client's task_struct: no wakeup, no
 * rescheduling and no full context switch is performed, so the server code
 * inherits the client's scheduling attributes in place.  CPU time consumed by
 * the server is therefore charged to the client, as intended.
 *
 * Payload is transferred through a bounded kernel bounce buffer so that the
 * request/reply copies happen against the correct address space on each side
 * of the switch, without exposing raw client pointers to the server.
 */
#include <linux/sched.h>
#include <linux/sched/mm.h>
#include <linux/cred.h>
#include <linux/mm.h>
#include <linux/slab.h>
#include <linux/uaccess.h>
#include <linux/rt_ipc.h>

#include "internal.h"
#include <trace/events/rt_ipc.h>

/* Upper bound on a single request/reply payload for the foundation. */
#define RT_IPC_MAX_PAYLOAD	(64 * 1024)

static int rt_ipc_check_depth(struct rt_ipc_endpoint *ep)
{
	struct rt_ipc_task *rt = current->rt_ipc;

	if (!rt)
		return 0;

	if (rt->depth >= RT_IPC_MAX_DEPTH)
		return -ELOOP;

	/*
	 * Nested calls are only permitted when the target endpoint opted in.
	 * The outermost call (depth 0) is always allowed.
	 */
	if (rt->depth > 0 && !(ep->flags & RT_IPC_EP_ALLOW_NESTED))
		return -EPERM;

	return 0;
}

/*
 * Reserve an in-flight slot on the endpoint.  Bounds concurrency so a busy
 * server cannot be driven past its declared @max_concurrency.
 */
static int rt_ipc_reserve_slot(struct rt_ipc_endpoint *ep)
{
	int ret = 0;

	raw_spin_lock(&ep->lock);
	if (ep->dead)
		ret = -ECONNRESET;
	else if (ep->inflight >= ep->max_concurrency)
		ret = -EAGAIN;
	else
		ep->inflight++;
	raw_spin_unlock(&ep->lock);

	return ret;
}

static void rt_ipc_release_slot(struct rt_ipc_endpoint *ep)
{
	raw_spin_lock(&ep->lock);
	if (!WARN_ON_ONCE(ep->inflight == 0))
		ep->inflight--;
	raw_spin_unlock(&ep->lock);
}

long rt_ipc_do_call(struct rt_ipc_connection *conn, struct rt_ipc_call *call)
{
	struct rt_ipc_endpoint *ep = conn->ep;
	struct rt_ipc_task *rt;
	struct rt_ipc_arch_ctx ctx = {};
	struct rt_ipc_frame *frame;
	const struct cred *old_cred = NULL;
	void *bounce = NULL;
	unsigned int slot;
	long ret;

	if (call->send_len > RT_IPC_MAX_PAYLOAD ||
	    call->recv_len > RT_IPC_MAX_PAYLOAD)
		return -EMSGSIZE;

	rt = rt_ipc_task_prepare();
	if (!rt)
		return -ENOMEM;

	ret = rt_ipc_check_depth(ep);
	if (ret)
		return ret;

	ret = rt_ipc_reserve_slot(ep);
	if (ret)
		return ret;

	/*
	 * Copy the request out of the client address space *before* the
	 * partial context switch, while the client mm is still current.
	 */
	if (call->send_len) {
		bounce = kvmalloc(call->send_len, GFP_KERNEL);
		if (!bounce) {
			ret = -ENOMEM;
			goto out_slot;
		}
		if (copy_from_user(bounce,
				   (void __user *)(unsigned long)call->send_buf,
				   call->send_len)) {
			ret = -EFAULT;
			goto out_free;
		}
	}

	trace_rt_ipc_call_enter(conn->id, ep->id, rt->depth, call->send_len);

	slot = rt->depth++;
	frame = &rt->frames[slot];
	frame->depth = slot + 1;
	frame->conn = conn;
	rt_ipc_connection_get(conn);

	/*
	 * Enter the server's protection domain: run with server credentials
	 * and pin the server address space for the duration of the call.
	 */
	old_cred = override_creds(get_cred(ep->owner_cred));
	frame->saved_cred = old_cred;

	mmgrab(current->mm);
	frame->saved_mm = current->mm;

	ctx.entry = ep->entry;
	ctx.stack_ptr = ep->stack_top;
	ctx.arg0 = call->send_len;
	ctx.arg1 = 0;

	trace_rt_ipc_migrate_mm(ep->id, frame->depth);

	/*
	 * Architecture-specific partial context switch: install the server
	 * mm and a subset of registers, run the server entry, then hand back.
	 * The reply length produced by the server is returned via ctx.arg1.
	 */
	ret = rt_ipc_arch_enter_server(frame, &ctx);

	rt_ipc_arch_return(frame);

	/* Restore client credentials. */
	revert_creds(frame->saved_cred);
	put_cred(ep->owner_cred);
	if (frame->saved_mm)
		mmdrop(frame->saved_mm);

	rt_ipc_connection_put(frame->conn);
	rt->depth--;

	if (ret == 0) {
		/*
		 * Copy the reply back into the client address space, now that
		 * the client mm is current again.  ctx.arg1 carries the number
		 * of reply bytes the server produced (0 in the base mechanism
		 * until user-mode server dispatch is wired up per-arch).
		 */
		u64 reply_len = min_t(u64, ctx.arg1, call->recv_len);

		if (reply_len && bounce &&
		    copy_to_user((void __user *)(unsigned long)call->recv_buf,
				 bounce, reply_len))
			ret = -EFAULT;
		else
			call->out_recv_len = reply_len;
	}

	trace_rt_ipc_call_exit(conn->id, ep->id, frame->depth, ret);

out_free:
	kvfree(bounce);
out_slot:
	rt_ipc_release_slot(ep);
	return ret;
}

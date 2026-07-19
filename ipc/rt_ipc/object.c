// SPDX-License-Identifier: GPL-2.0
/*
 * rt_ipc object model: endpoints and connections.
 *
 * Endpoints and connections are reference counted (kref) and freed via RCU so
 * that lookups on the call fast path never take a sleeping lock.  The teardown
 * ordering is:
 *
 *   fd close -> drop the fd's reference
 *   last endpoint reference -> rt_ipc_endpoint_free() (RCU) -> mmdrop/put_cred
 *   last connection reference -> rt_ipc_connection_free() (RCU) -> ep put
 *
 * An in-flight call always holds a connection reference (which pins the
 * endpoint), so an endpoint or connection cannot be freed underneath a
 * running RPC even if userspace closes the fds concurrently.
 */
#include <linux/slab.h>
#include <linux/sched.h>
#include <linux/sched/mm.h>
#include <linux/cred.h>
#include <linux/mm.h>
#include <linux/rcupdate.h>
#include <linux/atomic.h>

#include "internal.h"

static atomic64_t rt_ipc_id_counter = ATOMIC64_INIT(0);

LIST_HEAD(rt_ipc_endpoint_list);
/* Serialises the global endpoint registry used by debugfs/statistics. */
DEFINE_MUTEX(rt_ipc_endpoint_list_lock);

u64 rt_ipc_alloc_id(void)
{
	return atomic64_inc_return(&rt_ipc_id_counter);
}

/* Per-task migrating-thread state ---------------------------------------- */

void rt_ipc_task_init(struct task_struct *tsk)
{
	tsk->rt_ipc = NULL;
}

void rt_ipc_task_exit(struct task_struct *tsk)
{
	/*
	 * A task must never exit while inside a migrating-thread call: the
	 * call path holds references and runs to completion synchronously.
	 */
	if (tsk->rt_ipc) {
		WARN_ON_ONCE(tsk->rt_ipc->depth != 0);
		kfree(tsk->rt_ipc);
		tsk->rt_ipc = NULL;
	}
}

struct rt_ipc_task *rt_ipc_task_prepare(void)
{
	struct rt_ipc_task *rt = current->rt_ipc;

	if (rt)
		return rt;

	rt = kzalloc_obj(*rt);
	if (!rt)
		return NULL;

	current->rt_ipc = rt;
	return rt;
}

static void rt_ipc_endpoint_free_rcu(struct rcu_head *rcu)
{
	struct rt_ipc_endpoint *ep =
		container_of(rcu, struct rt_ipc_endpoint, rcu);

	if (ep->owner_cred)
		put_cred(ep->owner_cred);
	if (ep->owner_mm)
		mmdrop(ep->owner_mm);
	kfree(ep);
}

static void rt_ipc_endpoint_release(struct kref *kref)
{
	struct rt_ipc_endpoint *ep =
		container_of(kref, struct rt_ipc_endpoint, kref);

	rt_ipc_debugfs_del_endpoint(ep);

	mutex_lock(&rt_ipc_endpoint_list_lock);
	list_del(&ep->registry_node);
	mutex_unlock(&rt_ipc_endpoint_list_lock);

	call_rcu(&ep->rcu, rt_ipc_endpoint_free_rcu);
}

void rt_ipc_endpoint_get(struct rt_ipc_endpoint *ep)
{
	kref_get(&ep->kref);
}

void rt_ipc_endpoint_put(struct rt_ipc_endpoint *ep)
{
	kref_put(&ep->kref, rt_ipc_endpoint_release);
}

/*
 * Validate a user-provided server entry description against the current
 * address space.  The entry and the whole stack region must lie in user VA
 * space; we do not dereference them here, only bounds-check.
 */
static int rt_ipc_validate_entry(const struct rt_ipc_endpoint_create *req)
{
	unsigned long stack_end;

	if (!req->entry || !req->stack_top || !req->stack_size)
		return -EINVAL;

	/* Entry must be a user address. */
	if (req->entry >= TASK_SIZE)
		return -EFAULT;

	/* Stack region [stack_top - stack_size, stack_top) must be user VA. */
	if (req->stack_size > (unsigned long)(TASK_SIZE))
		return -EINVAL;
	if (check_sub_overflow(req->stack_top, req->stack_size, &stack_end))
		return -EINVAL;
	if (req->stack_top > TASK_SIZE)
		return -EFAULT;

	/* A single call needs at least one stack slot. */
	if (req->max_concurrency == 0)
		return -EINVAL;

	return 0;
}

struct rt_ipc_endpoint *
rt_ipc_endpoint_create(const struct rt_ipc_endpoint_create *req)
{
	struct rt_ipc_endpoint *ep;
	int err;

	if (req->flags & ~RT_IPC_EP_FLAGS_ALL)
		return ERR_PTR(-EINVAL);
	if (req->__reserved)
		return ERR_PTR(-EINVAL);

	err = rt_ipc_validate_entry(req);
	if (err)
		return ERR_PTR(err);

	if (!current->mm)
		return ERR_PTR(-EINVAL);

	ep = kzalloc_obj(*ep);
	if (!ep)
		return ERR_PTR(-ENOMEM);

	kref_init(&ep->kref);
	raw_spin_lock_init(&ep->lock);
	INIT_LIST_HEAD(&ep->conns);
	INIT_LIST_HEAD(&ep->registry_node);
	ep->id = rt_ipc_alloc_id();
	ep->entry = req->entry;
	ep->stack_top = req->stack_top;
	ep->stack_size = req->stack_size;
	ep->max_concurrency = req->max_concurrency;
	ep->flags = req->flags;

	/*
	 * Pin the owner's address space and credentials.  mmgrab() takes a
	 * reference on the mm_struct itself (not the page tables); we upgrade
	 * to an mm_users reference transiently during a call in migrate.c.
	 */
	mmgrab(current->mm);
	ep->owner_mm = current->mm;
	ep->owner_cred = get_current_cred();

	mutex_lock(&rt_ipc_endpoint_list_lock);
	list_add_tail(&ep->registry_node, &rt_ipc_endpoint_list);
	mutex_unlock(&rt_ipc_endpoint_list_lock);

	rt_ipc_debugfs_add_endpoint(ep);

	return ep;
}

void rt_ipc_endpoint_shutdown(struct rt_ipc_endpoint *ep)
{
	raw_spin_lock(&ep->lock);
	ep->dead = true;
	raw_spin_unlock(&ep->lock);
}

/* Connections ------------------------------------------------------------ */

static void rt_ipc_connection_free_rcu(struct rcu_head *rcu)
{
	struct rt_ipc_connection *conn =
		container_of(rcu, struct rt_ipc_connection, rcu);

	if (conn->client_mm)
		mmdrop(conn->client_mm);
	if (conn->ep)
		rt_ipc_endpoint_put(conn->ep);
	kfree(conn);
}

static void rt_ipc_connection_release(struct kref *kref)
{
	struct rt_ipc_connection *conn =
		container_of(kref, struct rt_ipc_connection, kref);
	struct rt_ipc_endpoint *ep = conn->ep;

	if (ep) {
		raw_spin_lock(&ep->lock);
		list_del_rcu(&conn->node);
		raw_spin_unlock(&ep->lock);
	}

	call_rcu(&conn->rcu, rt_ipc_connection_free_rcu);
}

void rt_ipc_connection_get(struct rt_ipc_connection *conn)
{
	kref_get(&conn->kref);
}

void rt_ipc_connection_put(struct rt_ipc_connection *conn)
{
	kref_put(&conn->kref, rt_ipc_connection_release);
}

struct rt_ipc_connection *rt_ipc_connection_create(struct rt_ipc_endpoint *ep)
{
	struct rt_ipc_connection *conn;

	if (!current->mm)
		return ERR_PTR(-EINVAL);

	raw_spin_lock(&ep->lock);
	if (ep->dead) {
		raw_spin_unlock(&ep->lock);
		return ERR_PTR(-ECONNREFUSED);
	}
	raw_spin_unlock(&ep->lock);

	conn = kzalloc_obj(*conn);
	if (!conn)
		return ERR_PTR(-ENOMEM);

	kref_init(&conn->kref);
	conn->id = rt_ipc_alloc_id();

	rt_ipc_endpoint_get(ep);
	conn->ep = ep;

	mmgrab(current->mm);
	conn->client_mm = current->mm;

	raw_spin_lock(&ep->lock);
	if (ep->dead) {
		raw_spin_unlock(&ep->lock);
		rt_ipc_connection_put(conn);
		return ERR_PTR(-ECONNREFUSED);
	}
	list_add_tail_rcu(&conn->node, &ep->conns);
	raw_spin_unlock(&ep->lock);

	return conn;
}

// SPDX-License-Identifier: GPL-2.0
/*
 * rt_ipc debugfs introspection: expose live endpoints and their in-flight
 * state under /sys/kernel/debug/rt_ipc/endpoints.
 */
#include <linux/debugfs.h>
#include <linux/seq_file.h>
#include <linux/mutex.h>
#include <linux/list.h>

#include "internal.h"

static struct dentry *rt_ipc_debugfs_dir;

static int rt_ipc_endpoints_show(struct seq_file *m, void *v)
{
	struct rt_ipc_endpoint *ep;

	seq_printf(m, "%-8s %-10s %-10s %-6s %-6s %-6s\n",
		   "id", "entry", "flags", "max", "inflt", "dead");

	mutex_lock(&rt_ipc_endpoint_list_lock);
	list_for_each_entry(ep, &rt_ipc_endpoint_list, registry_node) {
		u32 inflight;
		bool dead;

		raw_spin_lock(&ep->lock);
		inflight = ep->inflight;
		dead = ep->dead;
		raw_spin_unlock(&ep->lock);

		seq_printf(m, "%-8llu 0x%-8lx 0x%-8x %-6u %-6u %-6u\n",
			   ep->id, ep->entry, ep->flags,
			   ep->max_concurrency, inflight, dead);
	}
	mutex_unlock(&rt_ipc_endpoint_list_lock);

	return 0;
}
DEFINE_SHOW_ATTRIBUTE(rt_ipc_endpoints);

void rt_ipc_debugfs_init(void)
{
	rt_ipc_debugfs_dir = debugfs_create_dir("rt_ipc", NULL);
	debugfs_create_file("endpoints", 0444, rt_ipc_debugfs_dir, NULL,
			    &rt_ipc_endpoints_fops);
}

void rt_ipc_debugfs_exit(void)
{
	debugfs_remove_recursive(rt_ipc_debugfs_dir);
	rt_ipc_debugfs_dir = NULL;
}

/*
 * The registry list itself is maintained unconditionally in object.c, so the
 * add/del hooks here are only needed if per-endpoint debugfs files are added
 * later.  They are intentionally no-ops for now.
 */
void rt_ipc_debugfs_add_endpoint(struct rt_ipc_endpoint *ep) { }
void rt_ipc_debugfs_del_endpoint(struct rt_ipc_endpoint *ep) { }

/* SPDX-License-Identifier: GPL-2.0 */
/*
 * rt_ipc internal definitions shared between the subsystem source files.
 */
#ifndef _IPC_RT_IPC_INTERNAL_H
#define _IPC_RT_IPC_INTERNAL_H

#include <linux/rt_ipc.h>
#include <uapi/linux/rt_ipc.h>

struct file;

/* object.c: endpoint / connection lifetime ------------------------------- */

struct rt_ipc_endpoint *rt_ipc_endpoint_create(
		const struct rt_ipc_endpoint_create *req);
void rt_ipc_endpoint_get(struct rt_ipc_endpoint *ep);
void rt_ipc_endpoint_put(struct rt_ipc_endpoint *ep);
void rt_ipc_endpoint_shutdown(struct rt_ipc_endpoint *ep);

struct rt_ipc_connection *rt_ipc_connection_create(struct rt_ipc_endpoint *ep);
void rt_ipc_connection_get(struct rt_ipc_connection *conn);
void rt_ipc_connection_put(struct rt_ipc_connection *conn);

/* fd helpers implemented in core.c, used by object.c and connect path. */
struct rt_ipc_endpoint *rt_ipc_endpoint_from_fd(int fd);
int rt_ipc_endpoint_install_fd(struct rt_ipc_endpoint *ep);
int rt_ipc_connection_install_fd(struct rt_ipc_connection *conn);

/* migrate.c: the synchronous migrating-thread call path ------------------ */

long rt_ipc_do_call(struct rt_ipc_connection *conn,
		    struct rt_ipc_call *call);

/* debugfs.c -------------------------------------------------------------- */

#ifdef CONFIG_RT_IPC_DEBUGFS
void rt_ipc_debugfs_init(void);
void rt_ipc_debugfs_exit(void);
void rt_ipc_debugfs_add_endpoint(struct rt_ipc_endpoint *ep);
void rt_ipc_debugfs_del_endpoint(struct rt_ipc_endpoint *ep);
#else
static inline void rt_ipc_debugfs_init(void) { }
static inline void rt_ipc_debugfs_exit(void) { }
static inline void rt_ipc_debugfs_add_endpoint(struct rt_ipc_endpoint *ep) { }
static inline void rt_ipc_debugfs_del_endpoint(struct rt_ipc_endpoint *ep) { }
#endif

/* Shared allocator for stable object identifiers (tracing/debugfs). */
u64 rt_ipc_alloc_id(void);

/* Live-endpoint registry maintained for debugfs/statistics. */
extern struct list_head rt_ipc_endpoint_list;
extern struct mutex rt_ipc_endpoint_list_lock;

/* Per-task migrating-thread state (lazily allocated on first call). */
struct rt_ipc_task *rt_ipc_task_prepare(void);

#endif /* _IPC_RT_IPC_INTERNAL_H */

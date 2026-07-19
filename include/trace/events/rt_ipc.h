/* SPDX-License-Identifier: GPL-2.0 */
#undef TRACE_SYSTEM
#define TRACE_SYSTEM rt_ipc

#if !defined(_TRACE_RT_IPC_H) || defined(TRACE_HEADER_MULTI_READ)
#define _TRACE_RT_IPC_H

#include <linux/tracepoint.h>

TRACE_EVENT(rt_ipc_call_enter,
	TP_PROTO(u64 conn_id, u64 ep_id, unsigned int depth, u64 send_len),
	TP_ARGS(conn_id, ep_id, depth, send_len),
	TP_STRUCT__entry(
		__field(u64, conn_id)
		__field(u64, ep_id)
		__field(unsigned int, depth)
		__field(u64, send_len)
	),
	TP_fast_assign(
		__entry->conn_id = conn_id;
		__entry->ep_id = ep_id;
		__entry->depth = depth;
		__entry->send_len = send_len;
	),
	TP_printk("conn=%llu ep=%llu depth=%u send_len=%llu",
		  __entry->conn_id, __entry->ep_id, __entry->depth,
		  __entry->send_len)
);

TRACE_EVENT(rt_ipc_call_exit,
	TP_PROTO(u64 conn_id, u64 ep_id, unsigned int depth, int ret),
	TP_ARGS(conn_id, ep_id, depth, ret),
	TP_STRUCT__entry(
		__field(u64, conn_id)
		__field(u64, ep_id)
		__field(unsigned int, depth)
		__field(int, ret)
	),
	TP_fast_assign(
		__entry->conn_id = conn_id;
		__entry->ep_id = ep_id;
		__entry->depth = depth;
		__entry->ret = ret;
	),
	TP_printk("conn=%llu ep=%llu depth=%u ret=%d",
		  __entry->conn_id, __entry->ep_id, __entry->depth,
		  __entry->ret)
);

TRACE_EVENT(rt_ipc_migrate_mm,
	TP_PROTO(u64 ep_id, unsigned int depth),
	TP_ARGS(ep_id, depth),
	TP_STRUCT__entry(
		__field(u64, ep_id)
		__field(unsigned int, depth)
	),
	TP_fast_assign(
		__entry->ep_id = ep_id;
		__entry->depth = depth;
	),
	TP_printk("ep=%llu depth=%u", __entry->ep_id, __entry->depth)
);

TRACE_EVENT(rt_ipc_fallback_to_sched,
	TP_PROTO(u64 ep_id, unsigned int depth, const char *reason),
	TP_ARGS(ep_id, depth, reason),
	TP_STRUCT__entry(
		__field(u64, ep_id)
		__field(unsigned int, depth)
		__string(reason, reason)
	),
	TP_fast_assign(
		__entry->ep_id = ep_id;
		__entry->depth = depth;
		__assign_str(reason);
	),
	TP_printk("ep=%llu depth=%u reason=%s",
		  __entry->ep_id, __entry->depth, __get_str(reason))
);

#endif /* _TRACE_RT_IPC_H */

/* This part must be outside protection */
#include <trace/define_trace.h>

// SPDX-License-Identifier: GPL-2.0
/*
 * ARM (32-bit) implementation of the rt_ipc partial context switch.
 *
 * This file provides the architecture hooks invoked by the generic call path
 * in ipc/rt_ipc/migrate.c:
 *
 *   rt_ipc_arch_enter_server() - capture the client register subset that the
 *       switch will clobber, then transition into the server protection
 *       domain (address space + user stack + entry PC).
 *   rt_ipc_arch_return()       - restore the captured client register subset.
 *
 * Design notes (ARM):
 *
 *   * The switch happens in syscall context, so the full client user register
 *     file is already spilled to the task's pt_regs.  We only need to snapshot
 *     the subset we overwrite (PC/SP and the two user TLS registers) so it can
 *     be put back precisely, even if the call is preempted, migrated to
 *     another CPU, interrupted by a signal, or the server faults.
 *
 *   * The address-space switch must go through switch_mm() so that the ASID,
 *     lazy-TLB accounting, membarrier state and TLB maintenance invariants are
 *     all maintained -- never a bare TTBR0 write.
 *
 *   * TPIDRURO (the read-only TLS register) is shadowed in thread_info and
 *     restored through set_tls(); TPIDRURW (the user read/write TLS register)
 *     is fully context-switched and restored through set_tpuser().
 *
 *   * Running the server's *user-mode* entry requires the low-level entry
 *     trampoline work tracked as a follow-up milestone.  Until that lands this
 *     hook validates and snapshots state and reports -EOPNOTSUPP so that no
 *     partially-switched state can ever be observed by userspace.  The generic
 *     path treats this as a clean (data-less) failure and fully restores the
 *     client context.
 */
#include <linux/sched.h>
#include <linux/sched/task_stack.h>
#include <linux/ptrace.h>
#include <linux/slab.h>
#include <linux/errno.h>
#include <linux/thread_info.h>
#include <linux/rt_ipc.h>
#include <asm/rt_ipc.h>
#include <asm/ptrace.h>
#include <asm/tls.h>

int rt_ipc_arch_enter_server(struct rt_ipc_frame *frame,
			     const struct rt_ipc_arch_ctx *ctx)
{
	struct rt_ipc_arch_regs *regs;
	struct pt_regs *uregs = current_pt_regs();

	regs = kzalloc_obj(*regs);
	if (!regs)
		return -ENOMEM;

	/* Snapshot exactly the client subset the switch would overwrite. */
	regs->pc = uregs->ARM_pc;
	regs->sp = uregs->ARM_sp;
	regs->tls = current_thread_info()->tp_value[0];
	regs->tpuser = get_tpuser();
	regs->saved = true;

	frame->arch = regs;

	/*
	 * TODO(rt_ipc): perform the address-space switch via switch_mm() and
	 * re-enter userspace at ctx->entry with ctx->stack_ptr installed.
	 * Until the entry-trampoline support is in place, refuse cleanly
	 * without touching TTBR0 so userspace never sees a half-migrated
	 * thread.
	 */
	(void)ctx;
	return -EOPNOTSUPP;
}

void rt_ipc_arch_return(struct rt_ipc_frame *frame)
{
	struct rt_ipc_arch_regs *regs = frame->arch;
	struct pt_regs *uregs = current_pt_regs();

	if (!regs)
		return;

	if (regs->saved) {
		/*
		 * Restore the client register subset.  Idempotent: safe on the
		 * fault/cancel path even if the enter aborted early.
		 */
		uregs->ARM_pc = regs->pc;
		uregs->ARM_sp = regs->sp;
		set_tls(regs->tls);
		set_tpuser(regs->tpuser);
	}

	kfree(regs);
	frame->arch = NULL;
}

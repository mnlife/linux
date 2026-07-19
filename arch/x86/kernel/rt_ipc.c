// SPDX-License-Identifier: GPL-2.0
/*
 * x86-64 implementation of the rt_ipc partial context switch.
 *
 * This file provides the architecture hooks invoked by the generic call path
 * in ipc/rt_ipc/migrate.c:
 *
 *   rt_ipc_arch_enter_server() - capture the client register subset that the
 *       switch will clobber, then transition into the server protection
 *       domain (address space + user stack + entry RIP).
 *   rt_ipc_arch_return()       - restore the captured client register subset.
 *
 * Design notes (x86-64):
 *
 *   * The switch happens in syscall context, so the full client user register
 *     file is already spilled to the task's pt_regs.  We only need to snapshot
 *     the subset we overwrite (RIP/RSP and the TLS/GS bases) so it can be put
 *     back precisely, even if the call is preempted, migrated to another CPU,
 *     interrupted by a signal, or the server faults.
 *
 *   * The address-space switch must go through switch_mm_irqs_off() so that
 *     mm_cpumask, lazy-TLB accounting, membarrier state and TLB shootdown
 *     invariants are all maintained -- never a bare CR3 write.
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
#include <linux/rt_ipc.h>
#include <asm/rt_ipc.h>
#include <asm/ptrace.h>
#include <asm/fsgsbase.h>
#include <asm/msr.h>
#include <asm/msr-index.h>

int rt_ipc_arch_enter_server(struct rt_ipc_frame *frame,
			     const struct rt_ipc_arch_ctx *ctx)
{
	struct rt_ipc_arch_regs *regs;
	struct pt_regs *uregs = current_pt_regs();

	regs = kzalloc(sizeof(*regs), GFP_KERNEL);
	if (!regs)
		return -ENOMEM;

	/* Snapshot exactly the client subset the switch would overwrite. */
	regs->ip = uregs->ip;
	regs->sp = uregs->sp;
	regs->fsbase = x86_fsbase_read_cpu();
	rdmsrq(MSR_KERNEL_GS_BASE, regs->gsbase);
	regs->saved = true;

	frame->arch = regs;

	/*
	 * TODO(rt_ipc): perform the address-space switch via
	 * switch_mm_irqs_off() and re-enter userspace at ctx->entry with
	 * ctx->stack_ptr installed.  Until the entry-trampoline support is in
	 * place, refuse cleanly without touching CR3 so userspace never sees a
	 * half-migrated thread.
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
		uregs->ip = regs->ip;
		uregs->sp = regs->sp;
		x86_fsbase_write_cpu(regs->fsbase);
		wrmsrq(MSR_KERNEL_GS_BASE, regs->gsbase);
	}

	kfree(regs);
	frame->arch = NULL;
}

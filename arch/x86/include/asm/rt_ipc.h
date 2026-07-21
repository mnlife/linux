/* SPDX-License-Identifier: GPL-2.0 */
/*
 * x86-64 architecture hooks for rt_ipc's partial context switch.
 *
 * The migrating-thread model switches the address space and a *subset* of the
 * CPU register/user state (user stack pointer, TLS base, entry RIP) without
 * switching threads, priorities or invoking the scheduler.  This header
 * defines the x86-specific saved-state container; the logic lives in
 * arch/x86/kernel/rt_ipc.c.
 */
#ifndef _ASM_X86_RT_IPC_H
#define _ASM_X86_RT_IPC_H

#include <linux/types.h>

/**
 * struct rt_ipc_arch_regs - x86-64 client register subset saved across a call.
 * @ip:		client user instruction pointer (return target).
 * @sp:		client user stack pointer.
 * @fsbase:	client FS base (TLS) MSR value.
 * @gsbase:	client user GS base MSR value.
 * @saved:	true once the subset has been captured (return is idempotent).
 *
 * FPU/SIMD, PKRU and CET (shadow stack) state are intentionally *not* part of
 * this structure: they are handled separately when the user-mode server
 * dispatch is wired up, because each requires distinct save/restore and
 * validation policy (see Documentation/rt_ipc/design.rst).
 */
struct rt_ipc_arch_regs {
	unsigned long	ip;
	unsigned long	sp;
	unsigned long	fsbase;
	unsigned long	gsbase;
	bool		saved;
};

#endif /* _ASM_X86_RT_IPC_H */

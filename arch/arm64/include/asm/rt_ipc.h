/* SPDX-License-Identifier: GPL-2.0 */
/*
 * arm64 architecture hooks for rt_ipc's partial context switch.
 *
 * The migrating-thread model switches the address space and a *subset* of the
 * CPU register/user state (user stack pointer, TLS base, entry PC) without
 * switching threads, priorities or invoking the scheduler.  This header
 * defines the arm64-specific saved-state container; the logic lives in
 * arch/arm64/kernel/rt_ipc.c.
 */
#ifndef _ASM_ARM64_RT_IPC_H
#define _ASM_ARM64_RT_IPC_H

#include <linux/types.h>

/**
 * struct rt_ipc_arch_regs - arm64 client register subset saved across a call.
 * @pc:		client user program counter (return target).
 * @sp:		client user stack pointer.
 * @tpidr:	client TPIDR_EL0 (TLS base) register value.
 * @saved:	true once the subset has been captured (return is idempotent).
 *
 * FPU/SIMD/SVE, pointer-authentication keys and MTE tag state are
 * intentionally *not* part of this structure: they are handled separately
 * when the user-mode server dispatch is wired up, because each requires
 * distinct save/restore and validation policy (see
 * Documentation/rt_ipc/design.rst).
 */
struct rt_ipc_arch_regs {
	unsigned long	pc;
	unsigned long	sp;
	unsigned long	tpidr;
	bool		saved;
};

#endif /* _ASM_ARM64_RT_IPC_H */

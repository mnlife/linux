/* SPDX-License-Identifier: GPL-2.0 */
/*
 * ARM (32-bit) architecture hooks for rt_ipc's partial context switch.
 *
 * The migrating-thread model switches the address space and a *subset* of the
 * CPU register/user state (user stack pointer, TLS registers, entry PC)
 * without switching threads, priorities or invoking the scheduler.  This
 * header defines the ARM-specific saved-state container; the logic lives in
 * arch/arm/kernel/rt_ipc.c.
 */
#ifndef _ASM_ARM_RT_IPC_H
#define _ASM_ARM_RT_IPC_H

#include <linux/types.h>

/**
 * struct rt_ipc_arch_regs - ARM client register subset saved across a call.
 * @pc:		client user program counter (return target).
 * @sp:		client user stack pointer.
 * @tls:	client TPIDRURO (read-only TLS) register value.
 * @tpuser:	client TPIDRURW (user read/write TLS) register value.
 * @saved:	true once the subset has been captured (return is idempotent).
 *
 * VFP/NEON state is intentionally *not* part of this structure: it is handled
 * separately when the user-mode server dispatch is wired up, because it
 * requires distinct save/restore and validation policy (see
 * Documentation/rt_ipc/design.rst).
 */
struct rt_ipc_arch_regs {
	unsigned long	pc;
	unsigned long	sp;
	unsigned long	tls;
	unsigned long	tpuser;
	bool		saved;
};

#endif /* _ASM_ARM_RT_IPC_H */

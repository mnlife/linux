/* SPDX-License-Identifier: GPL-2.0 */
/*
 * RISC-V architecture hooks for rt_ipc's partial context switch.
 *
 * The migrating-thread model switches the address space and a *subset* of the
 * CPU register/user state (user stack pointer, thread pointer, entry PC)
 * without switching threads, priorities or invoking the scheduler.  This
 * header defines the RISC-V-specific saved-state container; the logic lives in
 * arch/riscv/kernel/rt_ipc.c.
 */
#ifndef _ASM_RISCV_RT_IPC_H
#define _ASM_RISCV_RT_IPC_H

#include <linux/types.h>

/**
 * struct rt_ipc_arch_regs - RISC-V client register subset saved across a call.
 * @pc:		client user program counter (return target; sepc on entry).
 * @sp:		client user stack pointer (x2).
 * @tp:		client thread pointer (x4), which anchors user TLS.
 * @saved:	true once the subset has been captured (return is idempotent).
 *
 * FP/vector state and pointer-masking configuration are intentionally *not*
 * part of this structure: they are handled separately when the user-mode
 * server dispatch is wired up, because each requires distinct save/restore and
 * validation policy (see Documentation/rt_ipc/design.rst).
 */
struct rt_ipc_arch_regs {
	unsigned long	pc;
	unsigned long	sp;
	unsigned long	tp;
	bool		saved;
};

#endif /* _ASM_RISCV_RT_IPC_H */

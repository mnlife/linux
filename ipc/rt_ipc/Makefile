# SPDX-License-Identifier: GPL-2.0
#
# Makefile for rt_ipc (real-time IPC, migrating-thread model).
#
# The policy core (rt_ipc_core.rs) is compiled as a submodule of the Rust
# kernel object rt_ipc_rust.o; the thin syscall layer is C.
#

obj-$(CONFIG_RT_IPC) += rt_ipc_rust.o
obj-$(CONFIG_RT_IPC) += rt_ipc_syscall.o

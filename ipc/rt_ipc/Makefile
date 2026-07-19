# SPDX-License-Identifier: GPL-2.0
#
# Makefile for rt_ipc, real-time IPC using the migrating-thread model.
#

obj-$(CONFIG_RT_IPC)		+= rt_ipc.o

rt_ipc-y			:= core.o object.o migrate.o
rt_ipc-$(CONFIG_RT_IPC_DEBUGFS)	+= debugfs.o

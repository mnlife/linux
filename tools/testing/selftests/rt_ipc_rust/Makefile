# SPDX-License-Identifier: GPL-2.0
# The rt_ipc_rust subsystem is written in Rust, so its selftest is too: the
# test program is compiled from Rust source with rustc instead of the default
# C rule, hence OVERRIDE_TARGETS.
OVERRIDE_TARGETS := 1

TEST_GEN_PROGS := rt_ipc_rust_test

RUSTC ?= rustc
# Optimised build with debug info, pinned to a stable edition.
RUSTFLAGS ?= -O -g --edition 2021

include ../lib.mk

$(OUTPUT)/rt_ipc_rust_test: rt_ipc_rust_test.rs
	$(call msg,RUSTC,,$@)
	$(Q)$(RUSTC) $(RUSTFLAGS) -o $@ $<

# SPDX-License-Identifier: GPL-2.0
#
# Selftests for rt_ipc (real-time IPC, migrating-thread model).
#
# The userspace reference library, demo and test suite are a Cargo workspace
# under rust/.  This Makefile builds that workspace and exposes the test runner
# to the kselftest framework.  The tests run on the portable AF_UNIX reference
# transport, so they pass on any kernel; on a kernel built with CONFIG_RT_IPC
# set RT_IPC_ENABLE_KERNEL=1 to exercise the migrating-thread fast path.

CARGO ?= cargo
RUST_DIR := rust

TEST_PROGS := run_rt_ipc_test.sh
TEST_FILES := $(RUST_DIR)

include ../lib.mk

.PHONY: cargo_build cargo_clean

all: cargo_build

cargo_build:
	$(CARGO) build --release --manifest-path $(RUST_DIR)/Cargo.toml

clean: cargo_clean

cargo_clean:
	-$(CARGO) clean --manifest-path $(RUST_DIR)/Cargo.toml

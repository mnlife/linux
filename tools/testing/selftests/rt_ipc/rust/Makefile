# SPDX-License-Identifier: GPL-2.0
#
# Build/test wrapper for the rt_ipc Rust userspace reference implementation
# and test runner.  Kept separate from the kselftest C Makefile so that the
# main selftest build never hard-depends on a Rust toolchain.
#
CARGO ?= cargo

.PHONY: all build test clean fmt clippy

all: build

build:
	$(CARGO) build --release

# Device-independent unit/integration tests (live cases skip without
# /dev/rt_ipc).  Add --offline in sandboxes without crates.io access.
test:
	$(CARGO) test

fmt:
	$(CARGO) fmt --check

clippy:
	$(CARGO) clippy

clean:
	$(CARGO) clean

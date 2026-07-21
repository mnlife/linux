#!/bin/sh
# SPDX-License-Identifier: GPL-2.0
#
# kselftest runner for rt_ipc.  Locates the Cargo-built test binary and runs
# the correctness suite plus the rt_ipc-vs-sockets benchmark.  Emits TAP output
# and exits non-zero on failure (kselftest treats that as a failed test).

set -eu

# Resolve the directory of this script so it works regardless of CWD.
here="$(cd "$(dirname "$0")" && pwd)"

# Prefer the release build; fall back to debug for local iteration.
bin=""
for candidate in \
	"$here/rust/target/release/rt_ipc_test" \
	"$here/rust/target/debug/rt_ipc_test"; do
	if [ -x "$candidate" ]; then
		bin="$candidate"
		break
	fi
done

if [ -z "$bin" ]; then
	echo "# rt_ipc_test binary not found; run 'make' first" >&2
	# kselftest skip code.
	exit 4
fi

# Keep the default iteration count modest so the benchmark stays quick in CI;
# override with RT_IPC_BENCH_ITERS for a heavier run.
export RT_IPC_BENCH_ITERS="${RT_IPC_BENCH_ITERS:-100000}"

exec "$bin"

// SPDX-License-Identifier: GPL-2.0
/*
 * Selftests for rt_ipc_rust (Rust reimplementation of the migrating-thread IPC).
 *
 * These exercise the /dev/rt_ipc_rust control device and its handle-based
 * ioctl uAPI: version query, endpoint creation and argument validation,
 * connection setup and the synchronous call path.
 *
 * The user-mode server dispatch is a follow-up milestone; until it lands a
 * well-formed RT_IPC_RUST_CALL is expected to be refused with EOPNOTSUPP.
 */
#include <errno.h>
#include <fcntl.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/ioctl.h>

#include <linux/rt_ipc_rust.h>

#include "../kselftest_harness.h"

#define RT_IPC_RUST_DEV	"/dev/rt_ipc_rust"

static char server_stack[64 * 1024];

/* A dummy server entry address; never actually invoked in the foundation. */
static void server_entry(void)
{
}

static int open_dev(struct __test_metadata *_metadata)
{
	int fd = open(RT_IPC_RUST_DEV, O_RDWR | O_CLOEXEC);

	if (fd < 0)
		SKIP(return -1, "%s unavailable (errno=%d)", RT_IPC_RUST_DEV,
		     errno);
	return fd;
}

static void fill_ep_req(struct rt_ipc_rust_endpoint_create *req)
{
	memset(req, 0, sizeof(*req));
	req->size = sizeof(*req);
	req->flags = RT_IPC_RUST_EP_SERVER_CREDS;
	req->entry = (unsigned long)&server_entry;
	req->stack_top = (unsigned long)server_stack + sizeof(server_stack);
	req->stack_size = sizeof(server_stack);
	req->max_concurrency = 4;
}

/* Create an endpoint and return its handle (> 0). */
static long create_endpoint(int dev)
{
	struct rt_ipc_rust_endpoint_create req;

	fill_ep_req(&req);
	return ioctl(dev, RT_IPC_RUST_ENDPOINT_CREATE, &req);
}

/* Connect to an endpoint handle and return the connection handle (> 0). */
static long connect_endpoint(int dev, long endpoint)
{
	struct rt_ipc_rust_connect req;

	memset(&req, 0, sizeof(req));
	req.size = sizeof(req);
	req.endpoint = endpoint;
	return ioctl(dev, RT_IPC_RUST_ENDPOINT_CONNECT, &req);
}

TEST(version)
{
	__u32 version = 0;
	int dev = open_dev(_metadata);

	ASSERT_EQ(0, ioctl(dev, RT_IPC_RUST_GET_VERSION, &version));
	EXPECT_EQ(RT_IPC_RUST_ABI_VERSION, version);
	close(dev);
}

TEST(endpoint_create_ok)
{
	int dev = open_dev(_metadata);
	long handle = create_endpoint(dev);

	ASSERT_GT(handle, 0);
	close(dev);
}

TEST(endpoint_create_rejects_bad_size)
{
	struct rt_ipc_rust_endpoint_create req;
	int dev = open_dev(_metadata);

	fill_ep_req(&req);
	req.size = 1;
	EXPECT_EQ(-1, ioctl(dev, RT_IPC_RUST_ENDPOINT_CREATE, &req));
	EXPECT_EQ(EINVAL, errno);
	close(dev);
}

TEST(endpoint_create_rejects_bad_flags)
{
	struct rt_ipc_rust_endpoint_create req;
	int dev = open_dev(_metadata);

	fill_ep_req(&req);
	req.flags = 0xffffffff;
	EXPECT_EQ(-1, ioctl(dev, RT_IPC_RUST_ENDPOINT_CREATE, &req));
	EXPECT_EQ(EINVAL, errno);
	close(dev);
}

TEST(endpoint_create_rejects_zero_entry)
{
	struct rt_ipc_rust_endpoint_create req;
	int dev = open_dev(_metadata);

	fill_ep_req(&req);
	req.entry = 0;
	EXPECT_EQ(-1, ioctl(dev, RT_IPC_RUST_ENDPOINT_CREATE, &req));
	EXPECT_EQ(EINVAL, errno);
	close(dev);
}

TEST(connect_ok)
{
	int dev = open_dev(_metadata);
	long endpoint = create_endpoint(dev);
	long conn;

	ASSERT_GT(endpoint, 0);
	conn = connect_endpoint(dev, endpoint);
	ASSERT_GT(conn, 0);
	close(dev);
}

TEST(connect_rejects_bad_handle)
{
	int dev = open_dev(_metadata);

	EXPECT_EQ(-1, connect_endpoint(dev, 999999));
	EXPECT_EQ(EINVAL, errno);
	close(dev);
}

TEST(call_reports_unsupported)
{
	struct rt_ipc_rust_call call;
	int dev = open_dev(_metadata);
	long endpoint = create_endpoint(dev);
	long conn;

	ASSERT_GT(endpoint, 0);
	conn = connect_endpoint(dev, endpoint);
	ASSERT_GT(conn, 0);

	memset(&call, 0, sizeof(call));
	call.size = sizeof(call);
	call.connection = conn;
	call.timeout_ms = -1;

	/*
	 * The partial context switch into the server is not wired up yet, so a
	 * well-formed call is refused cleanly with EOPNOTSUPP.
	 */
	EXPECT_EQ(-1, ioctl(dev, RT_IPC_RUST_CALL, &call));
	EXPECT_EQ(EOPNOTSUPP, errno);
	close(dev);
}

TEST(call_rejects_oversized_payload)
{
	struct rt_ipc_rust_call call;
	int dev = open_dev(_metadata);
	long endpoint = create_endpoint(dev);
	long conn;

	ASSERT_GT(endpoint, 0);
	conn = connect_endpoint(dev, endpoint);
	ASSERT_GT(conn, 0);

	memset(&call, 0, sizeof(call));
	call.size = sizeof(call);
	call.connection = conn;
	call.send_len = 1024 * 1024; /* exceeds the 64 KiB bound */

	EXPECT_EQ(-1, ioctl(dev, RT_IPC_RUST_CALL, &call));
	EXPECT_EQ(EMSGSIZE, errno);
	close(dev);
}

TEST_HARNESS_MAIN

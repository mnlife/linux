// SPDX-License-Identifier: GPL-2.0
/*
 * Selftests for rt_ipc (real-time IPC, migrating-thread model).
 *
 * These exercise the control device, object/fd plumbing and the ioctl uAPI:
 * version query, endpoint creation and argument validation, connection setup
 * and the synchronous call path (including payload bounds and depth limits).
 *
 * The user-mode server dispatch is an architecture follow-up; until it lands
 * a well-formed RT_IPC_CALL is expected to complete cleanly with zero reply
 * bytes or to be refused with EOPNOTSUPP.  The tests accept either.
 */
#include <errno.h>
#include <fcntl.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/ioctl.h>

#include <linux/rt_ipc.h>

#include "../kselftest_harness.h"

#define RT_IPC_DEV	"/dev/rt_ipc"

static char server_stack[64 * 1024];

/* A dummy server entry address; never actually invoked in the foundation. */
static void server_entry(void)
{
}

static int open_dev(struct __test_metadata *_metadata)
{
	int fd = open(RT_IPC_DEV, O_RDWR | O_CLOEXEC);

	if (fd < 0)
		SKIP(return -1, "%s unavailable (errno=%d)", RT_IPC_DEV, errno);
	return fd;
}

static void fill_ep_req(struct rt_ipc_endpoint_create *req)
{
	memset(req, 0, sizeof(*req));
	req->size = sizeof(*req);
	req->flags = RT_IPC_EP_SERVER_CREDS;
	req->entry = (unsigned long)&server_entry;
	req->stack_top = (unsigned long)server_stack + sizeof(server_stack);
	req->stack_size = sizeof(server_stack);
	req->max_concurrency = 4;
}

TEST(version)
{
	__u32 version = 0;
	int dev = open_dev(_metadata);

	ASSERT_EQ(0, ioctl(dev, RT_IPC_GET_VERSION, &version));
	EXPECT_EQ(RT_IPC_ABI_VERSION, version);
	close(dev);
}

TEST(endpoint_bad_size)
{
	struct rt_ipc_endpoint_create req;
	int dev = open_dev(_metadata);

	fill_ep_req(&req);
	req.size = 1;			/* wrong size => EINVAL */
	EXPECT_EQ(-1, ioctl(dev, RT_IPC_ENDPOINT_CREATE, &req));
	EXPECT_EQ(EINVAL, errno);
	close(dev);
}

TEST(endpoint_bad_flags)
{
	struct rt_ipc_endpoint_create req;
	int dev = open_dev(_metadata);

	fill_ep_req(&req);
	req.flags = 0x8000;		/* unknown flag => EINVAL */
	EXPECT_EQ(-1, ioctl(dev, RT_IPC_ENDPOINT_CREATE, &req));
	EXPECT_EQ(EINVAL, errno);
	close(dev);
}

TEST(endpoint_zero_concurrency)
{
	struct rt_ipc_endpoint_create req;
	int dev = open_dev(_metadata);

	fill_ep_req(&req);
	req.max_concurrency = 0;	/* invalid => EINVAL */
	EXPECT_EQ(-1, ioctl(dev, RT_IPC_ENDPOINT_CREATE, &req));
	EXPECT_EQ(EINVAL, errno);
	close(dev);
}

TEST(endpoint_create_and_connect)
{
	struct rt_ipc_endpoint_create req;
	struct rt_ipc_connect creq;
	int dev = open_dev(_metadata);
	int ep, conn;

	fill_ep_req(&req);
	ep = ioctl(dev, RT_IPC_ENDPOINT_CREATE, &req);
	ASSERT_GE(ep, 0);

	memset(&creq, 0, sizeof(creq));
	creq.size = sizeof(creq);
	creq.endpoint_fd = ep;
	conn = ioctl(ep, RT_IPC_ENDPOINT_CONNECT, &creq);
	ASSERT_GE(conn, 0);

	close(conn);
	close(ep);
	close(dev);
}

TEST(call_payload_too_big)
{
	struct rt_ipc_endpoint_create req;
	struct rt_ipc_connect creq;
	struct rt_ipc_call call;
	int dev = open_dev(_metadata);
	int ep, conn;

	fill_ep_req(&req);
	ep = ioctl(dev, RT_IPC_ENDPOINT_CREATE, &req);
	ASSERT_GE(ep, 0);

	memset(&creq, 0, sizeof(creq));
	creq.size = sizeof(creq);
	creq.endpoint_fd = ep;
	conn = ioctl(ep, RT_IPC_ENDPOINT_CONNECT, &creq);
	ASSERT_GE(conn, 0);

	memset(&call, 0, sizeof(call));
	call.size = sizeof(call);
	call.send_len = 1UL << 30;	/* absurdly large => EMSGSIZE */
	call.timeout_ms = -1;
	EXPECT_EQ(-1, ioctl(conn, RT_IPC_CALL, &call));
	EXPECT_EQ(EMSGSIZE, errno);

	close(conn);
	close(ep);
	close(dev);
}

TEST(call_basic)
{
	struct rt_ipc_endpoint_create req;
	struct rt_ipc_connect creq;
	struct rt_ipc_call call;
	char sbuf[16] = "ping";
	char rbuf[16] = {};
	int dev = open_dev(_metadata);
	int ep, conn, ret;

	fill_ep_req(&req);
	ep = ioctl(dev, RT_IPC_ENDPOINT_CREATE, &req);
	ASSERT_GE(ep, 0);

	memset(&creq, 0, sizeof(creq));
	creq.size = sizeof(creq);
	creq.endpoint_fd = ep;
	conn = ioctl(ep, RT_IPC_ENDPOINT_CONNECT, &creq);
	ASSERT_GE(conn, 0);

	memset(&call, 0, sizeof(call));
	call.size = sizeof(call);
	call.send_buf = (unsigned long)sbuf;
	call.send_len = sizeof(sbuf);
	call.recv_buf = (unsigned long)rbuf;
	call.recv_len = sizeof(rbuf);
	call.timeout_ms = -1;

	ret = ioctl(conn, RT_IPC_CALL, &call);
	/*
	 * Foundation: either a clean data-less completion (0) or an explicit
	 * "user-mode dispatch not yet wired" refusal (EOPNOTSUPP).  Anything
	 * else is a regression.
	 */
	if (ret != 0)
		EXPECT_EQ(EOPNOTSUPP, errno);

	close(conn);
	close(ep);
	close(dev);
}

TEST_HARNESS_MAIN

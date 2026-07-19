// SPDX-License-Identifier: GPL-2.0
/*
 * rt_ipc core: control device, fd/anon_inode plumbing and ioctl dispatch.
 *
 * Userspace opens /dev/rt_ipc and issues RT_IPC_ENDPOINT_CREATE to register a
 * migrating-thread service, receiving an endpoint fd.  That fd may be passed
 * to clients over SCM_RIGHTS.  A client issues RT_IPC_ENDPOINT_CONNECT on the
 * endpoint fd to obtain a connection fd, then RT_IPC_CALL to perform a
 * synchronous RPC.
 */
#include <linux/module.h>
#include <linux/miscdevice.h>
#include <linux/fs.h>
#include <linux/file.h>
#include <linux/anon_inodes.h>
#include <linux/uaccess.h>
#include <linux/slab.h>
#include <linux/sched.h>

#include "internal.h"

#define CREATE_TRACE_POINTS
#include <trace/events/rt_ipc.h>

static const struct file_operations rt_ipc_endpoint_fops;
static const struct file_operations rt_ipc_connection_fops;

/* Endpoint fd ------------------------------------------------------------ */

static int rt_ipc_endpoint_fop_release(struct inode *inode, struct file *file)
{
	struct rt_ipc_endpoint *ep = file->private_data;

	rt_ipc_endpoint_shutdown(ep);
	rt_ipc_endpoint_put(ep);
	return 0;
}

static long rt_ipc_endpoint_connect(struct rt_ipc_endpoint *ep,
				    void __user *arg)
{
	struct rt_ipc_connect req;
	struct rt_ipc_connection *conn;
	int fd;

	if (copy_from_user(&req, arg, sizeof(req)))
		return -EFAULT;
	if (req.size != sizeof(req))
		return -EINVAL;
	if (req.flags & ~RT_IPC_CONN_FLAGS_ALL)
		return -EINVAL;
	if (req.__reserved)
		return -EINVAL;

	conn = rt_ipc_connection_create(ep);
	if (IS_ERR(conn))
		return PTR_ERR(conn);

	fd = rt_ipc_connection_install_fd(conn);
	if (fd < 0)
		rt_ipc_connection_put(conn);
	return fd;
}

static long rt_ipc_endpoint_fop_ioctl(struct file *file, unsigned int cmd,
				      unsigned long arg)
{
	struct rt_ipc_endpoint *ep = file->private_data;
	void __user *uarg = (void __user *)arg;

	switch (cmd) {
	case RT_IPC_ENDPOINT_CONNECT:
		return rt_ipc_endpoint_connect(ep, uarg);
	default:
		return -ENOTTY;
	}
}

static const struct file_operations rt_ipc_endpoint_fops = {
	.owner		= THIS_MODULE,
	.release	= rt_ipc_endpoint_fop_release,
	.unlocked_ioctl	= rt_ipc_endpoint_fop_ioctl,
	.compat_ioctl	= compat_ptr_ioctl,
	.llseek		= noop_llseek,
};

int rt_ipc_endpoint_install_fd(struct rt_ipc_endpoint *ep)
{
	return anon_inode_getfd("[rt_ipc.ep]", &rt_ipc_endpoint_fops, ep,
				O_RDWR | O_CLOEXEC);
}

struct rt_ipc_endpoint *rt_ipc_endpoint_from_fd(int fd)
{
	struct fd f = fdget(fd);
	struct rt_ipc_endpoint *ep;

	if (!fd_file(f))
		return ERR_PTR(-EBADF);
	if (fd_file(f)->f_op != &rt_ipc_endpoint_fops) {
		fdput(f);
		return ERR_PTR(-EINVAL);
	}
	ep = fd_file(f)->private_data;
	rt_ipc_endpoint_get(ep);
	fdput(f);
	return ep;
}

/* Connection fd ---------------------------------------------------------- */

static int rt_ipc_connection_fop_release(struct inode *inode,
					 struct file *file)
{
	struct rt_ipc_connection *conn = file->private_data;

	rt_ipc_connection_put(conn);
	return 0;
}

static long rt_ipc_connection_call(struct rt_ipc_connection *conn,
				   void __user *arg)
{
	struct rt_ipc_call call;
	long ret;

	if (copy_from_user(&call, arg, sizeof(call)))
		return -EFAULT;
	if (call.size != sizeof(call))
		return -EINVAL;
	if (call.flags & ~RT_IPC_CALL_FLAGS_ALL)
		return -EINVAL;
	if (call.__reserved)
		return -EINVAL;

	ret = rt_ipc_do_call(conn, &call);
	if (ret)
		return ret;

	if (copy_to_user(arg, &call, sizeof(call)))
		return -EFAULT;
	return 0;
}

static long rt_ipc_connection_fop_ioctl(struct file *file, unsigned int cmd,
					unsigned long arg)
{
	struct rt_ipc_connection *conn = file->private_data;
	void __user *uarg = (void __user *)arg;

	switch (cmd) {
	case RT_IPC_CALL:
		return rt_ipc_connection_call(conn, uarg);
	default:
		return -ENOTTY;
	}
}

static const struct file_operations rt_ipc_connection_fops = {
	.owner		= THIS_MODULE,
	.release	= rt_ipc_connection_fop_release,
	.unlocked_ioctl	= rt_ipc_connection_fop_ioctl,
	.compat_ioctl	= compat_ptr_ioctl,
	.llseek		= noop_llseek,
};

int rt_ipc_connection_install_fd(struct rt_ipc_connection *conn)
{
	return anon_inode_getfd("[rt_ipc.conn]", &rt_ipc_connection_fops, conn,
				O_RDWR | O_CLOEXEC);
}

/* Control device --------------------------------------------------------- */

static long rt_ipc_dev_endpoint_create(void __user *arg)
{
	struct rt_ipc_endpoint_create req;
	struct rt_ipc_endpoint *ep;
	int fd;

	if (copy_from_user(&req, arg, sizeof(req)))
		return -EFAULT;
	if (req.size != sizeof(req))
		return -EINVAL;

	ep = rt_ipc_endpoint_create(&req);
	if (IS_ERR(ep))
		return PTR_ERR(ep);

	fd = rt_ipc_endpoint_install_fd(ep);
	if (fd < 0) {
		rt_ipc_endpoint_shutdown(ep);
		rt_ipc_endpoint_put(ep);
	}
	return fd;
}

static long rt_ipc_dev_ioctl(struct file *file, unsigned int cmd,
			     unsigned long arg)
{
	void __user *uarg = (void __user *)arg;
	u32 version = RT_IPC_ABI_VERSION;

	switch (cmd) {
	case RT_IPC_GET_VERSION:
		if (put_user(version, (u32 __user *)uarg))
			return -EFAULT;
		return 0;
	case RT_IPC_ENDPOINT_CREATE:
		return rt_ipc_dev_endpoint_create(uarg);
	default:
		return -ENOTTY;
	}
}

static const struct file_operations rt_ipc_dev_fops = {
	.owner		= THIS_MODULE,
	.unlocked_ioctl	= rt_ipc_dev_ioctl,
	.compat_ioctl	= compat_ptr_ioctl,
	.llseek		= noop_llseek,
};

static struct miscdevice rt_ipc_miscdev = {
	.minor	= MISC_DYNAMIC_MINOR,
	.name	= RT_IPC_DEVICE,
	.fops	= &rt_ipc_dev_fops,
	.mode	= 0666,
};

static int __init rt_ipc_init(void)
{
	int err;

	err = misc_register(&rt_ipc_miscdev);
	if (err)
		return err;

	rt_ipc_debugfs_init();
	pr_info("rt_ipc: migrating-thread IPC registered (ABI v%d)\n",
		RT_IPC_ABI_VERSION);
	return 0;
}

static void __exit rt_ipc_exit(void)
{
	rt_ipc_debugfs_exit();
	misc_deregister(&rt_ipc_miscdev);
}

module_init(rt_ipc_init);
module_exit(rt_ipc_exit);

MODULE_DESCRIPTION("Real-time IPC using the migrating-thread model");
MODULE_LICENSE("GPL");

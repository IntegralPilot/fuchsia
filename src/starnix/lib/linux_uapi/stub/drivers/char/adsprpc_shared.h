/*
 * This file is auto-generated. Modifications will be lost.
 *
 * See https://android.googlesource.com/platform/bionic/+/master/libc/kernel/
 * for more information.
 */
#ifndef ADSPRPC_SHARED_H
#define ADSPRPC_SHARED_H

#include <stddef.h>
#include <stdint.h>

#include <linux/types.h>

#define FASTRPC_IOCTL_INVOKE _IOWR('R', 1, struct fastrpc_ioctl_invoke)
#define FASTRPC_IOCTL_INVOKE_FD _IOWR('R', 4, struct fastrpc_ioctl_invoke_fd)
#define FASTRPC_IOCTL_INIT _IOWR('R', 6, struct fastrpc_ioctl_init)
#define FASTRPC_IOCTL_GETINFO _IOWR('R', 8, uint32_t)
#define FASTRPC_IOCTL_GET_DSP_INFO _IOWR('R', 17, struct fastrpc_ioctl_capability)
#define FASTRPC_IOCTL_INVOKE2 _IOWR('R', 18, struct fastrpc_ioctl_invoke2)

struct remote_buf {
  void* pv;
  size_t len;
};

struct fastrpc_ioctl_invoke {
  uint32_t handle;
  uint32_t sc;
  void* pra;
};

struct fastrpc_ioctl_invoke_fd {
  struct fastrpc_ioctl_invoke inv;
  int32_t* fds;
};

struct fastrpc_ioctl_invoke2 {
  uint32_t req;
  void* invparam;
  uint32_t size;
  int32_t err;
};

struct fastrpc_ioctl_init {
  uint32_t flags;
  void* file;
  uint32_t filelen;
  int32_t filefd;
  void* mem;
  uint32_t memlen;
  int32_t memfd;
};

struct fastrpc_ioctl_capability {
  uint32_t domain;
  uint32_t attribute_ID;
  uint32_t capability;
};

#endif

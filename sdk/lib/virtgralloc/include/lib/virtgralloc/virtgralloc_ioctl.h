// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This file is for use by a process using the linux uapi to communicate with
// virtgralloc using ioctl(s). We also use rust_bindgen to create corresponding
// generated rust code for use by starnix to back the ioctl(s).

#ifndef LIB_VIRTGRALLOC_VIRTGRALLOC_IOCTL_H_
#define LIB_VIRTGRALLOC_VIRTGRALLOC_IOCTL_H_

#if !defined(__linux__) && !defined(__Fuchsia__)
#warning "virtgralloc_ioctl.h only intended for use on fuchsia and linux"
#endif

#include <inttypes.h>

#if defined(__linux__)
#include <asm/ioctl.h>

#define VIRTGRALLOC_IOCTL_BASE 'g'
#define VIRTGRALOC_IO(nr) _IO(VIRTGRALLOC_IOCTL_BASE, nr)
#define VIRTGRALLOC_IOR(nr, type) _IOR(VIRTGRALLOC_IOCTL_BASE, nr, type)
#define VIRTGRALLOC_IOW(nr, type) _IOW(VIRTGRALLOC_IOCTL_BASE, nr, type)
#define VIRTGRALLOC_IOWR(nr, type) _IOWR(VIRTGRALLOC_IOCTL_BASE, nr, type)
#endif

#define VIRTGRALLOC_DEVICE_NAME "/dev/virtgralloc0"

typedef uint64_t virtgralloc_VulkanMode;
#define VIRTGRALLOC_VULKAN_MODE_INVALID ((virtgralloc_VulkanMode)0)
#define VIRTGRALLOC_VULKAN_MODE_SWIFTSHADER ((virtgralloc_VulkanMode)1)
#define VIRTGRALLOC_VULKAN_MODE_MAGMA ((virtgralloc_VulkanMode)2)

typedef uint64_t virtgralloc_SetVulkanModeResult;
#define VIRTGRALLOC_SET_VULKAN_MODE_RESULT_INVALID ((virtgralloc_SetVulkanModeResult)0)
#define VIRTGRALLOC_SET_VULKAN_MODE_RESULT_SUCCESS ((virtgralloc_SetVulkanModeResult)1)

struct virtgralloc_set_vulkan_mode {
  // Must be a VIRTGRALLOC_VULKAN_MODE_* value.
  virtgralloc_VulkanMode vulkan_mode;
  // Must be a VIRTGRALLOC_SET_VULKAN_MODE_RESULT_* value.
  virtgralloc_SetVulkanModeResult result;
};
static_assert(sizeof(virtgralloc_set_vulkan_mode) == 16);

// 0x00 is reserved for potential use as a version handshake or similar
#if defined(__linux__)
#define VIRTGRALLOC_IOCTL_SET_VULKAN_MODE VIRTGRALLOC_IOWR(0x01, struct virtgralloc_set_vulkan_mode)
static_assert(VIRTGRALLOC_IOCTL_SET_VULKAN_MODE == 0xC0106701);
#else
#define VIRTGRALLOC_IOCTL_SET_VULKAN_MODE (0xC0106701)
#endif

#endif  // LIB_VIRTGRALLOC_VIRTGRALLOC_IOCTL_H_

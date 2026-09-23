// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BUS_LIB_VIRTIO_INCLUDE_LIB_VIRTIO_INTERNAL_BARRIERS_H_
#define SRC_DEVICES_BUS_LIB_VIRTIO_INCLUDE_LIB_VIRTIO_INTERNAL_BARRIERS_H_

#if defined(__aarch64__)

#define hw_mb() __asm__ volatile("dmb sy" : : : "memory")
#define hw_rmb() __asm__ volatile("dmb ld" : : : "memory")
#define hw_wmb() __asm__ volatile("dmb st" : : : "memory")

#elif defined(__x86_64__)

#define hw_mb() __asm__ volatile("mfence" ::: "memory")
#define hw_rmb() __asm__ volatile("lfence" ::: "memory")
#define hw_wmb() __asm__ volatile("sfence" ::: "memory")

#elif defined(__riscv)

#define hw_mb() __asm__ volatile("fence iorw,iorw" ::: "memory")
#define hw_rmb() __asm__ volatile("fence ir,ir" ::: "memory")
#define hw_wmb() __asm__ volatile("fence ow,ow" ::: "memory")

#else
#error Unsupported platform
#endif

#endif  // SRC_DEVICES_BUS_LIB_VIRTIO_INCLUDE_LIB_VIRTIO_INTERNAL_BARRIERS_H_

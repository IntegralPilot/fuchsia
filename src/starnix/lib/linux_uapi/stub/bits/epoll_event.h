// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STARNIX_LIB_LINUX_UAPI_STUB_BITS_EPOLL_EVENT_H_
#define SRC_STARNIX_LIB_LINUX_UAPI_STUB_BITS_EPOLL_EVENT_H_

#include <stdint.h>

#ifdef __x86_64__
#define PACKED_ON_X64 __packed
#else
#define PACKED_ON_X64
#endif

struct epoll_event {
  uint32_t events;
  uint64_t data;
} PACKED_ON_X64;

#endif  // SRC_STARNIX_LIB_LINUX_UAPI_STUB_BITS_EPOLL_EVENT_H_

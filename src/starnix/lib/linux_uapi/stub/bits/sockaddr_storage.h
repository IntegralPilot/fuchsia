// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STARNIX_LIB_LINUX_UAPI_STUB_BITS_SOCKADDR_STORAGE_H_
#define SRC_STARNIX_LIB_LINUX_UAPI_STUB_BITS_SOCKADDR_STORAGE_H_

typedef unsigned short __kernel_sa_family_t;

struct sockaddr_storage {
  union {
    struct {
      __kernel_sa_family_t ss_family;
      char __data[128 - sizeof(__kernel_sa_family_t)];
    };
    void* __align;
  };
};

#endif  // SRC_STARNIX_LIB_LINUX_UAPI_STUB_BITS_SOCKADDR_STORAGE_H_

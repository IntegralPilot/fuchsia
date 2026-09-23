// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file

#ifndef LIB_DRIVER_FAKE_OBJECT_CPP_INTERNAL_H_
#define LIB_DRIVER_FAKE_OBJECT_CPP_INTERNAL_H_

#define REAL_SYSCALL(zx_name) _##zx_name

#define FAKE_OBJECT_TRACE 0
#if FAKE_OBJECT_TRACE
#define ftracef(...)                           \
  {                                            \
    printf("fake-object %16.16s: ", __func__); \
    printf(__VA_ARGS__);                       \
  }
#else
#define ftracef(...) ;
#endif

#endif  // LIB_DRIVER_FAKE_OBJECT_CPP_INTERNAL_H_

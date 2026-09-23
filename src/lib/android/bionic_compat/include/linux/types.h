// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ANDROID_BIONIC_COMPAT_INCLUDE_LINUX_TYPES_H_
#define SRC_LIB_ANDROID_BIONIC_COMPAT_INCLUDE_LINUX_TYPES_H_

#include <stdint.h>

typedef uint8_t __u8;
typedef uint32_t __u32;
typedef int32_t __s32;
typedef unsigned long long __u64;
typedef long long __s64;

typedef __s32 __kernel_pid_t;
typedef __s32 __kernel_uid32_t;

#endif  // SRC_LIB_ANDROID_BIONIC_COMPAT_INCLUDE_LINUX_TYPES_H_

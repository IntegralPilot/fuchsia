// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STARNIX_LIB_LINUX_UAPI_STUB_ARM_ARCH32_SYSCALLS_H_
#define SRC_STARNIX_LIB_LINUX_UAPI_STUB_ARM_ARCH32_SYSCALLS_H_

// The convention starnix uses always use __NR_ as a prefix for syscall number.
// Defines ARM specific constant using the same convention
#define __NR_ARM_breakpoint __ARM_NR_breakpoint
#define __NR_ARM_cacheflush __ARM_NR_cacheflush
#define __NR_ARM_usr26 __ARM_NR_usr26
#define __NR_ARM_usr32 __ARM_NR_usr32
#define __NR_ARM_set_tls __ARM_NR_set_tls
#define __NR_ARM_get_tls __ARM_NR_get_tls

#endif  // SRC_STARNIX_LIB_LINUX_UAPI_STUB_ARM_ARCH32_SYSCALLS_H_

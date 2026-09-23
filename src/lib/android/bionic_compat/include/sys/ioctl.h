// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ANDROID_BIONIC_COMPAT_INCLUDE_SYS_IOCTL_H_
#define SRC_LIB_ANDROID_BIONIC_COMPAT_INCLUDE_SYS_IOCTL_H_

#include_next <sys/ioctl.h>

#define _IOC_NRBITS 8
#define _IOC_NRMASK ((1 << _IOC_NRBITS) - 1)

#endif  // SRC_LIB_ANDROID_BIONIC_COMPAT_INCLUDE_SYS_IOCTL_H_

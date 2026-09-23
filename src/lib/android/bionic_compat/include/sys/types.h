// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ANDROID_BIONIC_COMPAT_INCLUDE_SYS_TYPES_H_
#define SRC_LIB_ANDROID_BIONIC_COMPAT_INCLUDE_SYS_TYPES_H_

#include_next <sys/types.h>

typedef off_t off64_t;

#endif  // SRC_LIB_ANDROID_BIONIC_COMPAT_INCLUDE_SYS_TYPES_H_

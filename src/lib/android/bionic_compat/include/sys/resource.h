// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ANDROID_BIONIC_COMPAT_INCLUDE_SYS_RESOURCE_H_
#define SRC_LIB_ANDROID_BIONIC_COMPAT_INCLUDE_SYS_RESOURCE_H_

#include <stdlib.h>

#define RLIMIT_NOFILE 7

#ifdef __cplusplus
extern "C" {
#endif

struct rlimit {
  size_t rlim_cur;
  size_t rlim_max;
};

int getrlimit(int resource, struct rlimit *rlim);

#define PRIO_MIN (-20)
#define PRIO_MAX 20
#define PRIO_PROCESS 0
#define PRIO_PGRP 1
#define PRIO_USER 2

int setpriority(int which, int who, int prio);

#ifdef __cplusplus
}
#endif

#endif  // SRC_LIB_ANDROID_BIONIC_COMPAT_INCLUDE_SYS_RESOURCE_H_

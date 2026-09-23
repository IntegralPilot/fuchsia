// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ANDROID_BIONIC_COMPAT_INCLUDE_UNISTD_H_
#define SRC_LIB_ANDROID_BIONIC_COMPAT_INCLUDE_UNISTD_H_

#include_next <unistd.h>

#ifndef TEMP_FAILURE_RETRY
#define TEMP_FAILURE_RETRY(exp)            \
  ({                                       \
    __typeof__(exp) _rc;                   \
    do {                                   \
      _rc = (exp);                         \
    } while (_rc == -1 && errno == EINTR); \
    _rc;                                   \
  })
#endif

#endif  // SRC_LIB_ANDROID_BIONIC_COMPAT_INCLUDE_UNISTD_H_

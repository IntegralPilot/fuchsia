// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_FDIO_FDIO_UNISTD_H_
#define LIB_FDIO_FDIO_UNISTD_H_

#include <zircon/types.h>

#include <cerrno>

int fdio_status_to_errno(zx_status_t status);

// Sets |errno| to the nearest match for |status| and returns -1;
static inline int ERROR(zx_status_t status) {
  errno = fdio_status_to_errno(status);
  return -1;
}

// Returns 0 if |status| is |ZX_OK|, otherwise delegates to |ERROR|.
static inline int STATUS(zx_status_t status) {
  if (status == ZX_OK) {
    return 0;
  }
  return ERROR(status);
}

// set errno to e, return -1
static inline int ERRNO(int e) {
  errno = e;
  return -1;
}

#endif  // LIB_FDIO_FDIO_UNISTD_H_

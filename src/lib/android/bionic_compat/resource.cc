// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <errno.h>
#include <lib/fdio/limits.h>
#include <sys/resource.h>

int getrlimit(int resource, struct rlimit *rlim) {
  if (resource == RLIMIT_NOFILE) {
    rlim->rlim_cur = FDIO_MAX_FD;
    rlim->rlim_max = FDIO_MAX_FD;
    return 0;
  }

  errno = EINVAL;
  return -1;
}

int setpriority(int which, int who, int prio) { return 0; }

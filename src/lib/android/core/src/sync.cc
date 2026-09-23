// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <errno.h>
#include <poll.h>

#if !defined(__BIONIC__) && !defined(__INTRODUCED_IN)
#define __INTRODUCED_IN(x)
#endif

#include <sync/sync.h>

int sync_wait(int fd, int timeout) {
  struct pollfd fds;
  int ret;

  if (fd < 0) {
    errno = EINVAL;
    return -1;
  }

  fds.fd = fd;
  fds.events = POLLIN;

  do {
    ret = poll(&fds, 1, timeout);
    if (ret > 0) {
      if (fds.revents & (POLLERR | POLLNVAL)) {
        errno = EINVAL;
        return -1;
      }
      return 0;
    } else if (ret == 0) {
      errno = ETIME;
      return -1;
    }
  } while (ret == -1 && (errno == EINTR || errno == EAGAIN));

  return ret;
}

// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>

int main(int argc, char** argv) {
  if (argc < 2) {
    fprintf(stderr, "Usage: %s <fd>\n", argv[0]);
    return 2;  // Invalid usage
  }
  int fd = atoi(argv[1]);

  int ret = fcntl(fd, F_GETFD);
  if (ret == -1 && errno == EBADF) {
    return 1;  // Invalid FD
  }
  return 0;  // Valid FD
}

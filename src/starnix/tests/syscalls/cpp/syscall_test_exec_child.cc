// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>

// This simple program is used for the
// RobustFutexTest.FutexStateAfterExecCheck test. That test ensures that robust
// futexes are notified when an exec happens.  Because robust futexes are also
// notified when the process exits, we wait until the test is finished before
// exiting the process.  We notify the test that we have started by printing
// "ready" to stdout.  The test indicates that it has finished checking the futex value
// by unlocking the file provided in argv[1], at which point this process can exit.
int main(int argc, char** argv) {
  if (argc < 2) {
    return 1;
  }
  fprintf(stdout, "ready");
  int fd = open(argv[1], O_RDWR | O_CREAT | O_TRUNC, 0777);
  struct flock fl = {.l_type = F_WRLCK, .l_whence = SEEK_SET, .l_start = 0, .l_len = 0};
  fcntl(fd, F_SETLK, &fl);
}

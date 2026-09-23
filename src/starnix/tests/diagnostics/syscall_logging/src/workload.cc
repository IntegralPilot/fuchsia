// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <unistd.h>

int main(int argc, char** argv) {
  int kmsg_fd = open("/dev/kmsg", O_WRONLY);
  const char* msg = "Hello from workload via kmsg\n";
  if (kmsg_fd >= 0) {
    write(kmsg_fd, msg, strlen(msg));
  }

  while (true) {
    syscall(SYS_getuid);
    const char* loop_msg = "looping\n";
    if (kmsg_fd >= 0) {
      write(kmsg_fd, loop_msg, strlen(loop_msg));
    }
    sleep(1);
  }
  return 0;
}

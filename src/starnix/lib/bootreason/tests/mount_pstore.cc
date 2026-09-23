// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <errno.h>
#include <stdio.h>
#include <sys/mount.h>
#include <sys/syscall.h>
#include <unistd.h>

int main(int argc, char** argv) {
  printf("mount_pstore running...\n");
  long ret = mount("sysfs", "/sys", "sysfs", MS_NOSUID | MS_NODEV | MS_NOEXEC, NULL);
  if (ret == -1 && errno != EBUSY) {
    perror("failed to mount /sys\n");
    return 1;
  }

  ret = mount("pstore", "/sys/fs/pstore", "pstore", MS_NOSUID | MS_NODEV | MS_NOEXEC, NULL);
  if (ret == -1 && errno != EBUSY) {
    perror("failed to mount /sys/fs/pstore\n");
    return 1;
  }

  printf("mount_pstore finished.\n");
  return 0;
}

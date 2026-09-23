// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <sys/syscall.h>
#include <unistd.h>

#include <linux/reboot.h>

int main(int argc, char** argv) {
  const char* reboot_arg = argc >= 2 ? argv[1] : "";
  syscall(SYS_reboot, LINUX_REBOOT_MAGIC1, LINUX_REBOOT_MAGIC2, LINUX_REBOOT_CMD_RESTART2,
          reboot_arg);
  return 0;
}

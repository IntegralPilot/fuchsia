// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <lib/syslog/cpp/macros.h>
#include <sched.h>
#include <string.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>

#include <string>
#include <vector>

#include <linux/capability.h>
#include <perftest/perftest.h>

namespace {

bool Unshare(perftest::RepeatState* state, int unshare_flags, int num_fds, int num_mounts) {
  // 1. Inflate the file descriptor table.
  std::vector<int> fds;
  for (int i = 0; i < num_fds; ++i) {
    int fd = open("/dev/null", O_RDONLY);
    FX_CHECK(fd >= 0);
    fds.push_back(fd);
  }

  // 2. Inflate the mount namespace.
  // We use /tmp to create a bunch of tmpfs mounts.
  std::vector<std::string> mount_paths;
  for (int i = 0; i < num_mounts; ++i) {
    std::string path = "/tmp/unshare_bench_mnt_" + std::to_string(i);
    FX_CHECK(mkdir(path.c_str(), 0777) == 0);
    FX_CHECK(mount("tmpfs", path.c_str(), "tmpfs", 0, nullptr) == 0);
    mount_paths.push_back(path);
  }

  // 3. Alter a filesystem attribute (umask is copied by CLONE_FS).
  mode_t old_umask = umask(0077);

  while (state->KeepRunning()) {
    FX_CHECK(unshare(unshare_flags) == 0);
  }

  // Cleanup FDs.
  for (int fd : fds) {
    FX_CHECK(close(fd) == 0);
  }

  // Cleanup mounts only in the latest namespace.
  for (const std::string& path : mount_paths) {
    FX_CHECK(umount(path.c_str()) == 0);
    FX_CHECK(rmdir(path.c_str()) == 0);
  }

  umask(old_umask);

  return true;
}

bool HasSysAdmin() {
  __user_cap_header_struct header;
  memset(&header, 0, sizeof(header));
  header.version = _LINUX_CAPABILITY_VERSION_3;
  __user_cap_data_struct caps[_LINUX_CAPABILITY_U32S_3];
  if (syscall(SYS_capget, &header, &caps) == -1) {
    return false;
  }
  return caps[CAP_TO_INDEX(CAP_SYS_ADMIN)].effective & CAP_TO_MASK(CAP_SYS_ADMIN);
}

void RegisterTests() {
  perftest::RegisterTest("Unshare/Files/Baseline", Unshare, CLONE_FILES, 0, 0);
  perftest::RegisterTest("Unshare/Files/1000", Unshare, CLONE_FILES, 1000, 0);

  perftest::RegisterTest("Unshare/Fs/Baseline", Unshare, CLONE_FS, 0, 0);

  // Skip tests if we don't have CAP_SYS_ADMIN.
  if (HasSysAdmin()) {
    perftest::RegisterTest("Unshare/NewNS/Baseline", Unshare, CLONE_NEWNS, 0, 0);
    perftest::RegisterTest("Unshare/NewNS/50", Unshare, CLONE_NEWNS, 0, 50);

    perftest::RegisterTest("Unshare/All/Baseline", Unshare, CLONE_FILES | CLONE_FS | CLONE_NEWNS, 0,
                           0);
    perftest::RegisterTest("Unshare/All/Complex", Unshare, CLONE_FILES | CLONE_FS | CLONE_NEWNS,
                           1000, 50);
  }
}
PERFTEST_CTOR(RegisterTests)

}  // namespace

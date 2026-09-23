// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/sysmacros.h>
#include <unistd.h>

#include <filesystem>
#include <string>
#include <vector>

#ifndef __linux__
#error "Intended for Linux only!"
#endif

namespace {

void Log(const char *fmt, ...) {
  printf("Init: ");
  va_list args;
  va_start(args, fmt);
  vfprintf(stdout, fmt, args);
  va_end(args);
  printf("\n");
}

[[noreturn]] void Fail(const char *fmt, ...) {
  fprintf(stderr, "Init: error: ");
  va_list args;
  va_start(args, fmt);
  vfprintf(stderr, fmt, args);
  va_end(args);
  printf("\n");
  abort();
}

}  // namespace

int main(int argc, char *argv[]) {
  Log("started!");

  if (pid_t pid = getpid(); pid != 1) {
    Fail("unexpected PID (%d)!?", pid);
  }

  // /proc: needs creation and mounting.
  mkdir("/proc", 0755);
  if (mount("none", "/proc", "proc", 0, NULL) == 0) {
    Log("/proc mounted");
  } else {
    Fail("failed to mount /proc: %s", strerror(errno));
  }

  // /sys: needs creation and mounting.
  mkdir("/sys", 0755);
  if (mount("none", "/sys", "sysfs", 0, NULL) == 0) {
    Log("/sys mounted");
  } else {
    Fail("failed to mount /sys: %s", strerror(errno));
  }

  // /dev: needs creation, but *not* mounting....
  mkdir("/dev", 0755);
  if (mknod("/dev/kvm", S_IFCHR | 0666, makedev(10, 232)) == 0) {
    Log("/dev/kvm created");
  } else {
    Fail("failed to create /dev/kvm");
  }

  std::vector<std::string> crosvm_args = {
      "/bin/crosvm",
      "--extended-status",
      "--log-level=debug",
      "run",
      "--disable-sandbox",
      "--mem",
      "8192",
      "--cpus",
      "1",
      "--serial",
      "type=stdout,stdin,hardware=serial,earlycon",
      "--initrd",
      "/data/ramdisk",
      "/data/kernel",
  };

  if (std::filesystem::exists({"/bin/firmware"})) {
    crosvm_args.push_back("--protected-vm-with-firmware");
    crosvm_args.push_back("/bin/firmware");
  } else {
    crosvm_args.push_back("--protected-vm-without-firmware");
  }

  if (std::filesystem::exists("/data/dtbo")) {
    crosvm_args.push_back("--device-tree-overlay");
    crosvm_args.push_back("/data/dtbo");
  }

  // Translate into an argv-friendly form.
  std::vector<char *> crosvm_argv;
  crosvm_argv.reserve(crosvm_args.size() + 1);  // + NUL
  for (auto &arg : crosvm_args) {
    crosvm_argv.push_back(arg.data());
  }
  crosvm_argv.push_back(nullptr);

  // Replace the current process (PID 1) with crosvm
  Log("attempting to exec crosvm...");
  execv("/bin/crosvm", crosvm_argv.data());

  Fail("failed to execv crosvm: %s", strerror(errno));
}

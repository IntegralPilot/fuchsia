// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <lib/fit/defer.h>
#include <net/if.h>
#include <netinet/in.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/reboot.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/sysmacros.h>
#include <sys/wait.h>
#include <unistd.h>

#include <atomic>
#include <string>
#include <vector>

#include <fbl/unique_fd.h>

namespace {

// Returns true if all modules were loaded successfully.
bool LoadModules(const std::string& modules_dir) {
  DIR* dir = opendir(modules_dir.c_str());
  if (!dir) {
    return true;
  }

  std::vector<std::string> modules;
  struct dirent* entry;
  while ((entry = readdir(dir)) != nullptr) {
    std::string name = entry->d_name;
    if (name.size() > 3 && name.substr(name.size() - 3) == ".ko") {
      modules.push_back(modules_dir + "/" + name);
    }
  }
  closedir(dir);

  // We need to load modules in topological order of their dependencies.
  // Since we don't have the dependency information, we load modules in passes.
  // In each pass we try to load all modules that haven't been loaded yet.
  // If we manage to load at least one module in a pass, we continue to the next pass.
  // If we don't manage to load any module in a pass, we stop.
  bool loaded_any = true;
  while (loaded_any && !modules.empty()) {
    loaded_any = false;
    std::vector<std::string> remaining;
    for (auto& mod : modules) {
      fbl::unique_fd fd(open(mod.c_str(), O_RDONLY | O_CLOEXEC));
      if (!fd.is_valid()) {
        continue;
      }
      long res = syscall(SYS_finit_module, fd.get(), "", 0);
      if (res >= 0) {
        // The module was loaded successfully.
        loaded_any = true;
      } else if (errno != EEXIST) {
        // The module was not already loaded, but still failed to load.
        // Lets try loading it again in the next pass.
        remaining.push_back(std::move(mod));
      }
    }
    modules = std::move(remaining);
  }
  return modules.empty();
}

bool SetupConsole() {
  // Create /dev/hvc0 if it doesn't exist.
  if (mknod("/dev/hvc0", S_IFCHR | 0600, makedev(229, 0)) < 0) {
    if (errno != EEXIST) {
      perror("mknod /dev/hvc0 failed");
      return false;
    }
  }

  // Redirect stdout and stderr to /dev/hvc0.
  // We use a polling loop because the kernel does asynchronous work in the
  // background.
  fbl::unique_fd fd;
  for (int i = 0; i < 100; i++) {
    fd.reset(open("/dev/hvc0", O_RDWR | O_CLOEXEC));
    if (fd.is_valid()) {
      break;
    }
    usleep(100000);  // Wait 0.1s
  }

  if (!fd.is_valid()) {
    perror("open /dev/hvc0 failed");
    return false;
  }

  dup2(fd.get(), STDOUT_FILENO);
  dup2(fd.get(), STDERR_FILENO);
  setbuf(stdout, NULL);
  setbuf(stderr, NULL);
  return true;
}

bool SetupLoopback() {
  fbl::unique_fd fd(socket(AF_INET, SOCK_DGRAM, 0));
  if (!fd.is_valid()) {
    perror("socket(AF_INET, SOCK_DGRAM) failed");
    return false;
  }
  ifreq ifr = {};
  strncpy(ifr.ifr_name, "lo", IFNAMSIZ);
  auto* addr = reinterpret_cast<sockaddr_in*>(&ifr.ifr_addr);
  addr->sin_family = AF_INET;
  addr->sin_addr.s_addr = htonl(INADDR_LOOPBACK);
  if (ioctl(fd.get(), SIOCSIFADDR, &ifr) < 0) {
    perror("ioctl(SIOCSIFADDR) failed");
    return false;
  }
  if (ioctl(fd.get(), SIOCGIFFLAGS, &ifr) < 0) {
    perror("ioctl(SIOCGIFFLAGS) failed");
    return false;
  }
  ifr.ifr_flags |= IFF_UP | IFF_RUNNING;
  if (ioctl(fd.get(), SIOCSIFFLAGS, &ifr) < 0) {
    perror("ioctl(SIOCSIFFLAGS) failed");
    return false;
  }
  return true;
}

}  // namespace

int main(int argc, char** argv) {
  auto reboot_before_exit = fit::defer([] { reboot(RB_POWER_OFF); });

  bool modules_loaded = LoadModules("lib/modules");
  if (!modules_loaded) {
    perror("Failed to load all modules");
    return 1;
  }

  if (!SetupConsole()) {
    perror("Failed to setup console");
    return 1;
  }

  if (!SetupLoopback()) {
    perror("Failed to setup loopback interface");
    return 1;
  }

  pid_t child_pid = fork();
  if (child_pid == -1) {
    perror("fork() failed");
    return 1;
  }

  if (child_pid == 0) {
    execv(argv[1], argv + 1);
    perror("exec failed");
    exit(1);
  }

  int wstatus;
  if (waitpid(child_pid, &wstatus, 0) == -1) {
    perror("waitpid() failed");
    return 1;
  }
  if (WIFEXITED(wstatus) && WEXITSTATUS(wstatus) == 0) {
    fprintf(stderr, "TEST SUCCESS\n");
  } else {
    fprintf(stderr, "TEST FAILURE\n");
  }

  return 0;
}

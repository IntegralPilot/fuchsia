// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <sys/resource.h>
#include <sys/time.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#include <cstring>
#include <iostream>
#include <vector>

namespace {
constexpr char kRssPrefix[] = "Maximum resident set size (kbytes): ";
}

int main(int argc, char* argv[]) {
  if (argc < 2) {
    std::cerr << "Usage: " << argv[0] << " <command> [args...]\n";
    return 1;
  }

  // Prepare arguments for execvp
  std::vector<char*> args;
  for (int i = 1; i < argc; ++i) {
    args.push_back(argv[i]);
  }
  args.push_back(nullptr);

  pid_t pid = fork();

  if (pid < 0) {
    std::cerr << "Fork failed: " << strerror(errno) << "\n";
    return 1;
  }

  if (pid == 0) {
    // Child process
    execvp(args[0], args.data());
    std::cerr << "Exec failed for " << args[0] << ": " << strerror(errno) << "\n";
    return 127;
  }

  // Parent process
  int status;
  struct rusage usage;
  if (wait4(pid, &status, 0, &usage) < 0) {
    std::cerr << "Wait4 failed: " << strerror(errno) << "\n";
    return 1;
  }

  std::cout << kRssPrefix << usage.ru_maxrss << "\n";

  if (WIFEXITED(status)) {
    return WEXITSTATUS(status);
  }

  if (WIFSIGNALED(status)) {
    return 128 + WTERMSIG(status);
  }

  return 1;
}

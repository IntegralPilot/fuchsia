// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <errno.h>
#include <pthread.h>
#include <sys/mount.h>
#include <sys/prctl.h>
#include <sys/resource.h>
#include <unistd.h>

#include <filesystem>
#include <fstream>
#include <functional>
#include <iostream>
#include <string>
#include <thread>

#include <linux/prctl.h>

using namespace std::chrono_literals;

namespace {

constexpr std::string kNormalThreadMessage = "thread";
constexpr std::string kFifoThreadMessage = "thread-fifo";
constexpr std::string kForkMessage = "fork";

void wait_for_control_message(const std::function<void(const std::string&)>& validator) {
  std::string control_message;
  std::getline(std::cin, control_message);
  std::cout << "got `" << control_message << "`\n";
  validator(control_message);
}

void wait_for_expected_control_message(const std::string& expected) {
  std::cout << "waiting for `" << expected << "` control message...\n";
  wait_for_control_message([expected](const std::string& msg) {
    if (msg != expected) {
      std::cerr << "expected `" << expected << "` control message, got `" << msg << "`\n";
      abort();
    }
  });
}

void set_priority_or_panic(int new_nice) {
  wait_for_control_message([new_nice](const std::string& msg) {
    int requested = std::atoi(msg.c_str());
    if (requested != new_nice) {
      std::cerr << "test controller requested an unexpected nice. code says " << new_nice
                << ", socket says `" << requested << "`\n";
      abort();
    }
  });

  if (setpriority(PRIO_PROCESS, 0, new_nice)) {
    std::cerr << "failed to update nice: " << std::strerror(errno) << "\n";
    abort();
  }
  std::cout << "set nice to " << new_nice << "\n";
}

void spawn_and_join_thread_with_nice(int child_nice) {
  wait_for_expected_control_message(kNormalThreadMessage);
  std::thread child([child_nice]() { set_priority_or_panic(child_nice); });
  child.join();
}

}  // namespace

int main(int argc, const char** argv) {
  std::cout << "starting starnix puppet...\n";
  std::filesystem::path child_fence_path("/tmp/child.done");

  set_priority_or_panic(10);
  spawn_and_join_thread_with_nice(12);

  wait_for_expected_control_message(kForkMessage);
  std::cout << "forking child process...\n";
  // TODO(b/297961833) test SCHED_RESET_ON_FORK
  pid_t child = fork();
  if (child > 0) {
    // parent process waits for child process to finish
    while (true) {
      if (std::filesystem::exists(child_fence_path)) {
        break;
      }
      std::this_thread::sleep_for(5ms);
    }
    std::cout << "child reported done.\n";
  } else {
    // child process emits some scheduler calls and writes to its fence when done
    set_priority_or_panic(14);
    spawn_and_join_thread_with_nice(16);
    std::ofstream child_fence(child_fence_path);
    child_fence << "done!";
    return 0;
  }

  wait_for_expected_control_message("cpuset");
  std::error_code ec;
  std::filesystem::create_directories("/tmp/cpuset", ec);
  if (ec) {
    std::cerr << "failed to create /tmp/cpuset: " << ec.message() << std::endl;
    abort();
  }
  if (mount("none", "/tmp/cpuset", "cpuset", 0, nullptr)) {
    std::cerr << "failed to mount cpuset: " << std::strerror(errno) << std::endl;
    abort();
  }
  std::filesystem::create_directory("/tmp/cpuset/bg_tasks", ec);
  if (ec) {
    std::cerr << "failed to create bg_tasks: " << ec.message() << std::endl;
    abort();
  }
  std::ofstream procs_file("/tmp/cpuset/bg_tasks/cgroup.procs");
  if (!procs_file.is_open()) {
    std::cerr << "failed to open cgroup.procs" << std::endl;
    abort();
  }
  procs_file << getpid() << std::endl;
  procs_file.close();

  wait_for_expected_control_message(kFifoThreadMessage);

  sched_param fifo_params = {.sched_priority = 1};
  if (sched_setscheduler(0, SCHED_FIFO, &fifo_params)) {
    std::cerr << "failed to set scheduler: " << std::strerror(errno) << "\n";
    abort();
  }

  const char* new_name = "renamed_puppet";
  prctl(PR_SET_NAME, new_name);

  fifo_params.sched_priority = 2;
  if (sched_setscheduler(0, SCHED_FIFO, &fifo_params)) {
    std::cerr << "failed to set scheduler after rename: " << std::strerror(errno) << "\n";
    abort();
  }

  new_name = "renamed_again";
  prctl(PR_SET_NAME, new_name);

  fifo_params.sched_priority = 3;
  if (sched_setscheduler(0, SCHED_FIFO, &fifo_params)) {
    std::cerr << "failed to set scheduler after rename: " << std::strerror(errno) << "\n";
    abort();
  }

  return 0;
}

// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ZXDUMP_CLI_H_
#define SRC_LIB_ZXDUMP_CLI_H_

#include <lib/zxdump/task.h>

#include <cstdlib>
#include <functional>
#include <iostream>
#include <queue>

class CommandLineHelper {
 public:
  using TaskQueue = std::queue<std::reference_wrapper<zxdump::Task>>;

  // These calls should be made while parsing switches.

  void DumpFileArgument(const char* arg) { dump_files_.push(arg); }

  void LiveArgument() { live_ = true; }

  void NoMemory() { read_memory_ = false; }

  void RootJobArgument() { use_root_job_ = true; }

  // This is the final argument-parsing call, called with (argc, argv, optind)
  // after parsing switches.  It parses arguments that are PID or KOID numbers
  // and it applies RootJobArgument if it was set earlier.
  void KoidArguments(int argc, char** argv, int i, bool allow_jobs = true);

  // After argument parsing, these can be called to ensure the TaskHolder has
  // the necessary data access.

  void NeedInfoResource(bool need = true);

  void NeedSystem(bool need = true);

  // After argument parsing, these report what tasks were requested.

  bool empty() const { return tasks_.empty(); }

  zxdump::TaskHolder& holder() { return holder_; }

  TaskQueue&& take_tasks() { return std::move(tasks_); }

  // These are helpers for user error reporting.  Ok(false) makes the eventual
  // exit_status() be failure.  Ok(result, "what") prints an error first if
  // result.is_error().

  void Ok(bool ok) { ok_ = ok && ok_; }

  template <typename T, typename U>
  bool Ok(T&& result, U&& failure) {
    if (result.is_ok()) {
      return true;
    }
    std::cerr << failure << ": " << result.error_value() << std::endl;
    ok_ = false;
    return false;
  }

  bool ok() const { return ok_; }

  int exit_status() const { return ok_ ? EXIT_SUCCESS : EXIT_FAILURE; }

 private:
  zxdump::TaskHolder holder_;
  std::queue<const char*> dump_files_;
  TaskQueue tasks_;
  bool live_ = false;
  bool use_root_job_ = false;
  bool read_memory_ = true;
  bool ok_ = true;
};

#endif  // SRC_LIB_ZXDUMP_CLI_H_

// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_TEST_LD_STARTUP_SPAWN_PROCESS_TESTS_POSIX_H_
#define LIB_LD_TEST_LD_STARTUP_SPAWN_PROCESS_TESTS_POSIX_H_

#include <signal.h>

#include <cstdint>
#include <initializer_list>
#include <string>
#include <vector>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

#include "ld-load-tests-posix-base.h"

// The spawned-process tests work by using the normal system program loader
// such that it loads the executable and finds the dynamic linker as its
// PT_INTERP in the standard way.

namespace ld::testing {

// On POSIX-like systems this means using posix_spawn.  There is just one big
// operation, really.  So all the methods before Run() just collect details of
// what to do.
class LdStartupSpawnProcessTests : public ::testing::Test, public LdLoadTestsPosixBase {
 public:
  static constexpr std::string_view kName = "LdStartupSpawnProcessTests";

  static constexpr int64_t kRunFailureForTrap = 128 + SIGILL;
  static constexpr int64_t kRunFailureForBadPointer = 128 + SIGSEGV;

  void Init(std::initializer_list<std::string_view> args = {},
            std::initializer_list<std::string_view> env = {});

  void Load(std::string_view executable_name);

  int64_t Run();

  template <class... Reports>
  void LoadAndFail(std::string_view name, Reports&&... reports) {
    ASSERT_NO_FATAL_FAILURE(StartupLoadAndFail(*this, name, std::forward<Reports>(reports)...));
  }

  ~LdStartupSpawnProcessTests();

 private:
  std::string executable_;
  std::vector<std::string> argv_, envp_;
  pid_t pid_ = -1;
};

}  // namespace ld::testing

#endif  // LIB_LD_TEST_LD_STARTUP_SPAWN_PROCESS_TESTS_POSIX_H_

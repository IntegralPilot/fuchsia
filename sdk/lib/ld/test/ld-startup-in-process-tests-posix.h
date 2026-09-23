// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_TEST_LD_STARTUP_IN_PROCESS_TESTS_POSIX_H_
#define LIB_LD_TEST_LD_STARTUP_IN_PROCESS_TESTS_POSIX_H_

#include <lib/elfldltl/testing/loader.h>

#include <cstdint>
#include <initializer_list>
#include <string_view>

#include "ld-load-tests-posix-base.h"

// The in-process here work by doing ELF loading approximately as the system
// program loader would, but into this process that's running the test.  Once
// the dynamic linker has been loaded, the InProcessTestLaunch object knows how
// its entry point wants to be called.  It's responsible for collecting the
// information to be passed to the dynamic linker, and then doing the call into
// its entry point to emulate what it would expect from the program loader
// starting an initial thread.

namespace ld::testing {

// On POSIX-like systems this means a canonical stack setup that transfers
// arguments, environment, and a set of integer key-value pairs called the
// auxiliary vector (auxv) that carries values important for bootstrapping.
class LdStartupInProcessTests : public elfldltl::testing::LoadTests<>, public LdLoadTestsPosixBase {
 public:
  static constexpr std::string_view kName = "LdStartupInProcessTests";

  static constexpr bool kCanCollectLog = false;

  static constexpr std::string_view kTestExecutableSuffix = kTestExecutableInProcessSuffix;

  void Init(std::initializer_list<std::string_view> args = {},
            std::initializer_list<std::string_view> env = {});

  void Load(std::string_view executable_name);

  int64_t Run();

  void ExpectLog(std::string_view expected_log);

  ~LdStartupInProcessTests();

 private:
  using Base = elfldltl::testing::LoadTests<>;
  using Base::Load;

  struct AuxvBlock;

  void AllocateStack();

  void PopulateStack(std::initializer_list<std::string_view> argv,
                     std::initializer_list<std::string_view> envp);

  Loader loader_, exec_loader_;
  void* stack_ = nullptr;
  void* sp_ = nullptr;
  AuxvBlock* auxv_ = nullptr;
  uintptr_t entry_ = 0;
};

}  // namespace ld::testing

#endif  // LIB_LD_TEST_LD_STARTUP_IN_PROCESS_TESTS_POSIX_H_

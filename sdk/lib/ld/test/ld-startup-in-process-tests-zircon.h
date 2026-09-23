// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_TEST_LD_STARTUP_IN_PROCESS_TESTS_ZIRCON_H_
#define LIB_LD_TEST_LD_STARTUP_IN_PROCESS_TESTS_ZIRCON_H_

#include <lib/elfldltl/testing/loader.h>
#include <lib/ld/testing/test-processargs.h>
#include <lib/zx/vmar.h>

#include <cstdint>
#include <initializer_list>
#include <optional>
#include <string_view>

#include "ld-load-zircon-ldsvc-tests-base.h"

// The in-process tests here work by doing ELF loading approximately as the
// system program loader would, but into this process that's running the test.
// Once the dynamic linker has been loaded, the InProcessTestLaunch object
// knows how its entry point wants to be called.  It's responsible for
// collecting the information to be passed to the dynamic linker, and then
// doing the call into its entry point to emulate what it would expect from the
// program loader starting an initial thread.

namespace ld::testing {

// On Fuchsia this means packing a message on the bootstrap channel.  The entry
// point receives the bootstrap channel (zx_handle_t) and the base address of
// the vDSO.
class LdStartupInProcessTests
    : public elfldltl::testing::LoadTests<elfldltl::testing::LocalVmarLoaderTraits>,
      public LdLoadZirconLdsvcTestsBase {
 public:
  static constexpr std::string_view kName = "LdStartupInProcessTests";

  static constexpr std::string_view kTestExecutableSuffix = kTestExecutableInProcessSuffix;

  void Init(std::initializer_list<std::string_view> args = {},
            std::initializer_list<std::string_view> env = {});

  void Load(std::string_view executable_name,
            std::optional<std::string_view> expected_config = std::nullopt);

  int64_t Run();
  std::pair<int64_t, zx::channel> RunWithCustomBootstrap();

  ~LdStartupInProcessTests();

 private:
  TestProcessArgs& bootstrap() { return procargs_; }

  using Base = elfldltl::testing::LoadTests<elfldltl::testing::LocalVmarLoaderTraits>;
  using Base::Load;

  uintptr_t entry_ = 0;
  TestProcessArgs procargs_;
  zx::vmar test_vmar_;
};

}  // namespace ld::testing

#endif  // LIB_LD_TEST_LD_STARTUP_IN_PROCESS_TESTS_ZIRCON_H_

// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_TEST_LD_STARTUP_SPAWN_PROCESS_TESTS_ZIRCON_H_
#define LIB_LD_TEST_LD_STARTUP_SPAWN_PROCESS_TESTS_ZIRCON_H_

#include <lib/zx/vmo.h>

#include <cstdint>
#include <optional>
#include <string_view>

#include <gtest/gtest.h>

#include "ld-load-zircon-process-tests-base.h"

// The spawned-process tests work by using the normal system program loader
// such that it loads the executable and finds the dynamic linker as its
// PT_INTERP in the standard way.

namespace ld::testing {

// On Fuchsia this means using fdio_spawn.  There is just one big operation,
// really.  So all the methods before Run() just collect details of what to do.
class LdStartupSpawnProcessTests : public ::testing::Test, public LdLoadZirconProcessTestsBase {
 public:
  static constexpr std::string_view kName = "LdStartupSpawnProcessTests";

  using LdLoadZirconLdsvcTestsBase::Init;

  void Load(std::string_view executable_name,
            std::optional<std::string_view> expected_config = std::nullopt);

  int64_t Run();

  template <class... Reports>
  void LoadAndFail(std::string_view name, Reports&&... reports) {
    ASSERT_NO_FATAL_FAILURE(StartupLoadAndFail(*this, name, std::forward<Reports>(reports)...));
  }

  ~LdStartupSpawnProcessTests();

 protected:
  static void SetUpTestSuite() {
    GTEST_SKIP() << "TODO(https://fxbug.dev/479521328): system program loader "
                    "testing disabled due to stack size issues "
                    "in startup dynamic linker";
  }

 private:
  zx::vmo executable_;
};

}  // namespace ld::testing

#endif  // LIB_LD_TEST_LD_STARTUP_SPAWN_PROCESS_TESTS_ZIRCON_H_

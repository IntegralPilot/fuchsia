// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_TEST_LD_LOAD_ZIRCON_LDSVC_TESTS_BASE_H_
#define LIB_LD_TEST_LD_LOAD_ZIRCON_LDSVC_TESTS_BASE_H_

#include <lib/elfldltl/soname.h>
#include <lib/elfldltl/testing/get-test-data.h>
#include <lib/ld/testing/mock-loader-service.h>
#include <lib/ld/testing/test-processargs.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>

#include <filesystem>
#include <initializer_list>
#include <string_view>

#include "ld-load-tests-base.h"

namespace ld::testing {

// This is the common base class for test fixtures that use a
// fuchsia.ldsvc.Loader service and set expectations for the dependencies
// loaded by it. This class proxies calls to the MockLoaderServiceForTest and
// passes the function it should use to retrieve test VMO files.
//
// It takes calls giving ordered expectations for Loader service requests from
// the process under test.  These must be used after Load() and before Run()
// in test cases.
class LdLoadZirconLdsvcTestsBase : public LdLoadTestsBase {
 public:
  // Optionally expect the dynamic linker to send a Config(config) message.
  void LdsvcExpectConfig(std::optional<std::string_view> config) {
    if (config) {
      mock_.ExpectConfig(*config);
    }
  }

  // Prime the MockLoaderService with the VMO for a dependency by name,
  // and expect the MockLoader to load that dependency for the test.
  void LdsvcExpectDependency(std::string_view name) { mock_.ExpectDependency(name); }

  zx::channel TakeLdsvc() { return mock_.TakeLdsvc(); }

  zx::vmo GetLibVmo(std::string_view name) { return mock_.GetVmo(name); }

  static std::string FindInterp(zx::unowned_vmo vmo);

  void VerifyAndClearNeeded() { mock_.VerifyAndClearExpectations(); }

  void LdsvcPathPrefix(std::string_view executable,
                       std::optional<std::string_view> libprefix = std::nullopt);

  // Use the PT_INTERP string to update LdsvcPathPrefix() and then return the
  // config found, which can be passed to LdsvcExpectConfig().  The optional
  // argument makes it a failure if the extract Config() string doesn't match,
  // and doesn't change LdsvcPathPrefix().
  std::optional<std::string> ConfigFromInterp(
      const std::filesystem::path& interp,
      std::optional<std::string_view> expected_config = std::nullopt);

  // The same, but extract the PT_INTERP string from the executable file VMO.
  std::optional<std::string> ConfigFromInterp(
      zx::unowned_vmo executable_vmo,
      std::optional<std::string_view> expected_config = std::nullopt) {
    return ConfigFromInterp(FindInterp(executable_vmo->borrow()), expected_config);
  }

  // This just combines GetExecutableVmo, FindInterp, ConfigFromInterp, and
  // LdsvcExpectConfig.
  zx::vmo GetExecutableVmoWithInterpConfig(
      std::string_view executable, std::optional<std::string_view> expected_config = std::nullopt);

  // Uses to TestElfLoadSet::Get(test_name) to do Needed() within the test's
  // package namespace based on the libprefix found in the TestElfObject data
  // rather than a PT_INTERP in the file.
  void NeededViaLoadSet(elfldltl::Soname<> set_name, std::initializer_list<std::string_view> names);

 protected:
  void Init(std::initializer_list<std::string_view> args = {},
            std::initializer_list<std::string_view> env = {});

  const std::vector<std::string>& argv() const { return args_; }
  const std::vector<std::string>& envp() const { return env_; }

  TestProcessArgs& LdStartupProcArgs(TestProcessArgs& bootstrap, zx::unowned_vmar allocation_vmar);

  zx::vmo GetInterp(std::string_view executable_name,
                    std::optional<std::string_view> expected_config);

  void LdsvcExpectNeeded() {
    for (const auto& [name, found] : TakeNeededLibs()) {
      if (found) {
        mock_.ExpectDependency(name);
      } else {
        mock_.ExpectMissing(name);
      }
    }
  }

  MockLoaderServiceForTest& mock() { return mock_; }

 private:
  MockLoaderServiceForTest mock_;
  std::vector<std::string> args_, env_;
};

}  // namespace ld::testing

#endif  // LIB_LD_TEST_LD_LOAD_ZIRCON_LDSVC_TESTS_BASE_H_

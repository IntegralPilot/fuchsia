// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <lib/async/default.h>
#include <lib/syslog/cpp/macros.h>

#include <zxtest/zxtest.h>

#include "src/ui/scenic/tests/utils/blocking_present.h"
#include "src/ui/scenic/tests/utils/scenic_ctf_test_base.h"

namespace integration_tests {

namespace {

struct TestParameters {
  std::string test_name;
  fuchsia_ui_composition::TrustedFlatlandConfig config;

  // Constructor for gtest value-parameterized tests.
  TestParameters(std::string name, fuchsia_ui_composition::TrustedFlatlandConfig cfg)
      : test_name(std::move(name)), config(std::move(cfg)) {}

  // Copy constructor for gtest value-parameterized tests.
  // A user-defined copy constructor is needed because TrustedFlatlandConfig is not copyable.
  TestParameters(const TestParameters& other)
      : test_name(other.test_name), config(CopyConfig(other.config)) {}

 private:
  // `TrustedFlatlandConfig` is a FIDL `resource table`, so it can't be copied even though it
  // currently doesn't contain any handle types (there is no equivalent to `fidl::Clone` for natural
  // resource types).
  static fuchsia_ui_composition::TrustedFlatlandConfig CopyConfig(
      const fuchsia_ui_composition::TrustedFlatlandConfig& other) {
    fuchsia_ui_composition::TrustedFlatlandConfig config;
    config.schedule_asap() = other.schedule_asap();
    config.pass_acquire_fences() = other.pass_acquire_fences();
    config.skips_present_credits() = other.skips_present_credits();
    config.skips_on_frame_presented() = other.skips_on_frame_presented();
    config.use_flatland2_api() = other.use_flatland2_api();
    return config;

    // This will break if another field is added to `TrustedFlatlandConfig`, to notify us that this
    // function needs updating.
    static_assert(sizeof(fuchsia_ui_composition::TrustedFlatlandConfig) ==
                  5 * sizeof(std::optional<bool>));
  }
};

fuchsia_ui_composition::TrustedFlatlandConfig ScheduleAsapConfig() {
  fuchsia_ui_composition::TrustedFlatlandConfig config;
  config.schedule_asap() = true;
  return config;
}

fuchsia_ui_composition::TrustedFlatlandConfig DirectAcquireFencesConfig() {
  fuchsia_ui_composition::TrustedFlatlandConfig config;
  config.pass_acquire_fences() = true;
  return config;
}

fuchsia_ui_composition::TrustedFlatlandConfig UseFlatland2Config() {
  fuchsia_ui_composition::TrustedFlatlandConfig config;
  config.use_flatland2_api() = true;
  return config;
}

fuchsia_ui_composition::TrustedFlatlandConfig AllOptionsConfig() {
  fuchsia_ui_composition::TrustedFlatlandConfig config;
  config.schedule_asap() = true;
  config.pass_acquire_fences() = true;
  config.use_flatland2_api() = true;
  return config;
}

}  // namespace

// Test fixture that sets up an environment with a Scenic we can connect to.
class TrustedFlatlandFactoryIntegrationTest : public ScenicCtfTest,
                                              public zxtest::WithParamInterface<TestParameters> {
 protected:
  void SetUp() override {
    ScenicCtfTest::SetUp();
    factory_ = ConnectSyncIntoRealm<fuchsia_ui_composition::TrustedFlatlandFactory>();
  }

  fidl::SyncClient<fuchsia_ui_composition::TrustedFlatlandFactory> factory_;
};

TEST_P(TrustedFlatlandFactoryIntegrationTest, CreateFlatland) {
  auto param = GetParam();

  auto [flatland_client_end, flatland_server_end] =
      fidl::CreateEndpoints<fuchsia_ui_composition::Flatland>().value();

  FlatlandClientWithEventHandler flatland(std::move(flatland_client_end), this->dispatcher());

  auto result = factory_->CreateFlatland(
      {{.server_end = std::move(flatland_server_end), .config = std::move(param.config)}});
  ASSERT_TRUE(result.is_ok());

  // Do a trivial operation to ensure the Flatland channel is usable.
  auto clear_result = flatland->Clear();
  ASSERT_TRUE(clear_result.is_ok());
  BlockingPresent(this, flatland);
}

INSTANTIATE_TEST_SUITE_P(
    TrustedFlatlandFactoryIntegrationTestWithParams, TrustedFlatlandFactoryIntegrationTest,
    zxtest::Values(TestParameters("DefaultConfig", {}),
                   TestParameters("ScheduleAsap", ScheduleAsapConfig()),
                   TestParameters("DirectAcquireFences", DirectAcquireFencesConfig()),
                   TestParameters("UseFlatland2", UseFlatland2Config()),
                   TestParameters("AllOptions", AllOptionsConfig())),
    [](const zxtest::TestParamInfo<TestParameters>& info) { return info.param.test_name; });

}  // namespace integration_tests

// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/power/drivers/fusb302/fusb302-identity.h"

#include <fidl/fuchsia.hardware.i2c/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/fpromise/single_threaded_executor.h>
#include <lib/inspect/cpp/hierarchy.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/inspect/cpp/reader.h>
#include <lib/inspect/testing/cpp/inspect.h>
#include <lib/mock-i2c/mock-i2c-gtest.h>

#include <optional>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"

namespace fusb302 {

namespace {

// Register addresses from Table 16 "Register Definitions" on page 18 of the
// Rev 5 datasheet.
constexpr int kDeviceIdAddress = 0x01;

class Fusb302IdentityTest : public ::testing::Test {
 public:
  void SetUp() override {
    auto endpoints = fidl::Endpoints<fuchsia_hardware_i2c::Device>::Create();
    mock_i2c_client_ = std::move(endpoints.client);

    EXPECT_OK(loop_.StartThread());
    fidl::BindServer<fuchsia_hardware_i2c::Device>(loop_.dispatcher(), std::move(endpoints.server),
                                                   &mock_i2c_);

    identity_.emplace(mock_i2c_client_, inspect_.GetRoot().CreateChild("Identity"));
  }

  void TearDown() override { mock_i2c_.VerifyAndClear(); }

  void ExpectInspectPropertyEquals(const char* property_name, const std::string& expected_value) {
    fpromise::result<inspect::Hierarchy> hierarchy_result =
        fpromise::run_single_threaded(inspect::ReadFromInspector(inspect_));
    ASSERT_TRUE(hierarchy_result.is_ok());

    inspect::Hierarchy hierarchy = std::move(hierarchy_result.value());
    const inspect::Hierarchy* identity_root = hierarchy.GetByPath({"Identity"});
    ASSERT_TRUE(identity_root);

    EXPECT_THAT(identity_root->node(),
                inspect::testing::PropertyList(
                    testing::Contains(inspect::testing::StringIs(property_name, expected_value))));
  }

 protected:
  fdf_testing::ScopedGlobalLogger logger_;

  inspect::Inspector inspect_;

  async::Loop loop_{&kAsyncLoopConfigNeverAttachToThread};
  mock_i2c::MockI2cGtest mock_i2c_;
  fidl::ClientEnd<fuchsia_hardware_i2c::Device> mock_i2c_client_;
  std::optional<Fusb302Identity> identity_;
};

TEST_F(Fusb302IdentityTest, Vim3Identity) {
  mock_i2c_.ExpectWrite({kDeviceIdAddress}).ExpectReadStop({0x91});

  EXPECT_OK(identity_->ReadIdentity());

  ExpectInspectPropertyEquals("Product", "FUSB302BMPX");
  ExpectInspectPropertyEquals("Version", "FUSB302B_revB");
}

}  // namespace

}  // namespace fusb302

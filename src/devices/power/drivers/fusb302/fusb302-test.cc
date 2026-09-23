// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/power/drivers/fusb302/fusb302.h"

#include <fidl/fuchsia.hardware.i2c/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fpromise/single_threaded_executor.h>
#include <lib/inspect/cpp/hierarchy.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/inspect/cpp/reader.h>
#include <lib/inspect/testing/cpp/inspect.h>
#include <lib/mock-i2c/mock-i2c-gtest.h>
#include <lib/zx/interrupt.h>
#include <zircon/types.h>

#include <cstdint>
#include <optional>
#include <utility>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/devices/power/drivers/fusb302/pd-sink-state-machine.h"
#include "src/devices/power/drivers/fusb302/typec-port-state-machine.h"
#include "src/devices/power/drivers/fusb302/usb-pd-defs.h"
#include "src/lib/testing/predicates/status.h"

namespace fusb302 {

namespace {

class Fusb302Test : public ::testing::Test {
 public:
  void SetUp() override {
    auto endpoints = fidl::Endpoints<fuchsia_hardware_i2c::Device>::Create();
    fidl::ClientEnd<fuchsia_hardware_i2c::Device> mock_i2c_client = std::move(endpoints.client);

    EXPECT_OK(loop_.StartThread());
    fidl::BindServer<fuchsia_hardware_i2c::Device>(loop_.dispatcher(), std::move(endpoints.server),
                                                   &mock_i2c_);
    zx::interrupt gpio_interrupt;
    ASSERT_OK(
        zx::interrupt::create(zx::resource(), /*vector=*/0, ZX_INTERRUPT_VIRTUAL, &gpio_interrupt));

    device_.emplace(fdf::SynchronizedDispatcher(), std::move(mock_i2c_client),
                    fidl::ClientEnd<fuchsia_hardware_gpio::Gpio>{}, std::move(gpio_interrupt));
  }

  void ExpectInspectPropertyEquals(const char* node_name, const char* property_name,
                                   int64_t expected_value) {
    fpromise::result<inspect::Hierarchy> hierarchy_result =
        fpromise::run_single_threaded(inspect::ReadFromInspector(device_->InspectorForTesting()));
    ASSERT_TRUE(hierarchy_result.is_ok());

    inspect::Hierarchy hierarchy = std::move(hierarchy_result.value());
    const inspect::Hierarchy* node_root = hierarchy.GetByPath({node_name});
    ASSERT_TRUE(node_root);

    EXPECT_THAT(node_root->node(), inspect::testing::PropertyList(testing::Contains(
                                       inspect::testing::IntIs(property_name, expected_value))));
  }

 protected:
  async::Loop loop_{&kAsyncLoopConfigNeverAttachToThread};
  mock_i2c::MockI2cGtest mock_i2c_;
  std::optional<Fusb302> device_;
};

TEST_F(Fusb302Test, ConstructorInspectState) {
  ExpectInspectPropertyEquals("Sensors", "CCTermination",
                              static_cast<int64_t>(usb_pd::ConfigChannelTermination::kUnknown));
  ExpectInspectPropertyEquals("PortStateMachine", "State",
                              static_cast<int64_t>(TypeCPortState::kSinkUnattached));
  ExpectInspectPropertyEquals("SinkPolicyEngineStateMachine", "State",
                              static_cast<int64_t>(SinkPolicyEngineState::kStartup));
}

}  // namespace

}  // namespace fusb302

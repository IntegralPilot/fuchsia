// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "../gpioimpl-visitor.h"

#include <fidl/fuchsia.hardware.gpio/cpp/fidl.h>
#include <fidl/fuchsia.hardware.pinimpl/cpp/fidl.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/testing/visitor-test-helper.h>
#include <lib/driver/devicetree/visitors/default/bind-property/bind-property.h>
#include <lib/driver/devicetree/visitors/default/mmio/mmio.h>
#include <lib/driver/devicetree/visitors/registry.h>

#include <cstdint>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/gpio/cpp/bind.h>
#include <gtest/gtest.h>

#include "dts/gpio.h"

namespace gpio_impl_dt {

class GpioImplVisitorTester : public fdf_devicetree::testing::VisitorTestHelper<GpioImplVisitor> {
 public:
  GpioImplVisitorTester(std::string_view dtb_path)
      : fdf_devicetree::testing::VisitorTestHelper<GpioImplVisitor>(dtb_path,
                                                                    "GpioImplVisitorTest") {}
};

TEST(GpioImplVisitorTest, TestGpiosProperty) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());
  ASSERT_TRUE(visitors.RegisterVisitor(std::make_unique<fdf_devicetree::MmioVisitor>()).is_ok());

  auto tester = std::make_unique<GpioImplVisitorTester>("/pkg/test-data/gpio.dtb");
  GpioImplVisitorTester* gpio_tester = tester.get();
  ASSERT_TRUE(visitors.RegisterVisitor(std::move(tester)).is_ok());

  ASSERT_EQ(ZX_OK, gpio_tester->manager()->Walk(visitors).status_value());
  ASSERT_TRUE(gpio_tester->DoPublish().is_ok());

  std::vector<fuchsia_hardware_platform_bus::Node> gpio_nodes_a =
      gpio_tester->GetPbusNodes("gpio-controller-ffffa000");
  uint32_t gpioA_id = 0;
  uint32_t gpioB_id = 0;
  ASSERT_EQ(1lu, gpio_nodes_a.size());
  auto metadata = gpio_nodes_a[0].metadata();

  // Test metadata properties.
  ASSERT_TRUE(metadata);
  ASSERT_EQ(1lu, metadata->size());

  // Pin metadata.
  std::vector<uint8_t> metadata_blob = std::move(*(*metadata)[0].data());
  fit::result controller_metadata =
      fidl::Unpersist<fuchsia_hardware_pinimpl::Metadata>(metadata_blob);
  ASSERT_TRUE(controller_metadata.is_ok());
  ASSERT_TRUE(controller_metadata->controller_id());
  gpioA_id = *controller_metadata->controller_id();

  ASSERT_TRUE(controller_metadata->init_steps());
  // 11 init steps:
  //   - 6 from gpio-hog (3 pins * 2 steps)
  //   - 4 from video group2 (2 pins * 2 steps)
  //   - 1 from video group6 (1 pin * 1 step)
  // Note: Audio node configurations are parsed as pin states and do not generate init steps.
  ASSERT_EQ((*controller_metadata).init_steps()->size(), 11u);

  // GPIO Hog init steps.
  const auto& init_steps = *controller_metadata->init_steps();
  ASSERT_TRUE(init_steps[0].call());
  ASSERT_EQ(init_steps[0].call()->pin(), static_cast<uint32_t>(HOG_PIN1));
  ASSERT_EQ(init_steps[0].call()->call(),
            fuchsia_hardware_pinimpl::InitCall::WithPinConfig(
                {{.pull = static_cast<fuchsia_hardware_pin::Pull>(0)}}));
  ASSERT_TRUE(init_steps[1].call());
  ASSERT_EQ(init_steps[1].call()->pin(), static_cast<uint32_t>(HOG_PIN1));
  ASSERT_EQ(init_steps[1].call()->call(), fuchsia_hardware_pinimpl::InitCall::WithBufferMode(
                                              fuchsia_hardware_gpio::BufferMode::kOutputLow));

  ASSERT_TRUE(init_steps[2].call());
  ASSERT_EQ(init_steps[2].call()->pin(), static_cast<uint32_t>(HOG_PIN2));
  ASSERT_EQ(init_steps[2].call()->call(),
            fuchsia_hardware_pinimpl::InitCall::WithPinConfig(
                {{.pull = static_cast<fuchsia_hardware_pin::Pull>(HOG_PIN2_FLAG)}}));
  ASSERT_TRUE(init_steps[3].call());
  ASSERT_EQ(init_steps[3].call()->pin(), static_cast<uint32_t>(HOG_PIN2));
  ASSERT_EQ(init_steps[3].call()->call(), fuchsia_hardware_pinimpl::InitCall::WithBufferMode(
                                              fuchsia_hardware_gpio::BufferMode::kInput));

  ASSERT_TRUE(init_steps[4].call());
  ASSERT_EQ(init_steps[4].call()->pin(), static_cast<uint32_t>(HOG_PIN3));
  ASSERT_EQ(init_steps[4].call()->call(),
            fuchsia_hardware_pinimpl::InitCall::WithPinConfig(
                {{.pull = static_cast<fuchsia_hardware_pin::Pull>(HOG_PIN3_FLAG)}}));
  ASSERT_TRUE(init_steps[5].call());
  ASSERT_EQ(init_steps[5].call()->pin(), static_cast<uint32_t>(HOG_PIN3));
  ASSERT_EQ(init_steps[5].call()->call(), fuchsia_hardware_pinimpl::InitCall::WithBufferMode(
                                              fuchsia_hardware_gpio::BufferMode::kInput));

  // Pin controller config init steps (from video node group2 which doesn't have pinctrl-names).
  ASSERT_TRUE(init_steps[6].call());
  ASSERT_EQ(init_steps[6].call()->pin(), static_cast<uint32_t>(GROUP2_PIN1));
  ASSERT_EQ(init_steps[6].call()->call(), fuchsia_hardware_pinimpl::InitCall::WithPinConfig(
                                              {{.power_source = GROUP2_POWER_SOURCE}}));

  ASSERT_TRUE(init_steps[7].call());
  ASSERT_EQ(init_steps[7].call()->pin(), static_cast<uint32_t>(GROUP2_PIN1));
  ASSERT_EQ(init_steps[7].call()->call(), fuchsia_hardware_pinimpl::InitCall::WithBufferMode(
                                              fuchsia_hardware_gpio::BufferMode::kOutputLow));

  ASSERT_TRUE(init_steps[8].call());
  ASSERT_EQ(init_steps[8].call()->pin(), static_cast<uint32_t>(GROUP2_PIN2));
  ASSERT_EQ(init_steps[8].call()->call(), fuchsia_hardware_pinimpl::InitCall::WithPinConfig(
                                              {{.power_source = GROUP2_POWER_SOURCE}}));

  ASSERT_TRUE(init_steps[9].call());
  ASSERT_EQ(init_steps[9].call()->pin(), static_cast<uint32_t>(GROUP2_PIN2));
  ASSERT_EQ(init_steps[9].call()->call(), fuchsia_hardware_pinimpl::InitCall::WithBufferMode(
                                              fuchsia_hardware_gpio::BufferMode::kOutputLow));

  // Pin controller config init steps (from video node group6 which doesn't have pinctrl-names).
  ASSERT_TRUE(init_steps[10].call());
  ASSERT_EQ(init_steps[10].call()->pin(), static_cast<uint32_t>(GROUP6_PIN1));
  ASSERT_EQ(init_steps[10].call()->call(),
            fuchsia_hardware_pinimpl::InitCall::WithPinConfig(
                {{.drive_strength = static_cast<uint64_t>(GROUP6_DRIVE_STRENGTH)}}));

  // Test device_pin_states metadata.
  ASSERT_TRUE(controller_metadata->device_pin_states().has_value());
  ASSERT_EQ(controller_metadata->device_pin_states()->size(), 1lu);

  const auto& dev_pin_states = *controller_metadata->device_pin_states();
  EXPECT_EQ(dev_pin_states[0].name(), "audio-ffffc000");
  ASSERT_EQ(dev_pin_states[0].states().size(), 2lu);

  EXPECT_EQ(dev_pin_states[0].states()[0].name(), "default");
  ASSERT_EQ(dev_pin_states[0].states()[0].pins().size(), 2lu);
  const auto& default_pins = dev_pin_states[0].states()[0].pins();
  EXPECT_EQ(default_pins[0].pin(), static_cast<uint32_t>(GROUP1_PIN1));
  ASSERT_EQ(default_pins[0].call().Which(), fuchsia_hardware_pinimpl::InitCall::Tag::kPinConfig);
  EXPECT_EQ(default_pins[0].call().pin_config()->function().value(),
            static_cast<uint64_t>(GROUP1_FUNCTION));
  EXPECT_EQ(default_pins[0].call().pin_config()->drive_strength_ua().value(),
            static_cast<uint32_t>(GROUP1_DRIVE_STRENGTH));
  EXPECT_EQ(default_pins[0].call().pin_config()->drive_type().value(),
            fuchsia_hardware_pin::DriveType::kOpenDrain);
  EXPECT_EQ(default_pins[1].pin(), static_cast<uint32_t>(GROUP1_PIN2));
  ASSERT_EQ(default_pins[1].call().Which(), fuchsia_hardware_pinimpl::InitCall::Tag::kPinConfig);
  EXPECT_EQ(default_pins[1].call().pin_config()->function().value(),
            static_cast<uint64_t>(GROUP1_FUNCTION));
  EXPECT_EQ(default_pins[1].call().pin_config()->drive_strength_ua().value(),
            static_cast<uint32_t>(GROUP1_DRIVE_STRENGTH));
  EXPECT_EQ(default_pins[1].call().pin_config()->drive_type().value(),
            fuchsia_hardware_pin::DriveType::kOpenDrain);

  EXPECT_EQ(dev_pin_states[0].states()[1].name(), "sleep");
  ASSERT_EQ(dev_pin_states[0].states()[1].pins().size(), 2lu);
  const auto& sleep_pins = dev_pin_states[0].states()[1].pins();
  EXPECT_EQ(sleep_pins[0].pin(), static_cast<uint32_t>(GROUP3_PIN1));
  ASSERT_EQ(sleep_pins[0].call().Which(), fuchsia_hardware_pinimpl::InitCall::Tag::kPinConfig);
  EXPECT_EQ(sleep_pins[0].call().pin_config()->pull().value(), fuchsia_hardware_pin::Pull::kNone);
  EXPECT_EQ(sleep_pins[0].call().pin_config()->drive_type().value(),
            fuchsia_hardware_pin::DriveType::kOpenSource);
  EXPECT_EQ(sleep_pins[0].call().pin_config()->wake_vector().value(), true);
  EXPECT_EQ(sleep_pins[1].pin(), static_cast<uint32_t>(GROUP3_PIN1));
  ASSERT_EQ(sleep_pins[1].call().Which(), fuchsia_hardware_pinimpl::InitCall::Tag::kBufferMode);
  EXPECT_EQ(sleep_pins[1].call().buffer_mode().value(), fuchsia_hardware_gpio::BufferMode::kInput);

  // GPIO pins.
  ASSERT_TRUE(controller_metadata->pins().has_value());
  ASSERT_EQ((*controller_metadata).pins()->size(), 2lu);
  std::span<fuchsia_hardware_pinimpl::Pin> gpio_pins = controller_metadata->pins().value();
  ASSERT_EQ(gpio_pins.size(), 2lu);
  EXPECT_EQ(gpio_pins[0].pin(), static_cast<uint32_t>(PIN1));
  EXPECT_EQ(gpio_pins[0].name(), PIN1_NAME);
  EXPECT_EQ(gpio_pins[0].id().value(), 0u);
  EXPECT_EQ(gpio_pins[1].pin(), static_cast<uint32_t>(PIN2));
  EXPECT_EQ(gpio_pins[1].name(), PIN2_NAME);
  EXPECT_EQ(gpio_pins[1].id().value(), 1u);

  std::vector<fuchsia_hardware_platform_bus::Node> gpio_nodes_b =
      gpio_tester->GetPbusNodes("gpio-controller-ffffb000");
  ASSERT_EQ(1lu, gpio_nodes_b.size());
  auto metadata_b = gpio_nodes_b[0].metadata();

  // Test metadata properties.
  ASSERT_TRUE(metadata_b);
  ASSERT_EQ(1lu, metadata_b->size());

  // Controller metadata.
  std::vector<uint8_t> metadata_blob_b = std::move(*(*metadata_b)[0].data());
  fit::result controller_metadata_b =
      fidl::Unpersist<fuchsia_hardware_pinimpl::Metadata>(metadata_blob_b);
  ASSERT_TRUE(controller_metadata_b.is_ok());
  ASSERT_TRUE(controller_metadata_b->controller_id());
  gpioB_id = *controller_metadata_b->controller_id();

  ASSERT_TRUE(controller_metadata_b->init_steps());
  ASSERT_EQ((*controller_metadata_b).init_steps()->size(), 2u);

  // Pin controller config init steps.
  ASSERT_TRUE((*controller_metadata_b->init_steps())[0].call());
  ASSERT_EQ((*controller_metadata_b->init_steps())[0].call()->pin(),
            static_cast<uint32_t>(GROUP4_PIN1));
  ASSERT_EQ((*controller_metadata_b->init_steps())[0].call()->call(),
            fuchsia_hardware_pinimpl::InitCall::WithPinConfig(
                {{.pull = fuchsia_hardware_pin::Pull::kUp,
                  .drive_type = fuchsia_hardware_pin::DriveType::kPushPull}}));

  ASSERT_TRUE((*controller_metadata_b->init_steps())[1].call());
  ASSERT_EQ((*controller_metadata_b->init_steps())[1].call()->pin(),
            static_cast<uint32_t>(GROUP5_PIN1));
  ASSERT_EQ((*controller_metadata_b->init_steps())[1].call()->call(),
            fuchsia_hardware_pinimpl::InitCall::WithPinConfig({{.function_name = "spi-bus"}}));

  ASSERT_FALSE(controller_metadata_b->device_pin_states().has_value());

  ASSERT_EQ(1lu, gpio_tester->GetCompositeNodeSpecs("audio").size());
  auto mgr_request_audio = gpio_tester->GetCompositeNodeSpecs("audio")[0];

  ASSERT_TRUE(mgr_request_audio.parents2().has_value());
  ASSERT_EQ(4lu, mgr_request_audio.parents2()->size());

  // 1st parent is pdev. Skipping that.
  // 2nd parent is GPIO PIN1.
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {{fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
        fdf::MakeProperty2(bind_fuchsia::NAME, std::string(PIN1_NAME))}},
      (*mgr_request_audio.parents2())[1].properties(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {{fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
        fdf::MakeAcceptBindRule(bind_fuchsia::ID, 0u)}},
      (*mgr_request_audio.parents2())[1].bind_rules(), false));

  // 3rd parent is GPIO PIN2.
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {{fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
        fdf::MakeProperty2(bind_fuchsia::NAME, std::string(PIN2_NAME))}},
      (*mgr_request_audio.parents2())[2].properties(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {{fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
        fdf::MakeAcceptBindRule(bind_fuchsia::ID, 1u)}},
      (*mgr_request_audio.parents2())[2].bind_rules(), false));

  // 4th parent is PIN STATES.
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {{fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.pin.PinStatesService"),
        fdf::MakeProperty2(bind_fuchsia::ID, static_cast<uint32_t>(0)),
        fdf::MakeProperty2(bind_fuchsia::NAME, std::string("pin-states-0"))}},
      (*mgr_request_audio.parents2())[3].properties(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {{fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.pin.PinStatesService"),
        fdf::MakeAcceptBindRule(bind_fuchsia::ID, gpioA_id),
        fdf::MakeAcceptBindRule(bind_fuchsia::NAME, std::string("audio-ffffc000"))}},
      (*mgr_request_audio.parents2())[3].bind_rules(), false));

  ASSERT_EQ(1lu, gpio_tester->GetCompositeNodeSpecs("video").size());
  auto mgr_request_video = gpio_tester->GetCompositeNodeSpecs("video")[0];
  ASSERT_TRUE(mgr_request_video.parents2().has_value());
  ASSERT_EQ(3lu, mgr_request_video.parents2()->size());

  // 1st parent is pdev. Skipping that.
  // 2nd and 3rd parents are GPIO INIT of different gpio controllers.
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {{fdf::MakeProperty2(bind_fuchsia::INIT_STEP, bind_fuchsia_gpio::BIND_INIT_STEP_GPIO),
        fdf::MakeProperty2(bind_fuchsia::ID, static_cast<uint32_t>(0)),
        fdf::MakeProperty2(bind_fuchsia::NAME, std::string("gpio-init-0"))}},
      (*mgr_request_video.parents2())[1].properties(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {{fdf::MakeAcceptBindRule(bind_fuchsia::INIT_STEP, bind_fuchsia_gpio::BIND_INIT_STEP_GPIO),
        fdf::MakeAcceptBindRule(bind_fuchsia::ID, gpioA_id)}},
      (*mgr_request_video.parents2())[1].bind_rules(), false));

  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {{fdf::MakeProperty2(bind_fuchsia::INIT_STEP, bind_fuchsia_gpio::BIND_INIT_STEP_GPIO),
        fdf::MakeProperty2(bind_fuchsia::ID, static_cast<uint32_t>(1)),
        fdf::MakeProperty2(bind_fuchsia::NAME, std::string("gpio-init-1"))}},
      (*mgr_request_video.parents2())[2].properties(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {{fdf::MakeAcceptBindRule(bind_fuchsia::INIT_STEP, bind_fuchsia_gpio::BIND_INIT_STEP_GPIO),
        fdf::MakeAcceptBindRule(bind_fuchsia::ID, gpioB_id)}},
      (*mgr_request_video.parents2())[2].bind_rules(), false));
}

}  // namespace gpio_impl_dt

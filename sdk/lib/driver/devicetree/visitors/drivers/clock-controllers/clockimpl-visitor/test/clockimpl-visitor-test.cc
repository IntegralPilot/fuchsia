// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "../clockimpl-visitor.h"

#include <fidl/fuchsia.hardware.clockimpl/cpp/fidl.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/testing/visitor-test-helper.h>
#include <lib/driver/devicetree/visitors/default/bind-property/bind-property.h>
#include <lib/driver/devicetree/visitors/default/mmio/mmio.h>
#include <lib/driver/devicetree/visitors/registry.h>

#include <cstdint>

#include <bind/fuchsia/clock/cpp/bind.h>
#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/platform/cpp/bind.h>
#include <gtest/gtest.h>

#include "dts/clock.h"

namespace clock_impl_dt {

class ClockImplVisitorTester : public fdf_devicetree::testing::VisitorTestHelper<ClockImplVisitor> {
 public:
  ClockImplVisitorTester(std::string_view dtb_path)
      : fdf_devicetree::testing::VisitorTestHelper<ClockImplVisitor>(dtb_path,
                                                                     "ClockImplVisitorTest") {}
};

TEST(ClockImplVisitorTest, TestClocksProperty) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());
  ASSERT_TRUE(visitors.RegisterVisitor(std::make_unique<fdf_devicetree::MmioVisitor>()).is_ok());

  auto tester = std::make_unique<ClockImplVisitorTester>("/pkg/test-data/clock.dtb");
  ClockImplVisitorTester* clock_tester = tester.get();
  ASSERT_TRUE(visitors.RegisterVisitor(std::move(tester)).is_ok());

  ASSERT_EQ(ZX_OK, clock_tester->manager()->Walk(visitors).status_value());
  ASSERT_TRUE(clock_tester->DoPublish().is_ok());

  std::vector<fuchsia_hardware_platform_bus::Node> my_clock_nodes =
      clock_tester->GetPbusNodes("my-clock-device");
  ASSERT_EQ(1lu, my_clock_nodes.size());
  auto metadata = my_clock_nodes[0].metadata();

  // Test metadata properties.
  ASSERT_TRUE(metadata);
#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  ASSERT_EQ(2lu, metadata->size());
#else
  ASSERT_EQ(1lu, metadata->size());
#endif

  // Init steps metadata
  std::vector<uint8_t> metadata_blob_1 = std::move(*(*metadata)[0].data());
  fit::result init_steps =
      fidl::Unpersist<fuchsia_hardware_clockimpl::InitMetadata>(cpp20::span(metadata_blob_1));
  ASSERT_TRUE(init_steps.is_ok());
  // Steps expected - Disable for CLK_ID3, SetInput as CLK_ID5, Enable for CLK_ID3
  ASSERT_EQ(init_steps->steps().size(), 3lu);
  EXPECT_EQ(init_steps->steps()[0].id(), static_cast<uint32_t>(CLK_ID3));
  EXPECT_EQ(init_steps->steps()[0].call()->Which(),
            fuchsia_hardware_clockimpl::InitCall::Tag::kDisable);
  EXPECT_EQ(init_steps->steps()[1].id(), static_cast<uint32_t>(CLK_ID3));
  EXPECT_EQ(init_steps->steps()[1].call()->Which(),
            fuchsia_hardware_clockimpl::InitCall::Tag::kInputIdx);
  EXPECT_EQ(init_steps->steps()[1].call()->input_idx().value(), static_cast<uint32_t>(CLK_ID5));
  EXPECT_EQ(init_steps->steps()[2].id(), static_cast<uint32_t>(CLK_ID3));
  EXPECT_EQ(init_steps->steps()[2].call()->Which(),
            fuchsia_hardware_clockimpl::InitCall::Tag::kEnable);

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  // Clock IDs metadata
  std::vector<uint8_t> metadata_blob_2 = std::move(*(*metadata)[1].data());
  fit::result clock_ids_metadata =
      fidl::Unpersist<fuchsia_hardware_clockimpl::ClockIdsMetadata>(cpp20::span(metadata_blob_2));
  const auto& clock_ids2 = clock_ids_metadata->clock_nodes();
  ASSERT_TRUE(clock_ids2.has_value());
  ASSERT_EQ(clock_ids2.value().size(), 3lu);
  ASSERT_TRUE(clock_ids2.value()[0].clock_id().has_value());
  EXPECT_EQ(clock_ids2.value()[0].clock_id().value(), static_cast<uint32_t>(CLK_ID1));
  ASSERT_TRUE(clock_ids2.value()[1].clock_id().has_value());
  EXPECT_EQ(clock_ids2.value()[1].clock_id().value(), static_cast<uint32_t>(CLK_ID2));
  ASSERT_TRUE(clock_ids2.value()[2].clock_id().has_value());
  EXPECT_EQ(clock_ids2.value()[2].clock_id().value(), static_cast<uint32_t>(CLK_ID6));
#endif

  std::vector<fuchsia_hardware_platform_bus::Node> clock_ctl_nodes =
      clock_tester->GetPbusNodes("clock-controller-ffffb000");
  ASSERT_EQ(1lu, clock_ctl_nodes.size());
  auto metadata_ctl = clock_ctl_nodes[0].metadata();

  // Test metadata properties.
  ASSERT_TRUE(metadata_ctl);
  ASSERT_EQ(1lu, metadata_ctl->size());

  // Init steps metadata
  std::vector<uint8_t> metadata_blob = std::move(*(*metadata_ctl)[0].data());
  fit::result init_steps_ctl =
      fidl::Unpersist<fuchsia_hardware_clockimpl::InitMetadata>(cpp20::span(metadata_blob));
  ASSERT_TRUE(init_steps_ctl.is_ok());
  // Steps expected - Disable for CLK_ID4, SetRateHz as CLK_ID4_RATE, Enable for CLK_ID4
  ASSERT_EQ(init_steps_ctl->steps().size(), 3lu);
  EXPECT_EQ(init_steps_ctl->steps()[0].id(), static_cast<uint32_t>(CLK_ID4));
  EXPECT_EQ(init_steps_ctl->steps()[0].call()->Which(),
            fuchsia_hardware_clockimpl::InitCall::Tag::kDisable);
  EXPECT_EQ(init_steps_ctl->steps()[1].id(), static_cast<uint32_t>(CLK_ID4));
  EXPECT_EQ(init_steps_ctl->steps()[1].call()->Which(),
            fuchsia_hardware_clockimpl::InitCall::Tag::kRateHz);
  EXPECT_EQ(init_steps_ctl->steps()[1].call()->rate_hz().value(),
            static_cast<uint32_t>(CLK_ID4_RATE));
  EXPECT_EQ(init_steps_ctl->steps()[2].id(), static_cast<uint32_t>(CLK_ID4));
  EXPECT_EQ(init_steps_ctl->steps()[2].call()->Which(),
            fuchsia_hardware_clockimpl::InitCall::Tag::kEnable);

  ASSERT_EQ(5lu, clock_tester->GetCompositeNodeSpecs().size());

  // Use name filter to get composite node spec? or just index?
  // The original test iterated over "video" pbus nodes and checked composite requests.
  // Assuming "video" node corresponds to one of the requests.
  // Actually, wait, the original code:
  // std::vector<fuchsia_hardware_platform_bus::Node> video_nodes =
  // clock_tester->GetPbusNodes("video"); for ([[maybe_unused]] const auto& node : video_nodes) It
  // iterates over pbus nodes but checks *composite specs* using a global index `mgr_request_idx`.
  // This seems fragile if order changes.
  // However, based on user request, we should just check the specs directly if possible.
  // The test seems to assume "video" node verification implies checking the FIRST composite spec.

  auto video_specs = clock_tester->GetCompositeNodeSpecs("video");
  ASSERT_EQ(1lu, video_specs.size());
  auto mgr_request_video = video_specs[0];

  ASSERT_TRUE(mgr_request_video.parents2().has_value());
  ASSERT_EQ(3lu, mgr_request_video.parents2()->size());

  // 1st parent is pdev. Skipping that.
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {{fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.clock.Service"),
        fdf::MakeProperty2(bind_fuchsia::NAME, std::string(CLK1_NAME))}},
      (*mgr_request_video.parents2())[1].properties(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {{fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.clock.Service"),
        // Clock Node IDs are monotonically increasing integers.
        fdf::MakeAcceptBindRule(bind_fuchsia::ID, 0u)}},
      (*mgr_request_video.parents2())[1].bind_rules(), false));

  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {{fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.clock.Service"),
        fdf::MakeProperty2(bind_fuchsia::NAME, std::string(CLK2_NAME))}},
      (*mgr_request_video.parents2())[2].properties(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {{fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.clock.Service"),
        // Clock Node IDs are monotonically increasing integers.
        fdf::MakeAcceptBindRule(bind_fuchsia::ID, 1u)}},
      (*mgr_request_video.parents2())[2].bind_rules(), false));

  auto audio_specs = clock_tester->GetCompositeNodeSpecs("audio");
  ASSERT_EQ(1lu, audio_specs.size());
  auto mgr_request_audio = audio_specs[0];

  ASSERT_TRUE(mgr_request_audio.parents2().has_value());
  ASSERT_EQ(4lu, mgr_request_audio.parents2()->size());

  // 1st parent is pdev. Skipping that.

  // 2nd is the clock impl parent.
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {{fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.clock.Service")}},
      (*mgr_request_audio.parents2())[1].properties(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {{fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.clock.Service"),
        // Clock Node IDs are monotonically increasing integers.
        fdf::MakeAcceptBindRule(bind_fuchsia::ID, 2u)}},
      (*mgr_request_audio.parents2())[1].bind_rules(), false));

  // The rest are init step clock parents2.
  for (size_t i = 2; i < 4; i++) {
    EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
        {{
            fdf::MakeProperty2(bind_fuchsia::INIT_STEP, bind_fuchsia_clock::BIND_INIT_STEP_CLOCK),
        }},
        (*mgr_request_audio.parents2())[i].properties(), false));
    EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
        {{fdf::MakeAcceptBindRule(bind_fuchsia::INIT_STEP,
                                  bind_fuchsia_clock::BIND_INIT_STEP_CLOCK)}},
        (*mgr_request_audio.parents2())[i].bind_rules(), false));
  }
}

}  // namespace clock_impl_dt

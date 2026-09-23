// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "../adc-visitor.h"

#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/testing/visitor-test-helper.h>
#include <lib/driver/devicetree/visitors/default/bind-property/bind-property.h>
#include <lib/driver/devicetree/visitors/default/mmio/mmio.h>
#include <lib/driver/devicetree/visitors/registry.h>

#include <cstdint>

#include <bind/fuchsia/adc/cpp/bind.h>
#include <bind/fuchsia/cpp/bind.h>
#include <gtest/gtest.h>

#include "dts/adc.h"

namespace adc_dt {

class AdcVisitorTester : public fdf_devicetree::testing::VisitorTestHelper<AdcVisitor> {
 public:
  explicit AdcVisitorTester(std::string_view dtb_path)
      : fdf_devicetree::testing::VisitorTestHelper<AdcVisitor>(dtb_path, "AdcVisitorTest") {}
};

TEST(AdcVisitorTester, TestAdcsProperty) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());
  ASSERT_TRUE(visitors.RegisterVisitor(std::make_unique<fdf_devicetree::MmioVisitor>()).is_ok());

  auto tester = std::make_unique<AdcVisitorTester>("/pkg/test-data/adc.dtb");
  AdcVisitorTester* adc_tester = tester.get();
  ASSERT_TRUE(visitors.RegisterVisitor(std::move(tester)).is_ok());

  ASSERT_EQ(ZX_OK, adc_tester->manager()->Walk(visitors).status_value());
  ASSERT_TRUE(adc_tester->DoPublish().is_ok());

  auto vadc_nodes = adc_tester->GetPbusNodes("vadc-ffffa000");
  ASSERT_EQ(1lu, vadc_nodes.size());
  const auto& node_vadc = vadc_nodes[0];
  auto metadata_vadc = node_vadc.metadata();

  // Test metadata properties.
  ASSERT_TRUE(metadata_vadc);
  ASSERT_EQ(1lu, metadata_vadc->size());

  // Controller metadata.
  std::vector<uint8_t> metadata_blob_vadc = std::move(*(*metadata_vadc)[0].data());
  fit::result controller_metadata_vadc =
      fidl::Unpersist<fuchsia_hardware_adcimpl::Metadata>(metadata_blob_vadc);
  ASSERT_TRUE(controller_metadata_vadc.is_ok());
  ASSERT_TRUE(controller_metadata_vadc->channels());
  ASSERT_EQ(controller_metadata_vadc->channels()->size(), 1lu);

  ASSERT_TRUE(controller_metadata_vadc->channels()->at(0).idx());
  ASSERT_EQ(*controller_metadata_vadc->channels()->at(0).idx(), static_cast<uint32_t>(ADC_CHAN1));
  ASSERT_TRUE(controller_metadata_vadc->channels()->at(0).name());
  EXPECT_EQ(strcmp(controller_metadata_vadc->channels()->at(0).name()->c_str(), ADC_CHAN1_NAME), 0);

  auto adc_nodes = adc_tester->GetPbusNodes("adc-ffffb000");
  ASSERT_EQ(1lu, adc_nodes.size());
  const auto& node_adc = adc_nodes[0];
  auto metadata_adc = node_adc.metadata();

  // Test metadata properties.
  ASSERT_TRUE(metadata_adc);
  ASSERT_EQ(1lu, metadata_adc->size());

  // Controller metadata.
  std::vector<uint8_t> metadata_blob_adc = std::move(*(*metadata_adc)[0].data());
  fit::result controller_metadata_adc =
      fidl::Unpersist<fuchsia_hardware_adcimpl::Metadata>(metadata_blob_adc);
  ASSERT_TRUE(controller_metadata_adc.is_ok());
  ASSERT_TRUE(controller_metadata_adc->channels());
  ASSERT_EQ(controller_metadata_adc->channels()->size(), 2lu);

  ASSERT_TRUE(controller_metadata_adc->channels()->at(0).idx());
  ASSERT_EQ(*controller_metadata_adc->channels()->at(0).idx(), static_cast<uint32_t>(ADC_CHAN2));
  ASSERT_TRUE(controller_metadata_adc->channels()->at(0).name());
  EXPECT_EQ(strcmp(controller_metadata_adc->channels()->at(0).name()->c_str(), ADC_CHAN2_NAME), 0);

  ASSERT_TRUE(controller_metadata_adc->channels()->at(1).idx());
  ASSERT_EQ(*controller_metadata_adc->channels()->at(1).idx(), static_cast<uint32_t>(ADC_CHAN3));
  ASSERT_TRUE(controller_metadata_adc->channels()->at(1).name());
  EXPECT_EQ(strcmp(controller_metadata_adc->channels()->at(1).name()->c_str(), ADC_CHAN3_NAME), 0);

  ASSERT_EQ(1lu, adc_tester->GetCompositeNodeSpecs("audio").size());
  auto mgr_request_audio = adc_tester->GetCompositeNodeSpecs("audio")[0];

  ASSERT_TRUE(mgr_request_audio.parents2().has_value());
  ASSERT_EQ(3lu, mgr_request_audio.parents2()->size());

  // 1st parent is pdev. Skipping that.
  // 2nd parent is ADC CHAN1.
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {{fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.adc.Service"),
        fdf::MakeProperty2(bind_fuchsia_adc::CHANNEL, static_cast<uint32_t>(ADC_CHAN1))}},
      (*mgr_request_audio.parents2())[1].properties(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {{fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.adc.Service"),
        fdf::MakeAcceptBindRule(bind_fuchsia_adc::CHANNEL, static_cast<uint32_t>(ADC_CHAN1))}},
      (*mgr_request_audio.parents2())[1].bind_rules(), false));

  // 3rd parent is ADC CHAN2.
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {{fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.adc.Service"),
        fdf::MakeProperty2(bind_fuchsia_adc::CHANNEL, static_cast<uint32_t>(ADC_CHAN2))}},
      (*mgr_request_audio.parents2())[2].properties(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {{fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.adc.Service"),
        fdf::MakeAcceptBindRule(bind_fuchsia_adc::CHANNEL, static_cast<uint32_t>(ADC_CHAN2))}},
      (*mgr_request_audio.parents2())[2].bind_rules(), false));

  ASSERT_EQ(1lu, adc_tester->GetCompositeNodeSpecs("video").size());
  auto mgr_request_video = adc_tester->GetCompositeNodeSpecs("video")[0];

  ASSERT_TRUE(mgr_request_video.parents2().has_value());
  ASSERT_EQ(2lu, mgr_request_video.parents2()->size());

  // 1st parent is pdev. Skipping that.
  // 2nd parent is ADC CHAN3.
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {{fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.adc.Service"),
        fdf::MakeProperty2(bind_fuchsia_adc::CHANNEL, static_cast<uint32_t>(ADC_CHAN3))}},
      (*mgr_request_video.parents2())[1].properties(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {{fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.adc.Service"),
        fdf::MakeAcceptBindRule(bind_fuchsia_adc::CHANNEL, static_cast<uint32_t>(ADC_CHAN3))}},
      (*mgr_request_video.parents2())[1].bind_rules(), false));
}

}  // namespace adc_dt

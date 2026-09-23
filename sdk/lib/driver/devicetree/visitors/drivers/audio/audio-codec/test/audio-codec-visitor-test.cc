// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "../audio-codec-visitor.h"

#include <lib/ddk/metadata.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/testing/visitor-test-helper.h>
#include <lib/driver/devicetree/visitors/default/bind-property/bind-property.h>
#include <lib/driver/devicetree/visitors/registry.h>

#include <bind/fuchsia/cpp/bind.h>
#include <gtest/gtest.h>

namespace audio_codec_visitor_dt {

class AudioCodecVisitorTester
    : public fdf_devicetree::testing::VisitorTestHelper<AudioCodecVisitor> {
 public:
  AudioCodecVisitorTester(std::string_view dtb_path)
      : fdf_devicetree::testing::VisitorTestHelper<AudioCodecVisitor>(dtb_path,
                                                                      "AudioCodecVisitorTest") {}
};

TEST(AudioCodecVisitorTest, TestLinkingAndInstanceId) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());

  auto tester = std::make_unique<AudioCodecVisitorTester>("/pkg/test-data/audio-codec.dtb");
  AudioCodecVisitorTester* audio_visitor_tester = tester.get();
  ASSERT_TRUE(visitors.RegisterVisitor(std::move(tester)).is_ok());

  ASSERT_EQ(ZX_OK, audio_visitor_tester->manager()->Walk(visitors).status_value());
  ASSERT_TRUE(audio_visitor_tester->DoPublish().is_ok());

  // Verify codecs have instance ID metadata.
  auto codec_nodes = audio_visitor_tester->GetPbusNodes("audio-codec");
  ASSERT_EQ(2lu, codec_nodes.size());

  // First codec node metadata check.
  {
    auto& node = codec_nodes[0];
    ASSERT_TRUE(node.metadata().has_value());
    ASSERT_EQ(1lu, node.metadata()->size());
    EXPECT_EQ(std::to_string(DEVICE_METADATA_PRIVATE), (*node.metadata())[0].id());

    uint32_t val = 0;
    std::memcpy(&val, (*node.metadata())[0].data()->data(), sizeof(val));
    EXPECT_EQ(1u, val);
  }

  // Second codec node metadata check.
  {
    auto& node = codec_nodes[1];
    ASSERT_TRUE(node.metadata().has_value());
    ASSERT_EQ(1lu, node.metadata()->size());
    EXPECT_EQ(std::to_string(DEVICE_METADATA_PRIVATE), (*node.metadata())[0].id());

    uint32_t val = 0;
    std::memcpy(&val, (*node.metadata())[0].data()->data(), sizeof(val));
    EXPECT_EQ(2u, val);
  }

  // Verify audio controller has linked codec parent specs.
  auto composite_specs = audio_visitor_tester->GetCompositeNodeSpecs("audio-ff");
  ASSERT_EQ(1lu, composite_specs.size());
  auto composite_spec = composite_specs[0];
  ASSERT_TRUE(composite_spec.parents2().has_value());
  // Expected parents: pdev (1st parent) + 2 codecs = 3 parents total.
  ASSERT_EQ(3lu, composite_spec.parents2()->size());

  // Check 1st codec parent spec bind rules.
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {{
          fdf::MakeAcceptBindRule(bind_fuchsia::ID, 1u),
          fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.audio.CodecService"),
      }},
      (*composite_spec.parents2())[1].bind_rules(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {{
          fdf::MakeProperty2(bind_fuchsia::ID, 1u),
          fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.audio.CodecService"),
      }},
      (*composite_spec.parents2())[1].properties(), false));

  // Check 2nd codec parent spec bind rules.
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {{
          fdf::MakeAcceptBindRule(bind_fuchsia::ID, 2u),
          fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.audio.CodecService"),
      }},
      (*composite_spec.parents2())[2].bind_rules(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {{
          fdf::MakeProperty2(bind_fuchsia::ID, 2u),
          fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.audio.CodecService"),
      }},
      (*composite_spec.parents2())[2].properties(), false));
}

}  // namespace audio_codec_visitor_dt

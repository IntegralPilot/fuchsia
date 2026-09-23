// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/fpromise/result.h>
#include <lib/fpromise/single_threaded_executor.h>
#include <lib/inspect/cpp/inspector.h>
#include <lib/inspect/cpp/reader.h>

#include <gtest/gtest.h>

#include "src/graphics/display/drivers/intel-display/dp-display.h"
#include "src/graphics/display/drivers/intel-display/fake-dp-aux-channel.h"

namespace intel_display {

namespace {

using testing::FakeDpAuxChannel;
using testing::kDefaultLaneCount;
using testing::kMaxLinkRateTableEntries;

class DpCapabilitiesTest : public ::testing::Test {
 private:
  fdf_testing::ScopedGlobalLogger logger_;
};

TEST_F(DpCapabilitiesTest, NoSupportedLinkRates) {
  FakeDpAuxChannel fake_dp_aux_channel;

  const fpromise::result<DpCapabilities> cap = DpCapabilities::Read(&fake_dp_aux_channel);
  EXPECT_TRUE(cap.is_error());
}

// Tests that invalid lane counts are rejected.
TEST_F(DpCapabilitiesTest, InvalidMaxLaneCount) {
  FakeDpAuxChannel fake_dp_aux_channel;
  fake_dp_aux_channel.SetMaxLinkRate(dpcd::LinkBw::k1620Mbps);

  fake_dp_aux_channel.SetMaxLaneCount(0);
  EXPECT_TRUE(DpCapabilities::Read(&fake_dp_aux_channel).is_error());

  fake_dp_aux_channel.SetMaxLaneCount(3);
  EXPECT_TRUE(DpCapabilities::Read(&fake_dp_aux_channel).is_error());

  fake_dp_aux_channel.SetMaxLaneCount(5);
  EXPECT_TRUE(DpCapabilities::Read(&fake_dp_aux_channel).is_error());
}

// Tests that the basic set of getters work for non-EDP.
TEST_F(DpCapabilitiesTest, BasicFields) {
  FakeDpAuxChannel fake_dp_aux_channel;

  fake_dp_aux_channel.SetDpcdRevision(dpcd::Revision::k1_4);
  fake_dp_aux_channel.SetMaxLaneCount(kDefaultLaneCount);
  fake_dp_aux_channel.SetMaxLinkRate(dpcd::LinkBw::k1620Mbps);
  fake_dp_aux_channel.SetSinkCount(1);

  const fpromise::result<DpCapabilities> cap = DpCapabilities::Read(&fake_dp_aux_channel);
  ASSERT_TRUE(cap.is_ok());
  EXPECT_EQ(dpcd::Revision::k1_4, cap.value().dpcd_revision());
  EXPECT_EQ(kDefaultLaneCount, cap.value().max_lane_count());
  EXPECT_EQ(1u, cap.value().sink_count());
  EXPECT_EQ(1u, cap.value().supported_link_rates_mbps().size());

  // eDP capabilities should be unavailable.
  EXPECT_EQ(std::nullopt, cap.value().edp_revision());
  EXPECT_FALSE(cap.value().backlight_aux_power());
  EXPECT_FALSE(cap.value().backlight_aux_brightness());
}

// Tests that eDP registers are processed when supported.
TEST_F(DpCapabilitiesTest, EdpRegisters) {
  FakeDpAuxChannel fake_dp_aux_channel;
  fake_dp_aux_channel.SetDefaults();
  fake_dp_aux_channel.SetEdpCapable(dpcd::EdpRevision::k1_2);

  const fpromise::result<DpCapabilities> cap = DpCapabilities::Read(&fake_dp_aux_channel);
  ASSERT_TRUE(cap.is_ok());
  EXPECT_EQ(std::optional{dpcd::EdpRevision::k1_2}, cap.value().edp_revision());
  EXPECT_FALSE(cap.value().backlight_aux_power());
  EXPECT_FALSE(cap.value().backlight_aux_brightness());
}

TEST_F(DpCapabilitiesTest, EdpBacklight) {
  FakeDpAuxChannel fake_dp_aux_channel;
  fake_dp_aux_channel.SetDefaults();
  fake_dp_aux_channel.SetEdpCapable(dpcd::EdpRevision::k1_2);

  dpcd::EdpGeneralCap1 gc;
  gc.set_tcon_backlight_adjustment_cap(1);
  gc.set_backlight_aux_enable_cap(1);
  fake_dp_aux_channel.registers[dpcd::DPCD_EDP_GENERAL_CAP1] = gc.reg_value();

  dpcd::EdpBacklightCap bc;
  bc.set_brightness_aux_set_cap(1);
  fake_dp_aux_channel.registers[dpcd::DPCD_EDP_BACKLIGHT_CAP] = bc.reg_value();

  const fpromise::result<DpCapabilities> cap = DpCapabilities::Read(&fake_dp_aux_channel);
  ASSERT_TRUE(cap.is_ok());
  EXPECT_TRUE(cap.value().backlight_aux_power());
  EXPECT_TRUE(cap.value().backlight_aux_brightness());
}

// Tests that the list of supported link rates is populated correctly using the "Max Link Rate"
// method.
TEST_F(DpCapabilitiesTest, MaxLinkRate1620NoEdp) {
  FakeDpAuxChannel fake_dp_aux_channel;
  fake_dp_aux_channel.SetDefaults();
  fake_dp_aux_channel.SetMaxLinkRate(dpcd::LinkBw::k1620Mbps);

  const fpromise::result<DpCapabilities> cap = DpCapabilities::Read(&fake_dp_aux_channel);
  ASSERT_TRUE(cap.is_ok());
  EXPECT_FALSE(cap.value().use_link_rate_table());
  ASSERT_EQ(1u, cap.value().supported_link_rates_mbps().size());
  EXPECT_EQ(1620u, cap.value().supported_link_rates_mbps()[0]);
}

// Tests that the list of supported link rates is populated correctly using the "Max Link Rate"
// method.
TEST_F(DpCapabilitiesTest, MaxLinkRate2700NoEdp) {
  FakeDpAuxChannel fake_dp_aux_channel;
  fake_dp_aux_channel.SetDefaults();
  fake_dp_aux_channel.SetMaxLinkRate(dpcd::LinkBw::k2700Mbps);

  const fpromise::result<DpCapabilities> cap = DpCapabilities::Read(&fake_dp_aux_channel);
  ASSERT_TRUE(cap.is_ok());
  EXPECT_FALSE(cap.value().use_link_rate_table());
  ASSERT_EQ(2u, cap.value().supported_link_rates_mbps().size());
  EXPECT_EQ(1620u, cap.value().supported_link_rates_mbps()[0]);
  EXPECT_EQ(2700u, cap.value().supported_link_rates_mbps()[1]);
}

// Tests that the list of supported link rates is populated correctly using the "Max Link Rate"
// method.
TEST_F(DpCapabilitiesTest, MaxLinkRate5400NoEdp) {
  FakeDpAuxChannel fake_dp_aux_channel;
  fake_dp_aux_channel.SetDefaults();
  fake_dp_aux_channel.SetMaxLinkRate(dpcd::LinkBw::k5400Mbps);

  const fpromise::result<DpCapabilities> cap = DpCapabilities::Read(&fake_dp_aux_channel);
  ASSERT_TRUE(cap.is_ok());
  EXPECT_FALSE(cap.value().use_link_rate_table());
  ASSERT_EQ(3u, cap.value().supported_link_rates_mbps().size());
  EXPECT_EQ(1620u, cap.value().supported_link_rates_mbps()[0]);
  EXPECT_EQ(2700u, cap.value().supported_link_rates_mbps()[1]);
  EXPECT_EQ(5400u, cap.value().supported_link_rates_mbps()[2]);
}

// Tests that the list of supported link rates is populated correctly using the "Max Link Rate"
// method.
TEST_F(DpCapabilitiesTest, MaxLinkRate8100NoEdp) {
  FakeDpAuxChannel fake_dp_aux_channel;
  fake_dp_aux_channel.SetDefaults();
  fake_dp_aux_channel.SetMaxLinkRate(dpcd::LinkBw::k8100Mbps);

  const fpromise::result<DpCapabilities> cap = DpCapabilities::Read(&fake_dp_aux_channel);
  ASSERT_TRUE(cap.is_ok());
  EXPECT_FALSE(cap.value().use_link_rate_table());
  ASSERT_EQ(4u, cap.value().supported_link_rates_mbps().size());
  EXPECT_EQ(1620u, cap.value().supported_link_rates_mbps()[0]);
  EXPECT_EQ(2700u, cap.value().supported_link_rates_mbps()[1]);
  EXPECT_EQ(5400u, cap.value().supported_link_rates_mbps()[2]);
  EXPECT_EQ(8100u, cap.value().supported_link_rates_mbps()[3]);
}

// Tests that link rate discovery falls back to MAX_LINK_RATE if eDP v1.4 is supported but the
// link rate table is empty.
TEST_F(DpCapabilitiesTest, FallbackToMaxLinkRateWhenLinkRateTableIsEmpty) {
  FakeDpAuxChannel fake_dp_aux_channel;
  fake_dp_aux_channel.SetDefaults();
  fake_dp_aux_channel.SetEdpCapable(dpcd::EdpRevision::k1_4);
  fake_dp_aux_channel.SetMaxLinkRate(dpcd::LinkBw::k1620Mbps);

  const fpromise::result<DpCapabilities> cap = DpCapabilities::Read(&fake_dp_aux_channel);
  ASSERT_TRUE(cap.is_ok());
  EXPECT_FALSE(cap.value().use_link_rate_table());
  EXPECT_FALSE(cap.value().supported_link_rates_mbps().empty());
}

// Tests that the list of supported link rates is populated correctly when using the "Link Rate
// Table" method.
TEST_F(DpCapabilitiesTest, LinkRateTableOneEntry) {
  FakeDpAuxChannel fake_dp_aux_channel;
  fake_dp_aux_channel.SetDefaults();
  fake_dp_aux_channel.SetEdpCapable(dpcd::EdpRevision::k1_4);
  fake_dp_aux_channel.SetMaxLinkRate(0);  // Not supported

  fake_dp_aux_channel.PopulateLinkRateTable({100});  // 100 * 200kHz ==> 20MHz

  const fpromise::result<DpCapabilities> cap = DpCapabilities::Read(&fake_dp_aux_channel);
  ASSERT_TRUE(cap.is_ok());
  EXPECT_TRUE(cap.value().use_link_rate_table());
  EXPECT_EQ(1u, cap.value().supported_link_rates_mbps().size());
  EXPECT_EQ(20u, cap.value().supported_link_rates_mbps()[0]);
}

// Tests that the list of supported link rates is populated correctly when using the "Link Rate
// Table" method.
TEST_F(DpCapabilitiesTest, LinkRateTableSomeEntries) {
  FakeDpAuxChannel fake_dp_aux_channel;
  fake_dp_aux_channel.SetDefaults();
  fake_dp_aux_channel.SetEdpCapable(dpcd::EdpRevision::k1_4);
  fake_dp_aux_channel.SetMaxLinkRate(0);  // Not supported

  // 100 * 200kHz ==> 20MHz
  // 200 * 200kHz ==> 40MHz
  // 300 * 200kHz ==> 60MHz
  fake_dp_aux_channel.PopulateLinkRateTable({100, 200, 300});

  const fpromise::result<DpCapabilities> cap = DpCapabilities::Read(&fake_dp_aux_channel);
  ASSERT_TRUE(cap.is_ok());
  EXPECT_TRUE(cap.value().use_link_rate_table());
  EXPECT_EQ(3u, cap.value().supported_link_rates_mbps().size());
  EXPECT_EQ(std::vector<uint32_t>({20, 40, 60}), cap.value().supported_link_rates_mbps());
}

// Tests that the list of supported link rates is populated correctly when using the "Link Rate
// Table" method.
TEST_F(DpCapabilitiesTest, LinkRateTableMaxEntries) {
  FakeDpAuxChannel fake_dp_aux_channel;
  fake_dp_aux_channel.SetDefaults();
  fake_dp_aux_channel.SetEdpCapable(dpcd::EdpRevision::k1_4);
  fake_dp_aux_channel.SetMaxLinkRate(0);  // Not supported

  // Link rate table entries are stored in units of 200kHz (or kbps). The DpCapabilities data
  // structure stores them in units of Mbps. 1 Mbps = 5 * 200kbps.
  constexpr uint16_t kConversionFactor = 5;
  std::vector<uint16_t> input;
  std::vector<uint32_t> output;
  for (unsigned i = 1; i <= kMaxLinkRateTableEntries; i++) {
    input.push_back(static_cast<uint16_t>(kConversionFactor * i));
    output.push_back(i);
  }
  fake_dp_aux_channel.PopulateLinkRateTable(std::move(input));

  const fpromise::result<DpCapabilities> cap = DpCapabilities::Read(&fake_dp_aux_channel);
  ASSERT_TRUE(cap.is_ok());
  EXPECT_TRUE(cap.value().use_link_rate_table());
  EXPECT_EQ(kMaxLinkRateTableEntries, cap.value().supported_link_rates_mbps().size());
  EXPECT_EQ(output, cap.value().supported_link_rates_mbps());
}

// Tests that the list of supported link rates is populated based on the "Link Rate Table" method
// when both the table and the MAX_LINK_RATE register hold valid values (which is optional but
// allowed by the eDP specification).
TEST_F(DpCapabilitiesTest, LinkRateTableUsedWhenMaxLinkRateIsAlsoPresent) {
  FakeDpAuxChannel fake_dp_aux_channel;
  fake_dp_aux_channel.SetDefaults();
  fake_dp_aux_channel.SetEdpCapable(dpcd::EdpRevision::k1_4);
  fake_dp_aux_channel.SetMaxLinkRate(dpcd::LinkBw::k2700Mbps);

  // Link rate table entries are stored in units of 200kHz (or kbps). The DpCapabilities data
  // structure stores them in units of Mbps. 1 Mbps = 5 * 200kbps.
  constexpr uint16_t kConversionFactor = 5;
  constexpr uint32_t kExpectedLinkRate = 5400;
  fake_dp_aux_channel.PopulateLinkRateTable({kExpectedLinkRate * kConversionFactor});

  const fpromise::result<DpCapabilities> cap = DpCapabilities::Read(&fake_dp_aux_channel);
  ASSERT_TRUE(cap.is_ok());
  EXPECT_TRUE(cap.value().use_link_rate_table());
  EXPECT_EQ(1u, cap.value().supported_link_rates_mbps().size());
  EXPECT_EQ(kExpectedLinkRate, cap.value().supported_link_rates_mbps()[0]);
}

// Tests that the DP capabilities can be inspected and the DP capability values
// are correctly propagated to the inspect node.
TEST_F(DpCapabilitiesTest, Inspect) {
  FakeDpAuxChannel fake_dp_aux_channel;
  fake_dp_aux_channel.SetDefaults();
  fake_dp_aux_channel.SetMaxLinkRate(dpcd::LinkBw::k2700Mbps);

  const fpromise::result<DpCapabilities> cap = DpCapabilities::Read(&fake_dp_aux_channel);
  ASSERT_TRUE(cap.is_ok());

  inspect::Inspector inspector;
  cap.value().PublishToInspect(&inspector.GetRoot());

  fpromise::single_threaded_executor executor;
  executor.schedule_task(
      inspect::ReadFromInspector(inspector).then([&](fpromise::result<inspect::Hierarchy>& result) {
        ASSERT_TRUE(result.is_ok());
        auto& node = result.value().node();

        auto dpcd_revision = node.get_property<inspect::StringPropertyValue>("dpcd_revision");
        ASSERT_TRUE(dpcd_revision);
        EXPECT_STREQ("DPCD r1.4", dpcd_revision->value().c_str());

        auto edp_revision = node.get_property<inspect::StringPropertyValue>("edp_revision");
        ASSERT_TRUE(edp_revision);
        EXPECT_STREQ("not supported", edp_revision->value().c_str());

        auto sink_count = node.get_property<inspect::UintPropertyValue>("sink_count");
        ASSERT_TRUE(sink_count);
        EXPECT_EQ(testing::kDefaultSinkCount, sink_count->value());

        auto max_lane_count = node.get_property<inspect::UintPropertyValue>("max_lane_count");
        ASSERT_TRUE(max_lane_count);
        EXPECT_EQ(testing::kDefaultLaneCount, max_lane_count->value());

        auto supported_link_rates_list =
            node.get_property<inspect::UintArrayValue>("supported_link_rates_mbps_per_lane");
        ASSERT_TRUE(supported_link_rates_list);
        ASSERT_EQ(2u, supported_link_rates_list->value().size());
        EXPECT_EQ(1620u, supported_link_rates_list->value()[0]);
        EXPECT_EQ(2700u, supported_link_rates_list->value()[1]);
      }));
  executor.run();
}

}  // namespace

}  // namespace intel_display

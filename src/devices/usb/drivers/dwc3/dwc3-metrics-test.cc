// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/usb/drivers/dwc3/dwc3-metrics.h"

#include <lib/inspect/cpp/hierarchy.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/inspect/cpp/reader.h>

#include <gtest/gtest.h>

namespace dwc3 {

TEST(Dwc3MetricsTest, InitResetsAndRecordsCorrectly) {
  Dwc3Metrics metrics;
  metrics.Init();

  // Increment some events
  metrics.IncrementEventCount(static_cast<uint32_t>(MetricEventType::kDevtDisconnect));
  metrics.IncrementEventCount(static_cast<uint32_t>(MetricEventType::kDevtDisconnect));
  metrics.IncrementEventCount(static_cast<uint32_t>(MetricEventType::kDevtUsbReset));

  // Call RecordMetrics (without mmio and dwc3 pointers)
  inspect::Inspector inspector = metrics.RecordMetrics(nullptr, nullptr);

  // Read hierarchy
  auto hierarchy = inspect::ReadFromVmo(inspector.DuplicateVmo()).take_value();

  // 1. Check root level properties
  const auto* time_start = hierarchy.node().get_property<inspect::UintPropertyValue>("time_start");
  ASSERT_NE(time_start, nullptr);
  EXPECT_GT(time_start->value(), 0u);

  const auto* time_stats = hierarchy.node().get_property<inspect::UintPropertyValue>("time_stats");
  ASSERT_NE(time_stats, nullptr);
  EXPECT_GE(time_stats->value(), time_start->value());

  // 2. Check event_counts node
  const auto* event_counts = hierarchy.GetByPath({"event_counts"});
  ASSERT_NE(event_counts, nullptr);

  // Verify disconnect count is 2
  const auto* disconnect =
      event_counts->node().get_property<inspect::UintPropertyValue>("DEVT_DISCONNECT");
  ASSERT_NE(disconnect, nullptr);
  EXPECT_EQ(disconnect->value(), 2u);

  // Verify usb reset count is 1
  const auto* usb_reset =
      event_counts->node().get_property<inspect::UintPropertyValue>("DEVT_USB_RESET");
  ASSERT_NE(usb_reset, nullptr);
  EXPECT_EQ(usb_reset->value(), 1u);

  // Verify other event types are initialized to 0
  const auto* sof = event_counts->node().get_property<inspect::UintPropertyValue>("DEVT_SOF");
  ASSERT_NE(sof, nullptr);
  EXPECT_EQ(sof->value(), 0u);
}

// Verifies that recording Inspect metrics when the driver pointer is null safely populates
// software-only metrics (e.g. timestamps and event counters) from memory without attempting
// any hardware MMIO accesses or creating hardware register inspect nodes.
TEST(Dwc3MetricsTest, NullControllerSkipsMmioSampling) {
  Dwc3Metrics metrics;
  metrics.Init();

  // Passing null driver and mmio pointers must not crash or read MMIO.
  inspect::Inspector inspector = metrics.RecordMetrics(nullptr, nullptr);
  auto hierarchy = inspect::ReadFromVmo(inspector.DuplicateVmo()).take_value();

  // Root properties and software event counts should be present.
  EXPECT_NE(nullptr, hierarchy.node().get_property<inspect::UintPropertyValue>("time_start"));
  EXPECT_NE(nullptr, hierarchy.GetByPath({"event_counts"}));

  const auto* hw_state =
      hierarchy.node().get_property<inspect::StringPropertyValue>("hardware_state");
  ASSERT_NE(nullptr, hw_state);
  EXPECT_EQ("powered_off", hw_state->value());

  // Hardware register nodes must not exist when controller is null.
  EXPECT_EQ(nullptr, hierarchy.GetByPath({"GCTL"}));
  EXPECT_EQ(nullptr, hierarchy.GetByPath({"GSTS"}));
  EXPECT_EQ(nullptr, hierarchy.GetByPath({"DCFG"}));
  EXPECT_EQ(nullptr, hierarchy.GetByPath({"DCTL"}));
  EXPECT_EQ(nullptr, hierarchy.GetByPath({"DSTS"}));
}

}  // namespace dwc3

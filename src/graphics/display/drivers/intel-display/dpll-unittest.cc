// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/intel-display/dpll.h"

#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/driver/mock-mmio/cpp/globally-ordered-region.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>

#include <cstdint>

#include <gtest/gtest.h>

#include "src/graphics/display/lib/driver-utils/scoped-value-change.h"

namespace intel_display {

namespace {

constexpr uint32_t kDpll0EnableOffset = 0x46010;
constexpr uint32_t kDisplayStrapsOffset = 0x51004;
constexpr uint32_t kDpll0DcoFrequencyOffset = 0x164284;
constexpr uint32_t kDpll0DcoDividersOffset = 0x164288;

class DisplayPllTigerLakeTest : public ::testing::Test {
 public:
  DisplayPllTigerLakeTest()
      : lock_wait_timeout_change_(
            DisplayPllTigerLake::OverrideLockWaitTimeoutUsForTesting(kLargeTimeout)),
        power_on_wait_timeout_change_(
            DisplayPllTigerLake::OverridePowerOnWaitTimeoutMsForTesting(kLargeTimeout)) {}
  ~DisplayPllTigerLakeTest() override = default;

  void SetUp() override {}
  void TearDown() override { mmio_range_.CheckAllAccessesReplayed(); }

 protected:
  // Ensures that the tests don't time out due to external factors (such as
  // being pre-empted by the scheduler) while replaying MMIO access lists.
  static constexpr int kLargeTimeout = 1'000'000'000;

  static constexpr int kMmioRangeSize = 0x140000;

  fdf_testing::ScopedGlobalLogger logger_;
  mock_mmio::GloballyOrderedRegion mmio_range_{kMmioRangeSize,
                                               mock_mmio::GloballyOrderedRegion::Size::k32};
  fdf::MmioBuffer mmio_buffer_{mmio_range_.GetMmioBuffer()};

  display::ScopedValueChange<int> lock_wait_timeout_change_;
  display::ScopedValueChange<int> power_on_wait_timeout_change_;
};

TEST_F(DisplayPllTigerLakeTest, EnableHdmi) {
  mmio_range_.Expect(mock_mmio::GloballyOrderedRegion::AccessList({
      {.address = kDpll0EnableOffset, .value = 0x00000000},
      {.address = kDpll0EnableOffset, .value = 0x08000000, .write = true},
      // Should wait until the powered on bit is set.
      {.address = kDpll0EnableOffset, .value = 0x08000000},
      {.address = kDpll0EnableOffset, .value = 0x0c000000},

      // Reference clock 38.4 MHz
      {.address = kDisplayStrapsOffset, .value = 0x40000000},
      // DCO frequency 8,910,000 kHz
      {.address = kDpll0DcoFrequencyOffset, .value = 0x001001d0, .write = true},
      // DCO divider: P = 2, Q = 3, K = 2
      {.address = kDpll0DcoDividersOffset, .value = 0x00003e84, .write = true},
      {.address = kDpll0DcoDividersOffset, .value = 0x00003e84},

      {.address = kDpll0EnableOffset, .value = 0x8c000000, .write = true},
      // Should wait until the locked bit is set.
      {.address = kDpll0EnableOffset, .value = 0x8c000000},
      {.address = kDpll0EnableOffset, .value = 0xcc000000},
  }));

  // Standard signal rate for 1920 x 1080 at 60 fps, 8 bpc, from
  // VESA Display Monitor Timing Standard, Version 1.0, Rev. 12.
  constexpr int kDdiClockKhz = 148'500;
  constexpr DdiPllConfig kDdiPllConfig = {
      .ddi_clock_khz = kDdiClockKhz,
      .spread_spectrum_clocking = false,
      .admits_display_port = false,
      .admits_hdmi = true,
  };

  DisplayPllTigerLake dpll(&mmio_buffer_, PllId::DPLL_0);
  ASSERT_TRUE(dpll.Enable(kDdiPllConfig));
}

TEST_F(DisplayPllTigerLakeTest, EnableDisplayPort) {
  mmio_range_.Expect(mock_mmio::GloballyOrderedRegion::AccessList({
      {.address = kDpll0EnableOffset, .value = 0x00000000},
      {.address = kDpll0EnableOffset, .value = 0x08000000, .write = true},
      // Should wait until the powered on bit is set.
      {.address = kDpll0EnableOffset, .value = 0x08000000},
      {.address = kDpll0EnableOffset, .value = 0x0c000000},

      // Reference clock 38.4 MHz
      {.address = kDisplayStrapsOffset, .value = 0x40000000},
      // DCO frequency 8,100,000 kHz
      {.address = kDpll0DcoFrequencyOffset, .value = 0x00E001A5, .write = true},
      // DCO divider: P = 3, Q = 1, K = 2
      {.address = kDpll0DcoDividersOffset, .value = 0x00000488, .write = true},
      {.address = kDpll0DcoDividersOffset, .value = 0x00000488},

      {.address = kDpll0EnableOffset, .value = 0x8c000000, .write = true},
      // Should wait until the locked bit is set.
      {.address = kDpll0EnableOffset, .value = 0x8c000000},
      {.address = kDpll0EnableOffset, .value = 0xcc000000},
  }));

  // DDI AFE Clock rate for DisplayPort with link bit rate 2.7 GHz.
  constexpr int kDdiClockKhz = 1'350'000;
  constexpr DdiPllConfig kDdiPllConfig = {
      .ddi_clock_khz = kDdiClockKhz,
      .spread_spectrum_clocking = false,
      .admits_display_port = true,
      .admits_hdmi = false,
  };

  DisplayPllTigerLake dpll(&mmio_buffer_, PllId::DPLL_0);
  ASSERT_TRUE(dpll.Enable(kDdiPllConfig));
}

}  // namespace

}  // namespace intel_display

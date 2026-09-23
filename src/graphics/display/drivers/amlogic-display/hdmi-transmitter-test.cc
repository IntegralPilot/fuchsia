// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/amlogic-display/hdmi-transmitter.h"

#include <lib/driver/mock-mmio/cpp/globally-ordered-region.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/zx/result.h>

#include <algorithm>
#include <iterator>
#include <list>
#include <memory>

#include <gtest/gtest.h>

#include "src/graphics/display/lib/api-types/cpp/display-timing.h"
#include "src/graphics/display/lib/designware-hdmi/hdmi-transmitter-controller.h"
#include "src/lib/testing/predicates/status.h"

namespace amlogic_display {

enum class HdmiTransmitterControllerCall : uint8_t {
  kConfigHdmitx,
  kSetupInterrupts,
  kReset,
  kSetupScdc,
  kResetFc,
  kSetFcScramblerCtrl,
};

// TODO(https://fxbug.dev/42085847): Consider replacing the mock class with a fake
// HdmiTransmitterController implementation.
class MockHdmiTransmitterController : public designware_hdmi::HdmiTransmitterController {
 public:
  MockHdmiTransmitterController() = default;
  ~MockHdmiTransmitterController() { EXPECT_TRUE(expected_calls_.empty()); }

  zx_status_t InitHw() override { return ZX_OK; }

  zx::result<fbl::Vector<uint8_t>> ReadExtendedEdid() override {
    return zx::ok(fbl::Vector<uint8_t>{});
  }

  void ConfigHdmitx(const designware_hdmi::ColorParam& color_param,
                    const display::DisplayTiming& mode,
                    const designware_hdmi::hdmi_param_tx& p) override {
    OnHdmiTransmitterControllerCall(HdmiTransmitterControllerCall::kConfigHdmitx);
  }
  void SetupInterrupts() override {
    OnHdmiTransmitterControllerCall(HdmiTransmitterControllerCall::kSetupInterrupts);
  }
  void Reset() override { OnHdmiTransmitterControllerCall(HdmiTransmitterControllerCall::kReset); }
  void SetupScdc(bool is4k) override {
    OnHdmiTransmitterControllerCall(HdmiTransmitterControllerCall::kSetupScdc);
  }
  void ResetFc() override {
    OnHdmiTransmitterControllerCall(HdmiTransmitterControllerCall::kResetFc);
  }
  void SetFcScramblerCtrl(bool is4k) override {
    OnHdmiTransmitterControllerCall(HdmiTransmitterControllerCall::kSetFcScramblerCtrl);
  }

  void OnHdmiTransmitterControllerCall(HdmiTransmitterControllerCall call) {
    ASSERT_FALSE(expected_calls_.empty());
    EXPECT_EQ(expected_calls_.front(), call);
    expected_calls_.pop_front();
  }

  void ExpectCalls(const std::span<const HdmiTransmitterControllerCall> expected_calls) {
    std::copy(expected_calls.begin(), expected_calls.end(), std::back_inserter(expected_calls_));
  }

  void PrintRegisters() override {}

 private:
  std::list<HdmiTransmitterControllerCall> expected_calls_;
};

class HdmiTransmitterTest : public testing::Test {
 public:
  void SetUp() override {
    std::unique_ptr<MockHdmiTransmitterController> mock_hdmitx_controller =
        std::make_unique<MockHdmiTransmitterController>();
    mock_hdmitx_controller_ = mock_hdmitx_controller.get();

    // TODO(https://fxbug.dev/42074342): Use a fake SMC resource, when the
    // implementation lands.
    dut_ = std::make_unique<HdmiTransmitter>(std::move(mock_hdmitx_controller),
                                             top_level_mmio_range_.GetMmioBuffer(),
                                             /*smc=*/zx::resource{});
    ASSERT_TRUE(dut_);
  }

  void TearDown() override { top_level_mmio_range_.CheckAllAccessesReplayed(); }

 protected:
  fdf_testing::ScopedGlobalLogger logger_;

  static constexpr int kTopLevelMmioRangeSize = 0x8000;
  mock_mmio::GloballyOrderedRegion top_level_mmio_range_{
      kTopLevelMmioRangeSize, mock_mmio::GloballyOrderedRegion::Size::k32};

  std::unique_ptr<HdmiTransmitter> dut_;

  // Owned by `dut_`.
  MockHdmiTransmitterController* mock_hdmitx_controller_ = nullptr;
};

// Register addresses from the A311D datasheet section 10.2.3.44 "HDCP2.2 IP
// Register Access".
constexpr int kHdmiTxTopSwResetOffset = 0x00 * 4;
constexpr int kHdmiTxTopClkCntlOffset = 0x01 * 4;
constexpr int kHdmiTxTopIntrMaskn = 0x03 * 4;
constexpr int kHdmiTxTopIntrStatClr = 0x05 * 4;
constexpr int kHdmiTxTopBistCntl = 0x06 * 4;
constexpr int kHdmiTxTopTmdsClkPttn01 = 0x0a * 4;
constexpr int kHdmiTxTopTmdsClkPttn23 = 0x0b * 4;
constexpr int kHdmiTxTopTmdsClkPttnCntl = 0x0c * 4;

TEST_F(HdmiTransmitterTest, ResetTest) {
  top_level_mmio_range_.Expect(mock_mmio::GloballyOrderedRegion::AccessList({
      {.address = kHdmiTxTopSwResetOffset, .value = 0, .write = true},
      {.address = kHdmiTxTopClkCntlOffset, .value = 0xff, .write = true},
  }));
  zx::result<> result = dut_->Reset();
  EXPECT_OK(result);
}

TEST_F(HdmiTransmitterTest, ModeSetTest) {
  // TODO(https://fxbug.dev/42085738): Use valid synthetic values for timings.
  display::DisplayTiming display_timing = {
      .horizontal_active_px = 0,
      .horizontal_front_porch_px = 0,
      .horizontal_sync_width_px = 0,
      .horizontal_back_porch_px = 0,
      .vertical_active_lines = 0,
      .vertical_front_porch_lines = 0,
      .vertical_sync_width_lines = 0,
      .vertical_back_porch_lines = 0,
      .pixel_clock_frequency_hz = 0,
      .fields_per_frame = display::FieldsPerFrame::kProgressive,
      .hsync_polarity = display::SyncPolarity::kNegative,
      .vsync_polarity = display::SyncPolarity::kNegative,
      .vblank_alternates = false,
      .pixel_repetition = 0,
  };
  designware_hdmi::ColorParam color_param = {
      .input_color_format = designware_hdmi::ColorFormat::kCfRgb,
      .output_color_format = designware_hdmi::ColorFormat::kCfRgb,
      .color_depth = designware_hdmi::ColorDepth::kCd24B,
  };

  top_level_mmio_range_.Expect(mock_mmio::GloballyOrderedRegion::AccessList({
      {.address = kHdmiTxTopBistCntl, .value = 1 << 12, .write = true},
      {.address = kHdmiTxTopIntrStatClr, .value = 0x1f, .write = true},
      {.address = kHdmiTxTopIntrMaskn, .value = 0x9f, .write = true},
      {.address = kHdmiTxTopTmdsClkPttn01, .value = 0x001f'001f, .write = true},
      {.address = kHdmiTxTopTmdsClkPttn23, .value = 0x001f'001f, .write = true},
      {.address = kHdmiTxTopTmdsClkPttnCntl, .value = 0x01, .write = true},
      {.address = kHdmiTxTopTmdsClkPttnCntl, .value = 0x02, .write = true},
  }));

  static constexpr HdmiTransmitterControllerCall kExpectedCalls[] = {
      HdmiTransmitterControllerCall::kConfigHdmitx,
      HdmiTransmitterControllerCall::kSetupInterrupts,
      HdmiTransmitterControllerCall::kReset,
      HdmiTransmitterControllerCall::kSetFcScramblerCtrl,
      HdmiTransmitterControllerCall::kSetupScdc,
      HdmiTransmitterControllerCall::kResetFc,
  };
  mock_hdmitx_controller_->ExpectCalls(kExpectedCalls);

  zx::result<> result = dut_->ModeSet(display_timing, color_param);
  EXPECT_OK(result);
}

}  // namespace amlogic_display

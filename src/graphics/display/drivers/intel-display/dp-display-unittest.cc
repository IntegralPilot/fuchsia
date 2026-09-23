// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/intel-display/dp-display.h"

#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/mmio-ptr/fake.h>

#include <cstdint>
#include <memory>
#include <optional>

#include <gtest/gtest.h>

#include "src/graphics/display/drivers/intel-display/ddi-physical-layer-manager.h"
#include "src/graphics/display/drivers/intel-display/dpll.h"
#include "src/graphics/display/drivers/intel-display/fake-dp-aux-channel.h"
#include "src/graphics/display/drivers/intel-display/hardware-common.h"
#include "src/graphics/display/drivers/intel-display/intel-display.h"
#include "src/graphics/display/drivers/intel-display/pch-engine.h"
#include "src/graphics/display/drivers/intel-display/pci-ids.h"
#include "src/graphics/display/drivers/intel-display/pipe-manager.h"
#include "src/graphics/display/drivers/intel-display/power.h"
#include "src/graphics/display/drivers/intel-display/registers-ddi.h"
#include "src/graphics/display/drivers/intel-display/registers-dpll.h"
#include "src/graphics/display/lib/api-protocols/cpp/display-engine-events-fidl.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"

namespace intel_display {

namespace {

// Value used to allocate space for the fake intel display register MMIO space.
// TODO(https://fxbug.dev/42164736): Remove this once DpDisplay no longer depends on Controller.
constexpr uint32_t kMmioSize = 0xd0000;

class TestDpll : public DisplayPll {
 public:
  explicit TestDpll(PllId pll_id) : DisplayPll(pll_id) {}
  ~TestDpll() override = default;

  bool DoEnable(const DdiPllConfig& pll_config) final {
    enabled_ = true;
    return enabled_;
  }
  bool DoDisable() final {
    enabled_ = false;
    return enabled_;
  }

 private:
  bool enabled_ = false;
};

class TestDpllManager : public DisplayPllManager {
 public:
  explicit TestDpllManager() {
    for (const auto dpll : kDplls) {
      plls_[dpll] = std::make_unique<TestDpll>(dpll);
      ref_count_[plls_[dpll].get()] = 0;
    }
  }

  DdiPllConfig LoadState(DdiId ddi_id) final {
    return DdiPllConfig{
        .ddi_clock_khz = 2'700'000,  // DisplayPort HBR2 5.4Gbps / lane
        .spread_spectrum_clocking = false,
        .admits_display_port = true,
        .admits_hdmi = false,
    };
  }

 private:
  static constexpr auto kDplls = {PllId::DPLL_0, PllId::DPLL_1, PllId::DPLL_2};

  bool SetDdiClockSource(DdiId ddi_id, PllId pll_id) final { return true; }
  bool ResetDdiClockSource(DdiId ddi_id) final { return true; }

  DisplayPll* FindPllFor(DdiId ddi_id, bool is_edp, const DdiPllConfig& desired_config) final {
    for (const auto dpll : kDplls) {
      if (ref_count_[plls_[dpll].get()] == 0) {
        return plls_[dpll].get();
      }
    }
    return nullptr;
  }
};

class TestPipeManager : public PipeManager {
 public:
  explicit TestPipeManager(Controller* controller) : PipeManager(DefaultPipes(controller)) {}

  static std::vector<std::unique_ptr<Pipe>> DefaultPipes(Controller* controller) {
    std::vector<std::unique_ptr<Pipe>> pipes;
    pipes.push_back(
        std::make_unique<PipeSkylake>(controller->mmio_space(), PipeId::PIPE_A, PowerWellRef{}));
    return pipes;
  }

  void ResetInactiveTranscoders() override {}

 private:
  Pipe* GetAvailablePipe() override { return At(PipeId::PIPE_A); }
  Pipe* GetPipeFromHwState(DdiId ddi_id, fdf::MmioBuffer* mmio_space) override {
    return At(PipeId::PIPE_A);
  }
};

class TestDdiPhysicalLayer final : public DdiPhysicalLayer {
 public:
  explicit TestDdiPhysicalLayer(DdiId ddi_id) : DdiPhysicalLayer(ddi_id) {}
  bool IsEnabled() const override { return enabled_; }
  bool IsHealthy() const override { return true; }
  bool Enable() override {
    enabled_ = true;
    return true;
  }
  bool Disable() override {
    enabled_ = false;
    return true;
  }

  PhysicalLayerInfo GetPhysicalLayerInfo() const override {
    return {
        .ddi_type = DdiPhysicalLayer::DdiType::kCombo,
        .connection_type = DdiPhysicalLayer::ConnectionType::kBuiltIn,
        .max_allowed_dp_lane_count = 4u,
    };
  }

 private:
  bool enabled_ = false;
};

class DpDisplayTest : public ::testing::Test {
 protected:
  DpDisplayTest()
      : controller_(&engine_events_, inspect::Inspector{}),
        mmio_buffer_({
            .vaddr = FakeMmioPtr(buffer_),
            .offset = 0,
            .size = kMmioSize,
            .vmo = ZX_HANDLE_INVALID,
        }) {
    std::memset(buffer_, 0, sizeof(buffer_));
  }

  void SetUp() override {
    controller_.SetMmioForTesting(mmio_buffer_.View(0));
    controller_.SetDpllManagerForTesting(std::make_unique<TestDpllManager>());
    controller_.SetPipeManagerForTesting(std::make_unique<TestPipeManager>(controller()));
    controller_.SetPowerWellForTesting(Power::New(controller_.mmio_space(), kTestDeviceDid));
    fake_dp_aux_channel_.SetDefaults();

    static constexpr int kAtlasGpuDeviceId = 0x591c;

    pch_engine_.emplace(controller_.mmio_space(), kAtlasGpuDeviceId);
    PchClockParameters clock_parameters = pch_engine_->ClockParameters();
    pch_engine_->FixClockParameters(clock_parameters);
    pch_engine_->SetClockParameters(clock_parameters);
    PchPanelParameters panel_parameters = pch_engine_->PanelParameters();
    panel_parameters.Fix();
    pch_engine_->SetPanelParameters(panel_parameters);
  }

  void TearDown() override {
    // Unset so controller teardown doesn't crash.
    controller_.ResetMmioSpaceForTesting();
  }

  std::unique_ptr<DpDisplay> MakeDisplay(DdiId ddi_id,
                                         display::DisplayId id = display::DisplayId{1}) {
    // TODO(https://fxbug.dev/42167004): In normal operation a DpDisplay is not fully constructed
    // until it receives a call to DisplayDevice::Query, then either DisplayDevice::Init() (for a
    // hotplug or initially powered-off display) OR DisplayDevice::AttachPipe() and
    // DisplayDevice::LoadACtiveMode() (for a pre-initialized display, e.g. bootloader-configured
    // eDP). For testing we only initialize until the Query() stage. The states of a DpDisplay
    // should become easier to reason about if remove the partially-initialized states.
    if (ddi_phys_[ddi_id] == nullptr) {
      ddi_phys_[ddi_id] = std::make_unique<TestDdiPhysicalLayer>(ddi_id);
      ddi_phys_[ddi_id]->Enable();
    }
    auto display = std::make_unique<DpDisplay>(&controller_, id, ddi_id, &fake_dp_aux_channel_,
                                               &pch_engine_.value(),
                                               DdiReference(ddi_phys_[ddi_id].get()), &node_);
    if (!display->Query()) {
      return nullptr;
    }
    return display;
  }

  Controller* controller() { return &controller_; }
  testing::FakeDpAuxChannel* fake_dp_aux_channel() { return &fake_dp_aux_channel_; }
  PchEngine* pch_engine() { return &pch_engine_.value(); }
  fdf::MmioBuffer* mmio_buffer() { return &mmio_buffer_; }

 private:
  fdf_testing::ScopedGlobalLogger logger_;

  // Must outlive `controller_`.
  display::DisplayEngineEventsFidl engine_events_;

  // TODO(https://fxbug.dev/42164736): Remove DpDisplay's dependency on Controller which will remove
  // the need for much of what's in SetUp() and TearDown().
  Controller controller_;
  uint8_t buffer_[kMmioSize];
  fdf::MmioBuffer mmio_buffer_;

  inspect::Node node_;
  testing::FakeDpAuxChannel fake_dp_aux_channel_;

  std::unordered_map<DdiId, std::unique_ptr<DdiPhysicalLayer>> ddi_phys_;

  std::optional<PchEngine> pch_engine_;
};

// Tests that display creation fails if the there is no DisplayPort sink.
TEST_F(DpDisplayTest, NoSinkNotSupported) {
  fake_dp_aux_channel()->SetSinkCount(0);
  ASSERT_EQ(nullptr, MakeDisplay(DdiId::DDI_A));
}

// Tests that display creation fails if the DP sink count is greater than 1, as
// MST is not supported.
TEST_F(DpDisplayTest, MultipleSinksNotSupported) {
  fake_dp_aux_channel()->SetSinkCount(2);
  ASSERT_EQ(nullptr, MakeDisplay(DdiId::DDI_A));
}

// Tests that the maximum supported lane count is 2 when DDI E is enabled.
TEST_F(DpDisplayTest, ReducedMaxLaneCountWhenDdiEIsEnabled) {
  auto buffer_control = registers::DdiRegs(DdiId::DDI_A).BufferControl().ReadFrom(mmio_buffer());
  buffer_control.set_ddi_e_disabled_kaby_lake(false).WriteTo(mmio_buffer());

  fake_dp_aux_channel()->SetMaxLaneCount(4);

  auto display = MakeDisplay(DdiId::DDI_A);
  ASSERT_NE(nullptr, display);
  EXPECT_EQ(2, display->lane_count());
}

// Tests that the maximum supported lane count is selected when DDI E is not enabled.
TEST_F(DpDisplayTest, MaxLaneCount) {
  auto buffer_control = registers::DdiRegs(DdiId::DDI_A).BufferControl().ReadFrom(mmio_buffer());
  buffer_control.set_ddi_e_disabled_kaby_lake(true).WriteTo(mmio_buffer());
  fake_dp_aux_channel()->SetMaxLaneCount(4);

  auto display = MakeDisplay(DdiId::DDI_A);
  ASSERT_NE(nullptr, display);
  EXPECT_EQ(4, display->lane_count());
}

// Tests that the link rate is set to the maximum supported rate based on DPCD data upon
// initialization via Init().
TEST_F(DpDisplayTest, LinkRateSelectionViaInit) {
  // Set up the IGD, DPLL, panel power control, and DisplayPort lane status registers for
  // DpDisplay::Init() to succeed. Configuring the IGD op region to indicate eDP will cause
  // Controller to assign DPLL0 to the display.

  // TODO(https://fxbug.dev/42164736): It shouldn't be necessary to rely on this logic in Controller
  // to test DpDisplay. Can DpDisplay be told that it is eDP during construction time instead of
  // querying Controller for it every time?
  controller()->igd_opregion_for_testing()->SetIsEdpForTesting(DdiId::DDI_A, true);
  auto dpll_status = registers::DisplayPllStatus::Get().ReadFrom(mmio_buffer());
  dpll_status.set_pll0_locked(true).WriteTo(mmio_buffer());

  // Mock the "Panel ready" status.
  auto panel_status = registers::PchPanelPowerStatus::Get().ReadFrom(mmio_buffer());
  panel_status.set_panel_on(1);
  panel_status.WriteTo(mmio_buffer());

  controller()->power()->SetDdiIoPowerState(DdiId::DDI_A, /* enable */ true);
  controller()->power()->SetAuxIoPowerState(DdiId::DDI_A, /* enable */ true);

  fake_dp_aux_channel()->registers[dpcd::DPCD_LANE0_1_STATUS] = 0xFF;
  fake_dp_aux_channel()->SetMaxLinkRate(dpcd::LinkBw::k5400Mbps);

  auto display = MakeDisplay(DdiId::DDI_A);
  ASSERT_NE(nullptr, display);

  EXPECT_TRUE(display->Init());
  EXPECT_EQ(5400u, display->link_rate_mhz());
}

// Tests that the link rate is set to a caller-assigned value upon initialization with
// InitWithDdiPllConfig.
TEST_F(DpDisplayTest, LinkRateSelectionViaInitWithDdiPllConfig) {
  // The max link rate should be disregarded by InitWithDdiPllConfig.
  fake_dp_aux_channel()->SetMaxLinkRate(dpcd::LinkBw::k5400Mbps);

  auto display = MakeDisplay(DdiId::DDI_A);
  ASSERT_NE(nullptr, display);

  const DdiPllConfig pll_config = {.ddi_clock_khz = 2'160'000,
                                   .spread_spectrum_clocking = false,
                                   .admits_display_port = true,
                                   .admits_hdmi = false};
  display->InitWithDdiPllConfig(pll_config);
  EXPECT_EQ(4320u, display->link_rate_mhz());
}

// Tests that the brightness value is obtained using the intel display south
// backlight control register when the related eDP DPCD capability is not supported.
TEST_F(DpDisplayTest, GetBacklightBrightnessUsesSouthBacklightRegister) {
  controller()->igd_opregion_for_testing()->SetIsEdpForTesting(DdiId::DDI_A, true);
  pch_engine()->SetPanelBrightness(0.5);

  auto display = MakeDisplay(DdiId::DDI_A);
  ASSERT_NE(nullptr, display);
  EXPECT_DOUBLE_EQ(0.5, display->GetBacklightBrightness());
}

// Tests that the brightness value is obtained from the related eDP DPCD registers when supported.
TEST_F(DpDisplayTest, GetBacklightBrightnessUsesDpcd) {
  constexpr uint16_t kDpcdBrightness100 = 0xFFFF;
  constexpr uint16_t kDpcdBrightness20 = 0x3333;

  // Intentionally configure the PCH PWM brightness value to something different
  // to prove that the PCH backlight is not used.
  pch_engine()->SetPanelBrightness(0.5);
  controller()->igd_opregion_for_testing()->SetIsEdpForTesting(DdiId::DDI_A, true);

  fake_dp_aux_channel()->SetEdpCapable(dpcd::EdpRevision::k1_4);
  fake_dp_aux_channel()->SetEdpBacklightBrightnessCapable();

  // Set the brightness to 100%.
  fake_dp_aux_channel()->registers[dpcd::DPCD_EDP_BACKLIGHT_BRIGHTNESS_LSB] =
      kDpcdBrightness100 & 0xFF;
  fake_dp_aux_channel()->registers[dpcd::DPCD_EDP_BACKLIGHT_BRIGHTNESS_MSB] =
      kDpcdBrightness100 >> 8;

  auto display = MakeDisplay(DdiId::DDI_A);
  ASSERT_NE(nullptr, display);
  EXPECT_DOUBLE_EQ(1.0, display->GetBacklightBrightness());

  // Set the brightness to 20%.
  fake_dp_aux_channel()->registers[dpcd::DPCD_EDP_BACKLIGHT_BRIGHTNESS_LSB] =
      kDpcdBrightness20 & 0xFF;
  fake_dp_aux_channel()->registers[dpcd::DPCD_EDP_BACKLIGHT_BRIGHTNESS_MSB] =
      kDpcdBrightness20 >> 8;

  display = MakeDisplay(DdiId::DDI_A);
  ASSERT_NE(nullptr, display);
  EXPECT_DOUBLE_EQ(0.20000000298023224, display->GetBacklightBrightness());
}

}  // namespace

}  // namespace intel_display

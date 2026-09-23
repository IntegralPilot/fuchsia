// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/intel-display/pipe-manager.h"

#include <lib/driver/fake-mmio-reg/cpp/fake-mmio-reg.h>
#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>

#include <cstddef>
#include <cstdint>
#include <memory>
#include <optional>

#include <gtest/gtest.h>

#include "src/graphics/display/drivers/intel-display/ddi-physical-layer-manager.h"
#include "src/graphics/display/drivers/intel-display/display-device.h"
#include "src/graphics/display/drivers/intel-display/dpll.h"
#include "src/graphics/display/drivers/intel-display/hardware-common.h"
#include "src/graphics/display/drivers/intel-display/intel-display.h"
#include "src/graphics/display/drivers/intel-display/pci-ids.h"
#include "src/graphics/display/drivers/intel-display/pipe.h"
#include "src/graphics/display/drivers/intel-display/power.h"
#include "src/graphics/display/lib/api-protocols/cpp/display-engine-events-fidl.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/display-timing.h"
#include "src/graphics/display/lib/api-types/cpp/mode-id.h"

namespace intel_display {

class PipeManagerTest : public ::testing::Test {
 public:
  PipeManagerTest() : controller_(&engine_events_, inspect::Inspector{}) {}

  void SetUp() override {
    mmio_buffer_.emplace(reg_region_.GetMmioBuffer());

    controller_.SetMmioForTesting(mmio_buffer_->View(0));
    controller_.SetPowerWellForTesting(Power::New(controller_.mmio_space(), kTestDeviceDid));
  }

  void TearDown() override {
    // Unset so controller teardown doesn't crash.
    controller_.ResetMmioSpaceForTesting();
  }

  Controller* controller() { return &controller_; }

 protected:
  static constexpr uint32_t kMinimumRegCount = 0xd0000 / sizeof(uint32_t);
  fdf_testing::ScopedGlobalLogger logger_;
  fake_mmio::FakeMmioRegRegion reg_region_{sizeof(uint32_t), kMinimumRegCount};
  std::optional<fdf::MmioBuffer> mmio_buffer_;

  // Must outlive `controller_`.
  display::DisplayEngineEventsFidl engine_events_;
  Controller controller_;
};

class FakeDisplay final : public DisplayDevice {
 public:
  FakeDisplay(Controller* controller, display::DisplayId id, DdiId ddi_id, Type type)
      : DisplayDevice(controller, id, ddi_id, DdiReference(), type) {}
  ~FakeDisplay() override = default;

  // DisplayDevice overrides:
  bool Query() final { return true; }
  bool InitWithDdiPllConfig(const DdiPllConfig& pll_config) final { return true; }
  AddedDisplayInfo CreateAddedDisplayInfo() final {
    return AddedDisplayInfo{
        .display_id = id(),
        .preferred_modes = {},
    };
  }
  std::optional<display::DisplayTiming> GetDisplayTiming(display::ModeId mode_id) const override {
    return std::nullopt;
  }

 private:
  // DisplayDevice overrides:
  bool InitDdi() final { return true; }
  bool DdiModeset(const display::DisplayTiming& mode) final { return true; }
  bool PipeConfigPreamble(const display::DisplayTiming& mode, PipeId pipe_id,
                          TranscoderId transcoder_id) final {
    return true;
  }
  bool PipeConfigEpilogue(const display::DisplayTiming& mode, PipeId pipe_id,
                          TranscoderId transcoder_id) final {
    return true;
  }
  DdiPllConfig ComputeDdiPllConfig(int32_t pixel_clock_khz) final { return {}; }
  int32_t LoadPixelRateForTranscoderKhz(TranscoderId transcoder_id) final { return 0; }
  bool CheckPixelRate(int64_t pixel_rate_hz) final { return true; }
};

// This tests if the PipeManager can allocate pipe for display devices and
// bind the display correctly.
TEST_F(PipeManagerTest, SkylakeAllocatePipe) {
  controller_.SetPipeManagerForTesting(std::make_unique<PipeManagerSkylake>(controller()));
  PipeManager* pm = controller_.pipe_manager();

  // Allocate pipe for DP display.
  display::DisplayId kDisplay1Id(1);
  std::unique_ptr<DisplayDevice> display1 = std::make_unique<FakeDisplay>(
      controller(), kDisplay1Id, DdiId::DDI_B, DisplayDevice::Type::kDp);
  Pipe* pipe1 = pm->RequestPipe(*display1);
  display1->set_pipe(pipe1);

  EXPECT_TRUE(pipe1);
  EXPECT_TRUE(pipe1->in_use());
  EXPECT_EQ(pipe1->attached_display_id(), kDisplay1Id);
  EXPECT_EQ(pipe1->tied_transcoder_id(), pipe1->connected_transcoder_id());

  // Allocate pipe for eDP display.
  controller()->igd_opregion_for_testing()->SetIsEdpForTesting(DdiId::DDI_A, true);

  display::DisplayId kDisplay2Id(2);
  std::unique_ptr<DisplayDevice> display2 = std::make_unique<FakeDisplay>(
      controller(), kDisplay2Id, DdiId::DDI_A, DisplayDevice::Type::kEdp);
  Pipe* pipe2 = pm->RequestPipe(*display2);
  display2->set_pipe(pipe2);

  EXPECT_TRUE(pipe2);
  EXPECT_NE(pipe2, pipe1);
  EXPECT_TRUE(pipe2->in_use());
  EXPECT_EQ(pipe2->attached_display_id(), kDisplay2Id);
  EXPECT_EQ(pipe2->connected_transcoder_id(), TranscoderId::TRANSCODER_EDP);

  display1.reset();
  EXPECT_FALSE(pipe1->in_use());

  display2.reset();
  EXPECT_FALSE(pipe2->in_use());
}

// This tests if the driver can reclaim used pipe and transcoder when the
// display is removed so that the pipes can be used for future devices.
TEST_F(PipeManagerTest, SkylakeReclaimUsedPipe) {
  controller_.SetPipeManagerForTesting(std::make_unique<PipeManagerSkylake>(controller()));
  PipeManager* pm = controller_.pipe_manager();

  for (size_t display_id_value = 1u;
       display_id_value <= PipeIds<registers::Platform::kKabyLake>().size() * 10;
       display_id_value++) {
    const display::DisplayId display_id{display_id_value};
    std::unique_ptr<DisplayDevice> display = std::make_unique<FakeDisplay>(
        controller(), display_id, DdiId::DDI_B, DisplayDevice::Type::kDp);
    Pipe* pipe = pm->RequestPipe(*display);
    display->set_pipe(pipe);

    EXPECT_TRUE(pipe);

    // On the end of each for loop, |display| is destroyed and |pipe| will be
    // reclaimed by PipeManager.
  }
}

}  // namespace intel_display

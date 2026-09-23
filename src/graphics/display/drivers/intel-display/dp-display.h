// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_DP_DISPLAY_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_DP_DISPLAY_H_

#include <lib/inspect/cpp/inspect.h>
#include <lib/zx/result.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <cstdint>
#include <optional>

#include <fbl/vector.h>

#include "src/graphics/display/drivers/intel-display/ddi-physical-layer.h"
#include "src/graphics/display/drivers/intel-display/display-device.h"
#include "src/graphics/display/drivers/intel-display/dp-aux-channel.h"
#include "src/graphics/display/drivers/intel-display/dp-capabilities.h"
#include "src/graphics/display/drivers/intel-display/dpcd.h"
#include "src/graphics/display/drivers/intel-display/pch-engine.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/display-timing.h"
#include "src/graphics/display/lib/edid/edid.h"

namespace intel_display {

class DpDisplay final : public DisplayDevice {
 public:
  DpDisplay(Controller* controller, display::DisplayId id, DdiId ddi_id,
            DpAuxChannel* dp_aux_channel, PchEngine* pch_engine, DdiReference ddi_reference,
            inspect::Node* parent_node);

  DpDisplay(const DpDisplay&) = delete;
  DpDisplay(DpDisplay&&) = delete;
  DpDisplay& operator=(const DpDisplay&) = delete;
  DpDisplay& operator=(DpDisplay&&) = delete;

  ~DpDisplay() override;

  // Gets the backlight brightness as a coefficient on the maximum brightness,
  // between the minimum brightness and 1.
  double GetBacklightBrightness();

  // DisplayDevice overrides:
  bool Query() final;
  bool InitWithDdiPllConfig(const DdiPllConfig& pll_config) final;

  AddedDisplayInfo CreateAddedDisplayInfo() override;
  std::optional<display::DisplayTiming> GetDisplayTiming(display::ModeId mode_id) const override;

  uint8_t lane_count() const { return dp_lane_count_; }
  uint32_t link_rate_mhz() const { return dp_link_rate_mhz_; }

 private:
  // DisplayDevice overrides:
  bool InitDdi() final;
  bool DdiModeset(const display::DisplayTiming& mode) final;
  bool PipeConfigPreamble(const display::DisplayTiming& mode, PipeId pipe_id,
                          TranscoderId transcoder_id) final;
  bool PipeConfigEpilogue(const display::DisplayTiming& mode, PipeId pipe_id,
                          TranscoderId transcoder_id) final;
  DdiPllConfig ComputeDdiPllConfig(int32_t pixel_clock_khz) final;
  int32_t LoadPixelRateForTranscoderKhz(TranscoderId transcoder_id) final;

  bool CheckPixelRate(int64_t pixel_rate_hz) final;

  // Returns true if the eDP panel is powered on.
  //
  // This method performs any configuration and power sequencing needed to get
  // the eDP panel powered on, which may include waiting for a significant
  // amount of time.
  //
  // This method returns fairly quickly if the panel is already configured and
  // powered on. It is almost idempotent, modulo the panel changing power
  // states independently.
  bool EnsureEdpPanelIsPoweredOn();

  bool DpcdWrite(uint32_t addr, const uint8_t* buf, size_t size);
  bool DpcdRead(uint32_t addr, uint8_t* buf, size_t size);
  bool DpcdRequestLinkTraining(const dpcd::TrainingPatternSet& tp_set,
                               const dpcd::TrainingLaneSet lanes[]);
  bool DpcdUpdateLinkTraining(const dpcd::TrainingLaneSet lanes[]);
  template <uint32_t addr, typename T>
  bool DpcdReadPairedRegs(hwreg::RegisterBase<T, typename T::ValueType>* status);
  bool DpcdHandleAdjustRequest(dpcd::TrainingLaneSet* training, dpcd::AdjustRequestLane* adjust);

  bool ProgramDpModeTigerLake();

  bool DoLinkTraining();

  // TODO(https://fxbug.dev/42065767): Move voltage swing configuration logic to a
  // DDI-specific class.
  void ConfigureVoltageSwingKabyLake(size_t phy_config_index);
  void ConfigureVoltageSwingTigerLake(size_t phy_config_index);
  void ConfigureVoltageSwingTypeCTigerLake(size_t phy_config_index);
  void ConfigureVoltageSwingComboTigerLake(size_t phy_config_index);

  bool LinkTrainingSetupKabyLake();
  bool LinkTrainingSetupTigerLake();

  // For locking Clock Recovery Circuit of the DisplayPort receiver
  bool LinkTrainingStage1(dpcd::TrainingPatternSet* tp_set, dpcd::TrainingLaneSet* lanes);
  // For optimizing equalization, determining symbol  boundary, and achieving inter-lane alignment
  bool LinkTrainingStage2(dpcd::TrainingPatternSet* tp_set, dpcd::TrainingLaneSet* lanes);

  bool SetBacklightOn(bool on);
  bool InitBacklightHw() override;

  bool IsBacklightOn();
  // Sets the backlight brightness with |val| as a coefficient on the maximum
  // brightness. |val| must be in [0, 1]. If the panel has a minimum fractional
  // brightness, then |val| will be clamped to [min, 1].
  bool SetBacklightBrightness(double val);

  bool HandleHotplug(bool long_pulse) override;
  bool HasBacklight() override;
  zx::result<> SetBacklightState(bool power, double brightness) override;
  zx::result<fuchsia_hardware_backlight::wire::State> GetBacklightState() override;

  void SetLinkRate(uint32_t value);

  // The object referenced by this pointer must outlive the DpDisplay.
  DpAuxChannel* dp_aux_channel_;  // weak

  // Used by eDP displays.
  PchEngine* pch_engine_;

  // Contains a value only if successfully initialized via Query().
  std::optional<DpCapabilities> capabilities_;

  // The current lane count and link rate. 0 if invalid/uninitialized.
  uint8_t dp_lane_count_ = 0;

  // The current per-lane link rate configuration. Use SetLinkRate to mutate the value which also
  // updates the related inspect properties.
  //
  // These values can be initialized by:
  //   1. InitWithDdiPllConfig based on an the current DPLL state
  //   2. Init, which selects the highest supported link rate
  //
  // The lane count is always initialized to the maximum value that the device can support in
  // Query().
  uint32_t dp_link_rate_mhz_ = 0;
  std::optional<uint8_t> dp_link_rate_table_idx_;

  // The backlight brightness coefficient, in the range [min brightness, 1].
  double backlight_brightness_ = 1.0f;

  // Initialized in `Query()`.
  std::optional<edid::Edid> edid_;

  // Initialized in `Query()`.
  // The timing parameter at index `k` corresponds to ModeId `k+1`.
  fbl::Vector<display::DisplayTiming> timings_;

  // Debug
  inspect::Node inspect_node_;
  inspect::Node dp_capabilities_node_;
  inspect::UintProperty dp_lane_count_inspect_;
  inspect::UintProperty dp_link_rate_mhz_inspect_;
};

}  // namespace intel_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_DP_DISPLAY_H_

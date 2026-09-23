// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/intel-display/hdmi-display.h"

#include <lib/driver/logging/cpp/logger.h>
#include <lib/fit/result.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>
#include <zircon/assert.h>
#include <zircon/syscalls.h>
#include <zircon/time.h>

#include <algorithm>
#include <cstddef>
#include <cstdint>
#include <optional>
#include <span>
#include <utility>

#include <fbl/alloc_checker.h>
#include <fbl/vector.h>

#include "src/graphics/display/drivers/intel-display/ddi-physical-layer-manager.h"
#include "src/graphics/display/drivers/intel-display/display-device.h"
#include "src/graphics/display/drivers/intel-display/display-timing-mode-conversion.h"
#include "src/graphics/display/drivers/intel-display/dpll-config.h"
#include "src/graphics/display/drivers/intel-display/dpll.h"
#include "src/graphics/display/drivers/intel-display/hardware-common.h"
#include "src/graphics/display/drivers/intel-display/i2c/gmbus-gpio.h"
#include "src/graphics/display/drivers/intel-display/i2c/gmbus-i2c.h"
#include "src/graphics/display/drivers/intel-display/intel-display.h"
#include "src/graphics/display/drivers/intel-display/pci-ids.h"
#include "src/graphics/display/drivers/intel-display/registers-ddi.h"
#include "src/graphics/display/drivers/intel-display/registers-gmbus.h"
#include "src/graphics/display/drivers/intel-display/registers-transcoder.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/display-timing.h"
#include "src/graphics/display/lib/api-types/cpp/mode-and-id.h"
#include "src/graphics/display/lib/api-types/cpp/mode-id.h"
#include "src/graphics/display/lib/driver-utils/poll-until.h"
#include "src/graphics/display/lib/edid/edid.h"

namespace intel_display {

// I2c functions

namespace {

// Recommended DDI buffer translation programming values

struct DdiPhyConfigEntry {
  uint32_t entry2;
  uint32_t entry1;
};

// The tables below have the values recommended by the documentation.
//
// Kaby Lake: IHD-OS-KBL-Vol 12-1.17 pages 187-190
// Skylake: IHD-OS-SKL-Vol 12-05.16 pages 181-183
//
// TODO(https://fxbug.dev/42059656): Per-entry Iboost values.

constexpr DdiPhyConfigEntry kPhyConfigHdmiSkylakeUhs[11] = {
    {0x000000ac, 0x00000018}, {0x0000009d, 0x00005012}, {0x00000088, 0x00007011},
    {0x000000a1, 0x00000018}, {0x00000098, 0x00000018}, {0x00000088, 0x00004013},
    {0x000000cd, 0x80006012}, {0x000000df, 0x00000018}, {0x000000cd, 0x80003015},
    {0x000000c0, 0x80003015}, {0x000000c0, 0x80000018},
};

constexpr DdiPhyConfigEntry kPhyConfigHdmiSkylakeY[11] = {
    {0x000000a1, 0x00000018}, {0x000000df, 0x00005012}, {0x000000cb, 0x80007011},
    {0x000000a4, 0x00000018}, {0x0000009d, 0x00000018}, {0x00000080, 0x00004013},
    {0x000000c0, 0x80006012}, {0x0000008a, 0x00000018}, {0x000000c0, 0x80003015},
    {0x000000c0, 0x80003015}, {0x000000c0, 0x80000018},
};

std::span<const DdiPhyConfigEntry> GetHdmiPhyConfigEntries(uint16_t device_id,
                                                           uint8_t* default_iboost) {
  if (is_skl_y(device_id) || is_kbl_y(device_id)) {
    *default_iboost = 3;
    return kPhyConfigHdmiSkylakeY;
  }

  *default_iboost = 1;
  return kPhyConfigHdmiSkylakeUhs;
}

}  // namespace

// Modesetting functions

// On DisplayDevice creation we cannot determine whether it is an HDMI
// display; this will be updated when intel-display Controller gets EDID
// information for this device (before Init()).
HdmiDisplay::HdmiDisplay(Controller* controller, display::DisplayId id, DdiId ddi_id,
                         DdiReference ddi_reference, GMBusI2c* gmbus_i2c)
    : DisplayDevice(controller, id, ddi_id, std::move(ddi_reference), Type::kHdmi),
      gmbus_i2c_(*gmbus_i2c) {
  ZX_DEBUG_ASSERT(controller != nullptr);
  ZX_DEBUG_ASSERT(gmbus_i2c != nullptr);
}

HdmiDisplay::~HdmiDisplay() = default;

bool HdmiDisplay::Query() {
  // HDMI isn't supported on these DDIs
  const registers::Platform platform = GetPlatform(controller()->device_id());
  if (!GMBusPinPair::HasValidPinPair(ddi_id(), platform)) {
    return false;
  }

  // Reset the GMBus registers and disable GMBus interrupts
  registers::GMBusClockPortSelect::Get().FromValue(0).WriteTo(mmio_space());
  registers::GMBusControllerInterruptMask::Get().FromValue(0).WriteTo(mmio_space());

  // The only way to tell if an HDMI monitor is actually connected is
  // to try to read an E-EDID byte.
  bool has_display = false;
  static constexpr int kMaxProbeDisplayAttemptCount = 3;
  for (int i = 0; i < kMaxProbeDisplayAttemptCount; ++i) {
    has_display = gmbus_i2c_.ProbeDisplay();
    if (has_display) {
      fdf::trace("Found a hdmi/dvi monitor");
      break;
    }
    zx_nanosleep(zx_deadline_after(ZX_MSEC(5)));
  }

  if (!has_display) {
    fdf::trace("Failed to find a display after {} attempts", kMaxProbeDisplayAttemptCount);
    return false;
  }

  zx::result<fbl::Vector<uint8_t>> read_extended_edid_result = gmbus_i2c_.ReadExtendedEdid();
  if (read_extended_edid_result.is_error()) {
    fdf::error("Failed to read E-EDID of the display: {}", read_extended_edid_result);
    return false;
  }

  fit::result<const char*, edid::Edid> edid_result =
      edid::Edid::Create(std::move(read_extended_edid_result).value());
  if (edid_result.is_error()) {
    fdf::error("Failed to parse E-EDID of the display: %s", edid_result.error_value());
    return false;
  }
  edid_ = std::move(edid_result).value();

  // Populate `timings_` with the display's supported timings.
  zx::result<fbl::Vector<display::DisplayTiming>> timings_result =
      edid_->GetSupportedDisplayTimings();
  if (timings_result.is_error()) {
    fdf::error("Failed to get supported display timings: {}", timings_result.status_string());
    return false;
  }
  timings_ = std::move(timings_result).value();
  return true;
}

bool HdmiDisplay::InitDdi() {
  // All the init happens during modeset
  return true;
}

bool HdmiDisplay::DdiModeset(const display::DisplayTiming& mode) {
  pipe()->Reset();
  controller()->ResetDdi(ddi_id(), pipe()->connected_transcoder_id());

  const int64_t pixel_clock_frequency_khz = mode.pixel_clock_frequency_hz / 1'000;
  DdiPllConfig pll_config = {
      .ddi_clock_khz = static_cast<int32_t>(pixel_clock_frequency_khz) * 5,
      .spread_spectrum_clocking = false,
      .admits_display_port = false,
      .admits_hdmi = true,
  };

  DisplayPll* dpll =
      controller()->dpll_manager()->SetDdiPllConfig(ddi_id(), /*is_edp=*/false, pll_config);
  if (dpll == nullptr) {
    return false;
  }

  ZX_DEBUG_ASSERT(controller()->power());
  controller()->power()->SetDdiIoPowerState(ddi_id(), /*enable=*/true);
  if (!display::PollUntil([&] { return controller()->power()->GetDdiIoPowerState(ddi_id()); },
                          zx::usec(1), 20)) {
    fdf::error("DDI {} IO power did not come up in 20us", ddi_id());
    return false;
  }

  controller()->power()->SetAuxIoPowerState(ddi_id(), /*enable=*/true);
  if (!display::PollUntil([&] { return controller()->power()->GetAuxIoPowerState(ddi_id()); },
                          zx::usec(1), 10)) {
    fdf::error("DDI {} IO power did not come up in 10us", ddi_id());
    return false;
  }

  return true;
}

bool HdmiDisplay::PipeConfigPreamble(const display::DisplayTiming& mode, PipeId pipe_id,
                                     TranscoderId transcoder_id) {
  ZX_DEBUG_ASSERT_MSG(transcoder_id != TranscoderId::TRANSCODER_EDP,
                      "The EDP transcoder doesn't do HDMI");

  registers::TranscoderRegs transcoder_regs(transcoder_id);

  // Configure Transcoder Clock Select
  auto transcoder_clock_select = transcoder_regs.ClockSelect().ReadFrom(mmio_space());
  if (is_tgl(controller()->device_id())) {
    transcoder_clock_select.set_ddi_clock_tiger_lake(ddi_id());
  } else {
    transcoder_clock_select.set_ddi_clock_kaby_lake(ddi_id());
  }
  transcoder_clock_select.WriteTo(mmio_space());

  return true;
}

bool HdmiDisplay::PipeConfigEpilogue(const display::DisplayTiming& mode, PipeId pipe_id,
                                     TranscoderId transcoder_id) {
  ZX_DEBUG_ASSERT(type() == DisplayDevice::Type::kHdmi || type() == DisplayDevice::Type::kDvi);
  ZX_DEBUG_ASSERT_MSG(transcoder_id != TranscoderId::TRANSCODER_EDP,
                      "The EDP transcoder doesn't do HDMI");

  registers::TranscoderRegs transcoder_regs(transcoder_id);

  auto transcoder_ddi_control = transcoder_regs.DdiControl().ReadFrom(mmio_space());
  transcoder_ddi_control.set_enabled(true);
  if (is_tgl(controller()->device_id())) {
    transcoder_ddi_control.set_ddi_tiger_lake(ddi_id());
  } else {
    transcoder_ddi_control.set_ddi_kaby_lake(ddi_id());
  }
  transcoder_ddi_control.set_ddi_mode(type() == DisplayDevice::Type::kHdmi
                                          ? registers::TranscoderDdiControl::kModeHdmi
                                          : registers::TranscoderDdiControl::kModeDvi);
  transcoder_ddi_control.set_bits_per_color(registers::TranscoderDdiControl::k8bpc)
      .set_vsync_polarity_not_inverted(mode.vsync_polarity == display::SyncPolarity::kPositive)
      .set_hsync_polarity_not_inverted(mode.hsync_polarity == display::SyncPolarity::kPositive)
      .set_is_port_sync_secondary_kaby_lake(false)
      .set_allocate_display_port_virtual_circuit_payload(false)
      .WriteTo(mmio_space());

  auto transcoder_config = transcoder_regs.Config().ReadFrom(mmio_space());
  transcoder_config.set_enabled_target(true)
      .set_interlaced_display(mode.fields_per_frame == display::FieldsPerFrame::kInterlaced)
      .WriteTo(mmio_space());

  // Configure voltage swing and related IO settings.
  //
  // TODO(https://fxbug.dev/42065767): Move voltage swing configuration logic to a
  // DDI-specific class.

  // kUseDefaultIdx always fails the idx-in-bounds check, so no additional handling is needed
  uint8_t idx = controller()->igd_opregion().GetHdmiBufferTranslationIndex(ddi_id());
  uint8_t i_boost_override = controller()->igd_opregion().GetIBoost(ddi_id(), false /* is_dp */);

  uint8_t default_iboost;
  const std::span<const DdiPhyConfigEntry> entries =
      GetHdmiPhyConfigEntries(controller()->device_id(), &default_iboost);
  if (idx >= entries.size()) {
    idx = 8;  // Default index
  }

  registers::DdiRegs ddi_regs(ddi_id());
  auto phy_config_entry1 = ddi_regs.PhyConfigEntry1(9).FromValue(0);
  phy_config_entry1.set_reg_value(entries[idx].entry1);
  if (i_boost_override) {
    phy_config_entry1.set_balance_leg_enable(1);
  }
  phy_config_entry1.WriteTo(mmio_space());

  auto phy_config_entry2 = ddi_regs.PhyConfigEntry2(9).FromValue(0);
  phy_config_entry2.set_reg_value(entries[idx].entry2).WriteTo(mmio_space());

  auto phy_balance_control = registers::DdiPhyBalanceControl::Get().ReadFrom(mmio_space());
  phy_balance_control.set_disable_balance_leg(0);
  phy_balance_control.balance_leg_select_for_ddi(ddi_id()).set(i_boost_override ? i_boost_override
                                                                                : default_iboost);
  phy_balance_control.WriteTo(mmio_space());

  // Configure and enable DDI_BUF_CTL
  auto buffer_control = ddi_regs.BufferControl().ReadFrom(mmio_space());
  buffer_control.set_enabled(true);
  buffer_control.WriteTo(mmio_space());

  return true;
}

DdiPllConfig HdmiDisplay::ComputeDdiPllConfig(int32_t pixel_clock_khz) {
  return DdiPllConfig{
      .ddi_clock_khz = pixel_clock_khz * 5,
      .spread_spectrum_clocking = false,
      .admits_display_port = false,
      .admits_hdmi = true,
  };
}

bool HdmiDisplay::CheckPixelRate(int64_t pixel_rate_hz) {
  // Pixel rates of 300M/165M pixels per second for HDMI/DVI. The Intel docs state
  // that the maximum link bit rate of an HDMI port is 3GHz, not 3.4GHz that would
  // be expected  based on the HDMI spec.
  if ((type() == DisplayDevice::Type::kHdmi ? 300'000'000 : 165'000'000) < pixel_rate_hz) {
    return false;
  }

  int32_t pixel_rate_khz = static_cast<int32_t>(pixel_rate_hz / 1'000);
  DdiPllConfig pll_config = ComputeDdiPllConfig(pixel_rate_khz);
  if (pll_config.IsEmpty()) {
    return false;
  }

  DpllOscillatorConfig dco_config = CreateDpllOscillatorConfigKabyLake(pll_config.ddi_clock_khz);
  return dco_config.frequency_khz != 0;
}

AddedDisplayInfo HdmiDisplay::CreateAddedDisplayInfo() {
  fbl::Vector<display::ModeAndId> preferred_modes;
  size_t preferred_modes_size =
      std::min(timings_.size(), size_t{AddedDisplayInfo::kMaxPreferredModes});
  fbl::AllocChecker alloc_checker;
  preferred_modes.reserve(preferred_modes_size, &alloc_checker);
  ZX_ASSERT(alloc_checker.check());

  for (uint16_t i = 0; i < preferred_modes_size; ++i) {
    preferred_modes.push_back({{.id = display::ModeId(i + 1), .mode = ToDisplayMode(timings_[i])}});
  }
  return AddedDisplayInfo{
      .display_id = id(),
      .preferred_modes = std::move(preferred_modes),
  };
}

std::optional<display::DisplayTiming> HdmiDisplay::GetDisplayTiming(display::ModeId mode_id) const {
  ZX_DEBUG_ASSERT(mode_id != display::kInvalidModeId);
  int index = int{mode_id.value()} - 1;
  ZX_DEBUG_ASSERT(index >= 0);
  if (static_cast<size_t>(index) >= timings_.size()) {
    return std::nullopt;
  }
  return timings_[index];
}

}  // namespace intel_display

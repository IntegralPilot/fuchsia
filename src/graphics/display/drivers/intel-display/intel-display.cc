// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/intel-display/intel-display.h"

#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <fuchsia/hardware/intelgpucore/c/banjo.h>
#include <lib/ddk/driver.h>
#include <lib/ddk/hw/inout.h>
#include <lib/device-protocol/pci.h>
#include <lib/driver/component/cpp/prepare_stop_completer.h>
#include <lib/driver/component/cpp/start_completer.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fit/function.h>
#include <lib/image-format/image_format.h>
#include <lib/pci/constants.h>
#include <lib/sysmem-version/sysmem-version.h>
#include <lib/zbi-format/graphics.h>
#include <lib/zbitl/items/graphics.h>
#include <lib/zx/clock.h>
#include <lib/zx/resource.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>
#include <lib/zx/vmo.h>
#include <limits.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/process.h>
#include <zircon/syscalls.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <algorithm>
#include <array>
#include <cinttypes>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <iterator>
#include <limits>
#include <memory>
#include <optional>
#include <span>
#include <string>
#include <utility>
#include <variant>
#include <vector>

#include <bind/fuchsia/sysmem/heap/cpp/bind.h>
#include <ddktl/device.h>
#include <fbl/algorithm.h>
#include <fbl/alloc_checker.h>
#include <fbl/auto_lock.h>
#include <fbl/vector.h>

#include "src/graphics/display/drivers/intel-display/clock/cdclk.h"
#include "src/graphics/display/drivers/intel-display/ddi-physical-layer-manager.h"
#include "src/graphics/display/drivers/intel-display/ddi.h"
#include "src/graphics/display/drivers/intel-display/display-device.h"
#include "src/graphics/display/drivers/intel-display/dp-display.h"
#include "src/graphics/display/drivers/intel-display/dpll.h"
#include "src/graphics/display/drivers/intel-display/fuse-config.h"
#include "src/graphics/display/drivers/intel-display/gtt.h"
#include "src/graphics/display/drivers/intel-display/hardware-common.h"
#include "src/graphics/display/drivers/intel-display/hdmi-display.h"
#include "src/graphics/display/drivers/intel-display/pch-engine.h"
#include "src/graphics/display/drivers/intel-display/pci-ids.h"
#include "src/graphics/display/drivers/intel-display/pipe-manager.h"
#include "src/graphics/display/drivers/intel-display/pipe.h"
#include "src/graphics/display/drivers/intel-display/power-controller.h"
#include "src/graphics/display/drivers/intel-display/power.h"
#include "src/graphics/display/drivers/intel-display/registers-ddi.h"
#include "src/graphics/display/drivers/intel-display/registers-dpll.h"
#include "src/graphics/display/drivers/intel-display/registers-pipe-scaler.h"
#include "src/graphics/display/drivers/intel-display/registers-pipe.h"
#include "src/graphics/display/drivers/intel-display/registers.h"
#include "src/graphics/display/drivers/intel-display/tiling.h"
#include "src/graphics/display/lib/api-types/cpp/color-conversion.h"
#include "src/graphics/display/lib/api-types/cpp/config-check-result.h"
#include "src/graphics/display/lib/api-types/cpp/coordinate-transformation.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/display-timing.h"
#include "src/graphics/display/lib/api-types/cpp/driver-buffer-collection-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-capture-image-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-config-stamp.h"
#include "src/graphics/display/lib/api-types/cpp/driver-image-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-layer.h"
#include "src/graphics/display/lib/api-types/cpp/engine-info.h"
#include "src/graphics/display/lib/api-types/cpp/image-buffer-usage.h"
#include "src/graphics/display/lib/api-types/cpp/image-metadata.h"
#include "src/graphics/display/lib/api-types/cpp/image-tiling-type.h"
#include "src/graphics/display/lib/api-types/cpp/mode-id.h"
#include "src/graphics/display/lib/api-types/cpp/pixel-format.h"
#include "src/graphics/display/lib/api-types/cpp/power-mode.h"
#include "src/graphics/display/lib/driver-utils/poll-until.h"
#include "src/lib/fxl/strings/string_printf.h"

namespace intel_display {

namespace {

constexpr std::array<display::PixelFormat, 2> kPixelFormatTypes = {
    display::PixelFormat::kB8G8R8A8,
    display::PixelFormat::kR8G8B8A8,
};

// TODO(https://fxbug.dev/42166519): Remove after YUV buffers can be imported to Intel display.
constexpr fuchsia_images2::wire::PixelFormat kYuvPixelFormatTypes[2] = {
    fuchsia_images2::wire::PixelFormat::kI420,
    fuchsia_images2::wire::PixelFormat::kNv12,
};

void GetPostTransformWidth(const display::DriverLayer& layer, uint32_t* width, uint32_t* height) {
  if (layer.image_source_transformation() == display::CoordinateTransformation::kIdentity ||
      layer.image_source_transformation() == display::CoordinateTransformation::kRotateCcw180 ||
      layer.image_source_transformation() == display::CoordinateTransformation::kReflectX ||
      layer.image_source_transformation() == display::CoordinateTransformation::kReflectY) {
    *width = layer.image_source().width();
    *height = layer.image_source().height();
  } else {
    *width = layer.image_source().height();
    *height = layer.image_source().width();
  }
}

struct FramebufferInfo {
  uint32_t size;
  uint32_t width;
  uint32_t height;
  uint32_t stride;
  zbi_pixel_format_t format;
  int bytes_per_pixel;
};

// The bootloader (UEFI and Depthcharge) informs zircon of the framebuffer information using a
// ZBI_TYPE_FRAMEBUFFER entry.
zx::result<FramebufferInfo> GetFramebufferInfo(std::optional<zbi_swfb_t> fb_info) {
  if (!fb_info.has_value()) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  FramebufferInfo info;
  info.width = fb_info->width;
  info.height = fb_info->height;
  info.stride = fb_info->stride;
  info.format = fb_info->format;
  info.bytes_per_pixel = zbitl::BytesPerPixel(info.format);
  info.size = info.stride * info.height * info.bytes_per_pixel;
  return zx::ok(info);
}

}  // namespace

void Controller::HandleHotplug(DdiId ddi_id, bool long_pulse) {
  fdf::trace("Hotplug detected on ddi {} (long_pulse={})", ddi_id, long_pulse);
  fbl::AutoLock lock(&display_lock_);

  std::unique_ptr<DisplayDevice> device = nullptr;
  for (size_t i = 0; i < display_devices_.size(); i++) {
    if (display_devices_[i]->ddi_id() == ddi_id) {
      if (display_devices_[i]->HandleHotplug(long_pulse)) {
        fdf::debug("hotplug handled by device");
        return;
      }
      device = display_devices_.erase(i);
      break;
    }
  }

  // An existing display device was unplugged.
  if (device != nullptr) {
    fdf::info("Display {} unplugged", device->id().value());
    display::DisplayId removed_display_id = device->id();
    RemoveDisplay(std::move(device));

    engine_events_.OnDisplayRemoved(removed_display_id);
    return;
  }

  // A new display device was plugged in.
  std::unique_ptr<DisplayDevice> new_device = QueryDisplay(ddi_id, next_id_);
  if (!new_device || !new_device->Init()) {
    fdf::error("Failed to initialize hotplug display");
    return;
  }

  DisplayDevice* new_device_ptr = new_device.get();
  zx_status_t add_display_status = AddDisplay(std::move(new_device));
  if (add_display_status != ZX_OK) {
    fdf::error("Failed to add a new display: {}", zx::make_result(add_display_status));
    return;
  }

  const AddedDisplayInfo added_display_info = new_device_ptr->CreateAddedDisplayInfo();
  engine_events_.OnDisplayAdded(added_display_info.display_id, added_display_info.preferred_modes,
                                kPixelFormatTypes);
}

void Controller::HandlePipeVsync(PipeId pipe_id, zx::time_monotonic timestamp) {
  fbl::AutoLock lock(&display_lock_);

  display::DisplayId pipe_attached_display_id = display::kInvalidDisplayId;

  display::DriverConfigStamp vsync_config_stamp = display::kInvalidDriverConfigStamp;

  Pipe* pipe = (*pipe_manager_)[pipe_id];
  if (pipe && pipe->in_use()) {
    pipe_attached_display_id = pipe->attached_display_id();

    registers::PipeRegs regs(pipe_id);
    std::vector<display::DriverImageId> image_ids;
    for (int i = 0; i < 3; i++) {
      auto live_surface = regs.PlaneSurfaceLive(i).ReadFrom(mmio_space());
      uint64_t handle = live_surface.surface_base_addr() << live_surface.kPageShift;

      if (handle) {
        image_ids.emplace_back(handle);
      }
    }

    auto live_surface = regs.CursorSurfaceLive().ReadFrom(mmio_space());
    uint64_t handle = live_surface.surface_base_addr() << live_surface.kPageShift;

    if (handle) {
      image_ids.emplace_back(handle);
    }

    vsync_config_stamp = pipe->GetVsyncConfigStamp(image_ids);
  }

  if (pipe_attached_display_id != display::kInvalidDisplayId &&
      vsync_config_stamp != display::kInvalidDriverConfigStamp) {
    engine_events_.OnDisplayVsync(pipe_attached_display_id, timestamp, vsync_config_stamp);
  }
}

DisplayDevice* Controller::FindDevice(display::DisplayId display_id) {
  for (auto& d : display_devices_) {
    if (d->id() == display_id) {
      return d.get();
    }
  }
  return nullptr;
}

bool Controller::BringUpDisplayEngine(bool resume) {
  // We follow the steps in the PRM section "Mode Set" > "Sequences to
  // Initialize Display" > "Initialize Sequence", with the tweak that we attempt
  // to reuse the setup left in place by the boot firmware.
  //
  // Tiger Lake: IHD-OS-TGL-Vol 12-1.22-Rev2.0 pages 141-142
  // DG1: IHD-OS-DG1-Vol 12-2.21 pages 119-120
  // Kaby Lake: IHD-OS-KBL-Vol 12-1.17 page 112-113
  // Skylake: IHD-OS-SKL-Vol 12-05.16 page 110

  pch_engine_->SetPchResetHandshake(true);
  if (resume) {
    // The PCH clocks must be set during the display engine initialization
    // sequence. The rest of the PCH configuration will be restored later.
    pch_engine_->RestoreClockParameters();
  } else {
    const PchClockParameters pch_clock_parameters = pch_engine_->ClockParameters();
    PchClockParameters fixed_pch_clock_parameters = pch_clock_parameters;
    pch_engine_->FixClockParameters(fixed_pch_clock_parameters);
    if (pch_clock_parameters != fixed_pch_clock_parameters) {
      fdf::warn("PCH clocking incorrectly configured. Re-configuring.");
    }
    pch_engine_->SetClockParameters(fixed_pch_clock_parameters);
  }

  // Wait for Power Well 0 distribution
  if (!display::PollUntil(
          [&] { return registers::FuseStatus::Get().ReadFrom(mmio_space()).pg0_dist_status(); },
          zx::usec(1), 20)) {
    fdf::error("Power Well 0 distribution failed");
    return false;
  }

  // TODO(https://fxbug.dev/42061147): Currently the driver relies on the assumption that
  // PG1 and Misc IO are always enabled by firmware. We should manually ensure
  // them they are enabled here and disable them on driver teardown.

  ZX_DEBUG_ASSERT(power_);
  if (resume) {
    power_->Resume();
  } else {
    cd_clk_power_well_ = power_->GetCdClockPowerWellRef();
  }

  if (is_tgl(device_id_)) {
    auto pwr_well_ctrl = registers::PowerWellControl::Get().ReadFrom(mmio_space());
    pwr_well_ctrl.power_request(1).set(1);
    pwr_well_ctrl.WriteTo(mmio_space());

    if (!display::PollUntil(
            [&] {
              return registers::PowerWellControl::Get().ReadFrom(mmio_space()).power_state(0).get();
            },
            zx::usec(1), 30)) {
      fdf::error("Power Well 1 state failed");
      return false;
    }

    if (!display::PollUntil(
            [&] { return registers::FuseStatus::Get().ReadFrom(mmio_space()).pg1_dist_status(); },
            zx::usec(1), 20)) {
      fdf::error("Power Well 1 distribution failed");
      return false;
    }

    // Enable cd_clk and set the frequency to minimum.
    cd_clk_ = std::make_unique<CoreDisplayClockTigerLake>(mmio_space());
    // PLL ratio for 38.4MHz: 16 -> CDCLK 307.2 MHz
    if (!cd_clk_->SetFrequency(307'200)) {
      fdf::error("Failed to configure CD clock frequency");
      return false;
    }
  } else {
    // Enable CDCLK PLL to 337.5mhz if the BIOS didn't already enable it. If it needs to be
    // something special (i.e. for eDP), assume that the BIOS already enabled it.
    auto lcpll1_control =
        registers::PllEnable::GetForSkylakeDpll(PllId::DPLL_0).ReadFrom(mmio_space());
    if (!lcpll1_control.pll_enabled()) {
      // Configure DPLL0 frequency before enabling it.
      const auto dpll = PllId::DPLL_0;
      auto dpll_control1 = registers::DisplayPllControl1::Get().ReadFrom(mmio_space());
      dpll_control1.set_pll_uses_hdmi_configuration_mode(dpll, false)
          .set_pll_spread_spectrum_clocking_enabled(dpll, false)
          .set_pll_display_port_ddi_frequency_mhz(dpll, 810)
          .set_pll_programming_enabled(dpll, true)
          .WriteTo(mmio_space());

      // Enable DPLL0 and wait for it.
      lcpll1_control.set_pll_enabled(true);
      lcpll1_control.WriteTo(mmio_space());

      // The PRM instructs us to use the LCPLL1 control register to find out
      // when DPLL0 locks. This is different from most DPLL enabling sequences,
      // which use the DPLL status registers.
      if (!display::PollUntil(
              [&] {
                return lcpll1_control.ReadFrom(mmio_space()).pll_locked_tiger_lake_and_lcpll1();
              },
              zx::msec(1), 5)) {
        fdf::error("DPLL0 / LCPLL1 did not lock in 5us");
        return false;
      }

      // Enable cd_clk and set the frequency to minimum.
      cd_clk_ = std::make_unique<CoreDisplayClockSkylake>(mmio_space());
      if (!cd_clk_->SetFrequency(337'500)) {
        fdf::error("Failed to configure CD clock frequency");
        return false;
      }
    } else {
      cd_clk_ = std::make_unique<CoreDisplayClockSkylake>(mmio_space());
      fdf::info("CDCLK already assigned by BIOS: frequency: {} KHz", cd_clk_->current_freq_khz());
    }
  }

  // Power up DBUF (Data Buffer) slices.
  fdf::trace("Powering up DBUF (Data Buffer) slices");
  const int display_buffer_slice_count = is_tgl(device_id_) ? 2 : 1;
  for (int slice_index = 0; slice_index < display_buffer_slice_count; ++slice_index) {
    auto display_buffer_control =
        registers::DataBufferControl::GetForSlice(slice_index).ReadFrom(mmio_space());
    display_buffer_control.set_powered_on_target(true).WriteTo(mmio_space());

    if (!display::PollUntil(
            [&] { return display_buffer_control.ReadFrom(mmio_space()).powered_on(); }, zx::usec(1),
            10)) {
      fdf::error("DBUF slice {} did not power up in time", slice_index + 1);
      return false;
    }
  }

  // We never use VGA, so just disable it at startup
  constexpr uint16_t kSequencerIdx = 0x3c4;
  constexpr uint16_t kSequencerData = 0x3c5;
  constexpr uint8_t kClockingModeIdx = 1;
  constexpr uint8_t kClockingModeScreenOff = (1 << 5);
  zx_status_t status = zx_ioports_request(resources_.ioport->get(), kSequencerIdx, 2);
  if (status != ZX_OK) {
    fdf::error("Failed to map vga ports");
    return false;
  }
  outp(kSequencerIdx, kClockingModeIdx);
  uint8_t clocking_mode = inp(kSequencerData);
  if (!(clocking_mode & kClockingModeScreenOff)) {
    outp(kSequencerIdx, inp(kSequencerData) | kClockingModeScreenOff);
    zx_nanosleep(zx_deadline_after(ZX_MSEC(100)));

    auto vga_ctl = registers::VgaCtl::Get().ReadFrom(mmio_space());
    vga_ctl.set_vga_display_disable(1);
    vga_ctl.WriteTo(mmio_space());
  }

  for (Pipe* pipe : *pipe_manager_) {
    pipe->Reset();
    ResetPipePlaneBuffers(pipe->pipe_id());

    registers::PipeRegs pipe_regs(pipe->pipe_id());

    // Disable the scalers (double buffered on PipeScalerWindowSize), since
    // we don't know what state they are in at boot.
    auto pipe_scaler_0_regs = registers::PipeScalerRegs(pipe->pipe_id(), 0);
    pipe_scaler_0_regs.PipeScalerControlSkylake()
        .ReadFrom(mmio_space())
        .set_is_enabled(0)
        .WriteTo(mmio_space());
    pipe_scaler_0_regs.PipeScalerWindowSize().ReadFrom(mmio_space()).WriteTo(mmio_space());
    if (pipe->pipe_id() != PipeId::PIPE_C) {
      auto pipe_scaler_1_regs = registers::PipeScalerRegs(pipe->pipe_id(), 1);
      pipe_scaler_1_regs.PipeScalerControlSkylake()
          .ReadFrom(mmio_space())
          .set_is_enabled(0)
          .WriteTo(mmio_space());
      pipe_scaler_1_regs.PipeScalerWindowSize().ReadFrom(mmio_space()).WriteTo(mmio_space());
    }

    // Disable the cursor watermark
    for (int wm_num = 0; wm_num < 8; wm_num++) {
      auto wm = pipe_regs.PlaneWatermark(0, wm_num).FromValue(0);
      wm.WriteTo(mmio_space());
    }

    // Disable the primary plane watermarks and reset their buffer allocation
    for (unsigned plane_num = 0; plane_num < registers::kImagePlaneCount; plane_num++) {
      for (int wm_num = 0; wm_num < 8; wm_num++) {
        auto wm = pipe_regs.PlaneWatermark(plane_num + 1, wm_num).FromValue(0);
        wm.WriteTo(mmio_space());
      }
    }
  }

  return true;
}

void Controller::ResetPipePlaneBuffers(PipeId pipe_id) {
  fbl::AutoLock lock(&plane_buffers_lock_);
  const uint16_t data_buffer_block_count = DataBufferBlockCount();
  for (unsigned plane_num = 0; plane_num < registers::kImagePlaneCount; plane_num++) {
    plane_buffers_[pipe_id][plane_num].start = data_buffer_block_count;
  }
}

bool Controller::ResetDdi(DdiId ddi_id, std::optional<TranscoderId> transcoder_id) {
  registers::DdiRegs ddi_regs(ddi_id);

  // Disable the port
  auto ddi_buffer_control = ddi_regs.BufferControl().ReadFrom(mmio_space());
  const bool was_enabled = ddi_buffer_control.enabled();
  ddi_buffer_control.set_enabled(false).WriteTo(mmio_space());

  if (!is_tgl(device_id_)) {
    auto dp_transport_control = ddi_regs.DpTransportControl().ReadFrom(mmio_space());
    dp_transport_control.set_enabled(false)
        .set_training_pattern(registers::DpTransportControl::kTrainingPattern1)
        .WriteTo(mmio_space());
  } else {
    if (transcoder_id.has_value()) {
      auto dp_transport_control =
          registers::DpTransportControl::GetForTigerLakeTranscoder(*transcoder_id)
              .ReadFrom(mmio_space());
      dp_transport_control.set_enabled(false)
          .set_training_pattern(registers::DpTransportControl::kTrainingPattern1)
          .WriteTo(mmio_space());
    }
  }

  if (was_enabled &&
      !display::PollUntil([&] { return ddi_buffer_control.ReadFrom(mmio_space()).is_idle(); },
                          zx::msec(1), 8)) {
    fdf::error("Port failed to go idle");
    return false;
  }

  // Disable IO power
  ZX_DEBUG_ASSERT(power_);
  power_->SetDdiIoPowerState(ddi_id, /* enable */ false);

  // Wait for DDI IO power to be fully disabled.
  // This step is not documented in Intel Display PRM, but this step occurs
  // in the drm/i915 driver and experiments on NUC11 hardware indicate that
  // display hotplug may fail without this step.
  if (!display::PollUntil([&] { return !power_->GetDdiIoPowerState(ddi_id); }, zx::usec(1), 1000)) {
    fdf::error("Disable IO power timeout");
    return false;
  }

  if (!dpll_manager_->ResetDdiPll(ddi_id)) {
    fdf::error("Failed to unmap DPLL for DDI {}", ddi_id);
    return false;
  }

  return true;
}

zx_status_t Controller::InitGttForTesting(const ddk::Pci& pci, fdf::MmioBuffer buffer,
                                          uint32_t fb_offset) {
  fbl::AutoLock gtt_lock(&gtt_lock_);
  return gtt_.Init(pci, std::move(buffer), fb_offset);
}

const GttRegion& Controller::SetupGttImage(
    const display::ImageMetadata& image_metadata, display::DriverImageId image_id,
    display::CoordinateTransformation coordinate_transformation) {
  const std::unique_ptr<GttRegionImpl>& region = GetGttRegionImpl(image_id);
  ZX_DEBUG_ASSERT(region);
  region->SetRotation(coordinate_transformation, image_metadata);
  return *region;
}

std::unique_ptr<DisplayDevice> Controller::QueryDisplay(DdiId ddi_id,
                                                        display::DisplayId display_id) {
  fbl::AllocChecker ac;
  if (!igd_opregion_.HasDdi(ddi_id)) {
    fdf::info("ddi {} not available.", ddi_id);
    return nullptr;
  }

  if (igd_opregion_.SupportsDp(ddi_id)) {
    fdf::debug("Checking for DisplayPort monitor at DDI {}", ddi_id);
    DdiReference ddi_reference_maybe = ddi_manager_->GetDdiReference(ddi_id);
    if (!ddi_reference_maybe) {
      fdf::debug("DDI {} PHY not available. Skip querying.", ddi_id);
    } else {
      auto dp_disp = fbl::make_unique_checked<DpDisplay>(
          &ac, this, display_id, ddi_id, &dp_aux_channels_[ddi_id], &pch_engine_.value(),
          std::move(ddi_reference_maybe), &root_node_);
      if (ac.check() && reinterpret_cast<DisplayDevice*>(dp_disp.get())->Query()) {
        return dp_disp;
      }
    }
  }
  if (igd_opregion_.SupportsHdmi(ddi_id) || igd_opregion_.SupportsDvi(ddi_id)) {
    fdf::debug("Checking for HDMI monitor at DDI {}", ddi_id);
    DdiReference ddi_reference_maybe = ddi_manager_->GetDdiReference(ddi_id);
    if (!ddi_reference_maybe) {
      fdf::debug("DDI {} PHY not available. Skip querying.", ddi_id);
    } else {
      auto hdmi_disp = fbl::make_unique_checked<HdmiDisplay>(
          &ac, this, display_id, ddi_id, std::move(ddi_reference_maybe), &gmbus_i2cs_[ddi_id]);
      if (ac.check() && reinterpret_cast<DisplayDevice*>(hdmi_disp.get())->Query()) {
        return hdmi_disp;
      }
    }
  }
  fdf::trace("Nothing found for ddi {}!", ddi_id);
  return nullptr;
}

bool Controller::LoadHardwareState(DdiId ddi_id, DisplayDevice* device) {
  registers::DdiRegs regs(ddi_id);

  if (!power_->GetDdiIoPowerState(ddi_id) ||
      !regs.BufferControl().ReadFrom(mmio_space()).enabled()) {
    return false;
  }

  DdiPllConfig pll_config = dpll_manager()->LoadState(ddi_id);
  if (pll_config.IsEmpty()) {
    fdf::error("Cannot load DPLL state for DDI {}", ddi_id);
    return false;
  }

  bool init_result = device->InitWithDdiPllConfig(pll_config);
  if (!init_result) {
    fdf::error("Cannot initialize the display with DPLL state for DDI {}", ddi_id);
    return false;
  }

  device->LoadActiveMode();
  return true;
}

void Controller::InitDisplays() {
  fbl::AutoLock lock(&display_lock_);
  BringUpDisplayEngine(false);

  if (!ReadMemoryLatencyInfo()) {
    return;
  }

  // This disables System Agent Geyserville (SAGV), which dynamically adjusts
  // the system agent voltage and clock frequencies depending on system power
  // and performance requirements.
  //
  // When SAGV is enabled, it could limit the display memory bandwidth (on Tiger
  // Lake+) and block the display engine from accessing system memory for a
  // certain amount of time (SAGV block time). Thus, SAGV must be disabled if
  // the display engine's memory latency exceeds the SAGV block time.
  //
  // Here, we unconditionally disable SAGV to guarantee the correctness of
  // the display engine memory accesses. However, this may cause the processor
  // to consume more power, even to the point of exceeding its thermal envelope.
  DisableSystemAgentGeyserville();

  for (const auto ddi_id : ddis_) {
    auto disp_device = QueryDisplay(ddi_id, next_id_);
    if (disp_device) {
      AddDisplay(std::move(disp_device));
    }
  }

  if (display_devices_.size() == 0) {
    fdf::info("intel-display: No displays detected.");
  }

  // Make a note of what needs to be reset, so we can finish querying the hardware state
  // before touching it, and so we can make sure transcoders are reset before ddis.
  std::vector<std::pair<DdiId, std::optional<TranscoderId>>> ddi_trans_needs_reset;
  std::vector<DisplayDevice*> device_needs_init;

  for (const auto ddi_id : ddis_) {
    DisplayDevice* device = nullptr;
    for (auto& display_device : display_devices_) {
      if (display_device->ddi_id() == ddi_id) {
        device = display_device.get();
        break;
      }
    }

    if (device == nullptr) {
      ddi_trans_needs_reset.emplace_back(ddi_id, std::nullopt);
    } else {
      if (!LoadHardwareState(ddi_id, device)) {
        auto transcoder_maybe = device->pipe()
                                    ? std::make_optional(device->pipe()->connected_transcoder_id())
                                    : std::nullopt;
        ddi_trans_needs_reset.emplace_back(ddi_id, transcoder_maybe);
        device_needs_init.push_back(device);
      } else {
        // On Tiger Lake, if a display device is already initialized by BIOS,
        // the pipe / transcoder / DDI should be all reset and reinitialized.
        // By doing this we can keep the display state fully controlled by the
        // driver.
        // TODO(https://fxbug.dev/42063039): Consider doing this on all platforms.
        if (is_tgl(device_id())) {
          device_needs_init.push_back(device);
        }
        device->InitBacklight();
      }
    }
  }

  // Reset any transcoders which aren't in use
  pipe_manager_->ResetInactiveTranscoders();

  // Reset any ddis which don't have a restored display. If we failed to restore a
  // display, try to initialize it here.
  for (const auto& [ddi, transcoder_maybe] : ddi_trans_needs_reset) {
    ResetDdi(ddi, transcoder_maybe);
  }

  for (DisplayDevice* device : device_needs_init) {
    ZX_ASSERT_MSG(device, "device_needs_init incorrectly populated above");
    for (unsigned i = 0; i < display_devices_.size(); i++) {
      if (display_devices_[i].get() == device) {
        if (is_tgl(device_id())) {
          // On Tiger Lake, devices pre-initialized by the BIOS must be reset
          // and reinitialized by the driver.
          // TODO(https://fxbug.dev/42063040): We should fix the device reset logic so
          // that we don't need to delete the old device.
          const DdiId ddi_id = device->ddi_id();
          const display::DisplayId display_id = device->id();
          display_devices_[i].reset();
          display_devices_[i] = QueryDisplay(ddi_id, display_id);
          device = display_devices_[i].get();
          if (!device || !device->Init()) {
            display_devices_.erase(i);
          }
        } else {
          if (!device->Init()) {
            display_devices_.erase(i);
          }
        }
        break;
      }
    }
  }
}

bool Controller::ReadMemoryLatencyInfo() {
  PowerController power_controller(&*mmio_space_);

  const zx::result<std::array<uint8_t, 8>> memory_latency =
      power_controller.GetRawMemoryLatencyDataUs();
  if (memory_latency.is_error()) {
    // We're not supposed to enable planes if we can't read the memory latency
    // data. This makes the display driver fairly useless, so bail.
    fdf::error("Error reading memory latency data from PCU firmware: {}", memory_latency);
    return false;
  }
  fdf::trace("Raw PCU memory latency data: {} {} {} {} {} {} {} {}", memory_latency.value()[0],
             memory_latency.value()[1], memory_latency.value()[2], memory_latency.value()[3],
             memory_latency.value()[4], memory_latency.value()[5], memory_latency.value()[6],
             memory_latency.value()[7]);

  // Pre-Tiger Lake, the SAGV blocking time is always modeled to 30us.
  const zx::result<uint32_t> blocking_time =
      is_tgl(device_id_) ? power_controller.GetSystemAgentBlockTimeUsTigerLake()
                         : power_controller.GetSystemAgentBlockTimeUsKabyLake();
  if (blocking_time.is_error()) {
    // We're not supposed to enable planes if we can't read the SAGV blocking
    // time. This makes the display driver fairly useless, so bail.
    fdf::error("Error reading SAGV blocking time from PCU firmware: {}", blocking_time);
    return false;
  }
  fdf::trace("System Agent Geyserville blocking time: {}", blocking_time.value());

  // The query below is only supported on Tiger Lake PCU firmware.
  if (!is_tgl(device_id_)) {
    return true;
  }

  const zx::result<MemorySubsystemInfo> memory_info =
      power_controller.GetMemorySubsystemInfoTigerLake();
  if (memory_info.is_error()) {
    // We can handle this error by unconditionally disabling SAGV.
    fdf::error("Error reading SAGV QGV point info from PCU firmware: {}", blocking_time);
    return true;
  }

  const MemorySubsystemInfo::GlobalInfo& global_info = memory_info.value().global_info;
  fdf::trace("PCU memory subsystem info: DRAM type {}, {} channels, {} SAGV points",
             static_cast<int>(global_info.ram_type), global_info.memory_channel_count,
             global_info.agent_point_count);
  for (int point_index = 0; point_index < global_info.agent_point_count; ++point_index) {
    const MemorySubsystemInfo::AgentPoint& point_info = memory_info.value().points[point_index];
    fdf::trace("SAGV point {} info: DRAM clock {} kHz, tRP {}, tRCD {}, tRDPRE {}, tRAS {}",
               point_index, point_info.dram_clock_khz, point_info.row_precharge_to_open_cycles,
               point_info.row_access_to_column_access_delay_cycles,
               point_info.read_to_precharge_cycles, point_info.row_activate_to_precharge_cycles);
  }
  return true;
}

void Controller::DisableSystemAgentGeyserville() {
  PowerController power_controller(&*mmio_space_);

  const zx::result<> sagv_disabled = power_controller.SetSystemAgentGeyservilleEnabled(
      false, PowerController::RetryBehavior::kRetryUntilStateChanges);
  if (sagv_disabled.is_error()) {
    fdf::error("Failed to disable System Agent Geyserville. Display corruption may occur.");
    return;
  }
  fdf::trace("System Agent Geyserville disabled.");
}

void Controller::RemoveDisplay(std::unique_ptr<DisplayDevice> display) {
  // Make sure the display's resources get freed before reallocating the pipe buffers by letting
  // "display" go out of scope.
}

zx_status_t Controller::AddDisplay(std::unique_ptr<DisplayDevice> display) {
  const display::DisplayId display_id = display->id();

  // Add the new device.
  fbl::AllocChecker ac;
  display_devices_.push_back(std::move(display), &ac);
  if (!ac.check()) {
    fdf::warn("Failed to add display device");
    return ZX_ERR_NO_MEMORY;
  }

  fdf::info("Display {} connected", display_id.value());
  next_id_++;
  return ZX_OK;
}

// DisplayEngine methods

display::EngineInfo Controller::CompleteCoordinatorConnection() {
  fbl::AutoLock lock(&display_lock_);

  if (!display_devices_.is_empty()) {
    for (const std::unique_ptr<DisplayDevice>& display_device : display_devices_) {
      const AddedDisplayInfo added_display_info = display_device->CreateAddedDisplayInfo();
      engine_events_.OnDisplayAdded(added_display_info.display_id,
                                    added_display_info.preferred_modes, kPixelFormatTypes);
    }
  }

  return display::EngineInfo({
      // Each Tiger Lake pipe supports at most 8 layers (7 planes + 1 cursor).
      //
      // The total limit equals the pipe limit while we only support a single display. This
      // limit must be revised when we implement multi-display support.
      .max_layer_count = 8,
      .max_connected_display_count = 1,
      .is_capture_supported = false,
  });
}

static bool IsSupportedPixelFormatForLinearTiling(
    fuchsia_sysmem2::wire::ImageFormatConstraints constraints) {
  const auto& format = constraints.pixel_format();
  if (format != fuchsia_images2::wire::PixelFormat::kB8G8R8A8 &&
      format != fuchsia_images2::wire::PixelFormat::kR8G8B8A8) {
    return false;
  }

  if (!constraints.has_pixel_format_modifier()) {
    return false;
  }

  return true;
}

zx::result<> Controller::ImportBufferCollection(
    display::DriverBufferCollectionId buffer_collection_id,
    fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> buffer_collection_token) {
  if (buffer_collections_.find(buffer_collection_id) != buffer_collections_.end()) {
    fdf::error("Buffer Collection (id={}) already exists", buffer_collection_id.value());
    return zx::error(ZX_ERR_ALREADY_EXISTS);
  }

  ZX_DEBUG_ASSERT_MSG(sysmem_.is_valid(), "sysmem allocator is not initialized");

  auto [collection_client_endpoint, collection_server_endpoint] =
      fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();

  fidl::Arena arena;
  auto bind_result = sysmem_->BindSharedCollection(
      fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
          .buffer_collection_request(std::move(collection_server_endpoint))
          .token(fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken>(
              std::move(buffer_collection_token)))
          .Build());
  if (!bind_result.ok()) {
    fdf::error("Cannot complete FIDL call BindSharedCollection: {}", bind_result.status_string());
    return zx::error(ZX_ERR_INTERNAL);
  }

  buffer_collections_[buffer_collection_id] =
      fidl::WireSyncClient(std::move(collection_client_endpoint));
  return zx::ok();
}

zx::result<> Controller::ReleaseBufferCollection(
    display::DriverBufferCollectionId buffer_collection_id) {
  if (buffer_collections_.find(buffer_collection_id) == buffer_collections_.end()) {
    fdf::error("Cannot release buffer collection {}: buffer collection doesn't exist",
               buffer_collection_id.value());
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  buffer_collections_.erase(buffer_collection_id);
  return zx::ok();
}

zx::result<display::DriverImageId> Controller::ImportImage(
    const display::ImageMetadata& image_metadata,
    display::DriverBufferCollectionId buffer_collection_id, uint32_t buffer_index) {
  const auto it = buffer_collections_.find(buffer_collection_id);
  if (it == buffer_collections_.end()) {
    fdf::error("ImportImage: Cannot find imported buffer collection (id={})",
               buffer_collection_id.value());
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  const fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection>& collection = it->second;

  if (image_metadata.tiling_type() != display::ImageTilingType::kLinear) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  fidl::WireResult check_result = collection->CheckAllBuffersAllocated();
  // TODO(https://fxbug.dev/42072690): The sysmem FIDL error logging patterns are
  // inconsistent across drivers. The FIDL error handling and logging should be
  // unified.
  if (!check_result.ok()) {
    fdf::error("Failed to check buffers allocated, {}", check_result.FormatDescription());
    return zx::error(check_result.status());
  }
  const auto& check_response = check_result.value();
  if (check_response.is_error()) {
    if (check_response.error_value() == fuchsia_sysmem2::wire::Error::kPending) {
      return zx::error(ZX_ERR_SHOULD_WAIT);
    }
    return zx::error(sysmem::V1CopyFromV2Error(check_response.error_value()));
  }

  fidl::WireResult wait_result = collection->WaitForAllBuffersAllocated();
  // TODO(https://fxbug.dev/42072690): The sysmem FIDL error logging patterns are
  // inconsistent across drivers. The FIDL error handling and logging should be
  // unified.
  if (!wait_result.ok()) {
    fdf::error("Failed to wait for buffers allocated, {}", wait_result.FormatDescription());
    return zx::error(wait_result.status());
  }
  const auto& wait_response = wait_result.value();
  if (wait_response.is_error()) {
    if (wait_response.error_value() == fuchsia_sysmem2::wire::Error::kPending) {
      return zx::error(ZX_ERR_SHOULD_WAIT);
    }
    return zx::error(sysmem::V1CopyFromV2Error(wait_response.error_value()));
  }
  const auto& wait_value = wait_response.value();
  fuchsia_sysmem2::wire::BufferCollectionInfo& collection_info =
      wait_value->buffer_collection_info();

  if (!collection_info.settings().has_image_format_constraints()) {
    fdf::error("No image format constraints");
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  if (buffer_index >= collection_info.buffers().size()) {
    fdf::error("Invalid index {} greater than buffer count {}", buffer_index,
               collection_info.buffers().size());
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }

  zx::vmo vmo = std::move(collection_info.buffers().at(buffer_index).vmo());

  const uint64_t page_size = zx_system_get_page_size();
  uint64_t offset = collection_info.buffers().at(buffer_index).vmo_usable_start();
  if (offset % page_size != 0) {
    fdf::error("Invalid offset");
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  ZX_DEBUG_ASSERT(collection_info.settings().image_format_constraints().pixel_format() !=
                      fuchsia_images2::wire::PixelFormat::kI420 &&
                  collection_info.settings().image_format_constraints().pixel_format() !=
                      fuchsia_images2::wire::PixelFormat::kNv12);

  ZX_DEBUG_ASSERT(image_metadata.tiling_type() == display::ImageTilingType::kLinear);
  if (!IsSupportedPixelFormatForLinearTiling(
          collection_info.settings().image_format_constraints())) {
    fdf::error("Invalid pixel format modifier");
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  fbl::AutoLock lock(&gtt_lock_);
  fbl::AllocChecker ac;
  imported_images_.reserve(imported_images_.size() + 1, &ac);
  if (!ac.check()) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  fidl::Arena arena;
  auto format =
      ImageConstraintsToFormat(arena, collection_info.settings().image_format_constraints(),
                               static_cast<uint32_t>(image_metadata.dimensions().width()),
                               static_cast<uint32_t>(image_metadata.dimensions().height()));
  if (!format.is_ok()) {
    fdf::error("Failed to get format from constraints");
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  const uint32_t length = [&]() {
    const uint64_t length = ImageFormatImageSize(format.value());
    ZX_DEBUG_ASSERT_MSG(length <= std::numeric_limits<uint32_t>::max(), "%lu overflows uint32_t",
                        length);
    return static_cast<uint32_t>(length);
  }();

  const uint32_t bytes_per_pixel =
      ImageFormatStrideBytesPerWidthPixel(PixelFormatAndModifierFromImageFormat(format.value()));

  ZX_DEBUG_ASSERT(length >= width_in_tiles(image_metadata.tiling_type(),
                                           image_metadata.dimensions().width(), bytes_per_pixel) *
                                height_in_tiles(image_metadata.tiling_type(),
                                                image_metadata.dimensions().height()) *
                                get_tile_byte_size(image_metadata.tiling_type()));

  ZX_DEBUG_ASSERT(image_metadata.tiling_type() == display::ImageTilingType::kLinear);
  static constexpr uint32_t kPlaneSurfaceAlignment = registers::PlaneSurface::kLinearAlignment;
  std::unique_ptr<GttRegionImpl> gtt_region;
  zx_status_t status = gtt_.AllocRegion(length, kPlaneSurfaceAlignment, &gtt_region);
  if (status != ZX_OK) {
    fdf::error("Failed to allocate GTT region, status {}", zx::make_result(status));
    return zx::error(status);
  }

  // The vsync logic requires that images not have base == 0
  if (gtt_region->base() == 0) {
    std::unique_ptr<GttRegionImpl> alt_gtt_region;
    zx_status_t status = gtt_.AllocRegion(length, kPlaneSurfaceAlignment, &alt_gtt_region);
    if (status != ZX_OK) {
      return zx::error(status);
    }
    gtt_region = std::move(alt_gtt_region);
  }

  status = gtt_region->PopulateRegion(vmo.release(), offset / page_size, length);
  if (status != ZX_OK) {
    fdf::error("Failed to populate GTT region, status {}", zx::make_result(status));
    return zx::error(status);
  }

  gtt_region->set_bytes_per_row(format.value().bytes_per_row());
  const display::DriverImageId image_id(gtt_region->base());
  imported_images_.push_back(std::move(gtt_region));

  ZX_DEBUG_ASSERT_MSG(
      imported_image_pixel_formats_.find(image_id) == imported_image_pixel_formats_.end(),
      "Image ID %" PRIu64 " exists in imported image pixel formats map", image_id.value());
  imported_image_pixel_formats_.emplace(image_id,
                                        PixelFormatAndModifierFromImageFormat(format.value()));

  return zx::ok(image_id);
}

zx::result<display::DriverCaptureImageId> Controller::ImportImageForCapture(
    display::DriverBufferCollectionId buffer_collection_id, uint32_t buffer_index) {
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

void Controller::ReleaseImage(display::DriverImageId driver_image_id) {
  fbl::AutoLock lock(&gtt_lock_);
  imported_image_pixel_formats_.erase(driver_image_id);
  for (unsigned i = 0; i < imported_images_.size(); i++) {
    if (imported_images_[i]->base() == driver_image_id.value()) {
      imported_images_[i]->ClearRegion();
      imported_images_.erase(i);
      return;
    }
  }
}

PixelFormatAndModifier Controller::GetImportedImagePixelFormat(
    display::DriverImageId image_id) const {
  fbl::AutoLock lock(&gtt_lock_);
  auto it = imported_image_pixel_formats_.find(image_id);
  if (it != imported_image_pixel_formats_.end()) {
    return it->second;
  }
  ZX_ASSERT_MSG(false, "Imported image ID %" PRIu64 " not found", image_id.value());
}

const std::unique_ptr<GttRegionImpl>& Controller::GetGttRegionImpl(
    display::DriverImageId image_id) {
  fbl::AutoLock lock(&gtt_lock_);
  for (auto& region : imported_images_) {
    if (region->base() == image_id.value()) {
      return region;
    }
  }
  ZX_ASSERT(false);
}

Pipe* Controller::GetPipeForDisplay(display::DisplayId display_id) {
  for (Pipe* pipe : *pipe_manager_) {
    if (!pipe->in_use()) {
      continue;
    }
    if (pipe->attached_display_id() == display_id) {
      return pipe;
    }
  }
  return nullptr;
}

std::optional<display::DriverLayer> Controller::GetDriverLayerForPlane(
    uint32_t plane_id, std::span<const display::DriverLayer> layers) {
  if (layers.empty()) {
    return std::nullopt;
  }
  bool has_color_layer =
      layers[0].image_source().width() == 0 || layers[0].image_source().height() == 0;
  for (size_t layer_index = 0; layer_index < layers.size(); ++layer_index) {
    const display::DriverLayer& layer = layers[layer_index];
    if (layer.image_source().width() != 0 && layer.image_source().height() != 0) {
      if (plane_id + (has_color_layer ? 1 : 0) != layer_index) {
        continue;
      }
    } else {
      // Solid color fill layers don't use planes.
      continue;
    }
    return layers[layer_index];
  }
  return std::nullopt;
}

uint16_t Controller::CalculateBuffersPerPipe(size_t active_pipe_count) {
  ZX_ASSERT(active_pipe_count < PipeIds<registers::Platform::kKabyLake>().size());
  return DataBufferBlockCount() / active_pipe_count;
}

bool Controller::CalculateMinimumAllocations(
    display::DisplayId display_id, std::span<const display::DriverLayer> layers,
    uint16_t min_allocs[PipeIds<registers::Platform::kKabyLake>().size()]
                       [registers::kImagePlaneCount]) {
  // This fn ignores layers after kImagePlaneCount. Displays with too many layers already
  // failed in ::CheckConfiguration, so it doesn't matter if we incorrectly say they pass here.
  bool success = true;
  for (Pipe* pipe : *pipe_manager_) {
    PipeId pipe_id = pipe->pipe_id();
    uint32_t total = 0;

    if (!pipe->in_use() || pipe->attached_display_id() != display_id) {
      std::ranges::fill(std::span(min_allocs[pipe_id], registers::kImagePlaneCount), 0);
      continue;
    }

    for (unsigned plane_num = 0; plane_num < registers::kImagePlaneCount; plane_num++) {
      std::optional<display::DriverLayer> layer_maybe = GetDriverLayerForPlane(plane_num, layers);
      if (!layer_maybe.has_value()) {
        min_allocs[pipe_id][plane_num] = 0;
        continue;
      }
      display::DriverLayer layer = std::move(layer_maybe).value();
      ZX_DEBUG_ASSERT_MSG(layer.image_metadata().tiling_type() == display::ImageTilingType::kLinear,
                          "Unsupported tiling type: %" PRIu32,
                          layer.image_metadata().tiling_type().ValueForLogging());

      ZX_ASSERT(layer.image_source().width() != 0);
      ZX_ASSERT(layer.image_source().height() != 0);

      min_allocs[pipe_id][plane_num] = 8;
      total += min_allocs[pipe_id][plane_num];
    }

    if (total && total > CalculateBuffersPerPipe(/*active_pipe_count=*/1)) {
      min_allocs[pipe_id][0] = UINT16_MAX;
      success = false;
    }
  }

  return success;
}

void Controller::UpdateAllocations(
    const uint16_t min_allocs[PipeIds<registers::Platform::kKabyLake>().size()]
                             [registers::kImagePlaneCount],
    const uint64_t data_rate_bytes_per_frame[PipeIds<registers::Platform::kKabyLake>().size()]
                                            [registers::kImagePlaneCount]) {
  uint16_t allocs[PipeIds<registers::Platform::kKabyLake>().size()][registers::kImagePlaneCount];

  for (unsigned pipe_num = 0; pipe_num < PipeIds<registers::Platform::kKabyLake>().size();
       pipe_num++) {
    uint64_t total_data_rate_bytes_per_frame = 0;
    for (unsigned plane_num = 0; plane_num < registers::kImagePlaneCount; plane_num++) {
      total_data_rate_bytes_per_frame += data_rate_bytes_per_frame[pipe_num][plane_num];
    }
    if (total_data_rate_bytes_per_frame == 0) {
      for (unsigned plane_num = 0; plane_num < registers::kImagePlaneCount; plane_num++) {
        allocs[pipe_num][plane_num] = 0;
      }
      continue;
    }

    // Allocate buffers based on the percentage of the total pixel bandwidth they take. If
    // that percentage isn't enough for a plane, give that plane its minimum allocation and
    // then try again.
    double buffers_per_pipe = pipe_buffers_[pipe_num].end - pipe_buffers_[pipe_num].start;
    bool forced_alloc[registers::kImagePlaneCount] = {};
    bool done = false;
    while (!done) {
      for (unsigned plane_num = 0; plane_num < registers::kImagePlaneCount; plane_num++) {
        if (forced_alloc[plane_num]) {
          continue;
        }

        double blocks = buffers_per_pipe *
                        static_cast<double>(data_rate_bytes_per_frame[pipe_num][plane_num]) /
                        static_cast<double>(total_data_rate_bytes_per_frame);
        allocs[pipe_num][plane_num] = static_cast<uint16_t>(blocks);
      }

      done = true;

      for (unsigned plane_num = 0; plane_num < registers::kImagePlaneCount; plane_num++) {
        if (allocs[pipe_num][plane_num] < min_allocs[pipe_num][plane_num]) {
          done = false;
          allocs[pipe_num][plane_num] = min_allocs[pipe_num][plane_num];
          forced_alloc[plane_num] = true;
          total_data_rate_bytes_per_frame -= data_rate_bytes_per_frame[pipe_num][plane_num];
          buffers_per_pipe -= allocs[pipe_num][plane_num];
        }
      }
    }
  }

  // Do the actual allocation, using the buffers that are assigned to each pipe.
  {
    fbl::AutoLock lock(&plane_buffers_lock_);
    const uint16_t data_buffer_block_count = DataBufferBlockCount();
    for (unsigned pipe_num = 0; pipe_num < PipeIds<registers::Platform::kKabyLake>().size();
         pipe_num++) {
      uint16_t start = pipe_buffers_[pipe_num].start;
      for (unsigned plane_num = 0; plane_num < registers::kImagePlaneCount; plane_num++) {
        auto cur = &plane_buffers_[pipe_num][plane_num];

        if (allocs[pipe_num][plane_num] == 0) {
          cur->start = data_buffer_block_count;
          cur->end = static_cast<uint16_t>(cur->start + 1);
        } else {
          cur->start = start;
          cur->end = static_cast<uint16_t>(start + allocs[pipe_num][plane_num]);
        }
        start = static_cast<uint16_t>(start + allocs[pipe_num][plane_num]);

        PipeId pipe_id = PipeIds<registers::Platform::kKabyLake>()[pipe_num];
        registers::PipeRegs pipe_regs(pipe_id);

        // These are latched on the surface address register, so we don't yet need to
        // worry about overlaps when updating planes during a pipe allocation.
        auto buf_cfg = pipe_regs.PlaneBufCfg(plane_num + 1).FromValue(0);
        buf_cfg.set_buffer_start(cur->start);
        buf_cfg.set_buffer_end(cur->end - 1);
        buf_cfg.WriteTo(mmio_space());

        // TODO(fxbug.com/111420): Follow the "Display Watermarks" guidelines.
        auto wm0 = pipe_regs.PlaneWatermark(plane_num + 1, 0).FromValue(0);
        wm0.set_enable(cur->start != data_buffer_block_count);
        wm0.set_blocks(cur->end - cur->start);
        wm0.WriteTo(mmio_space());

        // Give the buffers to both the cursor plane and plane 2, since
        // only one will actually be active.
        if (plane_num == registers::kCursorPlane) {
          auto buf_cfg = pipe_regs.PlaneBufCfg(0).FromValue(0);
          buf_cfg.set_buffer_start(cur->start);
          buf_cfg.set_buffer_end(cur->end - 1);
          buf_cfg.WriteTo(mmio_space());

          auto wm0 = pipe_regs.PlaneWatermark(0, 0).FromValue(0);
          wm0.set_enable(cur->start != data_buffer_block_count);
          wm0.set_blocks(cur->end - cur->start);
          wm0.WriteTo(mmio_space());
        }
      }
    }
  }
}

void Controller::ReallocatePlaneBuffers(display::DisplayId display_id,
                                        std::span<const display::DriverLayer> layers,
                                        bool reallocate_pipes) {
  uint16_t min_allocs[PipeIds<registers::Platform::kKabyLake>().size()]
                     [registers::kImagePlaneCount];
  if (!CalculateMinimumAllocations(display_id, layers, min_allocs)) {
    // The allocation should have been checked, so this shouldn't fail
    ZX_ASSERT(false);
  }

  // Calculate the data rates and store the minimum allocations
  uint64_t data_rate_bytes_per_frame[PipeIds<registers::Platform::kKabyLake>().size()]
                                    [registers::kImagePlaneCount];
  for (Pipe* pipe : *pipe_manager_) {
    PipeId pipe_id = pipe->pipe_id();
    if (!pipe->in_use() || pipe->attached_display_id() != display_id) {
      std::span data_rate_bytes_per_frame_for_pipe(data_rate_bytes_per_frame[pipe_id],
                                                   registers::kImagePlaneCount);
      std::ranges::fill(data_rate_bytes_per_frame_for_pipe, 0);
      continue;
    }

    for (unsigned plane_num = 0; plane_num < registers::kImagePlaneCount; plane_num++) {
      std::optional<display::DriverLayer> layer_maybe = GetDriverLayerForPlane(plane_num, layers);
      if (!layer_maybe.has_value()) {
        data_rate_bytes_per_frame[pipe_id][plane_num] = 0;
      } else {
        display::DriverLayer layer = std::move(layer_maybe).value();
        // Color fill layers don't use planes, so GetPlaneLayer() should have returned false.
        ZX_ASSERT(layer.image_source().width() != 0);
        ZX_ASSERT(layer.image_source().height() != 0);

        uint32_t scaled_width = layer.image_source().width() * layer.image_source().width() /
                                layer.display_destination().width();
        uint32_t scaled_height = layer.image_source().height() * layer.image_source().height() /
                                 layer.display_destination().height();

        // TODO(https://fxbug.dev/42076788): Currently we assume only RGBA/BGRA formats
        // are supported and hardcode the bytes-per-pixel value to avoid pixel
        // format check and stride calculation (which requires holding the GTT
        // lock). This may change when we need to support non-RGBA/BGRA images.
        constexpr int bytes_per_pixel = 4;
        // Plane buffers are recalculated only on valid configurations. So all
        // images must be valid.
        const display::DriverImageId primary_image_id = layer.image_id();
        ZX_DEBUG_ASSERT(primary_image_id != display::kInvalidDriverImageId);
        ZX_DEBUG_ASSERT(bytes_per_pixel == ImageFormatStrideBytesPerWidthPixel(
                                               GetImportedImagePixelFormat(primary_image_id)));

        data_rate_bytes_per_frame[pipe_id][plane_num] =
            uint64_t{scaled_width} * scaled_height * bytes_per_pixel;
      }
    }
  }

  if (initial_alloc_) {
    initial_alloc_ = false;
    reallocate_pipes = true;
  }

  buffer_allocation_t active_allocation[PipeIds<registers::Platform::kKabyLake>().size()];
  if (reallocate_pipes) {
    // Allocate buffers to each pipe, but save the old allocation to use
    // when progressively updating the allocation.
    memcpy(active_allocation, pipe_buffers_, sizeof(active_allocation));

    size_t active_pipes = std::count_if(pipe_manager_->begin(), pipe_manager_->end(),
                                        [](const Pipe* pipe) { return pipe->in_use(); });
    uint16_t buffers_per_pipe = CalculateBuffersPerPipe(active_pipes);

    int current_active_pipe = 0;
    for (Pipe* pipe : *pipe_manager_) {
      PipeId pipe_id = pipe->pipe_id();
      if (pipe->in_use()) {
        pipe_buffers_[pipe_id].start =
            static_cast<uint16_t>(buffers_per_pipe * current_active_pipe);
        pipe_buffers_[pipe_id].end =
            static_cast<uint16_t>(pipe_buffers_[pipe_id].start + buffers_per_pipe);
        current_active_pipe++;
      } else {
        pipe_buffers_[pipe_id].start = pipe_buffers_[pipe_id].end = 0;
      }
      fdf::info("Pipe {} buffers: [{}, {})", pipe_id, pipe_buffers_[pipe_id].start,
                pipe_buffers_[pipe_id].end);
    }
  }

  // It's not necessary to flush the buffer changes since the pipe allocs didn't change
  UpdateAllocations(min_allocs, data_rate_bytes_per_frame);

  if (reallocate_pipes) {
    DoPipeBufferReallocation(active_allocation);
  }
}

void Controller::DoPipeBufferReallocation(
    buffer_allocation_t active_allocation[PipeIds<registers::Platform::kKabyLake>().size()]) {
  // Given that the order of the allocations is fixed, an allocation X_i is contained completely
  // within its old allocation if {new len of allocations preceding X_i} >= {start of old X_i} and
  // {new len of allocations preceding X_i + new len of X_i} <= {end of old X_i}. For any i,
  // if condition 1 holds, either condition 2 is true and we're done, or condition 2 doesn't
  // and condition 1 holds for i + 1. Since condition 1 holds for i == 0 and because condition
  // 2 holds for the last allocation (since the allocation is valid), it is guaranteed that
  // at least one allocation is entirely within its old allocation. The remaining buffers
  // are guaranteed to be re-allocatable recursively in the same manner. Therefore the loop will
  // make progress every iteration.
  bool done = false;
  while (!done) {
    done = true;
    for (unsigned pipe_num = 0; pipe_num < PipeIds<registers::Platform::kKabyLake>().size();
         pipe_num++) {
      auto active_alloc = active_allocation + pipe_num;
      auto goal_alloc = pipe_buffers_ + pipe_num;

      if (active_alloc->start == goal_alloc->start && active_alloc->end == goal_alloc->end) {
        continue;
      }

      // Look through all the other active pipe allocations for overlap
      bool overlap = false;
      if (goal_alloc->start != goal_alloc->end) {
        for (unsigned other_pipe = 0; other_pipe < PipeIds<registers::Platform::kKabyLake>().size();
             other_pipe++) {
          if (other_pipe == pipe_num) {
            continue;
          }

          auto other_active = active_allocation + other_pipe;
          if (other_active->start == other_active->end) {
            continue;
          }

          if ((other_active->start <= goal_alloc->start && goal_alloc->start < other_active->end) ||
              (other_active->start < goal_alloc->end && goal_alloc->end <= other_active->end)) {
            overlap = true;
            break;
          }
        }
      }

      if (!overlap) {
        // Flush the pipe allocation, wait for it to be active, and update
        // what is current active.
        registers::PipeRegs pipe_regs(PipeIds<registers::Platform::kKabyLake>()[pipe_num]);
        for (unsigned j = 0; j < registers::kImagePlaneCount; j++) {
          pipe_regs.PlaneSurface(j).ReadFrom(mmio_space()).WriteTo(mmio_space());
        }
        pipe_regs.CursorBase().ReadFrom(mmio_space()).WriteTo(mmio_space());

        // TODO(stevensd): Wait for vsync instead of sleeping
        // TODO(stevesnd): Parallelize/reduce the number of vsyncs we wait for
        zx_nanosleep(zx_deadline_after(ZX_MSEC(33)));

        *active_alloc = *goal_alloc;
      } else {
        done = false;
      }
    }
  }
}

bool Controller::CheckDisplayLimits(display::DisplayId display_id,
                                    const display::DisplayTiming& display_timing,
                                    std::span<const display::DriverLayer> layers) {
  // The intel display controller doesn't support these flags
  if (display_timing.vblank_alternates) {
    return false;
  }
  if (display_timing.pixel_repetition > 0) {
    return false;
  }

  DisplayDevice* display = FindDevice(display_id);
  if (display == nullptr) {
    return true;
  }

  // Pipes don't support height of more than 4096. They support a width of up to
  // 2^14 - 1. However, planes don't support a width of more than 8192 and we need
  // to always be able to accept a single plane, fullscreen configuration.
  if (display_timing.vertical_active_lines > 4096 || display_timing.horizontal_active_px > 8192) {
    return false;
  }

  int64_t max_pipe_pixel_rate_hz;
  auto cd_freq_khz = registers::CdClockCtl::Get().ReadFrom(mmio_space()).cd_freq_decimal();

  if (cd_freq_khz == registers::CdClockCtl::FreqDecimal(307'200)) {
    max_pipe_pixel_rate_hz = 307'200'000;
  } else if (cd_freq_khz == registers::CdClockCtl::FreqDecimal(308'570)) {
    max_pipe_pixel_rate_hz = 308'570'000;
  } else if (cd_freq_khz == registers::CdClockCtl::FreqDecimal(337'500)) {
    max_pipe_pixel_rate_hz = 337'500'000;
  } else if (cd_freq_khz == registers::CdClockCtl::FreqDecimal(432'000)) {
    max_pipe_pixel_rate_hz = 432'000'000;
  } else if (cd_freq_khz == registers::CdClockCtl::FreqDecimal(450'000)) {
    max_pipe_pixel_rate_hz = 450'000'000;
  } else if (cd_freq_khz == registers::CdClockCtl::FreqDecimal(540'000)) {
    max_pipe_pixel_rate_hz = 540'000'000;
  } else if (cd_freq_khz == registers::CdClockCtl::FreqDecimal(617'140)) {
    max_pipe_pixel_rate_hz = 617'140'000;
  } else if (cd_freq_khz == registers::CdClockCtl::FreqDecimal(675'000)) {
    max_pipe_pixel_rate_hz = 675'000'000;
  } else {
    ZX_ASSERT(false);
  }

  // Either the pipe pixel rate or the link pixel rate can't support a simple
  // configuration at this display resolution.
  const int64_t pixel_clock_hz = display_timing.pixel_clock_frequency_hz;
  if (max_pipe_pixel_rate_hz < pixel_clock_hz || !display->CheckPixelRate(pixel_clock_hz)) {
    return false;
  }

  // Compute the maximum pipe pixel rate with the desired scaling. If the max rate
  // is too low, then make the client do any downscaling itself.
  double min_plane_ratio = 1.0;
  for (const display::DriverLayer& layer : layers) {
    if (layer.image_source().width() == 0 || layer.image_source().height() == 0) {
      continue;
    }
    uint32_t src_width, src_height;
    GetPostTransformWidth(layer, &src_width, &src_height);

    double downscale = std::max(1.0, 1.0 * src_height / layer.display_destination().height()) *
                       std::max(1.0, 1.0 * src_width / layer.display_destination().width());
    double plane_ratio = 1.0 / downscale;
    min_plane_ratio = std::min(plane_ratio, min_plane_ratio);
  }

  max_pipe_pixel_rate_hz =
      static_cast<int64_t>(min_plane_ratio * static_cast<double>(max_pipe_pixel_rate_hz));
  if (max_pipe_pixel_rate_hz < pixel_clock_hz) {
    for (const display::DriverLayer& layer : layers) {
      if (layer.image_source().width() == 0 || layer.image_source().height() == 0) {
        continue;
      }
      uint32_t src_width, src_height;
      GetPostTransformWidth(layer, &src_width, &src_height);
    }
  }

  return true;
}

display::ConfigCheckResult Controller::CheckConfiguration(
    display::DisplayId display_id, display::ModeId display_mode_id,
    display::ColorConversion color_conversion, std::span<const display::DriverLayer> layers) {
  fbl::AutoLock lock(&display_lock_);

  std::array<display::DisplayId, PipeIds<registers::Platform::kKabyLake>().size()>
      display_allocated_to_pipe;
  if (!CalculatePipeAllocation(display_id, display_allocated_to_pipe)) {
    return display::ConfigCheckResult::kTooManyDisplays;
  }

  DisplayDevice* display = nullptr;
  for (auto& d : display_devices_) {
    if (d->id() == display_id) {
      display = d.get();
      break;
    }
  }
  if (display == nullptr) {
    fdf::info("Got config with no display - assuming hotplug and skipping");
    return display::ConfigCheckResult::kOk;
  }

  std::optional<display::DisplayTiming> get_timing_result =
      display->GetDisplayTiming(display_mode_id);
  if (!get_timing_result.has_value()) {
    return display::ConfigCheckResult::kUnsupportedDisplayModes;
  }
  display::DisplayTiming display_timing = std::move(get_timing_result).value();

  if (!CheckDisplayLimits(display_id, display_timing, layers)) {
    return display::ConfigCheckResult::kUnsupportedDisplayModes;
  }

  // This overrides all checks about multi-layers.
  if (layers.size() > 1) {
    return display::ConfigCheckResult::kUnsupportedConfig;
  }

  static constexpr int kMaxPlaneCount = 3;
  if (layers.size() > kMaxPlaneCount + 1) {
    return display::ConfigCheckResult::kUnsupportedConfig;
  }

  bool layer0_is_solid_color_fill = (layers[0].image_metadata().dimensions().width() == 0 ||
                                     layers[0].image_metadata().dimensions().height() == 0);
  if (layers.size() == kMaxPlaneCount + 1 && !layer0_is_solid_color_fill) {
    return display::ConfigCheckResult::kUnsupportedConfig;
  }

  for (float preoffset : color_conversion.preoffsets()) {
    if (preoffset <= -1) {
      return display::ConfigCheckResult::kUnsupportedConfig;
    }
    if (preoffset >= 1) {
      return display::ConfigCheckResult::kUnsupportedConfig;
    }
  }
  for (float postoffset : color_conversion.postoffsets()) {
    if (postoffset <= -1) {
      return display::ConfigCheckResult::kUnsupportedConfig;
    }
    if (postoffset >= 1) {
      return display::ConfigCheckResult::kUnsupportedConfig;
    }
  }

  uint32_t total_scalers_needed = 0;
  for (const display::DriverLayer& layer : layers) {
    if (layer.image_metadata().dimensions().width() != 0 &&
        layer.image_metadata().dimensions().height() != 0) {
      if (layer.image_source_transformation() != display::CoordinateTransformation::kIdentity) {
        // Image rotation is not supported.
        return display::ConfigCheckResult::kUnsupportedConfig;
      }
      if (layer.image_metadata().tiling_type() != display::ImageTilingType::kLinear) {
        // Only linear tiling is supported.
        return display::ConfigCheckResult::kUnsupportedConfig;
      }

      uint32_t src_width, src_height;
      GetPostTransformWidth(layer, &src_width, &src_height);

      // If the plane is too wide, force the client to do all composition
      // and just give us a simple configuration.
      ZX_DEBUG_ASSERT(layer.image_metadata().tiling_type() == display::ImageTilingType::kLinear);
      static constexpr uint32_t kLinearLayerMaxWidth = 8192;
      if (src_width > kLinearLayerMaxWidth) {
        return display::ConfigCheckResult::kUnsupportedConfig;
      }

      if (layer.display_destination().width() != static_cast<int32_t>(src_width) ||
          layer.display_destination().height() != static_cast<int32_t>(src_height)) {
        float ratio = registers::PipeScalerControlSkylake::k7x5MaxRatio;
        int32_t max_width = static_cast<int32_t>(static_cast<float>(src_width) * ratio);
        int32_t max_height = static_cast<int32_t>(static_cast<float>(src_height) * ratio);
        uint32_t scalers_needed = 1;
        // The 7x5 scaler (i.e. 2 scaler resources) is required if the src width is
        // >2048 and the required vertical scaling is greater than 1.99.
        if (layer.image_source().width() > 2048) {
          float ratio = registers::PipeScalerControlSkylake::kDynamicMaxVerticalRatio2049;
          int32_t max_dynamic_height = static_cast<int32_t>(static_cast<float>(src_height) * ratio);
          if (max_dynamic_height < layer.display_destination().height()) {
            scalers_needed = 2;
          }
        }

        // Verify that there are enough scaler resources
        // Verify that the scaler input isn't too large or too small
        // Verify that the required scaling ratio isn't too large
        bool using_c = display_allocated_to_pipe[PipeId::PIPE_C] == display->id();
        if ((total_scalers_needed + scalers_needed) >
                (using_c ? registers::PipeScalerControlSkylake::kPipeCScalersAvailable
                         : registers::PipeScalerControlSkylake::kPipeABScalersAvailable) ||
            src_width > registers::PipeScalerControlSkylake::kMaxSrcWidthPx ||
            src_width < registers::PipeScalerControlSkylake::kMinSrcSizePx ||
            src_height < registers::PipeScalerControlSkylake::kMinSrcSizePx ||
            max_width < layer.display_destination().width() ||
            max_height < layer.display_destination().height()) {
          return display::ConfigCheckResult::kUnsupportedConfig;
        }
        total_scalers_needed += scalers_needed;
      }
      break;
    }

    const display::PixelFormat format = layer.fallback_color().format();
    if (format != display::PixelFormat::kB8G8R8A8 && format != display::PixelFormat::kR8G8B8A8) {
      return display::ConfigCheckResult::kUnsupportedConfig;
    }
    break;
  }

  // CalculateMinimumAllocations ignores layers after kImagePlaneCount. That's fine, since
  // that case already fails from an earlier check.
  uint16_t arr[PipeIds<registers::Platform::kKabyLake>().size()][registers::kImagePlaneCount];
  if (!CalculateMinimumAllocations(display_id, layers, arr)) {
    // Find any displays whose allocation fails and set the return code. Overwrite
    // any previous errors, since they get solved by the merge.
    for (Pipe* pipe : *pipe_manager_) {
      PipeId pipe_id = pipe->pipe_id();
      if (arr[pipe_id][0] != UINT16_MAX) {
        continue;
      }
      ZX_ASSERT(pipe->in_use());  // If the allocation failed, it should be in use
      display::DisplayId pipe_attached_display_id = pipe->attached_display_id();
      if (display_id != pipe_attached_display_id) {
        continue;
      }

      return display::ConfigCheckResult::kUnsupportedConfig;
    }
  }
  return display::ConfigCheckResult::kOk;
}

bool Controller::CalculatePipeAllocation(display::DisplayId display_id,
                                         std::span<display::DisplayId> display_allocated_to_pipe) {
  ZX_DEBUG_ASSERT(display_allocated_to_pipe.size() ==
                  PipeIds<registers::Platform::kKabyLake>().size());
  std::fill(display_allocated_to_pipe.begin(), display_allocated_to_pipe.end(),
            display::kInvalidDisplayId);

  // Keep any allocated pipes on the same display
  DisplayDevice* display = FindDevice(display_id);
  if (display != nullptr && display->pipe() != nullptr) {
    display_allocated_to_pipe[display->pipe()->pipe_id()] = display_id;
  }

  // Give unallocated pipes to the display that need them
  if (display != nullptr && display->pipe() == nullptr) {
    for (unsigned pipe_num = 0; pipe_num < display_allocated_to_pipe.size(); pipe_num++) {
      if (display_allocated_to_pipe[pipe_num] == display::kInvalidDisplayId) {
        display_allocated_to_pipe[pipe_num] = display_id;
        break;
      }
    }
  }
  return true;
}

uint16_t Controller::DataBufferBlockCount() const {
  // Data buffer sizes are documented in the "Display Buffer Programming" >
  // "Display Buffer Size" section in the display engine PRMs.

  // Kaby Lake and Skylake display engines have a single DBUF slice with
  // 892 blocks.
  // Kaby Lake: IHD-OS-KBL-Vol 12-1.17 page 167
  // Skylake: IHD-OS-KBL-Vol 12-1.17 page 164
  static constexpr uint16_t kKabyLakeDataBufferBlockCount = 892;

  // Tiger Lake display engines have two DBUF slice with 1024 blocks each.
  // TODO(https://fxbug.dev/42063006): We should be able to use 2048 blocks, since wee
  // power up both slices.
  // Tiger Lake: IHD-OS-TGL-Vol 12-1.22-Rev2.0 page 297
  // DG1: IHD-OS-DG1-Vol 12-2.21 page 250
  static constexpr uint16_t kTigerLakeDataBufferBlockCount = 1023;

  return is_tgl(device_id_) ? kTigerLakeDataBufferBlockCount : kKabyLakeDataBufferBlockCount;
}

void Controller::SubmitConfiguration(display::DisplayId display_id, display::ModeId display_mode_id,
                                     display::ColorConversion color_conversion,
                                     std::span<const display::DriverLayer> layers,
                                     display::DriverConfigStamp driver_config_stamp) {
  fbl::AutoLock lock(&display_lock_);

  ZX_DEBUG_ASSERT(display_devices_.size() <= kMaximumConnectedDisplayCount);
  display::DisplayId fake_vsync_display_ids[kMaximumConnectedDisplayCount];
  size_t fake_vsync_size = 0;

  ReallocatePlaneBuffers(display_id, layers,
                         /* reallocate_pipes */ pipe_manager_->PipeReallocated());

  for (std::unique_ptr<DisplayDevice>& display : display_devices_) {
    if (display->id() != display_id) {
      if (display->pipe()) {
        // Only reset the planes so that it will display a blank screen.
        display->pipe()->ResetPlanes();
        ResetPipePlaneBuffers(display->pipe()->pipe_id());
      }
      continue;
    }
    display->SubmitConfiguration(display_mode_id, color_conversion, layers, driver_config_stamp);

    // The hardware only gives vsyncs if at least one plane is enabled, so
    // fake one if we need to, to inform the client that we're done with the
    // images.
    bool needs_fake_vsync = false;

    // TODO(https://fxbug.dev/42079186): Remove this condition once we enforce
    // config layer count to be non-zero.
    if (layers.empty()) {
      needs_fake_vsync = true;
    }

    // No display plane is enabled on the pipe if there's only a color layer
    // in a display config, thus we need to issue a fake Vsync event.
    if (layers.size() == 1) {
      const display::DriverLayer& layer = layers[0];
      if (layer.image_source().width() == 0 && layer.image_source().height() == 0) {
        needs_fake_vsync = true;
      }
    }
    if (needs_fake_vsync) {
      fake_vsync_display_ids[fake_vsync_size++] = display->id();
    }
  }

  zx::time_monotonic now =
      (fake_vsync_size > 0) ? zx::clock::get_monotonic() : zx::time_monotonic(0);
  for (size_t i = 0; i < fake_vsync_size; i++) {
    engine_events_.OnDisplayVsync(fake_vsync_display_ids[i], now, driver_config_stamp);
  }
}

zx::result<> Controller::SetBufferCollectionConstraints(
    const display::ImageBufferUsage& usage,
    display::DriverBufferCollectionId driver_buffer_collection_id) {
  ZX_DEBUG_ASSERT_MSG(usage.tiling_type() == display::ImageTilingType::kLinear,
                      "Unsupported tiling type: %" PRIu32, usage.tiling_type().ValueForLogging());

  const auto it = buffer_collections_.find(driver_buffer_collection_id);
  if (it == buffer_collections_.end()) {
    fdf::error("SetBufferCollectionConstraints: Cannot find imported buffer collection (id=%lu)",
               driver_buffer_collection_id.value());
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  const fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection>& collection = it->second;

  fidl::Arena arena;

  // Loop over all combinations of supported image types and pixel formats, adding
  // an image format constraints for each unless the config is asking for a specific
  // format or type.
  std::vector<fuchsia_sysmem2::wire::ImageFormatConstraints> image_constraints_vec;

  // This assumes we only support linear tiling.
  for (display::PixelFormat pixel_format_type : kPixelFormatTypes) {
    auto image_constraints = fuchsia_sysmem2::wire::ImageFormatConstraints::Builder(arena);

    image_constraints.pixel_format(pixel_format_type.ToFidl());
    image_constraints.pixel_format_modifier(fuchsia_images2::wire::PixelFormatModifier::kLinear)
        .bytes_per_row_divisor(64)
        .start_offset_divisor(64);
    image_constraints.color_spaces(std::array{fuchsia_images2::wire::ColorSpace::kSrgb});
    image_constraints_vec.push_back(image_constraints.Build());
  }
  ZX_DEBUG_ASSERT(!image_constraints_vec.empty());
  for (unsigned i = 0; i < std::size(kYuvPixelFormatTypes); ++i) {
    auto image_constraints = fuchsia_sysmem2::wire::ImageFormatConstraints::Builder(arena);
    image_constraints.pixel_format(kYuvPixelFormatTypes[i]);
    image_constraints.color_spaces(std::array{fuchsia_images2::wire::ColorSpace::kRec709});
    image_constraints_vec.push_back(image_constraints.Build());
  }

  auto constraints = fuchsia_sysmem2::wire::BufferCollectionConstraints::Builder(arena)
                         .usage(fuchsia_sysmem2::wire::BufferUsage::Builder(arena)
                                    .display(fuchsia_sysmem2::kDisplayUsageLayer)
                                    .Build())
                         .buffer_memory_constraints(
                             fuchsia_sysmem2::wire::BufferMemoryConstraints::Builder(arena)
                                 .min_size_bytes(0)
                                 .max_size_bytes(0xffffffff)
                                 .physically_contiguous_required(false)
                                 .secure_required(false)
                                 .ram_domain_supported(true)
                                 .cpu_domain_supported(false)
                                 .inaccessible_domain_supported(false)
                                 .permitted_heaps(std::vector{
                                     fuchsia_sysmem2::wire::Heap::Builder(arena)
                                         .heap_type(bind_fuchsia_sysmem_heap::HEAP_TYPE_SYSTEM_RAM)
                                         .id(0)
                                         .Build()})
                                 .Build())
                         .image_format_constraints(image_constraints_vec);

  auto result = collection->SetConstraints(
      fuchsia_sysmem2::wire::BufferCollectionSetConstraintsRequest::Builder(arena)
          .constraints(constraints.Build())
          .Build());

  if (!result.ok()) {
    fdf::error("Failed to set constraints, {}", result.FormatDescription());
    return zx::error(result.status());
  }

  return zx::ok();
}

zx::result<> Controller::SetDisplayPowerMode(display::DisplayId display_id,
                                             display::PowerMode power_mode) {
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

// Intel GPU core methods

zx_status_t Controller::IntelGpuCoreReadPciConfig16(uint16_t addr, uint16_t* value_out) {
  return pci_.ReadConfig16(addr, value_out);
}

zx_status_t Controller::IntelGpuCoreMapPciMmio(uint32_t pci_bar, uint8_t** addr_out,
                                               uint64_t* size_out) {
  if (pci_bar > pci::kMaxBarCount) {
    return ZX_ERR_INVALID_ARGS;
  }
  fbl::AutoLock lock(&bar_lock_);
  if (!mapped_bars_[pci_bar]) {
    zx_status_t status =
        pci_.MapMmio(pci_bar, ZX_CACHE_POLICY_UNCACHED_DEVICE, &mapped_bars_[pci_bar]);
    if (status != ZX_OK) {
      return status;
    }
  }

  // TODO(https://fxbug.dev/42133972): Add MMIO_PTR to cast. This cannot be done as long as
  // IntelGpuCoreMapPciMmio is a signature provided by banjo.
  *addr_out = reinterpret_cast<uint8_t*>(reinterpret_cast<uintptr_t>(mapped_bars_[pci_bar]->get()));
  *size_out = mapped_bars_[pci_bar]->get_size();
  return ZX_OK;
}

zx_status_t Controller::IntelGpuCoreUnmapPciMmio(uint32_t pci_bar) {
  if (pci_bar > pci::kMaxBarCount) {
    return ZX_ERR_INVALID_ARGS;
  }
  // No work needs to be done with MmioBuffers in use.
  return ZX_OK;
}

zx_status_t Controller::IntelGpuCoreGetPciBti(uint32_t index, zx::bti* bti_out) {
  return pci_.GetBti(index, bti_out);
}

zx_status_t Controller::IntelGpuCoreRegisterInterruptCallback(
    const intel_gpu_core_interrupt_t* callback, uint32_t interrupt_mask) {
  ZX_DEBUG_ASSERT(callback);
  return interrupts_.SetGpuInterruptCallback(*callback, interrupt_mask);
}

zx_status_t Controller::IntelGpuCoreUnregisterInterruptCallback() {
  constexpr intel_gpu_core_interrupt_t kNoCallback = {nullptr, nullptr};
  interrupts_.SetGpuInterruptCallback(kNoCallback, 0);
  return ZX_OK;
}

uint64_t Controller::IntelGpuCoreGttGetSize() {
  fbl::AutoLock lock(&gtt_lock_);
  return gtt_.size();
}

zx_status_t Controller::IntelGpuCoreGttAlloc(uint64_t page_count, uint64_t* addr_out) {
  const uint32_t page_size = zx_system_get_page_size();
  uint64_t length = page_count * page_size;
  fbl::AutoLock lock(&gtt_lock_);
  if (length > gtt_.size()) {
    return ZX_ERR_INVALID_ARGS;
  }
  std::unique_ptr<GttRegionImpl> region;
  zx_status_t status =
      gtt_.AllocRegion(static_cast<uint32_t>(page_count * page_size), page_size, &region);
  if (status != ZX_OK) {
    return status;
  }
  *addr_out = region->base();

  imported_gtt_regions_.push_back(std::move(region));
  return ZX_OK;
}

zx_status_t Controller::IntelGpuCoreGttFree(uint64_t addr) {
  fbl::AutoLock lock(&gtt_lock_);
  for (unsigned i = 0; i < imported_gtt_regions_.size(); i++) {
    if (imported_gtt_regions_[i]->base() == addr) {
      imported_gtt_regions_.erase(i)->ClearRegion();
      return ZX_OK;
    }
  }
  return ZX_ERR_INVALID_ARGS;
}

zx_status_t Controller::IntelGpuCoreGttClear(uint64_t addr) {
  fbl::AutoLock lock(&gtt_lock_);
  for (unsigned i = 0; i < imported_gtt_regions_.size(); i++) {
    if (imported_gtt_regions_[i]->base() == addr) {
      imported_gtt_regions_[i]->ClearRegion();
      return ZX_OK;
    }
  }
  return ZX_ERR_INVALID_ARGS;
}

zx_status_t Controller::IntelGpuCoreGttInsert(uint64_t addr, zx::vmo buffer, uint64_t page_offset,
                                              uint64_t page_count) {
  fbl::AutoLock lock(&gtt_lock_);
  for (unsigned i = 0; i < imported_gtt_regions_.size(); i++) {
    if (imported_gtt_regions_[i]->base() == addr) {
      const uint64_t page_size = zx_system_get_page_size();
      return imported_gtt_regions_[i]->PopulateRegion(buffer.release(), page_offset,
                                                      page_count * page_size, true /* writable */);
    }
  }
  return ZX_ERR_INVALID_ARGS;
}

// Ddk methods

void Controller::Start(fdf::StartCompleter completer) {
  fdf::trace("intel-display: initializing displays");

  {
    fbl::AutoLock lock(&display_lock_);
    for (Pipe* pipe : *pipe_manager_) {
      interrupts()->EnablePipeInterrupts(pipe->pipe_id(), /*enabled=*/true);
    }
  }

  InitDisplays();

  {
    fbl::AutoLock lock(&display_lock_);
    driver_initialized_ = true;
  }

  interrupts_.FinishInit();

  fdf::trace("intel-display: display initialization done");
  completer(zx::ok());
}

void Controller::PrepareStopOnPowerOn(fdf::StopCompleter completer) {
  {
    fbl::AutoLock lock(&display_lock_);
    display_devices_.reset();
  }

  completer(zx::ok());
}

void Controller::PrepareStopOnPowerStateTransition(
    fuchsia_system_state::SystemPowerState power_state, fdf::StopCompleter completer) {
  // TODO(https://fxbug.dev/42119483): Implement the suspend hook based on suspendtxn
  if (power_state == fuchsia_system_state::SystemPowerState::kMexec) {
    zx::result<FramebufferInfo> fb_status = GetFramebufferInfo(framebuffer_info_);
    if (fb_status.is_error()) {
      completer(zx::ok());
      return;
    }

    // The bootloader framebuffer is most likely at the start of the display
    // controller's bar 2. Try to get that buffer working again across the
    // mexec by mapping gfx stolen memory to gaddr 0.

    auto bdsm_reg = registers::BaseDsm::Get().FromValue(0);
    zx_status_t status = pci_.ReadConfig32(bdsm_reg.kAddr, bdsm_reg.reg_value_ptr());
    if (status != ZX_OK) {
      fdf::trace("Failed to read dsm base");
      completer(zx::ok());
      return;
    }

    // The Intel docs say that the first page should be reserved for the gfx
    // hardware, but a lot of BIOSes seem to ignore that.
    uintptr_t fb = bdsm_reg.base_phys_addr() << bdsm_reg.base_phys_addr_shift;
    const auto& fb_info = fb_status.value();
    {
      fbl::AutoLock lock(&gtt_lock_);
      gtt_.SetupForMexec(fb, fb_info.size);
    }

    // It may be tempting to try to map the framebuffer and clear it here.
    // However, on Tiger Lake, mapping the framebuffer BAR after setting up
    // the display engine will cause the device to crash and reboot.
    // See https://fxbug.dev/42072946.

    {
      fbl::AutoLock lock(&display_lock_);
      for (auto& display : display_devices_) {
        if (display->pipe() == nullptr) {
          continue;
        }
        // TODO(https://fxbug.dev/42106271): Reset/scale the display to ensure the buffer displays
        // properly
        registers::PipeRegs pipe_regs(display->pipe()->pipe_id());

        auto plane_stride = pipe_regs.PlaneSurfaceStride(0).ReadFrom(mmio_space());
        plane_stride.set_stride(width_in_tiles(display::ImageTilingType::kLinear,
                                               static_cast<int>(fb_info.width),
                                               fb_info.bytes_per_pixel));
        plane_stride.WriteTo(mmio_space());

        auto plane_surface = pipe_regs.PlaneSurface(0).ReadFrom(mmio_space());
        plane_surface.set_surface_base_addr(0);
        plane_surface.WriteTo(mmio_space());
      }
    }
  }
  completer(zx::ok());
}

zx_koid_t GetKoid(zx_handle_t handle) {
  zx_info_handle_basic_t info;
  zx_status_t status =
      zx_object_get_info(handle, ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  return status == ZX_OK ? info.koid : ZX_KOID_INVALID;
}

zx_status_t Controller::Init() {
  fdf::trace("Binding to display controller");

  auto pid = GetKoid(zx_process_self());
  std::string debug_name = fxl::StringPrintf("intel-display[%lu]", pid);
  fidl::Arena arena;
  auto set_debug_status = sysmem_->SetDebugClientInfo(
      fuchsia_sysmem2::wire::AllocatorSetDebugClientInfoRequest::Builder(arena)
          .name(fidl::StringView::FromExternal(debug_name))
          .id(pid)
          .Build());
  if (!set_debug_status.ok()) {
    fdf::error("Cannot set sysmem allocator debug info: {}", set_debug_status.status_string());
    return set_debug_status.status();
  }

  ZX_DEBUG_ASSERT(pci_.is_valid());
  pci_.ReadConfig16(fuchsia_hardware_pci::Config::kDeviceId, &device_id_);
  fdf::trace("Device id {:x}", device_id_);

  const zx::unowned_resource& driver_mmio_resource = resources_.mmio;
  if (!driver_mmio_resource->is_valid()) {
    fdf::warn("Failed to get driver MMIO resource. VBT initialization skipped.");
  } else {
    zx_status_t status = igd_opregion_.Init(driver_mmio_resource->borrow(), pci_);
    if (status != ZX_OK && status != ZX_ERR_NOT_SUPPORTED) {
      fdf::error("VBT initializaton failed: {}", zx::make_result(status));
      return status;
    }
  }

  fdf::trace("Mapping registers");
  // map register window
  uint8_t* regs;
  uint64_t size;
  zx_status_t status = IntelGpuCoreMapPciMmio(0u, &regs, &size);
  if (status != ZX_OK) {
    fdf::error("Failed to map bar 0: {}", status);
    return status;
  }

  {
    fbl::AutoLock lock(&bar_lock_);
    fbl::AllocChecker ac;
    mmio_space_.emplace(mapped_bars_[0]->View(0));
  }

  fdf::trace("Reading fuses and straps");
  FuseConfig fuse_config = FuseConfig::ReadFrom(*mmio_space(), device_id_);
  fuse_config.Log();

  fdf::trace("Initializing DDIs");
  ddis_ = GetDdiIds(device_id_);

  fdf::trace("Initializing Power");
  power_ = Power::New(mmio_space(), device_id_);

  fdf::trace("Reading PCH display engine config");
  pch_engine_.emplace(mmio_space(), device_id_);
  pch_engine_->Log();

  for (unsigned i = 0; i < ddis_.size(); i++) {
    gmbus_i2cs_.push_back(GMBusI2c(ddis_[i], GetPlatform(device_id_), mmio_space()));

    dp_aux_channels_.push_back(DpAuxChannelImpl(mmio_space(), ddis_[i], device_id_));
    fdf::trace("DDI {} AUX channel initial configuration:", ddis_[i]);
    dp_aux_channels_[dp_aux_channels_.size() - 1].aux_channel().Log();
  }

  if (!is_tgl(device_id_)) {
    ddi_e_disabled_ = registers::DdiRegs(DdiId::DDI_A)
                          .BufferControl()
                          .ReadFrom(mmio_space())
                          .ddi_e_disabled_kaby_lake();
  }

  fdf::trace("Initializing interrupts");
  status = interrupts_.Init(fit::bind_member<&Controller::HandlePipeVsync>(this),
                            fit::bind_member<&Controller::HandleHotplug>(this), pci_, mmio_space(),
                            device_id_);
  if (status != ZX_OK) {
    fdf::error("Failed to initialize interrupts");
    return status;
  }

  fdf::trace("Mapping gtt");
  {
    // The bootloader framebuffer is located at the start of the BAR that gets mapped by GTT.
    // Prevent clients from allocating memory in this region by telling |gtt_| to exclude it from
    // the region allocator.
    uint32_t offset = 0u;
    auto fb = GetFramebufferInfo(framebuffer_info_);
    if (fb.is_error()) {
      fdf::info("Failed to obtain framebuffer size ({})", fb);
      // It is possible for zx_framebuffer_get_info to fail in a headless system as the bootloader
      // framebuffer information will be left uninitialized. Tolerate this failure by assuming
      // that the stolen memory contents won't be shown on any screen and map the global GTT at
      // offset 0.
      offset = 0u;
    } else {
      offset = fb.value().size;
    }

    fbl::AutoLock lock(&gtt_lock_);
    status = gtt_.Init(pci_, mmio_space()->View(GTT_BASE_OFFSET), offset);
    if (status != ZX_OK) {
      fdf::error("Failed to init gtt ({})", zx::make_result(status));
      return status;
    }
  }

  {
    fbl::AutoLock lock(&display_lock_);
    if (is_tgl(device_id())) {
      pipe_manager_ = std::make_unique<PipeManagerTigerLake>(this);
    } else {
      pipe_manager_ = std::make_unique<PipeManagerSkylake>(this);
    }
  }

  if (is_tgl(device_id())) {
    ddi_manager_ = std::make_unique<DdiManagerTigerLake>(this);
  } else {
    ddi_manager_ = std::make_unique<DdiManagerSkylake>();
  }

  if (is_tgl(device_id())) {
    dpll_manager_ = std::make_unique<DpllManagerTigerLake>(mmio_space());
  } else {
    dpll_manager_ = std::make_unique<DpllManagerSkylake>(mmio_space());
  }

  root_node_ = inspector_.GetRoot().CreateChild("intel-display");
  fdf::trace("bind done");
  return ZX_OK;
}

zx::result<ddk::AnyProtocol> Controller::GetProtocol(uint32_t proto_id) {
  switch (proto_id) {
    case ZX_PROTOCOL_INTEL_GPU_CORE:
      return zx::ok(ddk::AnyProtocol{
          .ops = &intel_gpu_core_protocol_ops_,
          .ctx = this,
      });
  }
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

Controller::Controller(fidl::ClientEnd<fuchsia_sysmem2::Allocator> sysmem,
                       fidl::ClientEnd<fuchsia_hardware_pci::Device> pci,
                       ControllerResources resources, std::optional<zbi_swfb_t> framebuffer_info,
                       display::DisplayEngineEventsInterface* engine_events,
                       inspect::Inspector inspector)
    : resources_(std::move(resources)),
      framebuffer_info_(framebuffer_info),
      sysmem_(std::move(sysmem)),
      engine_events_(*engine_events),
      pci_(std::move(pci)),
      inspector_(std::move(inspector)) {
  ZX_DEBUG_ASSERT(engine_events != nullptr);
}

Controller::Controller(display::DisplayEngineEventsInterface* engine_events,
                       inspect::Inspector inspector)
    : engine_events_(*engine_events), inspector_(std::move(inspector)) {
  ZX_DEBUG_ASSERT(engine_events != nullptr);
}

Controller::~Controller() {
  interrupts_.Destroy();
  if (mmio_space() && pipe_manager_.get()) {
    for (Pipe* pipe : *pipe_manager_) {
      fbl::AutoLock lock(&display_lock_);
      interrupts()->EnablePipeInterrupts(pipe->pipe_id(), /*enable=*/true);
    }
  }
}

// static
zx::result<std::unique_ptr<Controller>> Controller::Create(
    fidl::ClientEnd<fuchsia_sysmem2::Allocator> sysmem,
    fidl::ClientEnd<fuchsia_hardware_pci::Device> pci, ControllerResources resources,
    std::optional<zbi_swfb_t> framebuffer_info,
    display::DisplayEngineEventsInterface* engine_events, inspect::Inspector inspector) {
  ZX_DEBUG_ASSERT(engine_events != nullptr);

  fbl::AllocChecker alloc_checker;
  auto controller = fbl::make_unique_checked<Controller>(
      &alloc_checker, std::move(sysmem), std::move(pci), std::move(resources), framebuffer_info,
      engine_events, std::move(inspector));
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate memory for Controller");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  zx_status_t status = controller->Init();
  if (status != ZX_OK) {
    fdf::error("Failed to initialize Controller: {}", zx::make_result(status));
    return zx::error(status);
  }
  return zx::ok(std::move(controller));
}

}  // namespace intel_display

// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/amlogic-display/dsi-host.h"

#include <fidl/fuchsia.hardware.gpio/cpp/wire.h>
#include <fidl/fuchsia.hardware.platform.device/cpp/wire.h>
#include <lib/device-protocol/display-panel.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/fit/function.h>
#include <lib/mipi-dsi/mipi-dsi.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>

#include <fbl/alloc_checker.h>

#include "src/graphics/display/drivers/amlogic-display/board-resources.h"
#include "src/graphics/display/drivers/amlogic-display/common.h"
#include "src/graphics/display/drivers/amlogic-display/dsi.h"
#include "src/graphics/display/drivers/amlogic-display/hhi-regs.h"
#include "src/graphics/display/drivers/amlogic-display/lcd.h"
#include "src/graphics/display/drivers/amlogic-display/mipi-phy.h"
#include "src/graphics/display/drivers/amlogic-display/panel-config.h"
#include "src/graphics/display/lib/designware-dsi/dpi-interface-config.h"
#include "src/graphics/display/lib/designware-dsi/dsi-host-controller-config.h"
#include "src/graphics/display/lib/designware-dsi/dsi-host-controller.h"

namespace amlogic_display {

namespace {

zx::result<std::unique_ptr<designware_dsi::DsiHostController>> CreateDesignwareDsiHostController(
    fidl::UnownedClientEnd<fuchsia_hardware_platform_device::Device> platform_device) {
  zx::result<fdf::MmioBuffer> dsi_host_mmio_result =
      MapMmio(kMmioNameDsiController, platform_device);
  if (dsi_host_mmio_result.is_error()) {
    return dsi_host_mmio_result.take_error();
  }
  fdf::MmioBuffer dsi_host_mmio = std::move(dsi_host_mmio_result).value();

  fbl::AllocChecker alloc_checker;
  auto designware_dsi_host_controller = fbl::make_unique_checked<designware_dsi::DsiHostController>(
      &alloc_checker, std::move(dsi_host_mmio));
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate memory for designware_dsi::DsiHostController");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  return zx::ok(std::move(designware_dsi_host_controller));
}

}  // namespace

DsiHost::DsiHost(display::PanelType panel_type, const PanelConfig* panel_config,
                 fdf::MmioBuffer mipi_dsi_top_mmio, fdf::MmioBuffer hhi_mmio,
                 fidl::ClientEnd<fuchsia_hardware_gpio::Gpio> lcd_reset_gpio,
                 std::unique_ptr<designware_dsi::DsiHostController> designware_dsi_host_controller,
                 std::unique_ptr<Lcd> lcd, std::unique_ptr<MipiPhy> phy, bool enabled)
    : mipi_dsi_top_mmio_(std::move(mipi_dsi_top_mmio)),
      hhi_mmio_(std::move(hhi_mmio)),
      lcd_reset_gpio_(std::move(lcd_reset_gpio)),
      panel_type_(std::move(panel_type)),
      panel_config_(*panel_config),
      enabled_(enabled),
      designware_dsi_host_controller_(std::move(designware_dsi_host_controller)),
      lcd_(std::move(lcd)),
      phy_(std::move(phy)) {
  ZX_DEBUG_ASSERT(panel_config != nullptr);
  ZX_DEBUG_ASSERT(lcd_reset_gpio_.is_valid());
  ZX_DEBUG_ASSERT(designware_dsi_host_controller_ != nullptr);
  ZX_DEBUG_ASSERT(lcd_ != nullptr);
  ZX_DEBUG_ASSERT(phy_ != nullptr);
}

// static
zx::result<std::unique_ptr<DsiHost>> DsiHost::Create(fdf::Namespace& incoming,
                                                     display::PanelType panel_type,
                                                     const PanelConfig* panel_config) {
  ZX_DEBUG_ASSERT(panel_config != nullptr);

  static const char kLcdGpioFragmentName[] = "gpio-lcd-reset";
  zx::result lcd_reset_gpio_result =
      incoming.Connect<fuchsia_hardware_gpio::Service::Device>(kLcdGpioFragmentName);
  if (lcd_reset_gpio_result.is_error()) {
    fdf::error("Failed to get gpio protocol from fragment: {}", kLcdGpioFragmentName);
    return lcd_reset_gpio_result.take_error();
  }
  fidl::ClientEnd<fuchsia_hardware_gpio::Gpio> lcd_reset_gpio =
      std::move(lcd_reset_gpio_result).value();

  static constexpr char kPdevFragmentName[] = "pdev";
  zx::result<fidl::ClientEnd<fuchsia_hardware_platform_device::Device>> pdev_result =
      incoming.Connect<fuchsia_hardware_platform_device::Service::Device>(kPdevFragmentName);
  if (pdev_result.is_error()) {
    fdf::error("Failed to get the pdev client: {}", pdev_result);
    return pdev_result.take_error();
  }
  fidl::ClientEnd<fuchsia_hardware_platform_device::Device> platform_device =
      std::move(pdev_result).value();

  zx::result<fdf::MmioBuffer> dsi_top_mmio_result = MapMmio(kMmioNameDsiTop, platform_device);
  if (dsi_top_mmio_result.is_error()) {
    return dsi_top_mmio_result.take_error();
  }
  fdf::MmioBuffer mipi_dsi_top_mmio = std::move(dsi_top_mmio_result).value();

  zx::result<fdf::MmioBuffer> hhi_mmio_result = MapMmio(kMmioNameHhi, platform_device);
  if (hhi_mmio_result.is_error()) {
    return hhi_mmio_result.take_error();
  }
  fdf::MmioBuffer hhi_mmio = std::move(hhi_mmio_result).value();

  zx::result<std::unique_ptr<designware_dsi::DsiHostController>>
      designware_dsi_host_controller_result = CreateDesignwareDsiHostController(platform_device);
  if (designware_dsi_host_controller_result.is_error()) {
    fdf::error("Failed to Create Designware DsiHostController: {}",
               designware_dsi_host_controller_result);
    return designware_dsi_host_controller_result.take_error();
  }
  std::unique_ptr<designware_dsi::DsiHostController> designware_dsi_host_controller =
      std::move(designware_dsi_host_controller_result).value();

  zx::result<std::unique_ptr<Lcd>> lcd_result =
      Lcd::Create(incoming, panel_type, panel_config, designware_dsi_host_controller.get(),
                  kBootloaderDisplayEnabled);
  if (lcd_result.is_error()) {
    fdf::error("Failed to Create Lcd: {}", lcd_result);
    return lcd_result.take_error();
  }
  std::unique_ptr<Lcd> lcd = std::move(lcd_result).value();

  zx::result<std::unique_ptr<MipiPhy>> mipi_phy_result = MipiPhy::Create(
      platform_device, designware_dsi_host_controller.get(), kBootloaderDisplayEnabled);
  if (mipi_phy_result.is_error()) {
    fdf::error("Failed to Create MipiPhy: {}", mipi_phy_result);
    return mipi_phy_result.take_error();
  }
  std::unique_ptr<MipiPhy> phy = std::move(mipi_phy_result).value();

  fbl::AllocChecker alloc_checker;
  auto dsi_host = fbl::make_unique_checked<DsiHost>(
      &alloc_checker, panel_type, panel_config, std::move(mipi_dsi_top_mmio), std::move(hhi_mmio),
      std::move(lcd_reset_gpio), std::move(designware_dsi_host_controller), std::move(lcd),
      std::move(phy),
      /*enabled=*/kBootloaderDisplayEnabled);
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate memory for DsiHost");
    return zx::error(ZX_ERR_NO_MEMORY);
  }
  return zx::ok(std::move(dsi_host));
}

zx::result<> DsiHost::PerformPowerOpSequence(std::span<const PowerOp> commands,
                                             fit::callback<zx::result<>()> power_on) {
  if (commands.size() == 0) {
    fdf::error("No power commands to execute");
    return zx::ok();
  }
  uint8_t wait_count = 0;

  for (const auto op : commands) {
    fdf::trace("power_op {} index={} value={} sleep_ms={}", static_cast<uint8_t>(op.op), op.index,
               op.value, op.sleep_ms);
    switch (op.op) {
      case kPowerOpExit:
        fdf::trace("power_exit");
        return zx::ok();
      case kPowerOpGpio: {
        fdf::trace("power_set_gpio pin #{} value={}", op.index, op.value);
        if (op.index != 0) {
          fdf::error("Unrecognized GPIO pin #{}, ignoring", op.index);
          break;
        }
        fidl::WireResult result = lcd_reset_gpio_->SetBufferMode(
            op.value ? fuchsia_hardware_gpio::BufferMode::kOutputHigh
                     : fuchsia_hardware_gpio::BufferMode::kOutputLow);
        if (!result.ok()) {
          fdf::error("Failed to send Write request to lcd gpio: {}", result.status_string());
          return zx::error(result.status());
        }
        if (result->is_error()) {
          fdf::error("Failed to write to lcd gpio: {}", zx::make_result(result->error_value()));
          return result->take_error();
        }
        break;
      }
      case kPowerOpSignal: {
        fdf::trace("power_signal dsi_init");
        zx::result<> power_on_result = power_on();
        if (!power_on_result.is_ok()) {
          fdf::error("Failed to power on MIPI DSI display: {}", power_on_result);
          return power_on_result.take_error();
        }
        break;
      }
      case kPowerOpAwaitGpio:
        fdf::trace("power_await_gpio pin #{} value={} timeout={} msec", op.index, op.value,
                   op.sleep_ms);
        if (op.index != 0) {
          fdf::error("Unrecognized GPIO pin #{}, ignoring", op.index);
          break;
        }
        {
          fidl::WireResult result =
              lcd_reset_gpio_->SetBufferMode(fuchsia_hardware_gpio::BufferMode::kInput);
          if (!result.ok()) {
            fdf::error("Failed to send SetBufferMode request to lcd gpio: {}",
                       result.status_string());
            return zx::error(result.status());
          }

          auto& response = result.value();
          if (response.is_error()) {
            fdf::error("Failed to configure lcd gpio to input: {}",
                       zx::make_result(response.error_value()));
            return response.take_error();
          }
        }
        for (wait_count = 0; wait_count < op.sleep_ms; wait_count++) {
          fidl::WireResult read_result = lcd_reset_gpio_->Read();
          if (!read_result.ok()) {
            fdf::error("Failed to send Read request to lcd gpio: {}", read_result.status_string());
            return zx::error(read_result.status());
          }

          auto& read_response = read_result.value();
          if (read_response.is_error()) {
            fdf::error("Failed to read lcd gpio: {}", zx::make_result(read_response.error_value()));
            return read_response.take_error();
          }
          if (read_result.value()->value == static_cast<bool>(op.value)) {
            break;
          }
          zx::nanosleep(zx::deadline_after(zx::msec(1)));
        }
        if (wait_count == op.sleep_ms) {
          fdf::error("Timed out waiting for GPIO value={}", op.value);
        }
        break;
      default:
        fdf::error("Unrecognized power op {}", static_cast<uint8_t>(op.op));
        break;
    }
    if (op.op != kPowerOpAwaitGpio && op.sleep_ms != 0) {
      fdf::trace("power_sleep {} msec", op.sleep_ms);
      zx::nanosleep(zx::deadline_after(zx::msec(op.sleep_ms)));
    }
  }
  return zx::ok();
}

zx::result<> DsiHost::ConfigureDsiHostController(int64_t d_phy_data_lane_bitrate_bits_per_second) {
  // Setup relevant TOP_CNTL register -- Undocumented --
  mipi_dsi_top_mmio_.Write32(
      SetFieldValue32(mipi_dsi_top_mmio_.Read32(MIPI_DSI_TOP_CNTL), TOP_CNTL_DPI_CLR_MODE_START,
                      TOP_CNTL_DPI_CLR_MODE_BITS, SUPPORTED_DPI_FORMAT),
      MIPI_DSI_TOP_CNTL);
  mipi_dsi_top_mmio_.Write32(
      SetFieldValue32(mipi_dsi_top_mmio_.Read32(MIPI_DSI_TOP_CNTL), TOP_CNTL_IN_CLR_MODE_START,
                      TOP_CNTL_IN_CLR_MODE_BITS, SUPPORTED_VENC_DATA_WIDTH),
      MIPI_DSI_TOP_CNTL);
  mipi_dsi_top_mmio_.Write32(
      SetFieldValue32(mipi_dsi_top_mmio_.Read32(MIPI_DSI_TOP_CNTL), TOP_CNTL_CHROMA_SUBSAMPLE_START,
                      TOP_CNTL_CHROMA_SUBSAMPLE_BITS, 0),
      MIPI_DSI_TOP_CNTL);

  constexpr int kBitsPerByte = 8;
  constexpr int kDataLaneBitsPerClockLanePeriod = 2;
  const int64_t d_phy_data_lane_bytes_per_second =
      d_phy_data_lane_bitrate_bits_per_second / kBitsPerByte;
  const designware_dsi::DsiHostControllerConfig config = {
      .dphy_interface_config =
          {
              .data_lane_count = panel_config_.dphy_data_lane_count,
              .clock_lane_mode_automatic_control_enabled = true,

              .high_speed_mode_clock_lane_frequency_hz =
                  d_phy_data_lane_bitrate_bits_per_second / kDataLaneBitsPerClockLanePeriod,
              .escape_mode_clock_lane_frequency_hz =
                  d_phy_data_lane_bytes_per_second /
                  phy_->GetDataLaneByteRateToEscapeClockFrequencyRatio(),

              .max_data_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = PHY_TMR_HS_TO_LP,
              .max_data_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = PHY_TMR_LP_TO_HS,
              .max_clock_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles =
                  PHY_TMR_LPCLK_CLKHS_TO_LP,
              .max_clock_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles =
                  PHY_TMR_LPCLK_CLKLP_TO_HS,
          },

      .dpi_interface_config =
          {
              .color_component_mapping = designware_dsi::DpiColorComponentMapping::k24BitR8G8B8,
              .video_timing = panel_config_.display_timing,
              .low_power_command_timer_config =
                  {
                      .max_vertical_blank_escape_mode_command_size_bytes = LPCMD_PKT_SIZE,
                      .max_vertical_active_escape_mode_command_size_bytes = LPCMD_PKT_SIZE,
                  },
          },

      .dsi_packet_handler_config =
          {
              .packet_sequencing = mipi_dsi::DsiVideoModePacketSequencing::kBurst,
              .pixel_stream_packet_format = mipi_dsi::DsiPixelStreamPacketFormat::k24BitR8G8B8,
          },
  };
  zx::result config_result = designware_dsi_host_controller_->Config(config);
  if (config_result.is_error()) {
    fdf::error("Failed to configure the DSI Host Controller: {}", config_result);
    return config_result.take_error();
  }
  return zx::ok();
}

void DsiHost::PhyEnable() {
  hhi_mmio_.Write32(MIPI_CNTL0_CMN_REF_GEN_CTRL(0x29) | MIPI_CNTL0_VREF_SEL(VREF_SEL_VR) |
                        MIPI_CNTL0_LREF_SEL(LREF_SEL_L_ROUT) | MIPI_CNTL0_LBG_EN |
                        MIPI_CNTL0_VR_TRIM_CNTL(0x7) | MIPI_CNTL0_VR_GEN_FROM_LGB_EN,
                    HHI_MIPI_CNTL0);
  hhi_mmio_.Write32(MIPI_CNTL1_DSI_VBG_EN | MIPI_CNTL1_CTL, HHI_MIPI_CNTL1);
  hhi_mmio_.Write32(MIPI_CNTL2_DEFAULT_VAL, HHI_MIPI_CNTL2);  // 4 lane
}

void DsiHost::PhyDisable() {
  hhi_mmio_.Write32(0, HHI_MIPI_CNTL0);
  hhi_mmio_.Write32(0, HHI_MIPI_CNTL1);
  hhi_mmio_.Write32(0, HHI_MIPI_CNTL2);
}

void DsiHost::Disable() {
  // turn host off only if it's been fully turned on
  if (!enabled_) {
    return;
  }

  // Place dsi in command mode first
  designware_dsi_host_controller_->SetMode(mipi_dsi::DsiOperationMode::kCommand);
  fit::callback<zx::result<>()> power_off = [this]() -> zx::result<> {
    zx::result<> result = lcd_->Disable();
    if (!result.is_ok()) {
      return result.take_error();
    }
    PhyDisable();
    phy_->Shutdown();
    return zx::ok();
  };

  zx::result<> power_off_result =
      PerformPowerOpSequence(panel_config_.power_off, std::move(power_off));
  if (!power_off_result.is_ok()) {
    fdf::error("Failed to power off the DSI display: {}", power_off_result);
  }

  enabled_ = false;
}

zx::result<> DsiHost::Enable(int64_t dphy_data_lane_bits_per_second) {
  if (enabled_) {
    return zx::ok();
  }

  fit::callback<zx::result<>()> power_on = [&]() -> zx::result<> {
    // Enable MIPI PHY
    PhyEnable();

    // Load Phy configuration
    zx::result<> phy_config_load_result = phy_->PhyCfgLoad(dphy_data_lane_bits_per_second);
    if (!phy_config_load_result.is_ok()) {
      fdf::error("Error during phy config calculations: {}", phy_config_load_result);
      return phy_config_load_result.take_error();
    }

    // Enable dwc mipi_dsi_host's clock
    mipi_dsi_top_mmio_.Write32(
        SetFieldValue32(mipi_dsi_top_mmio_.Read32(MIPI_DSI_TOP_CNTL), /*field_begin_bit=*/4,
                        /*field_size_bits=*/2, /*field_value=*/0x3),
        MIPI_DSI_TOP_CNTL);
    // mipi_dsi_host's reset
    mipi_dsi_top_mmio_.Write32(
        SetFieldValue32(mipi_dsi_top_mmio_.Read32(MIPI_DSI_TOP_SW_RESET), /*field_begin_bit=*/0,
                        /*field_size_bits=*/4, /*field_value=*/0xf),
        MIPI_DSI_TOP_SW_RESET);
    // Release mipi_dsi_host's reset
    mipi_dsi_top_mmio_.Write32(
        SetFieldValue32(mipi_dsi_top_mmio_.Read32(MIPI_DSI_TOP_SW_RESET), /*field_begin_bit=*/0,
                        /*field_size_bits=*/4, /*field_value=*/0x0),
        MIPI_DSI_TOP_SW_RESET);
    // Enable dwc mipi_dsi_host's clock
    mipi_dsi_top_mmio_.Write32(
        SetFieldValue32(mipi_dsi_top_mmio_.Read32(MIPI_DSI_TOP_CLK_CNTL), /*field_begin_bit=*/0,
                        /*field_size_bits=*/2, /*field_value=*/0x3),
        MIPI_DSI_TOP_CLK_CNTL);

    mipi_dsi_top_mmio_.Write32(0, MIPI_DSI_TOP_MEM_PD);
    zx::nanosleep(zx::deadline_after(zx::msec(10)));

    // Initialize host in command mode first
    designware_dsi_host_controller_->SetMode(mipi_dsi::DsiOperationMode::kCommand);
    zx::result<> dsi_host_config_result =
        ConfigureDsiHostController(dphy_data_lane_bits_per_second);
    if (!dsi_host_config_result.is_ok()) {
      fdf::error("Failed to configure the MIPI DSI Host Controller: {}", dsi_host_config_result);
      return dsi_host_config_result.take_error();
    }

    // Initialize mipi dsi D-phy
    zx::result<> phy_startup_result = phy_->Startup();
    if (!phy_startup_result.is_ok()) {
      fdf::error("Error during MIPI D-PHY Initialization: {}", phy_startup_result);
      return phy_startup_result.take_error();
    }

    // Load LCD Init values while in command mode
    zx::result<> lcd_enable_result = lcd_->Enable();
    if (!lcd_enable_result.is_ok()) {
      fdf::error("Failed to enable LCD: {}", lcd_enable_result);
    }

    designware_dsi_host_controller_->SetMode(mipi_dsi::DsiOperationMode::kVideo);
    return zx::ok();
  };

  zx::result<> power_on_result =
      PerformPowerOpSequence(panel_config_.power_on, std::move(power_on));
  if (!power_on_result.is_ok()) {
    fdf::error("Failed to power on the DSI Display: {}", power_on_result);
    return power_on_result.take_error();
  }
  // Host is On and Active at this point
  enabled_ = true;
  return zx::ok();
}

void DsiHost::Dump() {
  fdf::info("MIPI_DSI_TOP_SW_RESET = 0x{:x}", mipi_dsi_top_mmio_.Read32(MIPI_DSI_TOP_SW_RESET));
  fdf::info("MIPI_DSI_TOP_CLK_CNTL = 0x{:x}", mipi_dsi_top_mmio_.Read32(MIPI_DSI_TOP_CLK_CNTL));
  fdf::info("MIPI_DSI_TOP_CNTL = 0x{:x}", mipi_dsi_top_mmio_.Read32(MIPI_DSI_TOP_CNTL));
  fdf::info("MIPI_DSI_TOP_SUSPEND_CNTL = 0x{:x}",
            mipi_dsi_top_mmio_.Read32(MIPI_DSI_TOP_SUSPEND_CNTL));
  fdf::info("MIPI_DSI_TOP_SUSPEND_LINE = 0x{:x}",
            mipi_dsi_top_mmio_.Read32(MIPI_DSI_TOP_SUSPEND_LINE));
  fdf::info("MIPI_DSI_TOP_SUSPEND_PIX = 0x{:x}",
            mipi_dsi_top_mmio_.Read32(MIPI_DSI_TOP_SUSPEND_PIX));
  fdf::info("MIPI_DSI_TOP_MEAS_CNTL = 0x{:x}", mipi_dsi_top_mmio_.Read32(MIPI_DSI_TOP_MEAS_CNTL));
  fdf::info("MIPI_DSI_TOP_STAT = 0x{:x}", mipi_dsi_top_mmio_.Read32(MIPI_DSI_TOP_STAT));
  fdf::info("MIPI_DSI_TOP_MEAS_STAT_TE0 = 0x{:x}",
            mipi_dsi_top_mmio_.Read32(MIPI_DSI_TOP_MEAS_STAT_TE0));
  fdf::info("MIPI_DSI_TOP_MEAS_STAT_TE1 = 0x{:x}",
            mipi_dsi_top_mmio_.Read32(MIPI_DSI_TOP_MEAS_STAT_TE1));
  fdf::info("MIPI_DSI_TOP_MEAS_STAT_VS0 = 0x{:x}",
            mipi_dsi_top_mmio_.Read32(MIPI_DSI_TOP_MEAS_STAT_VS0));
  fdf::info("MIPI_DSI_TOP_MEAS_STAT_VS1 = 0x{:x}",
            mipi_dsi_top_mmio_.Read32(MIPI_DSI_TOP_MEAS_STAT_VS1));
  fdf::info("MIPI_DSI_TOP_INTR_CNTL_STAT = 0x{:x}",
            mipi_dsi_top_mmio_.Read32(MIPI_DSI_TOP_INTR_CNTL_STAT));
  fdf::info("MIPI_DSI_TOP_MEM_PD = 0x{:x}", mipi_dsi_top_mmio_.Read32(MIPI_DSI_TOP_MEM_PD));
}

}  // namespace amlogic_display

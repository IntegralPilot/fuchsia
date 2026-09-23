// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/amlogic-display/lcd.h"

#include <lib/driver/incoming/cpp/namespace.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/mipi-dsi/mipi-dsi.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <array>
#include <cstdint>
#include <span>

#include <fbl/alloc_checker.h>

#include "src/graphics/display/drivers/amlogic-display/panel-config.h"
#include "src/graphics/display/lib/designware-dsi/dsi-host-controller.h"

namespace amlogic_display {

namespace {

constexpr int IsDsiCommandPayloadSizeValid(uint8_t cmd_type, int payload_size) {
  switch (cmd_type) {
    case kMipiDsiDtDcsShortWrite0:
      return payload_size == 1;
    case kMipiDsiDtDcsShortWrite1:
      return payload_size == 2;
    case kMipiDsiDtDcsLongWrite:
      return payload_size >= 1;
    case kMipiDsiDtGenShortWrite0:
      return payload_size == 0;
    case kMipiDsiDtGenShortWrite1:
      return payload_size == 1;
    case kMipiDsiDtGenShortWrite2:
      return payload_size == 2;
    case kMipiDsiDtGenLongWrite:
      return payload_size >= 0;
  }
  ZX_ASSERT_MSG(false, "Unsupported command type: 0x%02x", cmd_type);
}

constexpr int kReadRegisterMaximumValueCount = 4;

zx_status_t CheckDsiDeviceRegister(
    designware_dsi::DsiHostController& designware_dsi_host_controller, uint8_t reg, size_t count) {
  ZX_DEBUG_ASSERT(count > 0);
  ZX_DEBUG_ASSERT(count <= kReadRegisterMaximumValueCount);

  const std::array<uint8_t, 2> set_maximum_return_packet_size_payload = {
      // `count` must not exceed kReadRegisterMaximumValueCount = 4, so the
      // cast won't overflow which causes undefined behavior.
      static_cast<uint8_t>(count),
      0,
  };
  const std::array<uint8_t, 1> read_payload = {
      reg,
  };
  std::array<uint8_t, kReadRegisterMaximumValueCount> response_buffer = {};
  std::span<uint8_t> response_payload(response_buffer.data(), count);

  const mipi_dsi::DsiCommandAndResponse commands[] = {
      // Sets the maximum return packet size to avoid read buffer overflow on the
      // DSI host controller.
      {
          .virtual_channel_id = kMipiDsiVirtualChanId,
          .data_type = kMipiDsiDtSetMaxRetPkt,
          .payload = set_maximum_return_packet_size_payload,
          .response_payload = {},
      },
      {
          .virtual_channel_id = kMipiDsiVirtualChanId,
          .data_type = kMipiDsiDtGenShortRead1,
          .payload = read_payload,
          .response_payload = response_payload,
      },
  };

  zx::result<> result = designware_dsi_host_controller.IssueCommands(commands);
  if (result.is_error()) {
    fdf::error("Could not read register {}: {}", reg, result);
    return result.status_value();
  }
  return ZX_OK;
}

// Reads the display hardware ID from the MIPI-DSI interface.
//
// `dsiimpl` must be configured in DSI command mode.
zx::result<uint32_t> GetMipiDsiDisplayId(
    designware_dsi::DsiHostController& designware_dsi_host_controller) {
  // TODO(https://fxbug.dev/322450952): The Read Display Identification
  // Information (0x04) command is not guaranteed to be available on all
  // display driver ICs. The response size and the actual meaning of the
  // response may vary, depending on the DDIC models. Do not hardcode the
  // command address and the response size.
  //
  // The following command address and response size are specified on:
  // - JD9364 datasheet, Section 10.2.3 "RDDIDIF", page 146
  // - JD9365D datasheet, Section 10.2.3 "RDDIDIF", page 130
  // - NT35596 datasheet, Section 6.1 "User Command Set (Command 1)", page 158
  constexpr uint8_t kCommandReadDisplayIdentificationInformation = 0x04;
  constexpr int kCommandReadDisplayIdentificationInformationResponseSize = 3;

  const std::array<uint8_t, 2> set_maximum_return_packet_size_payload = {
      kCommandReadDisplayIdentificationInformationResponseSize & 0xff,
      kCommandReadDisplayIdentificationInformationResponseSize >> 8,
  };
  const std::array<uint8_t, 1> read_display_identification_payload = {
      kCommandReadDisplayIdentificationInformation,
  };
  std::array<uint8_t, kCommandReadDisplayIdentificationInformationResponseSize> response_payload;

  const mipi_dsi::DsiCommandAndResponse commands[] = {
      // Sets the maximum return packet size to avoid read buffer overflow on the
      // DSI host controller.
      {
          .virtual_channel_id = kMipiDsiVirtualChanId,
          .data_type = kMipiDsiDtSetMaxRetPkt,
          .payload = set_maximum_return_packet_size_payload,
          .response_payload = {},
      },
      {
          .virtual_channel_id = kMipiDsiVirtualChanId,
          .data_type = kMipiDsiDtGenShortRead1,
          .payload = read_display_identification_payload,
          .response_payload = response_payload,
      },
  };

  zx::result<> result = designware_dsi_host_controller.IssueCommands(commands);
  if (result.is_error()) {
    fdf::error("Failed to read out Display ID: {}", result);
    return result.take_error();
  }

  const uint32_t display_id =
      response_payload[0] << 16 | response_payload[1] << 8 | response_payload[2];
  return zx::ok(display_id);
}

}  // namespace

// static
zx::result<std::unique_ptr<Lcd>> Lcd::Create(
    fdf::Namespace& incoming, display::PanelType panel_type, const PanelConfig* panel_config,
    designware_dsi::DsiHostController* designware_dsi_host_controller, bool enabled) {
  ZX_DEBUG_ASSERT(panel_config != nullptr);
  ZX_DEBUG_ASSERT(designware_dsi_host_controller != nullptr);

  static constexpr char kLcdGpioFragmentName[] = "gpio-lcd-reset";
  zx::result<fidl::ClientEnd<fuchsia_hardware_gpio::Gpio>> lcd_reset_gpio_result =
      incoming.Connect<fuchsia_hardware_gpio::Service::Device>(kLcdGpioFragmentName);
  if (lcd_reset_gpio_result.is_error()) {
    fdf::error("Failed to get gpio protocol from fragment: {}", kLcdGpioFragmentName);
    return lcd_reset_gpio_result.take_error();
  }

  fbl::AllocChecker alloc_checker;
  auto lcd = fbl::make_unique_checked<Lcd>(&alloc_checker, panel_type, panel_config,
                                           designware_dsi_host_controller,
                                           std::move(lcd_reset_gpio_result).value(), enabled);
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate memory for Lcd");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  return zx::ok(std::move(lcd));
}

Lcd::Lcd(display::PanelType panel_type, const PanelConfig* panel_config,
         designware_dsi::DsiHostController* designware_dsi_host_controller,
         fidl::ClientEnd<fuchsia_hardware_gpio::Gpio> lcd_reset_gpio, bool enabled)
    : panel_type_(panel_type),
      panel_config_(*panel_config),
      designware_dsi_host_controller_(*designware_dsi_host_controller),
      lcd_reset_gpio_(std::move(lcd_reset_gpio)) {
  ZX_DEBUG_ASSERT(panel_config != nullptr);
  ZX_DEBUG_ASSERT(designware_dsi_host_controller != nullptr);
  ZX_DEBUG_ASSERT(lcd_reset_gpio_.is_valid());
}

// This function write DSI commands based on the input buffer.
zx::result<> Lcd::PerformDisplayInitCommandSequence(std::span<const uint8_t> encoded_commands) {
  zx_status_t status = ZX_OK;
  uint32_t delay_ms = 0;
  constexpr size_t kMinCmdSize = 2;

  for (size_t i = 0; i < encoded_commands.size() - kMinCmdSize;) {
    const uint8_t cmd_type = encoded_commands[i];
    const uint8_t payload_size = encoded_commands[i + 1];
    // This command has an implicit size=2, treat it specially.
    if (cmd_type == kDsiOpSleep) {
      if (payload_size == 0 || payload_size == 0xff) {
        return zx::make_result(status);
      }
      zx::nanosleep(zx::deadline_after(zx::msec(payload_size)));
      i += 2;
      continue;
    }
    if (payload_size == 0) {
      i += kMinCmdSize;
      continue;
    }
    if ((i + payload_size + kMinCmdSize) > encoded_commands.size()) {
      fdf::error("buffer[{}] command 0x{:x} size=0x{:x} would overflow buffer size={}", i, cmd_type,
                 payload_size, encoded_commands.size());
      return zx::error(ZX_ERR_OUT_OF_RANGE);
    }

    switch (cmd_type) {
      case kDsiOpDelay:
        delay_ms = 0;
        for (size_t j = 0; j < payload_size; j++) {
          delay_ms += encoded_commands[i + 2 + j];
        }
        if (delay_ms > 0) {
          zx::nanosleep(zx::deadline_after(zx::msec(delay_ms)));
        }
        break;
      case kDsiOpGpio:
        fdf::trace("dsi_set_gpio size={} value={}", payload_size, encoded_commands[i + 3]);
        if (encoded_commands[i + 2] != 0) {
          fdf::error("Unrecognized GPIO pin ({})", encoded_commands[i + 2]);
          // We _should_ bail here, but this spec-violating behavior is present
          // in the other drivers for this hardware.
          //
          // return ZX_ERR_UNKNOWN;
        } else {
          fidl::WireResult result = lcd_reset_gpio_->SetBufferMode(
              encoded_commands[i + 3] ? fuchsia_hardware_gpio::BufferMode::kOutputHigh
                                      : fuchsia_hardware_gpio::BufferMode::kOutputLow);
          if (!result.ok()) {
            fdf::error("Failed to send SetBufferMode request to gpio: {}", result.status_string());
            return zx::error(result.status());
          }
          if (result->is_error()) {
            fdf::error("Failed to configure gpio to output: {}",
                       zx::make_result(result->error_value()));
            return result->take_error();
          }
        }
        if (payload_size > 2 && encoded_commands[i + 4]) {
          fdf::trace("dsi_set_gpio sleep {}", encoded_commands[i + 4]);
          zx::nanosleep(zx::deadline_after(zx::msec(encoded_commands[i + 4])));
        }
        break;
      case kDsiOpReadReg: {
        if (payload_size != 2) {
          fdf::error(
              "Invalid MIPI-DSI read register payload size: "
              "expected 2 (register address and count), actual {}",
              payload_size);
          return zx::error(ZX_ERR_INVALID_ARGS);
        }

        uint8_t address = encoded_commands[i + 2];
        int count = encoded_commands[i + 3];
        if (count <= 0 || count > kReadRegisterMaximumValueCount) {
          fdf::error(
              "Invalid MIPI-DSI read register value count: {}. "
              "It must be positive and no more than {}",
              count, kReadRegisterMaximumValueCount);
          return zx::error(ZX_ERR_INVALID_ARGS);
        }

        fdf::trace("Read MIPI-DSI register: address=0x{:02x} count={}", address, count);
        status = CheckDsiDeviceRegister(designware_dsi_host_controller_, address, count);
        if (status != ZX_OK) {
          fdf::error("Error reading MIPI-DSI register 0x{:02x}: {}", address,
                     zx::make_result(status));
          return zx::error(status);
        }
        break;
      }
      // All other cmd_type bytes are real DSI commands
      case kMipiDsiDtDcsShortWrite0:
      case kMipiDsiDtDcsShortWrite1:
      case kMipiDsiDtDcsLongWrite:
      case kMipiDsiDtGenShortWrite0:
      case kMipiDsiDtGenShortWrite1:
      case kMipiDsiDtGenShortWrite2:
      case kMipiDsiDtGenLongWrite: {
        fdf::trace("DSI command type: 0x{:02x} payload size: {}", cmd_type, payload_size);

        if (!IsDsiCommandPayloadSizeValid(cmd_type, payload_size)) {
          fdf::error(
              "Invalid payload size for MIPI-DSI command 0x{:02x}: "
              "actual size {}",
              cmd_type, payload_size);
          return zx::error(ZX_ERR_INVALID_ARGS);
        }

        const std::span<const uint8_t> payload = encoded_commands.subspan(i + 2, payload_size);
        const mipi_dsi::DsiCommandAndResponse commands[] = {{
            .virtual_channel_id = kMipiDsiVirtualChanId,
            .data_type = cmd_type,
            .payload = payload,
            .response_payload = {},
        }};

        zx::result<> result = designware_dsi_host_controller_.IssueCommands(commands);
        if (result.is_error()) {
          fdf::error("Failed to send command to the MIPI-DSI peripheral: {}", result);
          return result.take_error();
        }
        break;
      }
      case kMipiDsiDtDcsRead0:
      case kMipiDsiDtGenShortRead0:
      case kMipiDsiDtGenShortRead1:
      case kMipiDsiDtGenShortRead2:
        // TODO(https://fxbug.dev/322438328): Support MIPI-DSI read commands.
        fdf::error("MIPI-DSI read command 0x{:02x} is not supported", cmd_type);
        return zx::error(ZX_ERR_NOT_SUPPORTED);
      default:
        fdf::error("MIPI-DSI / panel initialization command 0x{:02x} is not supported", cmd_type);
        return zx::error(ZX_ERR_NOT_SUPPORTED);
    }
    // increment by payload length
    i += payload_size + kMinCmdSize;
  }
  return zx::make_result(status);
}

zx::result<> Lcd::Disable() {
  if (!enabled_) {
    fdf::info("LCD is already off, no work to do");
    return zx::ok();
  }
  fdf::info("Powering off the LCD [type={}]", static_cast<uint32_t>(panel_type_));
  zx::result<> power_off_result = PerformDisplayInitCommandSequence(panel_config_.dsi_off);
  if (!power_off_result.is_ok()) {
    fdf::error("Failed to decode and execute panel off sequence: {}", power_off_result);
    return power_off_result.take_error();
  }
  enabled_ = false;
  return zx::ok();
}

zx::result<> Lcd::Enable() {
  if (enabled_) {
    fdf::info("LCD is already on, no work to do");
    return zx::ok();
  }

  fdf::info("Powering on the LCD [type={}]", static_cast<uint32_t>(panel_type_));
  zx::result<> power_on_result = PerformDisplayInitCommandSequence(panel_config_.dsi_on);
  if (!power_on_result.is_ok()) {
    fdf::error("Failed to decode and execute panel init sequence: {}", power_on_result);
    return power_on_result.take_error();
  }

  // Check LCD initialization status by reading the display hardware ID.
  zx::result<uint32_t> display_id_result = GetMipiDsiDisplayId(designware_dsi_host_controller_);
  if (!display_id_result.is_ok()) {
    fdf::error("Failed to communicate with LCD Panel to get the display hardware ID: {}",
               display_id_result);
    return display_id_result.take_error();
  }
  fdf::info("LCD MIPI DSI display hardware ID: 0x{:08x}", display_id_result.value());
  zx_nanosleep(zx_deadline_after(ZX_USEC(10)));

  // LCD is on now.
  enabled_ = true;
  return zx::ok();
}

}  // namespace amlogic_display

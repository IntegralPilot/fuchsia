// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ft_firmware.h"

#include "ft_device.h"

namespace {

constexpr uint8_t kFlashStatusReg = 0x6a;
constexpr uint16_t kFlashEccDone = 0xf055;
constexpr uint16_t kFlashEraseDone = 0xf0aa;

constexpr uint8_t kFirmwareEccReg = 0x66;

constexpr uint8_t kBootIdReg = 0x90;
constexpr int kGetBootIdRetries = 10;
constexpr zx::duration kBootIdWaitAfterUnlock = zx::msec(12);

constexpr uint16_t kRombootId = 0x582c;

constexpr uint8_t kChipCoreReg = 0xa3;
constexpr int kGetChipCoreRetries = 6;
constexpr uint8_t kChipCoreFirmwareValid = 0x58;

constexpr uint8_t kFirmwareVersionReg = 0xa6;

constexpr uint8_t kWorkModeReg = 0xfc;
constexpr uint8_t kWorkModeSoftwareReset1 = 0xaa;
constexpr uint8_t kWorkModeSoftwareReset2 = 0x55;

constexpr uint8_t kHidToStdReg = 0xeb;
constexpr uint16_t kHidToStdValue = 0xaa09;

// Commands and parameters
constexpr uint8_t kResetCommand = 0x07;
constexpr zx::duration kResetWait = zx::msec(400);

constexpr uint8_t kFlashEraseCommand = 0x09;
constexpr uint8_t kFlashEraseAppArea = 0x0b;

constexpr uint8_t kUnlockBootCommand = 0x55;

constexpr uint8_t kStartEraseCommand = 0x61;
constexpr zx::duration kEraseWait = zx::msec(1350);

constexpr uint8_t kEccInitializationCommand = 0x64;
constexpr uint8_t kEccCalculateCommand = 0x65;

constexpr uint8_t kFirmwarePacketCommand = 0xbf;

constexpr uint8_t kSetEraseSizeCommand = 0xb0;

// Firmware download
constexpr int kFirmwareDownloadRetries = 2;

constexpr size_t kFirmwareMinSize = 0x120;
constexpr size_t kFirmwareMaxSize = 64l * 1024;
constexpr size_t kFirmwareVersionOffset = 0x10a;

constexpr size_t kMaxPacketAddress = 0x00ff'ffff;
constexpr size_t kMaxPacketSize = 128;

constexpr size_t kMaxEraseSize = 0xfffe;

constexpr zx::duration CalculateEccSleep(const size_t check_size) {
  return zx::msec(static_cast<ssize_t>(check_size) / 256);
}

constexpr uint16_t ExpectedWriteStatus(const uint32_t address, const size_t packet_size) {
  return (0x1000 + (address / packet_size)) & 0xffff;
}

}  // namespace

namespace ft {

uint8_t FtDevice::CalculateEcc(std::span<const uint8_t> buffer, uint8_t initial) {
  for (const uint8_t byte : buffer) {
    initial ^= byte;
  }
  return initial;
}

zx_status_t FtDevice::UpdateFirmwareIfNeeded(
    const fuchsia_hardware_input_focaltech::Metadata& metadata, display::PanelType panel_type) {
  if (!metadata.needs_firmware()) {
    return ZX_OK;
  }

  cpp20::span<const uint8_t> firmware;
  const cpp20::span<const FirmwareEntry> entries(kFirmwareEntries, kNumFirmwareEntries);
  for (const auto& entry : entries) {
    if (entry.panel_type == panel_type) {
      firmware = cpp20::span(entry.firmware_data, entry.firmware_size);
      break;
    }
  }

  if (firmware.empty()) {
    fdf::error("No firmware found for panel type {}", static_cast<uint32_t>(panel_type));
    return ZX_OK;
  }

  if (firmware.size() < kFirmwareMinSize) {
    fdf::error("Firmware binary is too small: {}", firmware.size());
    return ZX_ERR_WRONG_TYPE;
  }
  if (firmware.size() > kFirmwareMaxSize) {
    fdf::error("Firmware binary is too big: {}", firmware.size());
    return ZX_ERR_WRONG_TYPE;
  }

  const uint8_t firmware_version = firmware[kFirmwareVersionOffset];
  for (int i = 0; i < kFirmwareDownloadRetries; i++) {
    const zx::result<bool> firmware_status = CheckFirmwareAndStartRomboot(firmware_version);
    if (firmware_status.is_error()) {
      fdf::warn("Failed to check firmware and start romboot: {}", firmware_status);
      zx::result _ = Write8(kResetCommand);
      continue;
    }
    if (!firmware_status.value()) {
      return ZX_OK;
    }

    if (const zx::result result = EraseFlash(firmware.size()); result.is_error()) {
      fdf::warn("Failed to erase flash: {}", result);
      zx::result _ = Write8(kResetCommand);
      continue;
    }

    if (const zx::result result = SendFirmware(firmware); result.is_error()) {
      fdf::warn("Failed to send firmware: {}", result);
      zx::result _ = Write8(kResetCommand);
      continue;
    }

    if (const zx::result result = Write8(kResetCommand); result.is_error()) {
      continue;
    }

    zx::nanosleep(zx::deadline_after(kResetWait));

    fdf::info("Firmware download completed");
    return ZX_OK;
  }

  fdf::error("Failed to update firmware");
  return ZX_ERR_INTERNAL;
}

zx::result<bool> FtDevice::CheckFirmwareAndStartRomboot(const uint8_t firmware_version) {
  bool firmware_valid = false;
  for (int i = 0; i < kGetChipCoreRetries; i++) {
    const zx::result<uint8_t> chip_core = ReadReg8(kChipCoreReg);
    if (chip_core.is_ok() && chip_core.value() == kChipCoreFirmwareValid) {
      firmware_valid = true;
      break;
    }
    zx::nanosleep(zx::deadline_after(zx::msec(200)));
  }
  if (!firmware_valid) {
    // Firmware is invalid, the chip must already be in romboot.
    return zx::ok(true);
  }

  const zx::result<uint8_t> current_firmware_version = ReadReg8(kFirmwareVersionReg);
  if (current_firmware_version.is_ok() && current_firmware_version.value() == firmware_version) {
    // Firmware is valid and the version matches what the driver has, no need to update.
    fdf::info("Firmware version is current, skipping download");
    return zx::ok(false);
  }
  if (current_firmware_version.is_ok()) {
    fdf::info("Chip firmware ({:#02x}) doesn't match our version ({:#02x}), starting download",
              current_firmware_version.value(), firmware_version);
  } else {
    fdf::warn("Failed to read chip firmware version, starting download");
  }

  zx_status_t status;
  if (zx::result result = StartRomboot(); result.is_error()) {
    fdf::error("Failed to start romboot: {}", result);
    return result.take_error();
  }
  if ((status = WaitForRomboot()) != ZX_OK) {
    return zx::error_result(status);
  }
  return zx::ok(true);
}

zx::result<> FtDevice::StartRomboot() {
  if (zx::result result = WriteReg8(kWorkModeReg, kWorkModeSoftwareReset1); result.is_error()) {
    return result.take_error();
  }
  zx::nanosleep(zx::deadline_after(zx::msec(10)));

  if (zx::result result = WriteReg8(kWorkModeReg, kWorkModeSoftwareReset2); result.is_error()) {
    return result.take_error();
  }
  zx::nanosleep(zx::deadline_after(zx::msec(80)));

  return zx::ok();
}

zx_status_t FtDevice::WaitForRomboot() {
  zx::result<uint16_t> boot_id;
  for (int i = 0; i < kGetBootIdRetries; i++) {
    boot_id = GetBootId();
    if (boot_id.is_ok() && boot_id.value() == kRombootId) {
      return ZX_OK;
    }
  }

  if (boot_id.is_error()) {
    return boot_id.error_value();
  }

  if (boot_id.value() != kRombootId) {
    fdf::error("Timed out waiting for boot ID {:#04x}, got {:#04x}", kRombootId, boot_id.value());
    return ZX_ERR_TIMED_OUT;
  }

  return ZX_OK;
}

zx::result<uint16_t> FtDevice::GetBootId() {
  zx::result _ = WriteReg16(kHidToStdReg, kHidToStdValue);

  if (zx::result result = Write8(kUnlockBootCommand); result.is_error()) {
    fdf::error("Failed to send unlock command: {}", result);
    return result.take_error();
  }

  zx::nanosleep(zx::deadline_after(kBootIdWaitAfterUnlock));

  return ReadReg16(kBootIdReg);
}

zx::result<bool> FtDevice::WaitForFlashStatus(const uint16_t expected_value, const int tries,
                                              const zx::duration retry_sleep) {
  zx::result<uint16_t> value;
  for (int i = 0; i < tries; i++) {
    value = ReadReg16(kFlashStatusReg);
    if (value.is_ok() && value.value() == expected_value) {
      return zx::ok(true);
    }

    zx::nanosleep(zx::deadline_after(retry_sleep));
  }

  if (value.is_error()) {
    return zx::error(value.error_value());
  }
  return zx::ok(false);
}

zx::result<> FtDevice::SendFirmwarePacket(const uint32_t address, std::span<const uint8_t> packet) {
  constexpr size_t kPacketHeaderSize = 1 + 3 + 2;  // command + address + length

  if (address > kMaxPacketAddress) {
    fdf::error("Packet address {:#08x} is too large: Max packet address is {:#08x}", address,
               kMaxPacketAddress);
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  if (packet.size() > kMaxPacketSize) {
    fdf::error("Packet size of {} bytes is too large: Max packet size is {} bytes", packet.size(),
               kMaxPacketSize);
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  std::array<uint8_t, kPacketHeaderSize + kMaxPacketSize> packet_buffer = {
      kFirmwarePacketCommand,
      static_cast<uint8_t>((address >> 16) & 0xff),
      static_cast<uint8_t>((address >> 8) & 0xff),
      static_cast<uint8_t>(address & 0xff),
      static_cast<uint8_t>((packet.size() >> 8) & 0xff),
      static_cast<uint8_t>(packet.size() & 0xff),
  };
  memcpy(packet_buffer.data() + kPacketHeaderSize, packet.data(), packet.size());

  zx::result result =
      i2c_.WriteSync(std::span(packet_buffer.begin(), kPacketHeaderSize + packet.size()));
  if (result.is_error()) {
    fdf::error("Failed to write {} bytes to {:#06x}: {}", packet.size(), address, result);
    return result.take_error();
  }

  return zx::ok();
}

zx::result<> FtDevice::EraseFlash(const size_t size) {
  if (zx::result result = WriteReg8(kFlashEraseCommand, kFlashEraseAppArea); result.is_error()) {
    return result.take_error();
  }

  std::array<uint8_t, 4> erase_size_buffer;
  erase_size_buffer[0] = kSetEraseSizeCommand;
  erase_size_buffer[1] = (size >> 16) & 0xff;
  erase_size_buffer[2] = (size >> 8) & 0xff;
  erase_size_buffer[3] = size & 0xff;
  if (zx::result result = i2c_.WriteSync(erase_size_buffer); result.is_error()) {
    fdf::error("Failed to write erase size: {}", result);
    return result.take_error();
  }

  if (zx::result result = Write8(kStartEraseCommand); result.is_error()) {
    return result.take_error();
  }

  zx::nanosleep(zx::deadline_after(kEraseWait));

  zx::result<bool> erase_done = WaitForFlashStatus(kFlashEraseDone, 50, zx::msec(400));
  if (erase_done.is_error()) {
    return erase_done.take_error();
  }
  if (!erase_done.value()) {
    fdf::error("Timed out waiting for flash erase");
    return zx::error(ZX_ERR_TIMED_OUT);
  }

  return zx::ok();
}

zx::result<> FtDevice::SendFirmware(cpp20::span<const uint8_t> firmware) {
  size_t offset = 0;
  uint8_t expected_ecc = 0;
  while (offset < firmware.size()) {
    const size_t remaining = firmware.size() - offset;
    const size_t send_size = std::min(kMaxPacketSize, remaining);
    const uint32_t address = static_cast<uint32_t>(offset);
    zx::result result = SendFirmwarePacket(address, firmware.subspan(offset, send_size));
    if (result.is_error()) {
      fdf::error("Failed to send firmware packet: {}", result);
      return result.take_error();
    }

    zx::nanosleep(zx::deadline_after(zx::msec(1)));

    const uint16_t expected_status = ExpectedWriteStatus(address, send_size);
    zx::result<bool> write_done = WaitForFlashStatus(expected_status, 100, zx::msec(1));
    if (write_done.is_error()) {
      return write_done.take_error();
    }
    if (!write_done.value()) {
      fdf::warn("Timed out waiting for correct flash write status");
    }

    expected_ecc = CalculateEcc(firmware.subspan(offset, send_size), expected_ecc);
    offset += send_size;
  }

  zx::result result = CheckFirmwareEcc(firmware.size(), expected_ecc);
  if (result.is_error()) {
    fdf::error("Failed to check firmware ecc: {}", result);
    return result.take_error();
  }

  return zx::ok();
}

zx::result<> FtDevice::CheckFirmwareEcc(const size_t size, const uint8_t expected_ecc) {
  if (zx::result result = Write8(kEccInitializationCommand); result.is_error()) {
    return result.take_error();
  }

  size_t address = 0;
  for (size_t bytes_remaining = size; bytes_remaining > 0;) {
    const size_t check_size = std::min<size_t>(kMaxEraseSize, bytes_remaining);

    const std::array<uint8_t, 6> check_buffer = {
        kEccCalculateCommand,
        static_cast<uint8_t>((address >> 16) & 0xff),
        static_cast<uint8_t>((address >> 8) & 0xff),
        static_cast<uint8_t>(address & 0xff),
        static_cast<uint8_t>((check_size >> 8) & 0xff),
        static_cast<uint8_t>(check_size & 0xff),
    };
    zx::result result = i2c_.WriteSync(check_buffer);
    if (result.is_error()) {
      fdf::error("Failed to send ECC calculate command: {}", result);
      return result.take_error();
    }

    zx::nanosleep(zx::deadline_after(CalculateEccSleep(check_size)));

    zx::result<bool> ecc_done = WaitForFlashStatus(kFlashEccDone, 10, zx::msec(50));
    if (ecc_done.is_error()) {
      return ecc_done.take_error();
    }
    if (!ecc_done.value()) {
      fdf::error("Timed out waiting for ECC calculation");
      return zx::error(ZX_ERR_TIMED_OUT);
    }

    bytes_remaining -= check_size;
    address += check_size;
  }

  zx::result<uint8_t> ecc = ReadReg8(kFirmwareEccReg);
  if (ecc.is_error()) {
    return ecc.take_error();
  }

  if (ecc.value() != expected_ecc) {
    fdf::error("Firmware ECC mismatch, got {:#02x}, expected {:#02x}", ecc.value(), expected_ecc);
    return zx::error(ZX_ERR_IO_DATA_LOSS);
  }

  return zx::ok();
}

zx::result<uint8_t> FtDevice::ReadReg8(const uint8_t address) {
  std::array<uint8_t, 1> value;
  zx::result result = i2c_.ReadSync(address, value);
  if (result.is_error()) {
    fdf::error("Failed to read from {:#02x}: {}", address, result);
    return result.take_error();
  }

  return zx::ok(value[0]);
}

zx::result<uint16_t> FtDevice::ReadReg16(const uint8_t address) {
  std::array<uint8_t, 2> buffer;
  zx::result result = i2c_.ReadSync(address, buffer);
  if (result.is_error()) {
    fdf::error("Failed to read from {:#02x}: {}", address, result);
    return result.take_error();
  }

  return zx::ok(static_cast<uint16_t>((buffer[0] << 8) | buffer[1]));
}

zx::result<> FtDevice::Write8(const uint8_t value) {
  const std::array<uint8_t, 1> write_data = {value};
  zx::result result = i2c_.WriteSync(write_data);
  if (result.is_error()) {
    fdf::error("Failed to write {:#02x}: {}", value, result);
    return result.take_error();
  }

  return zx::ok();
}

zx::result<> FtDevice::WriteReg8(const uint8_t address, const uint8_t value) {
  const std::array<uint8_t, 2> write_data = {address, value};
  zx::result result = i2c_.WriteSync(write_data);
  if (result.is_error()) {
    fdf::error("Failed to write {:#02x} to {:#02x}: {}", value, address, result);
    return result.take_error();
  }

  return zx::ok();
}

zx::result<> FtDevice::WriteReg16(const uint8_t address, const uint16_t value) {
  const std::array<uint8_t, 3> write_data = {
      address,
      static_cast<uint8_t>((value >> 8) & 0xff),
      static_cast<uint8_t>(value & 0xff),
  };
  zx::result result = i2c_.WriteSync(write_data);
  if (result.is_error()) {
    fdf::error("Failed to write {:#04x} to {:#02x}: {}", value, address, result);
    return result.take_error();
  }

  return zx::ok();
}

}  // namespace ft

// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "vendor_hci.h"

#include <endian.h>
#include <lib/zx/clock.h>
#include <lib/zx/object.h>
#include <lib/zx/time.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>

#include <algorithm>
#include <cinttypes>

#include <fbl/algorithm.h>

#include "logging.h"

namespace bt_hci_intel {
namespace fhbt = fuchsia_hardware_bluetooth;

namespace {

constexpr size_t kMaxSecureSendArgLen = 252;
constexpr auto kInitTimeoutMs = zx::sec(10);
constexpr size_t kCommandCompleteEventSize =
    pw::bluetooth::emboss::CommandCompleteEvent::IntrinsicSizeInBytes();

}  // namespace

VendorHci::VendorHci(fidl::SharedClient<fuchsia_hardware_bluetooth::HciTransport>& client,
                     HciEventHandler& event_handler)
    : hci_transport_client_(client), hci_event_handler_(event_handler), manufacturer_(false) {}

// Fetch unsigned integer values from 'p' with 'fetch_len' bytes at maximum (little-endian).
uint32_t fetch_tlv_value(const uint8_t* p, size_t fetch_len) {
  size_t len = p[1];
  uint32_t val = 0;  // store the return value.

  ZX_DEBUG_ASSERT(len <= sizeof(val));
  ZX_DEBUG_ASSERT(len >= fetch_len);

  // Only load the actual number of bytes .
  fetch_len = fetch_len > len ? len : fetch_len;

  p += 2;  // Skip 'type' and 'length'. Now points to 'value'.
  for (size_t i = fetch_len; i > 0; i--) {
    val = (val << 8) + p[i - 1];
  }

  return val;
}

ReadVersionReturnParamsTlv parse_tlv_version_return_params(const uint8_t* p, size_t len) {
  ReadVersionReturnParamsTlv params = {};

  // Ensure the given byte stream contains the status code, type, and length fields.
  if (len <= 2)
    return ReadVersionReturnParamsTlv{
        .status = pw::bluetooth::emboss::StatusCode::PARAMETER_OUT_OF_MANDATORY_RANGE};

  // The first byte is the status code. Extract and strip it before we traverse the TLVs.
  params.status = static_cast<pw::bluetooth::emboss::StatusCode>(*(p++));
  len--;

  for (size_t idx = 0; idx < len;) {
    // ensure at least the Tag and the Length fields.
    size_t remain_len = len - idx;
    if (remain_len < 2)
      return ReadVersionReturnParamsTlv{
          .status = pw::bluetooth::emboss::StatusCode::PARAMETER_OUT_OF_MANDATORY_RANGE};

    // After excluding the Type and the Length field, ensure there is still room for the Value
    // field.
    size_t len_of_value = p[idx + 1];
    if ((remain_len - 2) < len_of_value) {
      return ReadVersionReturnParamsTlv{
          .status = pw::bluetooth::emboss::StatusCode::PARAMETER_OUT_OF_MANDATORY_RANGE};
    }

    switch (p[idx]) {
      uint32_t v;

      case 0x00:  // End of the TLV records.
        break;

      case 0x10:  // CNVi hardware version
        v = fetch_tlv_value(&p[idx], 4);
        params.CNVi = (((v >> 0) & 0xf) << 12) | (((v >> 4) & 0xf) << 0) | (((v >> 8) & 0xf) << 4) |
                      (((v >> 24) & 0xf) << 8);
        break;

      case 0x11:  // CNVR hardware version
        v = fetch_tlv_value(&p[idx], 4);
        params.CNVR = (((v >> 0) & 0xf) << 12) | (((v >> 4) & 0xf) << 0) | (((v >> 8) & 0xf) << 4) |
                      (((v >> 24) & 0xf) << 8);
        break;

      case 0x12:  // hardware info
        v = fetch_tlv_value(&p[idx], 4);
        params.hw_platform = (v >> 8) & 0xff;  // 0x37 for now.
        params.hw_variant = (v >> 16) & 0x3f;  // 0x17 -- Typhoon Peak
                                               // 0x1c -- Gale Peak
        break;

      case 0x16:  // Device revision
        params.device_revision = fetch_tlv_value(&p[idx], 2);
        break;

      case 0x1c:  // Current mode of operation
        params.current_mode_of_operation = fetch_tlv_value(&p[idx], 1);
        break;

      case 0x1d:  // Timestamp
        v = fetch_tlv_value(&p[idx], 2);
        params.timestamp_calendar_week = v >> 0;
        params.timestamp_year = v >> 8;
        break;

      case 0x1e:  // Build type
        params.build_type = fetch_tlv_value(&p[idx], 1);
        break;

      case 0x1f:  // Build number (it can be either 1 or 4 bytes).
        params.build_number = fetch_tlv_value(&p[idx], 4);
        break;

      case 0x28:  // Secure boot
        params.secure_boot = fetch_tlv_value(&p[idx], 1);
        break;

      case 0x2a:  // OTP lock
        params.otp_lock = fetch_tlv_value(&p[idx], 1);
        break;

      case 0x2b:  // API lock
        params.api_lock = fetch_tlv_value(&p[idx], 1);
        break;

      case 0x2c:  // debug lock
        params.debug_lock = fetch_tlv_value(&p[idx], 1);
        break;

      case 0x2d:  // Firmware build
        v = fetch_tlv_value(&p[idx], 3);
        params.firmware_build_number = v >> 0;
        params.firmware_build_calendar_week = v >> 8;
        params.firmware_build_year = v >> 16;
        break;

      case 0x2f:  // Secure boot engine type
        params.secure_boot_engine_type = fetch_tlv_value(&p[idx], 1);
        infof("Secure boot engine type: 0x{:02x}", params.secure_boot_engine_type);
        break;

      case 0x30:                             // Bluetooth device address
        ZX_DEBUG_ASSERT(len_of_value == 6);  // expect the address length is 6.
        memcpy(params.bluetooth_address, &p[idx + 2], sizeof(params.bluetooth_address));
        break;

      default:
        // unknown tag. skip it.
        warnf("Unknown firmware version TLV tag=0x{:02x}", p[idx]);
        break;
    }
    idx += 2 + len_of_value;  // Skip the 'length' and 'value'.
  }

  return params;
}

std::vector<uint8_t> VendorHci::SendReadVersion() const {
  std::vector<uint8_t> cmd_packet(pw::bluetooth::emboss::CommandHeader::IntrinsicSizeInBytes(),
                                  0x00);
  auto view = pw::bluetooth::emboss::MakeCommandHeaderView(&cmd_packet);
  view.opcode_bits().ogf().Write(kVendorOgf);
  view.opcode_bits().ocf().Write(kReadVersionOcf);
  view.parameter_total_size().Write(0);

  SendCommand(std::move(cmd_packet));

  std::vector<uint8_t> evt_packet = WaitForEventBuffer();
  if (evt_packet.size() < ReadVersionCommandCompleteEvent::IntrinsicSizeInBytes()) {
    return {};
  }
  return evt_packet;
}

std::optional<ReadVersionReturnParamsTlv> VendorHci::SendReadVersionTlv() const {
  std::vector<uint8_t> packet(ReadVersionTlvCommand::IntrinsicSizeInBytes(), 0x00);
  auto view = MakeReadVersionTlvCommandView(&packet);
  view.header().opcode_bits().ogf().Write(kVendorOgf);
  view.header().opcode_bits().ocf().Write(kReadVersionOcf);
  view.header().parameter_total_size().Write(ReadVersionTlvCommand::parameter_size());
  view.para0().Write(kVersionSupportTlv);  // Only meaningful for AX210 and later.
  SendCommand(std::move(packet));

  std::vector<uint8_t> evt_packet = WaitForEventBuffer();
  if (evt_packet.empty() || evt_packet.size() < kCommandCompleteEventSize) {
    errorf("VendorHci: ReadVersionTlv: Error reading response!");
    return std::nullopt;
  }
  const size_t return_params_size = evt_packet.size() - kCommandCompleteEventSize;
  const uint8_t* return_params_data = evt_packet.data() + kCommandCompleteEventSize;
  return parse_tlv_version_return_params(return_params_data, return_params_size);
}

std::vector<uint8_t> VendorHci::SendReadBootParams() const {
  std::vector<uint8_t> cmd_packet(pw::bluetooth::emboss::CommandHeader::IntrinsicSizeInBytes(),
                                  0x00);
  auto view = pw::bluetooth::emboss::MakeCommandHeaderView(&cmd_packet);
  view.opcode_bits().ogf().Write(kVendorOgf);
  view.opcode_bits().ocf().Write(kReadBootParamsOcf);
  view.parameter_total_size().Write(0);

  SendCommand(std::move(cmd_packet));

  return WaitForEventBuffer();
}

pw::bluetooth::emboss::StatusCode VendorHci::SendHciReset() const {
  std::vector<uint8_t> cmd_packet(pw::bluetooth::emboss::CommandHeader::IntrinsicSizeInBytes(),
                                  0x00);
  auto view = pw::bluetooth::emboss::MakeCommandHeaderView(&cmd_packet);
  view.opcode().Write(pw::bluetooth::emboss::OpCode::RESET);
  view.parameter_total_size().Write(0);
  SendCommand(std::move(cmd_packet));

  // TODO(armansito): Consider collecting a metric for initialization time
  // (successful and failing) to provide us with a better sense of how long
  // these timeouts should be.
  std::vector<uint8_t> evt_packet =
      WaitForEventBuffer(kInitTimeoutMs, pw::bluetooth::emboss::EventCode::COMMAND_COMPLETE);
  if (evt_packet.empty()) {
    errorf("VendorHci: failed while waiting for HCI_Reset response");
    return pw::bluetooth::emboss::StatusCode::UNSPECIFIED_ERROR;
  }

  auto event_view = MakeSimpleCommandCompleteEventView(&evt_packet);
  if (!event_view.IsComplete()) {
    errorf("VendorHci: HCI_Reset: received malformed response");
    return pw::bluetooth::emboss::StatusCode::UNSPECIFIED_ERROR;
  }
  return event_view.status().Read();
}

void VendorHci::SendVendorReset(uint32_t boot_address) const {
  std::vector<uint8_t> cmd_packet(VendorResetCommand::IntrinsicSizeInBytes(), 0x00);
  auto view = MakeVendorResetCommandView(&cmd_packet);
  view.header().opcode_bits().ogf().Write(kVendorOgf);
  view.header().opcode_bits().ocf().Write(kVendorResetOcf);
  view.header().parameter_total_size().Write(VendorResetCommand::parameter_size());
  view.reset_type().Write(0x00);
  view.patch_enable().Write(0x01);
  view.ddc_reload().Write(0x00);
  view.boot_option().Write(0x01);
  view.boot_address().Write(boot_address);
  SendCommand(std::move(cmd_packet));

  // Sleep for 2 seconds to let the controller process the reset.
  zx_nanosleep(zx_deadline_after(ZX_SEC(2)));
}

bool VendorHci::SendSecureSend(uint8_t type, cpp20::span<const uint8_t> bytes) const {
  size_t left = bytes.size();
  while (left > 0) {
    const size_t frag_len = std::min(left, kMaxSecureSendArgLen);
    cpp20::span<const uint8_t> frag_data = bytes.subspan(bytes.size() - left, frag_len);
    const size_t payload_size = frag_len + 1;  // +1 for type byte
    const size_t packet_size =
        pw::bluetooth::emboss::CommandHeader::IntrinsicSizeInBytes() + payload_size;

    std::vector<uint8_t> packet(packet_size, 0x00);
    auto view = pw::bluetooth::emboss::MakeCommandHeaderView(&packet);
    view.opcode_bits().ogf().Write(kVendorOgf);
    view.opcode_bits().ocf().Write(kSecureSendOcf);
    view.parameter_total_size().Write(payload_size);
    cpp20::span<uint8_t> payload(
        packet.data() + pw::bluetooth::emboss::CommandHeader::IntrinsicSizeInBytes(), payload_size);
    payload[0] = type;
    std::copy(frag_data.begin(), frag_data.end(), payload.begin() + 1);

    SendAcl(std::move(packet));
    std::vector<uint8_t> event = WaitForEventBuffer();
    if (event.empty()) {
      errorf("VendorHci: SecureSend: Error reading response!");
      return false;
    }
    auto event_view = pw::bluetooth::emboss::MakeEventHeaderView(&event);
    if (event_view.event_code().Read() == pw::bluetooth::emboss::EventCode::COMMAND_COMPLETE) {
      auto view = MakeSecureSendCommandCompleteEventView(&event);
      if (!view.IsComplete()) {
        errorf("VendorHci: SecureSend command complete event is too small ({} , expected: {})",
               event.size(), SecureSendCommandCompleteEvent::IntrinsicSizeInBytes());
        return false;
      }
      if (view.command_complete().command_opcode_bits().ogf().Read() != kVendorOgf ||
          view.command_complete().command_opcode_bits().ocf().Read() != kSecureSendOcf) {
        errorf("VendorHci: Received command complete for something else!");
      } else if (view.param().Read() != 0x00) {
        errorf("VendorHci: Received 0x{:x} instead of zero in command complete!",
               view.param().Read());
        return false;
      }
    } else if (event_view.event_code().Read() == pw::bluetooth::emboss::EventCode::VENDOR_DEBUG) {
      auto view = MakeSecureSendEventView(&event);
      infof("VendorHci: SecureSend result {:#x}, opcode: {:#x}, status: {:#x}",
            view.result().Read(), view.opcode().Read(), view.status().Read());
      if (view.result().Read()) {
        errorf("VendorHci: Result of {} indicates some error!", view.result().Read());
        return false;
      }
    }
    left -= frag_len;
  }
  return true;
}

bool VendorHci::SendAndExpect(cpp20::span<const uint8_t> command,
                              std::deque<cpp20::span<const uint8_t>> events) const {
  SendCommand(std::vector(command.begin(), command.end()));

  while (events.size() > 0) {
    auto evt_packet = WaitForEventBuffer();
    if (evt_packet.empty()) {
      return false;
    }
    auto expected = events.front();
    if ((evt_packet.size() != expected.size()) ||
        (memcmp(evt_packet.data(), expected.data(), expected.size()) != 0)) {
      errorf("VendorHci: SendAndExpect: unexpected event received");
      return false;
    }
    events.pop_front();
  }

  return true;
}

void VendorHci::EnterManufacturerMode() {
  if (manufacturer_)
    return;

  std::vector<uint8_t> cmd_packet(MfgModeChangeCommand::IntrinsicSizeInBytes(), 0x00);
  auto view = MakeMfgModeChangeCommandView(&cmd_packet);
  view.header().opcode_bits().ogf().Write(kVendorOgf);
  view.header().opcode_bits().ocf().Write(kMfgModeChangeOcf);
  view.header().parameter_total_size().Write(MfgModeChangeCommand::parameter_size());
  view.enable().Write(pw::bluetooth::emboss::GenericEnableParam::ENABLE);
  view.disable_mode().Write(MfgDisableMode::NO_PATCHES);
  SendCommand(std::move(cmd_packet));

  std::vector<uint8_t> evt_packet = WaitForEventBuffer();
  auto event = pw::bluetooth::emboss::EventHeaderView(&evt_packet);
  if (!event.IsComplete() ||
      event.event_code().Read() != pw::bluetooth::emboss::EventCode::COMMAND_COMPLETE) {
    errorf("VendorHci: EnterManufacturerMode failed");
    return;
  }

  manufacturer_ = true;
}

bool VendorHci::ExitManufacturerMode(MfgDisableMode mode) {
  if (!manufacturer_)
    return false;

  manufacturer_ = false;

  std::vector<uint8_t> cmd_packet(MfgModeChangeCommand::IntrinsicSizeInBytes(), 0x00);
  auto view = MakeMfgModeChangeCommandView(&cmd_packet);
  view.header().opcode_bits().ogf().Write(kVendorOgf);
  view.header().opcode_bits().ocf().Write(kMfgModeChangeOcf);
  view.header().parameter_total_size().Write(MfgModeChangeCommand::parameter_size());
  view.enable().Write(pw::bluetooth::emboss::GenericEnableParam::DISABLE);
  view.disable_mode().Write(mode);
  SendCommand(std::move(cmd_packet));

  std::vector<uint8_t> evt_packet = WaitForEventBuffer();
  auto event = pw::bluetooth::emboss::EventHeaderView(&evt_packet);
  if (!event.IsComplete() ||
      event.event_code().Read() != pw::bluetooth::emboss::EventCode::COMMAND_COMPLETE) {
    errorf("VendorHci: ExitManufacturerMode failed");
    return false;
  }

  return true;
}

void VendorHci::SendCommand(std::vector<uint8_t> command) const {
  hci_transport_client_->Send(fhbt::SentPacket::WithCommand(std::move(command)))
      .Then([](fidl::Result<fhbt::HciTransport::Send>& result) {
        if (!result.is_ok()) {
          errorf("VendorHci: SendCommand failed: {}", result.error_value().status_string());
          return;
        }
      });
}

void VendorHci::SendAcl(std::vector<uint8_t> command) const {
  hci_transport_client_->Send(fhbt::SentPacket::WithAcl(std::move(command)))
      .Then([](fidl::Result<fhbt::HciTransport::Send>& result) {
        if (!result.is_ok()) {
          errorf("VendorHci: SendAcl failed: {}", result.error_value().status_string());
          return;
        }
      });
}

std::vector<uint8_t> VendorHci::WaitForEventBuffer(
    zx::duration timeout, std::optional<pw::bluetooth::emboss::EventCode> expected_event) const {
  std::vector<uint8_t> buffer = hci_event_handler_.WaitForPacket(timeout);
  if (buffer.empty()) {
    errorf("VendorHci: timed out waiting for event");
    return {};
  }

  const size_t read_size = buffer.size();
  if (read_size > fhbt::kEventMax) {
    errorf("VendorHci: Packet too large, size: {}", read_size);
    return {};
  }

  if (read_size < pw::bluetooth::emboss::EventHeader::IntrinsicSizeInBytes()) {
    errorf("VendorHci: Malformed event packet expected >{} bytes, got {}",
           pw::bluetooth::emboss::EventHeader::IntrinsicSizeInBytes(), read_size);
    return {};
  }
  auto view = pw::bluetooth::emboss::MakeEventHeaderView(&buffer);

  // Compare the received payload size to what is in the header.
  const size_t rx_payload_size =
      read_size - pw::bluetooth::emboss::EventHeader::IntrinsicSizeInBytes();
  const size_t size_from_header = view.parameter_total_size().Read();
  if (size_from_header != rx_payload_size) {
    errorf(
        "VendorHci: Malformed event packet - header payload size ({}) != "
        "received ({})",
        size_from_header, rx_payload_size);
    return {};
  }

  if (expected_event && *expected_event != view.event_code().Read()) {
    tracef("VendorHci: keep waiting (expected: 0x{:02x}, got: 0x{:02x})",
           static_cast<uint8_t>(*expected_event), static_cast<uint8_t>(view.event_code().Read()));
    return {};
  }

  return buffer;
}

}  // namespace bt_hci_intel

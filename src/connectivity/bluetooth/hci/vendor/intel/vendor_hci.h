// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_BLUETOOTH_HCI_VENDOR_INTEL_VENDOR_HCI_H_
#define SRC_CONNECTIVITY_BLUETOOTH_HCI_VENDOR_INTEL_VENDOR_HCI_H_

#include <lib/zx/channel.h>

#include "hci_event_handler.h"
#include "packets.emb.h"

namespace bt_hci_intel {

constexpr uint16_t kReadVersionOcf = 0x0005;

constexpr uint8_t kVersionSupportTlv = 0xff;

constexpr uint8_t kLegacyVersionParams = 0x37;  // For AX201 and older.

constexpr uint8_t kCurrentModeOfOperationBootloader = 0x01;
constexpr uint8_t kCurrentModeOfOperationOperationalFirmware = 0x03;

struct ReadVersionReturnParamsTlv {
  pw::bluetooth::emboss::StatusCode status;
  uint16_t CNVi;
  uint16_t CNVR;
  uint8_t hw_platform;  // 0x37: only supported value at the moment
  uint8_t hw_variant;   //   * 0x17: Typhoon Peak
                        //   * 0x1c: Gale Peak
  uint16_t device_revision;
  uint8_t current_mode_of_operation;  // see kCurrentModeOfOperation*.
  uint8_t timestamp_calendar_week;
  uint8_t timestamp_year;
  uint8_t build_type;
  uint32_t build_number;
  uint8_t secure_boot;
  uint8_t otp_lock;
  uint8_t api_lock;
  uint8_t debug_lock;
  uint8_t firmware_build_number;
  uint8_t firmware_build_calendar_week;
  uint8_t firmware_build_year;
  uint8_t unknown;
  uint8_t secure_boot_engine_type;  // - 0x00: RSA
                                    // - 0x01: ECDSA
  uint8_t bluetooth_address[6];     // BD_ADDR
} __PACKED;

constexpr uint8_t kBootloaderFirmwareVariant = 0x06;
constexpr uint8_t kFirmwareFirmwareVariant = 0x23;

constexpr uint8_t kVendorOgf = 0x3F;
constexpr uint16_t kLoadPatchOcf = 0x008e;
constexpr uint16_t kSecureSendOcf = 0x0009;
constexpr uint16_t kReadBootParamsOcf = 0x000D;
constexpr uint16_t kVendorResetOcf = 0x0001;
constexpr uint16_t kMfgModeChangeOcf = 0x0011;

struct BootloaderVendorEventParams {
  uint8_t vendor_event_code;
  uint8_t vendor_params[];
} __PACKED;

// Helper functions.
uint32_t fetch_tlv_value(const uint8_t* p, size_t fetch_len);
ReadVersionReturnParamsTlv parse_tlv_version_return_params(const uint8_t* p, size_t len);

class VendorHci {
 public:
  explicit VendorHci(fidl::SharedClient<fuchsia_hardware_bluetooth::HciTransport>& client,
                     HciEventHandler& event_handler);

  // Read the version info from the hardware.
  //
  // The legacy method.
  // Returns an empty vector on error.
  std::vector<uint8_t> SendReadVersion() const;
  // The new method that supports TLV.
  std::optional<ReadVersionReturnParamsTlv> SendReadVersionTlv() const;

  std::vector<uint8_t> SendReadBootParams() const;

  pw::bluetooth::emboss::StatusCode SendHciReset() const;

  void SendVendorReset(uint32_t boot_address) const;

  bool SendSecureSend(uint8_t type, cpp20::span<const uint8_t> bytes) const;

  bool SendAndExpect(cpp20::span<const uint8_t> command,
                     std::deque<cpp20::span<const uint8_t>> events) const;

  void EnterManufacturerMode();

  bool ExitManufacturerMode(MfgDisableMode mode);

 private:
  // Intel controllers that support the "secure send" can send
  // vendor events over the bulk endpoint while in bootloader mode. We listen on
  // both types of incoming events, if provided.
  fidl::SharedClient<fuchsia_hardware_bluetooth::HciTransport>& hci_transport_client_;
  HciEventHandler& hci_event_handler_;

  // True when we are in Manufacturer Mode
  bool manufacturer_;

  void SendCommand(std::vector<uint8_t> command) const;
  // Send commands as ACL packets. When |FirmwareLoader::LoadSfi| invokes |SendSecureSend|, it
  // assumes that the commands will be sent as ACL packets. This logic is to match behavior before
  // HciTransport migration, where all the packets were sent through channels, and |SendSecureSend|
  // sends commands through acl channel at that time.
  void SendAcl(std::vector<uint8_t> command) const;

  std::vector<uint8_t> WaitForEventBuffer(
      zx::duration timeout = zx::sec(5),
      std::optional<pw::bluetooth::emboss::EventCode> expected_event = std::nullopt) const;
};

}  // namespace bt_hci_intel

#endif  // SRC_CONNECTIVITY_BLUETOOTH_HCI_VENDOR_INTEL_VENDOR_HCI_H_

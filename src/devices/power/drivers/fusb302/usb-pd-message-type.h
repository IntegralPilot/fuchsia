// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_POWER_DRIVERS_FUSB302_USB_PD_MESSAGE_TYPE_H_
#define SRC_DEVICES_POWER_DRIVERS_FUSB302_USB_PD_MESSAGE_TYPE_H_

// The comments in this file reference the USB Power Delivery Specification,
// downloadable at https://usb.org/document-library/usb-power-delivery
//
// usbpd3.1 is Revision 3.1, Version 1.7, published January 2023.

#include <zircon/assert.h>

#include <cstdint>

namespace usb_pd {

// Message type codes for messages with no data objects and no extended flag.
//
// Each message type is described in a sub-section of usbpd3.1 6.3. The
// sub-section number matches the message type's code.
//
// usbpd3.1 6.3 "Control Message", Table 6-5 "Control Message Types"
enum class ControlMessageType : uint8_t {
  kGoodCrc = 0b0'0001,                      // GoodCRC
  kGoToMinimumOperatingCurrent = 0b0'0010,  // GotoMin
  kAccept = 0b0'0011,                       // Accept
  kReject = 0b0'0100,                       // Reject
  kPing = 0b0'0101,                         // Ping
  kPowerSupplyReady = 0b0'0110,             // PS_RDY
  kGetSourceCapabilities = 0b0'0111,        // Get_Source_Cap
  kGetSinkCapabilities = 0b0'1000,          // Get_Sink_Cap
  kDataRoleSwap = 0b0'1001,                 // DR_Swap
  kPowerRoleSwap = 0b0'1010,                // PR_Swap
  kVconnSourceSwap = 0b0'1011,              // VCONN_Swap
  kWait = 0b0'1100,                         // Wait
  kSoftReset = 0b0'1101,                    // Soft_Reset

  // Introduced in PD Revision 3.0
  kDataReset = 0b0'1110,                         // Data_Reset
  kDataResetComplete = 0b0'1111,                 // Data_Reset_Complete
  kNotSupported = 0b1'0000,                      // Not_Supported
  kGetExtendedSourceCapabilities = 0b1'0001,     // Get_Source_Cap_Extended
  kGetStatus = 0b1'0010,                         // Get_Status
  kFastRoleSwap = 0b1'0011,                      // FR_Swap
  kGetProgrammablePowerSupplyStatus = 0b1'0100,  // Get_PPS_Status
  kGetCountryCodes = 0b1'0101,                   // Get_Country_Codes
  kGetExtendedSinkCapabilities = 0b1'0110,       // Get_Sink_Cap_Extended
  kGetSourceInfo = 0b1'0111,                     // Get_Source_Info
  kGetMaximumPdSpecRevision = 0b1'1000,          // Get_Revision
};

// Message type codes for messages with data objects and no extended flag.
//
// usbpd3.1 6.4 "Data Message", Table 6-6 "Data Message Types"
enum class DataMessageType : uint8_t {
  kSourceCapabilities = 0b0'0001,  // Source_Capabilities, Section 6.4.1.2
  kRequestPower = 0b0'0010,        // Request, Section 6.4.2
  kBuiltInSelfTest = 0b0'0011,     // BIST, Section 6.4.3

  // Introduced in PD Revision 3.0
  kSinkCapabilities = 0b0'0100,           // Sink_Capabilities, Section 6.4.1.3
  kBatteryStatus = 0b0'0101,              // Battery_Status, Section 6.4.5
  kAlert = 0b0'0110,                      // Alert, Section 6.4.6
  kGetCountryInfo = 0b0'0111,             // Get_Country_Info, Section 6.4.7
  kEnterUsb = 0b0'1000,                   // Enter_USB, Section 6.4.8
  kExtendedPowerRangeRequest = 0b0'1001,  // EPR_Request, Section 6.4.9
  kExtendedPowerRangeMode = 0b0'1010,     // EPR_Mode, Section 6.4.10
  kSourceInfo = 0b0'1011,                 // Source_Info, Section 6.4.11
  kMaximumPdSpecRevision = 0b0'1100,      // Revision, Section 6.4.12

  kVendorDefined = 0b0'1111,  // Vendor_Defined, Section 6.4.4
};

// Set on `MessageType` values that represent data message types.
constexpr uint8_t kMessageTypeDataBit = 64;

// Extracts the USB PD message header bit pattern from a `MessageType` value.
constexpr uint8_t kMessageTypeUsbPdValueMask = 63;

// Message type codes for messages with data objects and no extended flag.
//
// This is effectively a hand-rolled sum type (like std::variant) of
// `ControlMessageType` and `DataMessageType`. The discriminant (the type is of
// the variant value) is stored in the kMessageTypeDataBit`. Values are
// allocated such that they easily map (via `kMessageTypeUsbPdValueMask`) to the
// bit patterns used in USB PD message headers.
//
// usbpd3.1 6.4 "Data Message", Table 6-6 "Data Message Types"
enum class MessageType : uint8_t {
  kGoodCrc = static_cast<uint8_t>(ControlMessageType::kGoodCrc),
  kGoToMinimumOperatingCurrent =
      static_cast<uint8_t>(ControlMessageType::kGoToMinimumOperatingCurrent),
  kAccept = static_cast<uint8_t>(ControlMessageType::kAccept),
  kReject = static_cast<uint8_t>(ControlMessageType::kReject),
  kPing = static_cast<uint8_t>(ControlMessageType::kPing),
  kPowerSupplyReady = static_cast<uint8_t>(ControlMessageType::kPowerSupplyReady),
  kGetSourceCapabilities = static_cast<uint8_t>(ControlMessageType::kGetSourceCapabilities),
  kGetSinkCapabilities = static_cast<uint8_t>(ControlMessageType::kGetSinkCapabilities),
  kDataRoleSwap = static_cast<uint8_t>(ControlMessageType::kDataRoleSwap),
  kPowerRoleSwap = static_cast<uint8_t>(ControlMessageType::kPowerRoleSwap),
  kVconnSourceSwap = static_cast<uint8_t>(ControlMessageType::kVconnSourceSwap),
  kWait = static_cast<uint8_t>(ControlMessageType::kWait),
  kSoftReset = static_cast<uint8_t>(ControlMessageType::kSoftReset),
  kDataReset = static_cast<uint8_t>(ControlMessageType::kDataReset),
  kDataResetComplete = static_cast<uint8_t>(ControlMessageType::kDataResetComplete),
  kNotSupported = static_cast<uint8_t>(ControlMessageType::kNotSupported),
  kGetExtendedSourceCapabilities =
      static_cast<uint8_t>(ControlMessageType::kGetExtendedSourceCapabilities),
  kGetStatus = static_cast<uint8_t>(ControlMessageType::kGetStatus),
  kFastRoleSwap = static_cast<uint8_t>(ControlMessageType::kFastRoleSwap),
  kGetProgrammablePowerSupplyStatus =
      static_cast<uint8_t>(ControlMessageType::kGetProgrammablePowerSupplyStatus),
  kGetCountryCodes = static_cast<uint8_t>(ControlMessageType::kGetCountryCodes),
  kGetExtendedSinkCapabilities =
      static_cast<uint8_t>(ControlMessageType::kGetExtendedSinkCapabilities),
  kGetSourceInfo = static_cast<uint8_t>(ControlMessageType::kGetSourceInfo),
  kGetMaximumPdSpecRevision = static_cast<uint8_t>(ControlMessageType::kGetMaximumPdSpecRevision),

  kSourceCapabilities =
      kMessageTypeDataBit | static_cast<uint8_t>(DataMessageType::kSourceCapabilities),
  kRequestPower = kMessageTypeDataBit | static_cast<uint8_t>(DataMessageType::kRequestPower),
  kBuiltInSelfTest = kMessageTypeDataBit | static_cast<uint8_t>(DataMessageType::kBuiltInSelfTest),
  kSinkCapabilities =
      kMessageTypeDataBit | static_cast<uint8_t>(DataMessageType::kSinkCapabilities),
  kBatteryStatus = kMessageTypeDataBit | static_cast<uint8_t>(DataMessageType::kBatteryStatus),
  kAlert = kMessageTypeDataBit | static_cast<uint8_t>(DataMessageType::kAlert),
  kGetCountryInfo = kMessageTypeDataBit | static_cast<uint8_t>(DataMessageType::kGetCountryInfo),
  kEnterUsb = kMessageTypeDataBit | static_cast<uint8_t>(DataMessageType::kEnterUsb),
  kExtendedPowerRangeRequest =
      kMessageTypeDataBit | static_cast<uint8_t>(DataMessageType::kExtendedPowerRangeRequest),
  kExtendedPowerRangeMode =
      kMessageTypeDataBit | static_cast<uint8_t>(DataMessageType::kExtendedPowerRangeMode),
  kSourceInfo = kMessageTypeDataBit | static_cast<uint8_t>(DataMessageType::kSourceInfo),
  kMaximumPdSpecRevision =
      kMessageTypeDataBit | static_cast<uint8_t>(DataMessageType::kMaximumPdSpecRevision),
  kVendorDefined = kMessageTypeDataBit | static_cast<uint8_t>(DataMessageType::kVendorDefined),
};

constexpr bool IsDataMessageType(MessageType type) {
  return (static_cast<uint8_t>(type) & kMessageTypeDataBit) != 0;
}
constexpr MessageType MessageTypeFromDataMessageType(DataMessageType type) {
  return static_cast<MessageType>(kMessageTypeDataBit | static_cast<uint8_t>(type));
}
constexpr MessageType MessageTypeFromControlMessageType(ControlMessageType type) {
  return static_cast<MessageType>(static_cast<uint8_t>(type));
}
constexpr DataMessageType DataMessageTypeFromMessageType(MessageType type) {
  ZX_DEBUG_ASSERT(IsDataMessageType(type));
  return static_cast<DataMessageType>(static_cast<uint8_t>(type) & kMessageTypeUsbPdValueMask);
}
constexpr ControlMessageType ControlMessageTypeFromMessageType(MessageType type) {
  ZX_DEBUG_ASSERT(!IsDataMessageType(type));
  return static_cast<ControlMessageType>(static_cast<uint8_t>(type) & kMessageTypeUsbPdValueMask);
}

// Descriptor for logging and debugging.
const char* MessageTypeToString(MessageType type);

// Descriptor for logging and debugging.
inline const char* ControlMessageTypeToString(ControlMessageType type) {
  return MessageTypeToString(MessageTypeFromControlMessageType(type));
}
// Descriptor for logging and debugging.
inline const char* DataMessageTypeToString(DataMessageType type) {
  return MessageTypeToString(MessageTypeFromDataMessageType(type));
}

}  // namespace usb_pd

#endif  // SRC_DEVICES_POWER_DRIVERS_FUSB302_USB_PD_MESSAGE_TYPE_H_

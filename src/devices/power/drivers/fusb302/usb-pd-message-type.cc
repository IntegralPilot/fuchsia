// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/power/drivers/fusb302/usb-pd-message-type.h"

namespace usb_pd {

const char* MessageTypeToString(MessageType type) {
  switch (type) {
    case MessageType::kGoodCrc:
      return "GoodCRC";
    case MessageType::kGoToMinimumOperatingCurrent:
      return "GotoMin";
    case MessageType::kAccept:
      return "Accept";
    case MessageType::kReject:
      return "Reject";
    case MessageType::kPing:
      return "Ping";
    case MessageType::kPowerSupplyReady:
      return "PS_RDY";
    case MessageType::kGetSourceCapabilities:
      return "Get_Source_Cap";
    case MessageType::kGetSinkCapabilities:
      return "Get_Sink_Cap";
    case MessageType::kDataRoleSwap:
      return "DR_Swap";
    case MessageType::kPowerRoleSwap:
      return "PR_Swap";
    case MessageType::kVconnSourceSwap:
      return "VCONN_Swap";
    case MessageType::kWait:
      return "Wait";
    case MessageType::kSoftReset:
      return "Soft_Reset";
    case MessageType::kDataReset:
      return "Data_Reset";
    case MessageType::kDataResetComplete:
      return "Data_Reset_Complete";
    case MessageType::kNotSupported:
      return "Not_Supported";
    case MessageType::kGetExtendedSourceCapabilities:
      return "Get_Source_Cap_Extended";
    case MessageType::kGetStatus:
      return "Get_Status";
    case MessageType::kFastRoleSwap:
      return "FR_Swap";
    case MessageType::kGetProgrammablePowerSupplyStatus:
      return "Get_PPS_Status";
    case MessageType::kGetCountryCodes:
      return "Get_Country_Codes";
    case MessageType::kGetExtendedSinkCapabilities:
      return "Get_Sink_Cap_Extended";
    case MessageType::kGetSourceInfo:
      return "Get_Source_Info";
    case MessageType::kGetMaximumPdSpecRevision:
      return "Get_Revision";
    case MessageType::kSourceCapabilities:
      return "Source_Capabilities";
    case MessageType::kRequestPower:
      return "Request";
    case MessageType::kBuiltInSelfTest:
      return "BIST";
    case MessageType::kSinkCapabilities:
      return "Sink_Capabilities";
    case MessageType::kBatteryStatus:
      return "Battery_Status";
    case MessageType::kAlert:
      return "Alert";
    case MessageType::kGetCountryInfo:
      return "Get_Country_Info";
    case MessageType::kEnterUsb:
      return "Enter_USB";
    case MessageType::kExtendedPowerRangeRequest:
      return "EPR_Request";
    case MessageType::kExtendedPowerRangeMode:
      return "EPR_Mode";
    case MessageType::kSourceInfo:
      return "Source_Info";
    case MessageType::kMaximumPdSpecRevision:
      return "Revision";
    case MessageType::kVendorDefined:
      return "Vendor_Defined";
  }
  return "(Reserved)";
}

}  // namespace usb_pd

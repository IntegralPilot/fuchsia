// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_DRIVERS_USB_PERIPHERAL_CONFIG_PARSER_H_
#define SRC_DEVICES_USB_DRIVERS_USB_PERIPHERAL_CONFIG_PARSER_H_

#include <fidl/fuchsia.hardware.usb.peripheral/cpp/wire.h>
#include <fidl/fuchsia.hardware.usb.peripheral/cpp/wire_types.h>
#include <lib/driver/logging/cpp/logger.h>
#include <stdint.h>

#include <cstdint>
#include <map>
#include <ranges>
#include <string_view>
#include <vector>

#include <usb/cdc.h>
#include <usb/peripheral.h>
#include <usb/usb.h>

namespace usb_peripheral {

namespace peripheral = fuchsia_hardware_usb_peripheral;

// clang-format off
constexpr uint8_t kCdcMask         = 1 << 0;
constexpr uint8_t kUmsMask         = 1 << 1;
constexpr uint8_t kRndisMask       = 1 << 2;
constexpr uint8_t kAdbMask         = 1 << 3;
constexpr uint8_t kFastbootMask    = 1 << 4;
constexpr uint8_t kTestMask        = 1 << 5;
constexpr uint8_t kVsockBridgeMask = 1 << 6;
// clang-format on

// A PID lookup table for each supported function, or combination of functions.
const std::map<uint8_t, uint16_t> kPidLookup = {
    {kCdcMask, GOOGLE_USB_CDC_PID},
    {kUmsMask, GOOGLE_USB_UMS_PID},
    {kRndisMask, GOOGLE_USB_RNDIS_PID},
    {kAdbMask, GOOGLE_USB_ADB_PID},
    {kFastbootMask, GOOGLE_USB_FASTBOOT_PID},
    {kTestMask, GOOGLE_USB_FUNCTION_TEST_PID},
    {kVsockBridgeMask, GOOGLE_USB_VSOCK_BRIDGE_PID},
    // Composite device PIDs
    {kCdcMask | kTestMask, GOOGLE_USB_CDC_AND_FUNCTION_TEST_PID},
    {kCdcMask | kAdbMask, GOOGLE_USB_CDC_AND_ADB_PID},
    {kAdbMask | kVsockBridgeMask, GOOGLE_USB_ADB_AND_VSOCK_BRIDGE_PID},
    {kCdcMask | kVsockBridgeMask, GOOGLE_USB_CDC_AND_VSOCK_BRIDGE_PID},
    {kCdcMask | kFastbootMask, GOOGLE_USB_CDC_AND_FASTBOOT_PID},
    {kCdcMask | kAdbMask | kVsockBridgeMask, GOOGLE_USB_CDC_AND_ADB_AND_VSOCK_BRIDGE_PID},
    {kCdcMask | kAdbMask | kFastbootMask, GOOGLE_USB_CDC_AND_ADB_AND_FASTBOOT_PID},
};

constexpr std::string_view kDefaultSerialNumber = "0123456789ABCDEF";
constexpr std::string_view kManufacturer = "Zircon";
constexpr std::string_view kCompositeDeviceConnector = " & ";
constexpr std::string_view kCDCProductDescription = "CDC Ethernet";
constexpr std::string_view kUMSProductDescription = "USB Mass Storage";
constexpr std::string_view kRNDISProductDescription = "RNDIS Ethernet";
constexpr std::string_view kTestProductDescription = "USB Function Test";
constexpr std::string_view kADBProductDescription = "ADB";
constexpr std::string_view kVsockBridgeProductDescription = "VSOCK Bridge";
constexpr std::string_view kFastbootProductDescription = "Fastboot";

constexpr peripheral::wire::FunctionDescriptor kCDCFunctionDescriptor = {
    .interface_class = USB_CLASS_COMM,
    .interface_subclass = USB_CDC_SUBCLASS_ETHERNET,
    .interface_protocol = 0,
};

constexpr peripheral::wire::FunctionDescriptor kUMSFunctionDescriptor = {
    .interface_class = USB_CLASS_MSC,
    .interface_subclass = USB_SUBCLASS_MSC_SCSI,
    .interface_protocol = USB_PROTOCOL_MSC_BULK_ONLY,
};

constexpr peripheral::wire::FunctionDescriptor kRNDISFunctionDescriptor = {
    .interface_class = USB_CLASS_MISC,
    .interface_subclass = USB_SUBCLASS_MSC_RNDIS,
    .interface_protocol = USB_PROTOCOL_MSC_RNDIS_ETHERNET,
};

constexpr peripheral::wire::FunctionDescriptor kADBFunctionDescriptor = {
    .interface_class = USB_CLASS_VENDOR,
    .interface_subclass = USB_SUBCLASS_ADB,
    .interface_protocol = USB_PROTOCOL_ADB,
};

constexpr peripheral::wire::FunctionDescriptor kFfxFunctionDescriptor = {
    .interface_class = USB_CLASS_VENDOR,
    .interface_subclass = USB_SUBCLASS_VSOCK_BRIDGE,
    .interface_protocol = USB_PROTOCOL_VSOCK_BRIDGE,
};

constexpr peripheral::wire::FunctionDescriptor kFastbootFunctionDescriptor = {
    .interface_class = USB_CLASS_VENDOR,
    .interface_subclass = USB_SUBCLASS_FASTBOOT,
    .interface_protocol = USB_PROTOCOL_FASTBOOT,
};

constexpr peripheral::wire::FunctionDescriptor kTestFunctionDescriptor = {
    .interface_class = USB_CLASS_VENDOR,
    .interface_subclass = 0,
    .interface_protocol = 0,
};

struct FunctionDefinition {
  uint8_t tag_mask;
  fuchsia_hardware_usb_peripheral::wire::FunctionDescriptor descriptor;
  std::string_view description;
};

const std::map<std::string_view, FunctionDefinition> kAllFunctions = {
    {"cdc",
     {
         .tag_mask = kCdcMask,
         .descriptor = kCDCFunctionDescriptor,
         .description = kCDCProductDescription,
     }},
    {"ums",
     {
         .tag_mask = kUmsMask,
         .descriptor = kUMSFunctionDescriptor,
         .description = kUMSProductDescription,
     }},
    {"rndis",
     {
         .tag_mask = kRndisMask,
         .descriptor = kRNDISFunctionDescriptor,
         .description = kRNDISProductDescription,
     }},
    {"adb",
     {
         .tag_mask = kAdbMask,
         .descriptor = kADBFunctionDescriptor,
         .description = kADBProductDescription,
     }},
    {"fastboot",
     {
         .tag_mask = kFastbootMask,
         .descriptor = kFastbootFunctionDescriptor,
         .description = kFastbootProductDescription,
     }},
    {"test",
     {
         .tag_mask = kTestMask,
         .descriptor = kTestFunctionDescriptor,
         .description = kTestProductDescription,
     }},
    {"vsock_bridge",
     {
         .tag_mask = kVsockBridgeMask,
         .descriptor = kFfxFunctionDescriptor,
         .description = kVsockBridgeProductDescription,
     }},
};

// Class for generating USB peripheral config struct.
// Currently supports getting a CDC Ethernet config by default, or parse the boot args
// `driver.usb.peripheral` string to compose different functionality.
class PeripheralConfigParser {
 public:
  template <std::ranges::range R>
  zx_status_t AddFunctions(R&& functions) {
    TRACE_DURATION("usb-peripheral", __func__);

    if (functions.empty()) {
      fdf::info("No functions found");
      return ZX_OK;
    }

    // resolve and then sort the functions by their product ids so that they are added and combined
    // in a predictable order.
    std::vector<FunctionDefinition> function_defs;
    for (const auto& subrange : functions) {
      const std::string_view function{subrange.begin(), subrange.end()};
      const auto& function_def = kAllFunctions.find(function);
      if (function_def != kAllFunctions.end()) {
        if (!kPidLookup.contains(function_def->second.tag_mask)) {
          fdf::error("No supported USB PID for function {}", function);
          return ZX_ERR_INVALID_ARGS;
        }
        function_defs.push_back(function_def->second);
      } else {
        fdf::error("Function not supported: {}", function);
        return ZX_ERR_INVALID_ARGS;
      }
    }

    std::ranges::sort(function_defs, [](FunctionDefinition& left, FunctionDefinition& right) {
      return kPidLookup.at(left.tag_mask) < kPidLookup.at(right.tag_mask);
    });

    zx_status_t status = ZX_OK;
    for (const auto& function : function_defs) {
      function_configs_.push_back(function.descriptor);
      status = SetCompositeProductDescription(function.tag_mask, function.description);

      if (status != ZX_OK) {
        fdf::error("Failed to set product description for function: {}", function.description);
        return status;
      }
    }

    return ZX_OK;
  }

  uint16_t vid() const { return GOOGLE_USB_VID; }
  uint16_t pid() const { return pid_; }
  std::string manufacturer() const { return std::string(kManufacturer); }
  std::string product() const { return product_desc_; }

  std::vector<fuchsia_hardware_usb_peripheral::wire::FunctionDescriptor>& functions() {
    return function_configs_;
  }

 private:
  // Helper function for determining the pid and product description.
  zx_status_t SetCompositeProductDescription(uint8_t tag_mask, const std::string_view& desc);

  // USB PID descriptor value.
  uint16_t pid_ = 0;

  // A value used to accumulate configured functions. This is a bitfield, whose individual bits map
  // to the following functions:
  //   00000001: CDC
  //   00000010: UMS
  //   00000100: RNDIS
  //   00001000: ADB
  //   00010000: Fastboot
  //   00100000: Test
  //   01000000: Vsock bridge
  //   10000000: <unused>
  uint8_t tag_ = 0;

  std::string product_desc_;
  std::vector<fuchsia_hardware_usb_peripheral::wire::FunctionDescriptor> function_configs_;
};

}  // namespace usb_peripheral

#endif  // SRC_DEVICES_USB_DRIVERS_USB_PERIPHERAL_CONFIG_PARSER_H_

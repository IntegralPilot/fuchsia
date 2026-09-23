// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_POWER_DRIVERS_FUSB302_USB_PD_SINK_POLICY_H_
#define SRC_DEVICES_POWER_DRIVERS_FUSB302_USB_PD_SINK_POLICY_H_

// The comments in this file reference the USB Power Delivery Specification,
// downloadable at https://usb.org/document-library/usb-power-delivery
//
// usbpd3.1 is Revision 3.1, Version 1.7, published January 2023.

#include <lib/stdcompat/inplace_vector.h>
#include <lib/stdcompat/span.h>
#include <zircon/assert.h>

#include <cstdint>
#include <utility>

#include "src/devices/power/drivers/fusb302/usb-pd-message-objects.h"
#include "src/devices/power/drivers/fusb302/usb-pd-message.h"

namespace usb_pd {

struct SinkPolicyInfo {
  // Minimum voltage accepted by the power system, in mV. Must be positive.
  int32_t min_voltage_mv;

  // Maximum voltage accepted by the power system, in mV.
  int32_t max_voltage_mv;

  // Maximum power consumption, in mW. Must be positive.
  int32_t max_power_mw;

  bool supports_usb_communications = true;

  bool IsValid() const;
};

// Conveys device-specific knowledge to the USB PD subsystem.
class SinkPolicy {
 public:
  explicit SinkPolicy(const SinkPolicyInfo& policy_info);

  SinkPolicy(const SinkPolicy&) = delete;
  SinkPolicy& operator=(const SinkPolicy&) = delete;

  ~SinkPolicy();

  // Adjusts to new power information from the Source.
  //
  // `capabilities` must be a Source_Capabilities message.
  //
  // `GetRequestData()` will return an RDO (power Request Data Object) based on
  // the PDOs (Power Data Objects) in `capabilities`.
  void DidReceiveSourceCapabilities(const Message& capabilities);

  // An RDO (power Request Data Object) that best conveys the Sink policy.
  //
  // The policy must always produce a valid RDO.
  PowerRequestData GetPowerRequest() const;

  // The PDOs to be included in a Sink_Capabilities message.
  cpp20::span<const uint32_t> GetSinkCapabilities() const;

 private:
  struct PowerDataSuitability;

  // Fills in `sink_capabilities_` based on the policy.
  void PopulateSinkCapabilities();

  // Logs the data in `sink_capabilities_`.
  void LogSourcePowerCapabilities();

  // Evaluates a single Fixed Power Supply against the policy.
  //
  // The returned PowerRequestData must have RDO-specific fields set, as well as
  // the `capability_mismatch` common field. All other common fields will be
  // set elsewhere.
  std::pair<PowerRequestData, PowerDataSuitability> ScoreFixedPower(FixedPowerSupplyData power_data,
                                                                    int32_t data_position) const;

  // Returns a PDO based on `fixed_power` but with bits 29-20 set to the policy.
  //
  // This method sets the bits expected on the first PDO in a Sink_Capabilities
  // message.
  SinkFixedPowerSupplyData SetAdditionalInformation(SinkFixedPowerSupplyData fixed_power) const;

  // Returns a RDO based on `request` but with common fields set to the policy.
  //
  // This method sets the fields common to all RDO formats.
  PowerRequestData SetCommonRequestFields(PowerRequestData request) const;

  SinkPolicyInfo policy_info_;

  cpp26::inplace_vector<uint32_t, Header::kMaxDataObjectCount> sink_capabilities_;

  // Empty until `DidReceiveSourceCapabilities()` is called.
  cpp26::inplace_vector<PowerData, Header::kMaxDataObjectCount> source_capabilities_;
};

}  // namespace usb_pd

#endif  // SRC_DEVICES_POWER_DRIVERS_FUSB302_USB_PD_SINK_POLICY_H_

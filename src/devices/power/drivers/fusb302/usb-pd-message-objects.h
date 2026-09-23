// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_POWER_DRIVERS_FUSB302_USB_PD_MESSAGE_OBJECTS_H_
#define SRC_DEVICES_POWER_DRIVERS_FUSB302_USB_PD_MESSAGE_OBJECTS_H_

// The comments in this file reference the USB Power Delivery Specification,
// downloadable at https://usb.org/document-library/usb-power-delivery
//
// usbpd3.1 is Revision 3.1, Version 1.7, published January 2023.

#include <zircon/assert.h>

#include <cstdint>

#include <hwreg/bitfields.h>

namespace usb_pd {

// Power supply type
//
// usbpd3.1 6.4.1 "Capabilities Message", Table 6-7 "Power Data Object"
enum class PowerSupplyType : uint8_t {
  kFixedSupply = 0b00,
  kBattery = 0b01,
  kVariableSupply = 0b10,
  kAugmentedPowerDataObject = 0b11,  // APDO
};

// Precursor for all PDO (Power Data Object) types.
//
// Conceptually, the PDO in the USB PD spec is a sum type (like std::variant).
// Values have 32 bits, and the top 2 bits are used as the type discriminant.
// `PowerData` contains the logic for encoding and decoding the discriminant.
//
// This high-level representation is defined in the introduction of usbpd3.1
// usbpd3.1 6.4.1 "Capabilities Message". Subtypes are defined in the
// subsections of usbpd3.1 6.4.1.2 "Source_Capabilities Message" and of 6.4.1.3
// "Sink Capabilities Message"
struct PowerData {
 public:
  uint32_t bits;

  // Discriminant for PDO (Powey Data Object) types.
  DEF_ENUM_SUBFIELD(bits, PowerSupplyType, 31, 30, supply_type);

  // Support explicit casting from uint32_t.
  explicit PowerData(uint32_t bits) : bits(bits) {}

  // Value type, copying is allowed.
  PowerData(const PowerData&) = default;
  PowerData& operator=(const PowerData&) = default;

  // Trivially destructible.
  ~PowerData() = default;

  // Support explicit casting to uint32_t.
  explicit operator uint32_t() const { return bits; }

  constexpr bool operator==(const PowerData& other) const = default;
};

// A fixed power source's ability to briefly exceed the operating current.
//
// usbpd3.1 measures the Source's ability to handle spikes of three maximum
// durations (1ms / 2ms / 10ms) out of a 20ms duty cycle. In all cases, the
// current consumption over an entire duty cycle must still match the source's
// operating current. So, a spike must be compensated by lower current
// consumption during the rest of the duty cycle.
//
// usbpd3.1 6.4.1.2.2.8 "Peak Current"
enum class PeakCurrentSupport {
  kNoOverload = 0b00,

  // Overload limit: 150% for 1ms / 125% for 2ms / 110% for 10ms
  kOverloadLevel1 = 0b01,

  // Overload limit: 200% for 1ms / 150% for 2ms / 125% for 10ms
  kOverloadLevel2 = 0b10,

  // Overload limit: 200% for 1ms / 175% for 2ms / 150% for 10ms
  kOverloadLevel3 = 0b11,
};

// Well-regulated power supply that maintains a fixed voltage.
//
// See `PowerData` for a general description of the PDO (Power Data Object)
// representation.
//
// Sink requirements for a well-regulated power supply are expressed via
// `SinkFixedPowerSupplyData` instances.
//
// usbpd3.1 6.4.1.2.2 "Fixed Supply Power Data Object"
class FixedPowerSupplyData {
 private:
  // Needed for the following to compile.
  uint32_t bits_;

 public:
  // Indicates support for the PR_Swap message.
  //
  // Only used on the first PDO in a Source_Capabilities message.
  DEF_SUBBIT(bits_, 29, supports_dual_role_power);

  // Indicates whether the Sink must follow USB suspend rules.
  //
  // usbpd3.1 names this field USB Suspend Supported, but it conveys
  // requirements for the Sink. The requirements apply after a PD Contract is
  // negotiated. 6.4.1.2.2 "USB Suspend Supported", states that a peripheral may
  // draw up to 25 mW (pSnkSusp) and a hub may draw up to 125 mW (pHubSusp)
  // during suspend, and defers to usb2, usb3.2 and usb4.
  //
  // Only used on the first PDO in a Source_Capabilities message.
  DEF_SUBBIT(bits_, 28, requires_usb_suspend);

  // True iff the Source draws power from an "unlimited" power source.
  //
  // Only used on the first PDO in a Source_Capabilities message.
  DEF_SUBBIT(bits_, 27, has_unconstrained_power);

  // Indicates if the Source can communicate over a USB data channel.
  //
  // The data channels are D+/- introduced in usb2.0 and SS Tx/Rx introduced in
  // usb3.0.
  //
  // Only used on the first PDO in a Source_Capabilities message.
  DEF_SUBBIT(bits_, 26, supports_usb_communications);

  // Indicates support for the DR_Swap message.
  //
  // Only used on the first PDO in a Source_Capabilities message.
  DEF_SUBBIT(bits_, 25, supports_dual_role_data);

  // True iff the Port can receive messages with more than 26 data bytes.
  //
  // If true, the underlying BMC PHY must be designed to receive and transmit
  // extended messages up to 260-bytes.
  //
  // Only used on the first PDO in a Source_Capabilities message.
  DEF_SUBBIT(bits_, 24, supports_unchunked_extended_messages);

  // True iff the Source supports EPR (Extended Power Range) messages.
  //
  // EPR power sources can supply more than 100 W.
  //
  // Only used on the first PDO in a Source_Capabilities message.
  DEF_SUBBIT(bits_, 23, supports_extended_power_range);

  // The power source's ability to briefly exceed the operating current.
  DEF_ENUM_SUBFIELD(bits_, PeakCurrentSupport, 21, 20, peak_current);

  // The power source's fixed voltage, in multiplies of 50 mV.
  DEF_SUBFIELD(bits_, 19, 10, voltage_50mv);

  // The maximum current offered by the source, in multiples of 10 mA.
  DEF_SUBFIELD(bits_, 9, 0, maximum_current_10ma);

  // The power source's fixed voltage, in mV (millivolts).
  int32_t voltage_mv() const {
    // The multiplication will not overflow (causing UB) because
    // `voltage_50mv` is a 10-bit field. The maximum product is 51,150.
    return static_cast<int32_t>(static_cast<int32_t>(voltage_50mv()) * 50);
  }
  FixedPowerSupplyData& set_voltage_mv(int32_t voltage_mv) {
    return set_voltage_50mv(voltage_mv / 50);
  }

  // The maximum current offered by the source, in mA (milliamperes).
  int32_t maximum_current_ma() const {
    // The multiplication will not overflow (causing UB) because
    // `maximum_current_10ma` is a 10-bit field. The maximum product is 10,240.
    return static_cast<int32_t>(static_cast<int32_t>(maximum_current_10ma()) * 10);
  }
  FixedPowerSupplyData& set_maximum_current_ma(int32_t current_ma) {
    return set_maximum_current_10ma(current_ma / 10);
  }

  // Debug-checked casting from PowerData.
  explicit FixedPowerSupplyData(PowerData power_data) : bits_(power_data.bits) {
    ZX_DEBUG_ASSERT(supply_type() == PowerSupplyType::kFixedSupply);
  }

  // Instance with all fields except for type set to zero.
  //
  // This is an invalid PDO (power data object). At a minimum, voltage and
  // maximum current must be set before use in a PD message.
  FixedPowerSupplyData()
      : FixedPowerSupplyData(PowerData(0).set_supply_type(PowerSupplyType::kFixedSupply)) {}

  // Value type, copying is allowed.
  FixedPowerSupplyData(const FixedPowerSupplyData&) = default;
  FixedPowerSupplyData& operator=(const FixedPowerSupplyData&) = default;

  // Trivially destructible.
  ~FixedPowerSupplyData() = default;

  // Support explicit casting to uint32_t.
  explicit operator uint32_t() const { return bits_; }

  constexpr bool operator==(const FixedPowerSupplyData& other) const = default;

 private:
  // Must be kFixedSupply.
  DEF_ENUM_SUBFIELD(bits_, PowerSupplyType, 31, 30, supply_type);
};

// Power supply that lacks good voltage regulation, so its voltage may vary.
//
// See `PowerData` for a general description of the PDO (Power Data Object)
// representation.
//
// usbpd3.1 6.4.1.2.3 "Variable Supply (non-Battery) Power Data Object" and
// 6.4.1.3.2 "Variable Supply (non-Battery) Power Data Object".
class VariablePowerSupplyData {
 private:
  // Needed for the following to compile.
  uint32_t bits_;

 public:
  // The power source's maximum voltage, in multiplies of 50 mV.
  DEF_SUBFIELD(bits_, 29, 20, maximum_voltage_50mv);

  // The power source's minimum voltage, in multiplies of 50 mV.
  DEF_SUBFIELD(bits_, 19, 10, minimum_voltage_50mv);

  // The maximum current offered by the source, in multiples of 10 mA.
  DEF_SUBFIELD(bits_, 9, 0, maximum_current_10ma);

  // The power source's maximum voltage, in mV (millivolts).
  int32_t maximum_voltage_mv() const {
    // The multiplication will not overflow (causing UB) because
    // `maximum_voltage_50mv` is a 10-bit field. The maximum product is 51,150.
    return static_cast<int32_t>(static_cast<int32_t>(maximum_voltage_50mv()) * 50);
  }
  VariablePowerSupplyData& set_maximum_voltage_mv(int32_t voltage_mv) {
    return set_maximum_voltage_50mv(voltage_mv / 50);
  }

  // The power source's minimum voltage, in mV (millivolts).
  int32_t minimum_voltage_mv() const {
    return static_cast<int32_t>(static_cast<int32_t>(minimum_voltage_50mv()) * 50);
  }
  VariablePowerSupplyData& set_minimum_voltage_mv(int32_t voltage_mv) {
    // The multiplication will not overflow (causing UB) because
    // `minimum_voltage_50mv` is a 10-bit field. The maximum product is 51,150.
    return set_minimum_voltage_50mv(voltage_mv / 50);
  }

  // The maximum current offered by the source, in mA (milliamperes).
  int32_t maximum_current_ma() const {
    // The multiplication will not overflow (causing UB) because
    // `maximum_current_10ma` is a 10-bit field. The maximum product is 10,240.
    return static_cast<int32_t>(static_cast<int32_t>(maximum_current_10ma()) * 10);
  }
  VariablePowerSupplyData& set_maximum_current_ma(int32_t current_ma) {
    return set_maximum_current_10ma(current_ma / 10);
  }

  // Debug-checked casting from PowerData.
  explicit VariablePowerSupplyData(PowerData power_data) : bits_(power_data.bits) {
    ZX_DEBUG_ASSERT(supply_type() == PowerSupplyType::kVariableSupply);
  }

  // Instance with all fields except for type set to zero.
  //
  // This is an invalid PDO (power data object). At a minimum, voltage and
  // maximum current must be set before use in a PD message.
  VariablePowerSupplyData()
      : VariablePowerSupplyData(PowerData(0).set_supply_type(PowerSupplyType::kVariableSupply)) {}

  // Value type, copying is allowed.
  VariablePowerSupplyData(const VariablePowerSupplyData&) = default;
  VariablePowerSupplyData& operator=(const VariablePowerSupplyData&) = default;

  // Trivially destructible.
  ~VariablePowerSupplyData() = default;

  // Support explicit casting to uint32_t.
  explicit operator uint32_t() const { return bits_; }

  constexpr bool operator==(const VariablePowerSupplyData& other) const = default;

 private:
  // Must be kVariableSupply.
  DEF_ENUM_SUBFIELD(bits_, PowerSupplyType, 31, 30, supply_type);
};

// Power supply that lacks good voltage regulation, so its voltage may vary.
//
// See `PowerData` for a general description of the PDO (Power Data Object)
// representation.
//
// usbpd3.1 6.4.1.2.4 "Variable Supply (non-Battery) Power Data Object" and
// 6.4.1.3.3 "Battery Supply Power Data Object"
class BatteryPowerSupplyData {
 private:
  // Needed for the following to compile.
  uint32_t bits_;

 public:
  // The power source's maximum voltage, in multiplies of 50 mV.
  DEF_SUBFIELD(bits_, 29, 20, maximum_voltage_50mv);

  // The power source's minimum voltage, in multiplies of 50 mV.
  DEF_SUBFIELD(bits_, 19, 10, minimum_voltage_50mv);

  // The maximum power offered by the source, in multiples of 250 mW.
  DEF_SUBFIELD(bits_, 9, 0, maximum_power_250mw);

  // The power source's maximum voltage, in mV (millivolts).
  int32_t maximum_voltage_mv() const {
    // The multiplication will not overflow (causing UB) because
    // `maximum_voltage_50mv` is a 10-bit field. The maximum product is 51,150.
    return static_cast<int32_t>(static_cast<int32_t>(maximum_voltage_50mv()) * 50);
  }
  BatteryPowerSupplyData& set_maximum_voltage_mv(int32_t voltage_mv) {
    return set_maximum_voltage_50mv(voltage_mv / 50);
  }

  // The power source's minimum voltage, in mV (millivolts).
  int32_t minimum_voltage_mv() const {
    // The multiplication will not overflow (causing UB) because
    // `minimum_voltage_50mv` is a 10-bit field. The maximum product is 51,150.
    return static_cast<int32_t>(static_cast<int32_t>(minimum_voltage_50mv()) * 50);
  }
  BatteryPowerSupplyData& set_minimum_voltage_mv(int32_t voltage_mv) {
    return set_minimum_voltage_50mv(voltage_mv / 50);
  }

  // The maximum power offered by the source, in mW (milliwatts).
  int32_t maximum_power_mw() const {
    // The multiplication will not overflow (causing UB) because
    // `maximum_power_250mw` is a 10-bit field. The maximum product is 255,750.
    return static_cast<int32_t>(static_cast<int32_t>(maximum_power_250mw()) * 250);
  }
  BatteryPowerSupplyData& set_maximum_power_mw(int32_t power_ma) {
    return set_maximum_power_250mw(power_ma / 250);
  }

  // Debug-checked casting from PowerData.
  explicit BatteryPowerSupplyData(PowerData power_data) : bits_(power_data.bits) {
    ZX_DEBUG_ASSERT(supply_type() == PowerSupplyType::kBattery);
  }

  // Instance with all fields except for type set to zero.
  //
  // This is an invalid PDO (power data object). At a minimum, voltage and
  // maximum current must be set before use in a PD message.
  BatteryPowerSupplyData()
      : BatteryPowerSupplyData(PowerData(0).set_supply_type(PowerSupplyType::kBattery)) {}

  // Value type, copying is allowed.
  BatteryPowerSupplyData(const BatteryPowerSupplyData&) = default;
  BatteryPowerSupplyData& operator=(const BatteryPowerSupplyData&) = default;

  // Trivially destructible.
  ~BatteryPowerSupplyData() = default;

  // Support explicit casting to uint32_t.
  explicit operator uint32_t() const { return bits_; }

  constexpr bool operator==(const BatteryPowerSupplyData& other) const = default;

 private:
  // Must be kBattery.
  DEF_ENUM_SUBFIELD(bits_, PowerSupplyType, 31, 30, supply_type);
};

// Values for `fast_swap_current_requirement` in SinkFixedPowerSupplyData.
//
// usbpd3.1 6.4.1.3.1 "Sink Fixed Supply Power Data Object", sub-table inside
// the "Fast Role Swap required USB Type-C Current" row
enum class FastSwapCurrentRequirement {
  kNotSupported = 0b00,
  kDefaultUsb = 0b01,
  k1500mA = 0b10,
  k3000mA = 0b11,
};

// Well-regulated power supply that maintains a fixed voltage.
//
// See `PowerData` for a general description of the PDO (Power Data Object)
// representation.
//
// Source capabilities for a well-regulated power supply are expressed via
// `FixedPowerSupplyData` instances.
//
// usbpd3.1 6.4.1.3.1 "Sink Fixed Supply Power Data Object"
class SinkFixedPowerSupplyData {
 private:
  // Needed for the following to compile.
  uint32_t bits_;

 public:
  // Indicates support for the PR_Swap message.
  //
  // Only used on the first PDO in a Sink_Capabilities message.
  DEF_SUBBIT(bits_, 29, supports_dual_role_power);

  // True if the Sink needs more than vSafe5V for full functionality.
  //
  // Only used on the first PDO in a Sink_Capabilities message.
  DEF_SUBBIT(bits_, 28, requires_more_than_5v);

  // True iff the Sink draws power from an "unlimited" power source.
  //
  // Only used on the first PDO in a Sink_Capabilities message.
  DEF_SUBBIT(bits_, 27, has_unconstrained_power);

  // Indicates if the Sink can communicate over a USB data channel.
  //
  // The data channels are D+/- introduced in usb2.0 and SS Tx/Rx introduced in
  // usb3.0.
  //
  // Only used on the first PDO in a Sink_Capabilities message.
  DEF_SUBBIT(bits_, 26, supports_usb_communications);

  // Indicates support for the DR_Swap message.
  //
  // Only used on the first PDO in a Sink_Capabilities message.
  DEF_SUBBIT(bits_, 25, supports_dual_role_data);

  // Current level needed by Sink after a Fast Role Swap.
  //
  // If the value is `kNotSupported`, the Source will not initiate a Fast Role
  // Swap request.
  //
  // Only used on the first PDO in a Sink_Capabilities message.
  DEF_ENUM_SUBFIELD(bits_, FastSwapCurrentRequirement, 24, 23, fast_swap_current_requirement);

  // The power source's fixed voltage, in multiplies of 50 mV.
  DEF_SUBFIELD(bits_, 19, 10, voltage_50mv);

  // The maximum current offered by the source, in multiples of 10 mA.
  DEF_SUBFIELD(bits_, 9, 0, maximum_current_10ma);

  // The power source's fixed voltage, in mV (millivolts).
  int32_t voltage_mv() const {
    // The multiplication will not overflow (causing UB) because
    // `voltage_50mv` is a 10-bit field. The maximum product is 51,150.
    return static_cast<int32_t>(static_cast<int32_t>(voltage_50mv()) * 50);
  }
  SinkFixedPowerSupplyData& set_voltage_mv(int32_t voltage_mv) {
    return set_voltage_50mv(voltage_mv / 50);
  }

  // The maximum current offered by the source, in mA (milliamperes).
  int32_t maximum_current_ma() const {
    // The multiplication will not overflow (causing UB) because
    // `maximum_current_10ma` is a 10-bit field. The maximum product is 10,240.
    return static_cast<int32_t>(static_cast<int32_t>(maximum_current_10ma()) * 10);
  }
  SinkFixedPowerSupplyData& set_maximum_current_ma(int32_t current_ma) {
    return set_maximum_current_10ma(current_ma / 10);
  }

  // Debug-checked casting from PowerData.
  explicit SinkFixedPowerSupplyData(PowerData power_data) : bits_(power_data.bits) {
    ZX_DEBUG_ASSERT(supply_type() == PowerSupplyType::kFixedSupply);
  }

  // Instance with all fields except for type set to zero.
  //
  // This is an invalid PDO (power data object). At a minimum, voltage and
  // maximum current must be set before use in a PD message.
  SinkFixedPowerSupplyData()
      : SinkFixedPowerSupplyData(PowerData(0).set_supply_type(PowerSupplyType::kFixedSupply)) {}

  // Value type, copying is allowed.
  SinkFixedPowerSupplyData(const SinkFixedPowerSupplyData&) = default;
  SinkFixedPowerSupplyData& operator=(const SinkFixedPowerSupplyData&) = default;

  // Trivially destructible.
  ~SinkFixedPowerSupplyData() = default;

  // Support explicit casting to uint32_t.
  explicit operator uint32_t() const { return bits_; }

  constexpr bool operator==(const SinkFixedPowerSupplyData& other) const = default;

 private:
  // Must be kFixedSupply.
  DEF_ENUM_SUBFIELD(bits_, PowerSupplyType, 31, 30, supply_type);
};

// Fields common to all RDO (power Request Data Object) types.
//
// Conceptually, the RDO in the USB PD spec is a sum type (like std::variant).
// Values have 32 bits. The type discriminant is conveyed indirectly by the top
// 4 bits, which must be combined with out-of-band information. Concretely, the
// top 4 bits point to a PDO (Power Data Object) in a different message, and the
// PDO's type determines the RDO type.
//
// usbpd3.1 6.4.2 "Request Message" defines the high-level representation and
// the formats for all subtypes. Subsections define field semantics.
class PowerRequestData {
 protected:
  // Needed for the following to compile.
  uint32_t bits_;

 public:
  // The position of the related PDO (Power Data Object) in a related message.
  //
  // Each RDO is based on a PDO in a message that lists power Source
  // capabilities. The PDO subtype determines the RDO's subtype, and PDO fields
  // set limits on the RDO fields. For example, an RDO based on a Fixed Supply
  // PDO may not request more current than is advertised in the PDO.
  //
  // This field uses 1-based indexing, and the value 0 is reserved.
  DEF_SUBFIELD(bits_, 31, 28, related_power_data_object_position);

  // True if the Sink can honor GotoMin messages.
  //
  // If true, the Sink must react to GotoMin messages by reducing its power
  // consumption to the minimum stated in the request object.
  DEF_SUBBIT(bits_, 27, supports_power_give_back);

  // True if the requested power is not sufficient for all the Sink's features.
  //
  // Sinks are required to submit a power Request even when the Source's
  // capabilities are not sufficient for full operation. In that case, the Sink
  // must request enough power for a subset of its features. The subset could be
  // "do nothing" or "flash an LED indicating insufficient power".
  //
  // Sources may also alert the user when this bit is set. For example, some
  // operating systems show insufficient power warnings.
  DEF_SUBBIT(bits_, 26, capability_mismatch);

  // Indicates if the Sink can communicate over a USB data channel.
  //
  // The data channels are D+/- introduced in usb2.0 and SS Tx/Rx introduced in
  // usb3.0.
  DEF_SUBBIT(bits_, 25, supports_usb_communications);

  // If true, the Sink is asking the Source to waive USB Suspend requirements.
  //
  // This hint is meaningful when the related PDO (Power Data Object) has the
  // `requires_usb_suspend` bit set. The Source may react to the hint by making
  // a new offer (via a new Source_Capabilities message) that includes the same
  // PDO without the `requires_usb_suspend` bit set.
  //
  // Setting this bit is not sufficient for the Sink to ignore USB Suspend
  // requirements. The Sink is only free to disregard USB Suspend if it
  // establishes a PD Contract based on a PDO without the `requires_usb_suspend`
  // bit.
  DEF_SUBBIT(bits_, 24, prefers_waiving_usb_suspend);

  // True iff the Port can receive messages with more than 26 data bytes.
  //
  // If true, the underlying BMC PHY must be designed to receive and transmit
  // extended messages up to 260-bytes.
  DEF_SUBBIT(bits_, 23, supports_unchunked_extended_messages);

  // True iff the Sink supports EPR (Extended Power Range) operation.
  DEF_SUBBIT(bits_, 22, supports_extended_power_range);

  // Used to decode the fields common to all RDOs.
  explicit PowerRequestData(uint32_t bits) : bits_(bits) {
    ZX_DEBUG_ASSERT(related_power_data_object_position() != 0);
  }

  // Value type, copying is allowed.
  PowerRequestData(const PowerRequestData&) = default;
  PowerRequestData& operator=(const PowerRequestData&) = default;

  // Trivially destructible.
  ~PowerRequestData() = default;

  // Support explicit casting to uint32_t.
  explicit operator uint32_t() const { return bits_; }

  constexpr bool operator==(const PowerRequestData&) const = default;

 protected:
  // Distinguishes constructors that take a PDO position.
  struct PositionTag {};

  // Instance with all fields except for PDO position set to zero.
  //
  // This is most likely an invalid RDO. At a minimum, power consumption must be
  // set before use in a PD message.
  explicit PowerRequestData(int32_t related_power_data_object_position, PositionTag) : bits_(0) {
    ZX_DEBUG_ASSERT(related_power_data_object_position > 0);
    set_related_power_data_object_position(related_power_data_object_position);
  }
};

// RDO based on a fixed or variable power supply PDO.
//
// See `PowerRequestData` for a general description of the RDO (power
// Request Data Object) representation.
//
// See `PowerData` for a general description of the PDO (Power Data Object)
// representation, `FixedPowerSupplyData` for fixed power supply PDOs, and
// `VariablePowerSupplyData` for variable power supply PDOs.
//
// usbpd3.1 6.4.2 "Request Message", Tables 6-21 "Fixed and Variable Request
// Data Object" and 6-22 "Fixed and Variable Request Data Object with GiveBack
// Support"
class FixedVariableSupplyPowerRequestData : public PowerRequestData {
 public:
  // The Sink's (estimated) current consumption, in multiples of 10 mA.
  DEF_SUBFIELD(bits_, 19, 10, operating_current_10ma);

  // The Sink's maximum or minimum current consumption, in multiples of 10 mA.
  DEF_SUBFIELD(bits_, 9, 0, limit_current_10ma);

  // The Sink's (estimated) current consumption, in mA (milliamps).
  //
  // The Source uses this estimate to manage its power reserve, and power
  // distribution across multiple ports.
  //
  // The Sink is expected to send a new Request message when its estimated
  // current consumption changes.
  //
  // This field must be at most `maximum_current_ma()` in the related
  // FixedPowerSupplyData or VariablePowerSupplyData object.
  int32_t operating_current_ma() const {
    // The multiplication will not overflow (causing UB) because
    // `operating_current_10ma` is a 10-bit field. The maximum product is
    // 10,240.
    return static_cast<int32_t>(static_cast<int32_t>(operating_current_10ma()) * 10);
  }
  FixedVariableSupplyPowerRequestData& set_operating_current_ma(int32_t current_ma) {
    return set_operating_current_10ma(current_ma / 10);
  }

  // The Sink's maximum or minimum current consumption, in mA (milliamps).
  //
  // If `supports_power_give_back` is true, this is the Sink's minimum operating
  // current, which can be requested via a GotoMin message.
  //
  // If `supports_power_give_back` is false, this is the maximum amount of
  // current that the Sink may ever consume. `operating_current_ma()` must be
  // below this value.
  //
  // If `capabilities_mismatch` is true, this field may exceed
  // `maximum_current_ma()` in the related FixedPowerSupplyData or
  // VariablePowerSupplyData object. If `capabilities_mismatch` is false, this
  // field must be at most `maximum_current_ma()`.
  int32_t limit_current_ma() const {
    // The multiplication will not overflow (causing UB) because
    // `limit_current_10ma` is a 10-bit field. The maximum product is 10,240.
    return static_cast<int32_t>(static_cast<int32_t>(limit_current_10ma()) * 10);
  }
  FixedVariableSupplyPowerRequestData& set_limit_current_ma(int32_t current_ma) {
    return set_limit_current_10ma(current_ma / 10);
  }

  static FixedVariableSupplyPowerRequestData CreateForPosition(
      int32_t related_power_data_object_position) {
    return FixedVariableSupplyPowerRequestData(related_power_data_object_position, PositionTag{});
  }

  explicit FixedVariableSupplyPowerRequestData(uint32_t bits) : PowerRequestData(bits) {}
  explicit FixedVariableSupplyPowerRequestData(PowerRequestData request_data)
      : PowerRequestData(static_cast<uint32_t>(request_data)) {}

  // Instance with all fields except for PDO position set to zero.
  //
  // This is most likely an invalid RDO. At a minimum, power consumption must be
  // set before use in a PD message.
  explicit FixedVariableSupplyPowerRequestData(int32_t related_power_data_object_position,
                                               PositionTag)
      : PowerRequestData(related_power_data_object_position, PositionTag{}) {}

  // Value type, copying is allowed.
  FixedVariableSupplyPowerRequestData(const FixedVariableSupplyPowerRequestData&) = default;
  FixedVariableSupplyPowerRequestData& operator=(const FixedVariableSupplyPowerRequestData&) =
      default;

  constexpr bool operator==(const FixedVariableSupplyPowerRequestData& other) const = default;
};

// RDO based on a battery power supply PDO.
//
// See `PowerRequestData` for a general description of the RDO (power
// Request Data Object) representation.
//
// See `PowerData` for a general description of the PDO (Power Data Object)
// representation, and `BatteryPowerSupplyData` for battery power supply PDOs.
//
// usbpd3.1 6.4.2 "Request Message", Tables 6-23 "Battery Request Data Object"
// and 6-24 "Battery Request Data Object with GiveBack Support"
class BatteryPowerRequestData : public PowerRequestData {
 public:
  // The Sink's (estimated) power consumption, in multiples of 250 mW.
  DEF_SUBFIELD(bits_, 19, 10, operating_power_250mw);

  // The Sink's maximum or minimum power consumption, in multiples of 250 mW.
  DEF_SUBFIELD(bits_, 9, 0, limit_power_250mw);

  // The Sink's (estimated) power consumption, in mW (millwatts).
  //
  // The Source uses this estimate to manage its power reserve, and power
  // distribution across multiple ports.
  //
  // The Sink is expected to send a new Request message when its estimated
  // current consumption changes.
  //
  // This field must be at most `maximum_power_mw()` in the related
  // BatteryPowerSupplyData object.
  int32_t operating_power_mw() const {
    // The multiplication will not overflow (causing UB) because
    // `operating_power_250mw` is a 10-bit field. The maximum product is
    // 255,750.
    return static_cast<int32_t>(static_cast<int32_t>(operating_power_250mw()) * 250);
  }
  BatteryPowerRequestData& set_operating_power_mw(int32_t power_mw) {
    return set_operating_power_250mw(power_mw / 250);
  }

  // The Sink's maximum or minimum power consumption, in mW (milliwatts).
  //
  // If `supports_power_give_back` is true, this is the Sink's minimum operating
  // power, which can be requested via a GotoMin message.
  //
  // If `supports_power_give_back` is false, this is the maximum amount of power
  // that the Sink may ever consume. `operating_power_mw()` must be below this
  // value.
  //
  // If `capabilities_mismatch` is true, this field may exceed
  // `maximum_power_mw()` in the related BatteryPowerSupplyData object. If
  // `capabilities_mismatch` is false, this field must be at most
  // `maximum_power_mw()`.
  int32_t limit_power_mw() const {
    // The multiplication will not overflow (causing UB) because
    // `limit_power_250mw` is a 10-bit field. The maximum product is 255,750.
    return static_cast<int32_t>(static_cast<int32_t>(limit_power_250mw()) * 250);
  }
  BatteryPowerRequestData& set_limit_power_mw(int32_t power_mw) {
    return set_limit_power_250mw(power_mw / 250);
  }

  static BatteryPowerRequestData CreateForPosition(int32_t related_power_data_object_position) {
    return BatteryPowerRequestData(related_power_data_object_position, PositionTag{});
  }

  explicit BatteryPowerRequestData(uint32_t bits) : PowerRequestData(bits) {}
  explicit BatteryPowerRequestData(PowerRequestData request_data)
      : PowerRequestData(static_cast<uint32_t>(request_data)) {}

  // Instance with all fields except for PDO position set to zero.
  //
  // This is most likely an invalid RDO. At a minimum, power consumption must be
  // set before use in a PD message.
  explicit BatteryPowerRequestData(int32_t related_power_data_object_position, PositionTag)
      : PowerRequestData(related_power_data_object_position, PositionTag{}) {}

  // Value type, copying is allowed.
  BatteryPowerRequestData(const BatteryPowerRequestData&) = default;
  BatteryPowerRequestData& operator=(const BatteryPowerRequestData&) = default;

  constexpr bool operator==(const BatteryPowerRequestData& other) const = default;
};

}  // namespace usb_pd

#endif  // SRC_DEVICES_POWER_DRIVERS_FUSB302_USB_PD_MESSAGE_OBJECTS_H_

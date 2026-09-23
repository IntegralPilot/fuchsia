// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/intel-display/firmware-bridge.h"

#include <lib/driver/logging/cpp/logger.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <cstddef>

#include <hwreg/bitfields.h>

#include "src/graphics/display/lib/driver-utils/poll-until.h"

// Our implementation is based on the following versions of Intel's ACPI IGD
// (Integrated Graphics Device) OpRegion Specification:
// * First release: OpRegion versions 1.0-2.0, Revision 1.0, October 2008 -
//   at https://01.org/sites/default/files/documentation/acpi_igd_opregion_spec_0.pdf
//   announced at
//   https://lists.freedesktop.org/archives/xorg/2008-October/039119.html
// * Document 728117: for Skylake Processors, Revision 0.5, September 2016 -
//   at https://01.org/sites/default/files/documentation/skl_opregion_rev0p5.pdf
// * Document 621530: OpRegion versions 2.0-2.4, Revision 0.7.1, August 2020
//
// We consider the specification's first release obsoleted, because there are
// areas where it is inconsistent with the newer versions, and the hardware that
// we support matches the description in the newer versions. The first release
// should only be used to supplement the information in the other documents, and
// it should never be the only reference for any aspect of our implementation.

namespace intel_display {

PciConfigOpRegion::PciConfigOpRegion(ddk::Pci& pci) : pci_(pci) {}

PciConfigOpRegion::~PciConfigOpRegion() = default;

namespace {

// The ASLS (ASL Storage) register is reserved for the system boot firmware to
// pass the physical address of the Memory OpRegion to us (the graphics driver).
//
// The Memory OpRegion was intended for passing data between the system firmware
// (BIOS), which was expected to be using ASL (ACPI Source Language), and the
// graphics driver. This explains both the ASLS (ASL Storage) register name, and
// the name "Mailboxes" for the areas of the Memory OpRegion.
//
// First release: Section 2.3 "OpRegion Initialization", page 27
// 728117: Section 2.2 "GMCH PCI Config OpRegion", page 9
// 621530: Section 2.3 "GMCH PCI Config OpRegion", page 10
constexpr uint16_t kAslStoragePciOffset = 0xfc;

// Delivery method for GMCH trigger events.
//
// This describes the behavior of GMCH (Graphics and Memory Controller Hub)
// hardware when software triggers an event via the SWSCI (driver Software /
// System Control Interrupt) register.
enum class GraphicsMediaHubEventDelivery {
  // SWSCI delivered using an SMI (System Management Interrupt).
  //
  // Indicates that the boot firmware does not support the OpRegion protocol.
  kSystemManagementInterrupt = 0,

  // SWSCI delivered using an ACPI SCI (System Control Interrupt).
  //
  // SCI are introduced in the ACPI (Advanced Configuration and Power Interface)
  // specification. The operating system's ACPI driver is expected to execute
  // ALS code in response to the events.
  kSystemControlInterrupt = 1,
};

// The SWSCI (driver Software / System Control Interrupts) register allows the
// graphics driver to invoke system firmware (BIOS) code.
//
// This register is not covered in Document 621530.
//
// First release: Section 7.1.1 "Software SCI Register", pages 91-92
// 728117: Section 4 "Software SCI Mechanism", page 76 and Section 5.1.1 "GMCH
//         SWSCI Register", pages 112-113
class SoftwareSystemControlInterrupt
    : public hwreg::RegisterBase<SoftwareSystemControlInterrupt, uint16_t> {
 public:
  // If true, the GMCH hardware delivers trigger events using ACPI SCI.
  //
  // If false, the GMCH (Graphics and Media Controller Hub) hardware delivers
  // events via SMI (System Maangement Interrupt). This setting indicates that
  // the boot firmware does not support the OpRegion protocol.
  DEF_ENUM_FIELD(GraphicsMediaHubEventDelivery, 15, 15, event_delivery);

  // "Software scratchpad" field that we treat as reserved.
  DEF_FIELD(14, 1, scratchpad);

  // Transitioning this bit 0->1 triggers the GMCH hardware to deliver an event.
  //
  // The event delivered by the GMCH hardware is configured using the
  // `event_delivery_select` field.
  //
  // The graphics driver should only transition this field from 0 to 1, to make
  // invoke the system firmware. The system firmware transitions the field back
  // from 1 to 0 when it finishes servicing the driver's request.
  DEF_BIT(0, entry_trigger);

  // First release: Section 2.4.1 "SWSCI - Software SCI Register", page 29
  // 728117: Section 2.2 "GMCH PCI Config OpRegion", pages 8-9
  static constexpr uint16_t kPciOffset = 0xe8;

  // This register does not live in the MMIO space. It must be read from the PCI
  // configuration space, at `kPciOffset`.
  static auto GetFromValue(uint16_t value) {
    return hwreg::RegisterAddr<SoftwareSystemControlInterrupt>(0).FromValue(value);
  }
};

}  // namespace

zx::result<zx_paddr_t> PciConfigOpRegion::ReadMemoryOpRegionAddress() {
  uint32_t asl_storage_value;
  const zx_status_t status = pci_.ReadConfig32(kAslStoragePciOffset, &asl_storage_value);
  if (status != ZX_OK) {
    fdf::error("PCI OpRegion register read error: {}", zx::make_result(status));
    return zx::error_result(status);
  }

  // The introduction to Section 3 "OpRegion Memory Layout" states that a zero
  // value in the ASLS (ASL Storage) register indicates lack of BIOS support.
  if (!asl_storage_value) {
    return zx::error_result(ZX_ERR_NOT_SUPPORTED);
  }

  return zx::ok(asl_storage_value);
}

zx::result<bool> PciConfigOpRegion::IsSystemControlInterruptInUse() {
  uint16_t swsci_value;
  const zx_status_t status =
      pci_.ReadConfig16(SoftwareSystemControlInterrupt::kPciOffset, &swsci_value);
  if (status != ZX_OK) {
    return zx::error_result(status);
  }

  auto swsci = SoftwareSystemControlInterrupt::GetFromValue(swsci_value);
  if (swsci.event_delivery() != GraphicsMediaHubEventDelivery::kSystemControlInterrupt) {
    // The event delivery mechanism does not match the OpRegion specification.
    return zx::error_result(ZX_ERR_NOT_SUPPORTED);
  }
  return zx::ok(swsci.entry_trigger() != 0);
}

zx::result<> PciConfigOpRegion::TriggerSystemControlInterrupt() {
  // This code below is very similar to IsSystemControlInterruptInUse(). The
  // key difference is that we reuse the raw register value in a
  // read-modify-write operation, in order to preserve the `scratchpad` field.
  uint16_t swsci_value;
  const zx_status_t read_status =
      pci_.ReadConfig16(SoftwareSystemControlInterrupt::kPciOffset, &swsci_value);
  if (read_status != ZX_OK) {
    fdf::error("PCI OpRegion register read error: {}", zx::make_result(read_status));
    return zx::error_result(read_status);
  }
  auto swsci = SoftwareSystemControlInterrupt::GetFromValue(swsci_value);
  if (swsci.event_delivery() != GraphicsMediaHubEventDelivery::kSystemControlInterrupt) {
    return zx::error_result(ZX_ERR_NOT_SUPPORTED);
  }
  if (swsci.entry_trigger() != 0) {
    fdf::error("PCI OpRegion interrupt trigger already armed. Bad boot firmware handoff?");
    return zx::error_result(ZX_ERR_BAD_STATE);
  }

  const zx_status_t write_status = pci_.WriteConfig16(SoftwareSystemControlInterrupt::kPciOffset,
                                                      swsci.set_entry_trigger(1).reg_value());
  if (write_status != ZX_OK) {
    fdf::error("PCI OpRegion register write error: {}", zx::make_result(write_status));
  }
  return zx::make_result(write_status);
}

}  // namespace intel_display

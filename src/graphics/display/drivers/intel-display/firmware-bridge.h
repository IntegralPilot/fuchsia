// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_FIRMWARE_BRIDGE_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_FIRMWARE_BRIDGE_H_

// This file implements Intel's ACPI (Advanced Configuration and
// Power Interface) IGD (Integrated Graphics Device) OpRegion Specification.
//
// See the .cc file for full documentation references.

#include <lib/device-protocol/pci.h>
#include <lib/zx/result.h>
#include <zircon/types.h>

#include <cstddef>

namespace intel_display {

// The GMCH (Graphics Memory Controller Hub) PCI configuration space OpRegion.
//
// First release: Section 2.3 "OpRegion Initialization", pages 27-30
// 728117: Section 2.2 "GMCH PCI Config OpRegion", pages 8-9
// 621530: Section 2.3 "GMCH PCI Config OpRegion", page 10
class PciConfigOpRegion {
 public:
  explicit PciConfigOpRegion(ddk::Pci& pci);

  PciConfigOpRegion(const PciConfigOpRegion&) = delete;
  PciConfigOpRegion& operator=(const PciConfigOpRegion&) = delete;

  ~PciConfigOpRegion();

  // Reads the Memory OpRegion address from the PCI configuration space.
  zx::result<zx_paddr_t> ReadMemoryOpRegionAddress();

  // Invokes the system firmware using the OpRegion SWSCI mechanism.
  //
  // SWSCI (Software System Control Interrupt) is a mechanism for the display
  // driver to invoke the system firmware (BIOS). This mechanism was introduced
  // in the OpRegion spec, with the goal of removing the need to grant drivers
  // the capability of invoking SMM interrupts and video BIOS interrupts.
  //
  // SWSCI is relegated to fringe functionality that is not covered by the ACPI
  // specification and by the VBT (Video BIOS Table). Document 621530 suggests
  // that SWSCI has been removed from recent OpRegion versions, as all
  // references to it (Mailbox 2, private ACPI calls) were removed.
  //
  // Returns ZX_ERR_BAD_STATE if the SWSCI mechanism is in use at the time of
  // the call, or ZX_ERR_NOT_SUPPORTED if the firmware doesn't appear to support
  // the OpRegion protocol. If any PCI operation fails, returns the underlying
  // zx_status_t error.
  //
  // The caller must set up Mailbox 2 correctly in the Memory OpRegion before
  // invoking the system firmware.
  //
  // The caller should ensure that SWSCI mechanism is not in use before the
  // call. This can be accomplished by calling IsSystemControlInterruptInUse()
  // and/or additional synchronization in the layer above PciConfigOpRegion.
  //
  // After the call, the caller should track when the system firmware completes
  // the request, by polling IsSystemControlInterruptInUse() together with the
  // "SCI issued" field in the SCIC (Software SCI Entry/Exit parameter) member
  // of Mailbox 2 in the Memory OpRegion.
  zx::result<> TriggerSystemControlInterrupt();

  // Checks if the OpRegion SWSCI mechanism is currently in use.
  //
  // See TriggerSystemControlInterrupt() for a description of SWSCI.
  //
  // This check involves reading the SWSCI register from the PCI configuration
  // space. If the PCI operation fails, returns the underlying zx_status_t
  // error. Returns ZX_ERR_NOT_SUPPORTED if the SWSCI register state indicates
  // that the firmware does not support the OpRegion protocol.
  //
  // If the system firmware supports the OpRegion protocol, this method should
  // succeed and return false (indicating that SWSCI is not currently in use)
  // when the display driver starts up.
  zx::result<bool> IsSystemControlInterruptInUse();

 private:
  ddk::Pci& pci_;
};

}  // namespace intel_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_FIRMWARE_BRIDGE_H_

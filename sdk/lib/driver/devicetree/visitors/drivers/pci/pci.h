// Copyright 2025 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_PCI_PCI_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_PCI_PCI_H_

#include <lib/devicetree/devicetree.h>
#include <lib/driver/devicetree/visitors/driver-visitor.h>
#include <lib/driver/devicetree/visitors/drivers/pci/interrupt.h>

#include <span>
#include <vector>

#include <hwreg/bitfields.h>

// Parser compatible with the "pci-host-cam-generic" and "pci-host-ecam-generic" host controller
// nodes.
// Schema: https://www.kernel.org/doc/Documentation/devicetree/bindings/pci/host-generic-pci.yaml

namespace pci_dt {

enum AddressSpace : uint8_t {
  Configuration = 0b00,
  Io = 0b01,
  Mmio32 = 0b10,
  Mmio64 = 0b11,
};

struct PciRange {
  // The bus addresses described by this field contain only the mid and low cells of the specified
  // addresses, i.e. the lower 64 bits of the bus addresses.
  devicetree::RangesPropertyElement range;
  // High 32 bits of the bus address.
  uint32_t bus_address_high_cell;
  DEF_SUBBIT(bus_address_high_cell, 30, prefetchable);
  DEF_SUBBIT(bus_address_high_cell, 29, aliased_or_below);
  DEF_ENUM_SUBFIELD(bus_address_high_cell, AddressSpace, 25, 24, address_space);
  DEF_SUBFIELD(bus_address_high_cell, 23, 16, bus_number);
  DEF_SUBFIELD(bus_address_high_cell, 15, 11, device_number);
  DEF_SUBFIELD(bus_address_high_cell, 10, 8, function_number);
  DEF_SUBFIELD(bus_address_high_cell, 7, 0, register_number);
};

class PciVisitor : public fdf_devicetree::DriverVisitor {
 public:
  PciVisitor();

  zx::result<> DriverVisit(fdf_devicetree::Node& node,
                           const devicetree::PropertyDecoder& decoder) final;

  // Only available after visiting.

  // Configuration space address parsed from 'reg' field.
  std::optional<devicetree::RegPropertyElement> reg() const { return reg_; }

  // Memory mapped ranges parsed from the 'ranges' field.
  const std::vector<PciRange>& ranges() const { return ranges_; }

  // Interrupt specifications if this device is gicv3.
  // TODO: Add support for other interrupt controllers as needed.
  std::span<const Gicv3InterruptMapElement> gic_v3_interrupt_map_elements() const {
    return gic_v3_interrupt_map_elements_;
  }

  // Whether this node is extended (ecam) or not (cam).
  bool is_extended() const { return is_extended_; }

 private:
  bool is_extended_ = false;
  std::optional<devicetree::RegPropertyElement> reg_;
  std::vector<PciRange> ranges_;
  std::vector<Gicv3InterruptMapElement> gic_v3_interrupt_map_elements_;
};

constexpr const char* AddressSpaceLabel(AddressSpace e) {
  switch (e) {
    case AddressSpace::Configuration:
      return "Configuration";
    case AddressSpace::Io:
      return "Io";
    case AddressSpace::Mmio32:
      return "32-bit";
    case AddressSpace::Mmio64:
      return "64-bit";
  }
}

}  // namespace pci_dt

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_PCI_PCI_H_

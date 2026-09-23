// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_DEVICES_PCI_LIB_PCI_INCLUDE_LIB_PCI_DEVICETREE_H_
#define SRC_DEVICES_PCI_LIB_PCI_INCLUDE_LIB_PCI_DEVICETREE_H_

#include <lib/stdcompat/span.h>

#include <hwreg/bitfields.h>

namespace pci {

// The implementation in this file should be considered temporary and not relied upon. It is a
// temporary solution before PCI visitor support has been added to our devicetree library.
// TODO(366042146): Remove this header and migrate users to a pci visitor when possible.

struct BusRangeElement {
  uint32_t bus_start;
  uint32_t bus_end;
};

struct RegPropertyElement {
  constexpr uintptr_t base() const { return (static_cast<uintptr_t>(phys_hi) << 32) | phys_lo; }
  constexpr uintptr_t size() const { return (static_cast<uintptr_t>(size_hi) << 32) | size_lo; }
  uint32_t phys_hi;
  uint32_t phys_lo;
  uint32_t size_hi;
  uint32_t size_lo;
};

enum AddressSpace : uint8_t {
  Configuration = 0b00,
  Io = 0b01,
  Mmio32 = 0b10,
  Mmio64 = 0b11,
};

struct RangePropertyElement {
  constexpr uintptr_t child_address() const {
    return (static_cast<uintptr_t>(phys_mid) << 32) | phys_lo;
  }
  constexpr size_t size() const { return (static_cast<size_t>(size_hi) << 32) | size_lo; }
  uint32_t phys_hi;
  DEF_SUBBIT(phys_hi, 30, prefetchable);
  DEF_SUBBIT(phys_hi, 29, aliased_or_below);
  DEF_ENUM_SUBFIELD(phys_hi, AddressSpace, 25, 24, address_space);
  DEF_SUBFIELD(phys_hi, 23, 16, bus_number);
  DEF_SUBFIELD(phys_hi, 15, 11, device_number);
  DEF_SUBFIELD(phys_hi, 10, 8, function_number);
  DEF_SUBFIELD(phys_hi, 7, 0, register_number);
  uint32_t phys_mid;
  uint32_t phys_lo;
  uint32_t parent_hi;
  uint32_t parent_lo;
  uint32_t size_hi;
  uint32_t size_lo;
};

struct BusAddress {
  constexpr explicit BusAddress(uint32_t value) : value(value) {}
  uint32_t value;
  DEF_SUBFIELD(value, 23, 16, bus);
  DEF_SUBFIELD(value, 15, 11, device);
  DEF_SUBFIELD(value, 10, 8, function);
};

enum Gicv3InterruptType : uint32_t {
  SPI = 0,
  PPI = 1,
};

enum Gicv3InterruptFlags : uint32_t {
  EdgeTriggered = 1,
  LevelTriggered = 4,
};

constexpr const char* Gicv3InterruptTypeLabel(Gicv3InterruptType e) {
  switch (e) {
    case Gicv3InterruptType::SPI:
      return "spi";
    case Gicv3InterruptType::PPI:
      return "ppi";
  }
}
constexpr const char* Gicv3InterruptFlagsLabel(Gicv3InterruptFlags e) {
  switch (e) {
    case Gicv3InterruptFlags::EdgeTriggered:
      return "edge-triggered";
    case Gicv3InterruptFlags::LevelTriggered:
      return "level-triggered";
  }
}
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

struct Gicv3ParentInterruptSpecifier {
  constexpr Gicv3ParentInterruptSpecifier(uint32_t t, uint32_t i, uint32_t f)
      : type(static_cast<Gicv3InterruptType>(t)),
        int_number((t == Gicv3InterruptType::PPI) ? i + 16 : i + 32),
        flags(static_cast<Gicv3InterruptFlags>(f)) {}
  Gicv3InterruptType type;
  uint32_t int_number;
  Gicv3InterruptFlags flags;
};

// Gicv3 devicetree bindings:
// https://www.kernel.org/doc/Documentation/devicetree/bindings/interrupt-controller/arm%2Cgic-v3.txt
// 0x03 bytes: child unit address
// 0x01 bytes: child interrupt specifier
// 0x01 bytes: interrupt parent
// 0x02 bytes: parent unit address
// 0x03 bytes: parent interrupt specifier
constexpr uint32_t Gicv3InterruptMapCellCount = 10;
struct Gicv3InterruptMapElement {
 public:
  explicit constexpr Gicv3InterruptMapElement(uint32_t cua0, uint32_t /*cua1*/, uint32_t /*cua2*/,
                                              uint32_t cis, uint32_t ip, uint32_t /*pua0*/,
                                              uint32_t /*pua1*/, uint32_t pis0, uint32_t pis1,
                                              uint32_t pis2)
      : child_unit_address(cua0), pin(ip), parent(pis0, pis1, pis2) {}
  const BusAddress child_unit_address;
  const uint32_t pin;
  const Gicv3ParentInterruptSpecifier parent;
};

}  // namespace pci

#endif  // SRC_DEVICES_PCI_LIB_PCI_INCLUDE_LIB_PCI_DEVICETREE_H_

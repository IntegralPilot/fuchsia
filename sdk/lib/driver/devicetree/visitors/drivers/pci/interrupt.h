// Copyright 2025 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_PCI_INTERRUPT_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_PCI_INTERRUPT_H_

#include <stdint.h>

#include <hwreg/bitfields.h>

// Interrupt types specified for pci devices in devicetree.

namespace pci_dt {

// Gicv3
// Gicv3 devicetree bindings:
// https://www.kernel.org/doc/Documentation/devicetree/bindings/interrupt-controller/arm%2Cgic-v3.txt
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

struct Gicv3ParentInterruptSpecifier {
  constexpr Gicv3ParentInterruptSpecifier(uint32_t t, uint32_t i, uint32_t f)
      : type(static_cast<Gicv3InterruptType>(t)),
        int_number((t == Gicv3InterruptType::PPI) ? i + 16 : i + 32),
        flags(static_cast<Gicv3InterruptFlags>(f)) {}
  Gicv3InterruptType type;
  uint32_t int_number;
  Gicv3InterruptFlags flags;
};

struct BusAddress {
  constexpr explicit BusAddress(uint32_t value) : value(value) {}
  uint32_t value;
  DEF_SUBFIELD(value, 23, 16, bus);
  DEF_SUBFIELD(value, 15, 11, device);
  DEF_SUBFIELD(value, 10, 8, function);
};

// 0x03 bytes: child unit address
// 0x01 bytes: child interrupt specifier
// 0x01 bytes: interrupt parent
// 0x02 bytes: parent unit address
// 0x03 bytes: parent interrupt specifier

struct Gicv3InterruptMapElement {
 public:
  const BusAddress child_unit_address;
  const uint32_t pin;
  const Gicv3ParentInterruptSpecifier parent;
};

}  // namespace pci_dt

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_PCI_INTERRUPT_H_

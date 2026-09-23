// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_PCI_PCI_CHILD_VISITOR_PCI_CHILD_VISITOR_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_PCI_PCI_CHILD_VISITOR_PCI_CHILD_VISITOR_H_

#include <lib/driver/devicetree/manager/node.h>
#include <lib/driver/devicetree/manager/visitor.h>

#include <cstdint>
#include <optional>
#include <vector>

namespace pci_child_dt {

// A PCI bus/device/function address parsed from a child's `reg`, optionally
// qualified by the domain of the bus the child lives on.
struct PciChildBdf {
  static PciChildBdf FromValue(std::optional<uint16_t> domain, uint32_t phys_hi) {
    constexpr uint32_t kBusShift = 16;
    constexpr uint32_t kBusMask = 0xff;
    constexpr uint32_t kDeviceShift = 11;
    constexpr uint32_t kDeviceMask = 0x1f;
    constexpr uint32_t kFunctionShift = 8;
    constexpr uint32_t kFunctionMask = 0x7;

    return PciChildBdf{
        .domain = domain,
        .bus = static_cast<uint8_t>((phys_hi >> kBusShift) & kBusMask),
        .device = static_cast<uint8_t>((phys_hi >> kDeviceShift) & kDeviceMask),
        .function = static_cast<uint8_t>((phys_hi >> kFunctionShift) & kFunctionMask),
    };
  }

  // The PCI domain (segment group) of the bus this device is on. Read from the
  // bus's `fuchsia,pci-domain` property, 0 if it does not declare one, or nullopt
  // when omitted.
  std::optional<uint16_t> domain;
  uint8_t bus;
  uint8_t device;
  uint8_t function;
};

// Visits PCI bus nodes (those with `device_type = "pci"`) and wires each of
// their children to the PCI device that the PCI bus driver discovers at the
// child's bus/device/function (BDF) address.
//
// Unlike the host-controller visitor (//sdk/lib/driver/devicetree/visitors/
// drivers/pci), which parses the controller's own resources, this visitor is
// controller agnostic: it only needs the standard `device_type = "pci"` marker
// and the children's `reg` BDF encoding, so it works for any PCI bus (including
// PCI-to-PCI bridges, which also carry `device_type = "pci"`).
//
// For each child it adds a composite node spec parent that accepts the PCI bus
// driver's per-device fragment, identified by its PCI topology (BDF). The
// child's other devicetree resources (gpios, regulators, clocks, ...) are added
// as additional parents by their respective visitors, so the resulting
// composite gives the child driver access to its sideband resources alongside
// the PCI bus.
class PciChildVisitor : public fdf_devicetree::Visitor {
 public:
  zx::result<> Visit(fdf_devicetree::Node& node,
                     const devicetree::PropertyDecoder& decoder) override;

  zx::result<> FinalizeNode(fdf_devicetree::Node& node) override;

  // The BDFs of every PCI child this visitor wired up, each qualified by the
  // domain of the bus it is on. A board driver passes these to the PCI bus driver
  // (via PciPlatformInfo.devicetree_bdfs) so the bus driver publishes only the
  // fragment for these devices and lets this visitor's composite take over.
  const std::vector<PciChildBdf>& child_bdfs() const { return child_bdfs_; }

 private:
  // True if |node| describes a PCI port or bridge, i.e. it has
  // `device_type = "pci"` and can accommodate downstream devices.
  static bool is_match(fdf_devicetree::Node& node);

  // Returns the PCI domain (segment group) that |node| belongs to. The domain is
  // declared by the host bridge in its `fuchsia,pci-domain` property, so
  // the nearest ancestor declaring one wins. Defaults to domain 0 when no
  // ancestor declares one.
  static zx::result<uint16_t> ParseDomain(fdf_devicetree::Node& node);

  // Parses a single child of a PCI bus node and, if it describes a PCI device,
  // adds the composite node spec parent that connects it to the PCI bus driver.
  zx::result<> ParseChild(fdf_devicetree::Node& parent, fdf_devicetree::ChildNode& child,
                          const PciChildBdf& bdf);

  // Helper to parse the BDF address from a child node. |domain| is the optional
  // domain of the bus |child| is on, as returned by ParseDomain().
  static zx::result<PciChildBdf> ParseBdf(const fdf_devicetree::ChildNode& child,
                                          std::optional<uint16_t> domain = std::nullopt);

  // Adds the PCI fragment parent spec to |child|. The parent is selected by the
  // child's PCI topology (BDF) and domain. When |vendor_id|/|device_id| are present
  // (from the optional `pci-id` property) they are advertised as properties so the
  // child can bind a driver by PCI vendor/device id.
  static void AddChildNodeSpec(fdf_devicetree::ChildNode& child, uint32_t pci_topo, uint32_t domain,
                               std::optional<uint32_t> vendor_id,
                               std::optional<uint32_t> device_id);

  std::vector<PciChildBdf> child_bdfs_;
};

}  // namespace pci_child_dt

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_PCI_PCI_CHILD_VISITOR_PCI_CHILD_VISITOR_H_

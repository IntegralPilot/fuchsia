// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "sdk/lib/driver/devicetree/visitors/drivers/pci/pci-child-visitor/pci-child-visitor.h"

#include <fidl/fuchsia.hardware.pci/cpp/fidl.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/visitors/registration.h>
#include <lib/driver/logging/cpp/logger.h>
#include <zircon/assert.h>
#include <zircon/errors.h>

#include <limits>
#include <optional>
#include <vector>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/pci/cpp/bind.h>

namespace pci_child_dt {

namespace {

// The PCI domain (segment group) that the host bridge and everything below it lives in.
constexpr char kPciDomainProp[] = "fuchsia,pci-domain";

// Domain used when the devicetree does not declare one. Devicetrees describing a
// single PCI domain may omit `fuchsia,pci-domain` entirely.
constexpr uint16_t kDefaultPciDomain = 0;

// Packs a bus/device/function into the value used for the bind_fuchsia::PCI_TOPO
// property. Mirrors BIND_PCI_TOPO_PACK() in <lib/ddk/binding_priv.h>, which the
// PCI bus driver uses to label the fragment it publishes for each device.
constexpr uint32_t PackPciTopo(uint32_t bus, uint32_t device, uint32_t function) {
  return (bus << 8) | (device << 3) | function;
}

}  // namespace

zx::result<> PciChildVisitor::Visit(fdf_devicetree::Node& node,
                                    const devicetree::PropertyDecoder& /*decoder*/) {
  if (!is_match(node)) {
    return zx::ok();
  }

  zx::result<uint16_t> domain = ParseDomain(node);
  if (domain.is_error()) {
    return domain.take_error();
  }

  for (auto& child : node.children()) {
    auto bdf = ParseBdf(child, *domain);
    if (bdf.is_error()) {
      if (bdf.error_value() == ZX_ERR_NOT_FOUND) {
        continue;
      }
      fdf::error("Failed to parse BDF for child '{}': {}", child.name(), bdf.status_string());
      return bdf.take_error();
    }
    // Mark the child node's register type as PCI. Because the devicetree traversal is
    // pre-order, this parent visitor runs and sets the register type before the child
    // node is visited by the MmioVisitor. This prevents the MmioVisitor from attempting
    // to parse the PCI-packed `reg` property as standard MMIO (which would fail).
    child.set_register_type(fdf_devicetree::RegisterType::kPci);
    if (zx::result<> result = ParseChild(node, child, *bdf); result.is_error()) {
      return result.take_error();
    }
  }

  return zx::ok();
}

zx::result<> PciChildVisitor::FinalizeNode(fdf_devicetree::Node& node) {
  if (!is_match(node)) {
    return zx::ok();
  }

  // The metadata is attached to this bus node, so it only ever describes devices
  // in this node's domain. fuchsia_hardware_pci::Address has no domain field, so
  // the domain is not repeated per entry.
  std::vector<fuchsia_hardware_pci::Address> local_bdfs;

  for (auto& child : node.children()) {
    auto bdf = ParseBdf(child);
    if (bdf.is_error()) {
      // ZX_ERR_NOT_FOUND is returned when the reg property is missing (which is a
      // legal state). Any other parse error is fatal and is expected to be caught
      // during the `Visit` phase.
      ZX_ASSERT(bdf.error_value() == ZX_ERR_NOT_FOUND);
      continue;
    }
    local_bdfs.emplace_back(bdf->bus, bdf->device, bdf->function);
  }

  if (!local_bdfs.empty()) {
    fuchsia_hardware_pci::BoardConfiguration board_config;
    board_config.devicetree_bdfs(std::move(local_bdfs));

    fit::result persisted_metadata = fidl::Persist(board_config);
    if (persisted_metadata.is_error()) {
      fdf::error("Failed to persist PCI board config metadata: {}",
                 persisted_metadata.error_value().FormatDescription());
      return zx::error(persisted_metadata.error_value().status());
    }

    fuchsia_hardware_platform_bus::Metadata metadata{{
        .id = std::string(fuchsia_hardware_pci::BoardConfiguration::kSerializableName),
        .data = std::move(persisted_metadata.value()),
    }};
    node.AddMetadata(std::move(metadata));
  }

  return zx::ok();
}

bool PciChildVisitor::is_match(fdf_devicetree::Node& node) {
  auto device_type = node.GetProperty<std::string>("device_type");
  return device_type.is_ok() && *device_type == "pci";
}

zx::result<uint16_t> PciChildVisitor::ParseDomain(fdf_devicetree::Node& node) {
  // Only the host bridge declares the domain. Buses nested below it (PCI-to-PCI
  // bridges, which also carry `device_type = "pci"`) are part of the same domain,
  // so walk up the enclosing chain of PCI buses until one declares it.
  zx::result<uint32_t> domain = node.GetProperty<uint32_t>(kPciDomainProp);
  fdf_devicetree::ParentNode parent = node.parent();
  while (domain.status_value() == ZX_ERR_NOT_FOUND && parent && is_match(*parent.GetNode())) {
    domain = parent.GetProperty<uint32_t>(kPciDomainProp);
    parent = parent.parent();
  }

  if (domain.status_value() == ZX_ERR_NOT_FOUND) {
    return zx::ok(kDefaultPciDomain);
  }
  if (domain.is_error()) {
    fdf::error("Failed to parse '{}' for PCI node '{}': {}", kPciDomainProp, node.name(),
               domain.status_string());

    return domain.take_error();
  }

  // The domain becomes the PCI segment group, which is 16 bits wide.
  if (*domain > std::numeric_limits<uint16_t>::max()) {
    fdf::error("PCI node '{}' has a '{}' of {:#x}, which does not fit in a segment group",
               node.name(), kPciDomainProp, *domain);

    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  return zx::ok(static_cast<uint16_t>(*domain));
}

zx::result<> PciChildVisitor::ParseChild(fdf_devicetree::Node& parent,
                                         fdf_devicetree::ChildNode& child, const PciChildBdf& bdf) {
  const uint32_t pci_topo = PackPciTopo(bdf.bus, bdf.device, bdf.function);

  // Optional, Fuchsia-specific `pci-id = <VID DID>`. When present it lets the
  // child bind a driver by PCI vendor/device id (rather than only by
  // compatible string). It is not part of the standard PCI devicetree binding.
  std::optional<uint32_t> vendor_id;
  std::optional<uint32_t> device_id;
  auto pci_id = child.GetProperty<std::vector<uint32_t>>("pci-id");
  if (pci_id.is_ok()) {
    if (pci_id->size() != 2) {
      fdf::error("PCI child '{}' has a 'pci-id' property with {} cells, expected 2", child.name(),
                 pci_id->size());

      return zx::error(ZX_ERR_INVALID_ARGS);
    }
    vendor_id = (*pci_id)[0];
    device_id = (*pci_id)[1];
  }

  AddChildNodeSpec(child, pci_topo, bdf.domain.value_or(kDefaultPciDomain), vendor_id, device_id);
  child_bdfs_.push_back(bdf);
  fdf::debug("PCI device {:04x}:{:02x}:{:02x}.{:x} on bus '{}' wired to child '{}'",
             bdf.domain.value_or(0), bdf.bus, bdf.device, bdf.function, parent.name(),
             child.name());

  return zx::ok();
}

zx::result<PciChildBdf> PciChildVisitor::ParseBdf(const fdf_devicetree::ChildNode& child,
                                                  std::optional<uint16_t> domain) {
  auto reg = child.GetProperty<std::vector<uint32_t>>("reg");
  if (reg.is_error()) {
    return reg.take_error();
  }
  if (reg->empty()) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  return zx::ok(PciChildBdf::FromValue(domain, (*reg)[0]));
}

void PciChildVisitor::AddChildNodeSpec(fdf_devicetree::ChildNode& child, uint32_t pci_topo,
                                       uint32_t domain, std::optional<uint32_t> vendor_id,
                                       std::optional<uint32_t> device_id) {
  // The fragment for this device is selected by its PCI topology (BDF) and domain.
  // The PCI bus driver publishes exactly one such fragment per discovered device.
  std::vector<fuchsia_driver_framework::BindRule2> bind_rules = {
      fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.pci.Service"),
      fdf::MakeAcceptBindRule(bind_fuchsia::PCI_TOPO, pci_topo),
      fdf::MakeAcceptBindRule(bind_fuchsia_pci::SEGMENT, domain),
  };

  std::vector<fuchsia_driver_framework::NodeProperty2> properties = {
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.pci.Service"),
  };
  // Advertise the vendor/device id so a driver can bind this device by id. We
  // intentionally do not constrain the fragment selection (bind rules) by id:
  // the BDF already identifies the device uniquely, and the id here is a
  // devicetree-supplied override rather than a fact read from the hardware.
  if (vendor_id.has_value()) {
    properties.push_back(fdf::MakeProperty2(bind_fuchsia::PCI_VID, *vendor_id));
  }
  if (device_id.has_value()) {
    properties.push_back(fdf::MakeProperty2(bind_fuchsia::PCI_DID, *device_id));
  }

  child.AddNodeSpec(fuchsia_driver_framework::ParentSpec2{{
      .bind_rules = std::move(bind_rules),
      .properties = std::move(properties),
  }});
}

}  // namespace pci_child_dt

REGISTER_DEVICETREE_VISITOR(pci_child_dt::PciChildVisitor);

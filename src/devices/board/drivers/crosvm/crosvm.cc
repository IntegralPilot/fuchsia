// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "crosvm.h"

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.kernel/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_export.h>
#include <lib/driver/devicetree/manager/manager.h>
#include <lib/driver/devicetree/visitors/default/bind-property/bind-property.h>
#include <lib/driver/devicetree/visitors/registry.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/zx/resource.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/syscalls.h>

#include <bind/fuchsia/pci/cpp/bind.h>

#include "fidl/fuchsia.hardware.pci/cpp/natural_types.h"
#include "fuchsia/hardware/pciroot/c/banjo.h"

namespace board_crosvm {

namespace {
const std::string kPcirootNodeName = "PCI0";

// DFv2 does not expose get_mmio_resource() and the other methods for acquiring higher privilege
// resources so we need to obtain them ourselves.
template <class ResourceMoniker>
zx::result<zx::resource> GetResource(const std::shared_ptr<fdf::Namespace>& incoming) {
  zx::result result = incoming->Connect<ResourceMoniker>();
  if (result.is_error()) {
    return result.take_error();
  }
  fidl::WireResult wire_result = fidl::WireCall(result.value())->Get();
  if (!wire_result.ok()) {
    return zx::error(wire_result.status());
  }
  return zx::ok(std::move(wire_result.value().resource));
}
}  // namespace

zx::result<> Crosvm::CreateRoothost(const pci_dt::PciVisitor& pci_visitor,
                                    const std::shared_ptr<fdf::Namespace>& incoming) {
  // Root host resource and construction is handled first.
  zx::result<zx::resource> msi{};
  if (msi = GetResource<fuchsia_kernel::MsiResource>(incoming); msi.is_error()) {
    fdf::error("Couldn't obtain MSI resource: {}", msi.status_string());
    return msi.take_error();
  }
  msi_resource_ = *std::move(msi);

  // We need the MMIO resource to allocate the ECAM, as well as allowing the
  // root host to allocate exclusive MMIO regions for PCI BAR allocations.
  zx::result<zx::resource> mmio{};
  if (mmio = GetResource<fuchsia_kernel::MmioResource>(incoming); mmio.is_error()) {
    fdf::error("Couldn't obtain MMIO resource: {}", mmio.status_string());
    return mmio.take_error();
  }
  mmio_resource_ = *std::move(mmio);

  // io_resource by design should not be used within Crosvm due to PCIe
  // standards with devicetree only using MMIO space.
  root_host_.emplace(msi_resource_.borrow(), mmio_resource_.borrow(), io_resource_.borrow(),
                     PCI_ADDRESS_SPACE_MEMORY);

  for (const auto& pci_range : pci_visitor.ranges()) {
    fdf::debug("{}.{}.{}: {} base {:#0x} size {:#0x} {}, {}", pci_range.bus_number(),
               pci_range.device_number(), pci_range.function_number(),
               pci_dt::AddressSpaceLabel(pci_range.address_space()),
               *pci_range.range.child_bus_address(), *pci_range.range.length(),
               (pci_range.prefetchable()) ? "" : "non-",
               (pci_range.aliased_or_below()) ? "" : "not ");
    ZX_DEBUG_ASSERT_MSG(pci_range.address_space() == pci_dt::AddressSpace::Mmio64,
                        "Expecting only 64 bit addresses.");
    uint64_t address = *pci_range.range.child_bus_address();
    uint64_t length = *pci_range.range.length();

    switch (pci_range.address_space()) {
      case pci_dt::AddressSpace::Mmio32:
      case pci_dt::AddressSpace::Mmio64: {
        if (zx::result result = root_host_->AddMmioRange(address, length); result.is_error()) {
          fdf::error("failed to add region [{:#0x}, {:#0x}) to MMIO allocators: {}", address,
                     address + length, result.status_string());
        }
        break;
      }

      case pci_dt::AddressSpace::Io: {
        ralloc_region_t io = {
            .base = address,
            .size = length,
        };
        if (zx_status_t status = root_host_->Io().AddRegion(io, RegionAllocator::AllowOverlap::No);
            status != ZX_OK) {
          fdf::error("Failed to add IO region [ {:#0x} - {:#0x} ] to the PCI root allocator: {}",
                     address, address + length, zx_status_get_string(status));
        }
        break;
      }

      case pci_dt::AddressSpace::Configuration: {
        fdf::warn("Unsupported \"Configuration\" address space entry in pci ranges, ignoring.");
        break;
      }
    }
  }

  return zx::ok();
}

zx::result<> Crosvm::CreateMetadata() {
  fuchsia_hardware_pci::BoardConfiguration board_config{
      {fuchsia_hardware_pci::UseIntxWorkaroundType()}};
  if (zx::result result = metadata_server_.Serve(*outgoing(), dispatcher(), board_config);
      result.is_error()) {
    return result.take_error();
  }

  return zx::ok();
}

zx::result<> Crosvm::CreatePciroot(const pci_dt::PciVisitor& pci_visitor,
                                   const std::shared_ptr<fdf::Namespace>& incoming) {
  const auto& pci_reg = pci_visitor.reg();
  zx_paddr_t ecam_address = *pci_reg->address();
  size_t ecam_size = *pci_reg->size();
  const size_t vmo_size = fbl::round_up<size_t>(ecam_size, zx_system_get_page_size());
  zx::vmo ecam;
  zx_status_t status = zx::vmo::create_physical(/*resource=*/mmio_resource_, /*paddr=*/ecam_address,
                                                /*size=*/vmo_size, /*result=*/&ecam);
  if (status != ZX_OK) {
    fdf::error("Failed to create allocate ECAM for PCI: {}", zx_status_get_string(status));
    return zx::error(status);
  }
  fdf::debug("ecam [{:#0x}, {:#0x})", ecam_address, ecam_size + ecam_address);
  root_host_->mcfgs().push_back(
      {.address = ecam_address, .pci_segment = 0, .start_bus_number = 0, .end_bus_number = 0});

  zx::result<zx::resource> irq;
  if (irq = GetResource<fuchsia_kernel::IrqResource>(incoming); irq.is_error()) {
    fdf::error("Couldn't obtain IRQ resource: {}", irq.status_string());
    return irq.take_error();
  }

  zx::result<zx::resource> iommu;
  if (iommu = GetResource<fuchsia_kernel::IommuResource>(incoming); iommu.is_error()) {
    fdf::error("Couldn't obtain IRQ resource: {}", iommu.status_string());
    return iommu.take_error();
  }

  pciroot_.emplace(kPcirootNodeName, &*root_host_, std::move(ecam), std::move(irq.value()),
                   std::move(iommu.value()), pci_visitor.is_extended(), std::vector<pci_bdf_t>{});

  if (zx::result<> result =
          pciroot_->CreateInterruptsAndRouting(pci_visitor.gic_v3_interrupt_map_elements());
      result.is_error()) {
    return result.take_error();
  }

  return zx::ok();
}

zx::result<> Crosvm::StartBanjoServer(const std::shared_ptr<fdf::Namespace>& incoming,
                                      const std::string& node_name) {
  banjo_server_.emplace(bind_fuchsia_pci::BIND_PROTOCOL_ROOT, &*pciroot_,
                        pciroot_->pciroot_protocol_ops());
  compat::DeviceServer::BanjoConfig banjo_config{
      .default_proto_id = bind_fuchsia_pci::BIND_PROTOCOL_ROOT,
  };
  banjo_config.callbacks[bind_fuchsia_pci::BIND_PROTOCOL_ROOT] = banjo_server_->callback();

  // Spin up the compat server for serving fuchsia.hardware.pciroot.
  zx::result<> result =
      compat_server_.Initialize(incoming, outgoing(), node_name, kPcirootNodeName,
                                compat::ForwardMetadata::All(), std::move(banjo_config));
  if (result.is_error()) {
    return result.take_error();
  }

  std::vector offers = compat_server_.CreateOffers2();
  std::optional metadata_offer = metadata_server_.CreateOffer();
  if (metadata_offer.has_value()) {
    offers.push_back(std::move(metadata_offer.value()));
  }

  zx::result child = AddChild(kPcirootNodeName, {{banjo_server_->property()}}, offers);
  if (child.is_error()) {
    return child.take_error();
  }

  controller_.Bind(std::move(child.value()), dispatcher());
  return zx::ok();
}

zx::result<> Crosvm::Start(fdf::DriverContext context) {
  auto incoming = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  auto node_name = context.node_name().value_or("");
  auto manager = fdf_devicetree::Manager::CreateFromNamespace(*incoming);

  fdf_devicetree::VisitorRegistry visitors;
  auto pci_visitor = std::make_unique<pci_dt::PciVisitor>();
  const pci_dt::PciVisitor& pci_visitor_ref = *pci_visitor;
  if (zx::result<> result =
          visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>());
      result.is_error()) {
    return result.take_error();
  }
  if (zx::result<> result = visitors.RegisterVisitor(std::move(pci_visitor)); result.is_error()) {
    return result.take_error();
  }

  if (zx::result<> result = manager->Walk(visitors); result.is_error()) {
    return result.take_error();
  }

  if (zx::result<> result = CreateRoothost(pci_visitor_ref, incoming); result.is_error()) {
    return result.take_error();
  }

  if (zx::result<> result = CreateMetadata(); result.is_error()) {
    return result.take_error();
  }

  if (zx::result<> result = CreatePciroot(pci_visitor_ref, incoming); result.is_error()) {
    return result.take_error();
  }

  if (zx::result<> result = StartBanjoServer(incoming, node_name); result.is_error()) {
    return result.take_error();
  }

  return zx::ok();
}

}  // namespace board_crosvm

FUCHSIA_DRIVER_EXPORT2(board_crosvm::Crosvm);

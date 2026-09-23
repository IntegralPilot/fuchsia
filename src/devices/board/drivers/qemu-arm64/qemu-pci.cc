// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <fuchsia/hardware/pciroot/c/banjo.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/driver.h>
#include <lib/ddk/platform-defs.h>
#include <lib/pci/constants.h>
#include <lib/zx/bti.h>
#include <lib/zx/interrupt.h>
#include <lib/zx/iommu.h>
#include <lib/zx/resource.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <stdint.h>
#include <zircon/errors.h>
#include <zircon/syscalls/iommu.h>
#include <zircon/syscalls/types.h>
#include <zircon/types.h>

#include <array>
#include <limits>
#include <memory>

#include <fbl/alloc_checker.h>
#include <src/devices/board/drivers/qemu-arm64/qemu-bus.h>
#include <src/devices/board/drivers/qemu-arm64/qemu-pciroot.h>
#include <src/devices/board/drivers/qemu-arm64/qemu-virt.h>

namespace board_qemu_arm64 {

zx_status_t QemuArm64Pciroot::Create(PciRootHost* root_host, QemuArm64Pciroot::Context ctx,
                                     zx_device_t* parent, const char* name) {
  auto pciroot = std::make_unique<QemuArm64Pciroot>(root_host, ctx, parent, name);
  if (zx::result<> result = pciroot->CreateInterrupts(); result.is_error()) {
    zxlogf(ERROR, "Failed to create legacy PCI interrupts: %s", result.status_string());
    return result.status_value();
  }

  zx_status_t status = pciroot->DdkAdd(ddk::DeviceAddArgs(name).set_proto_id(ZX_PROTOCOL_PCIROOT));
  if (status == ZX_OK) {
    // Driver Framework owns QemuArm64Pciroot, object is intentionally leaked on success.
    [[maybe_unused]] auto ptr = pciroot.release();
  }
  return status;
}

// QEMU's hw/arm/virt.c wires each PCIe INTx pin directly to a GIC SPI in a
// fixed range (PCIE_INT_BASE .. PCIE_INT_BASE + PCIE_INT_COUNT - 1) and then
// relies on the standard PCI pin swizzle at each device. The values are stable
// across QEMU versions as long as the machine model does not change, so we
// hardcode them here instead of parsing the device tree.
// TODO(b/507938746): In the future, we could consider obtaining this IRQ routing
// from the devicetree.
zx::result<> QemuArm64Pciroot::CreateInterrupts() {
  zx::unowned_resource irq_resource(get_irq_resource(parent()));

  for (uint32_t pin = 0; pin < PCIE_INT_COUNT; pin++) {
    const uint32_t vector = PCIE_INT_BASE + pin;
    zx::interrupt interrupt;
    if (zx_status_t status =
            zx::interrupt::create(*irq_resource, vector, ZX_INTERRUPT_MODE_LEVEL_HIGH, &interrupt);
        status != ZX_OK) {
      zxlogf(ERROR, "Failed to create interrupt for vector %u: %s", vector,
             zx_status_get_string(status));
      return zx::error(status);
    }
    interrupts_[pin] = pci_legacy_irq_t{
        .interrupt = interrupt.release(),
        .vector = vector,
    };
  }

  // All devices on the QEMU virt board attach directly to the root complex and
  // follow the PCI pin swizzle starting at INTA for device 0.
  for (uint8_t device_id = 0; device_id < DEVICES_PER_BUS; device_id++) {
    pci_irq_routing_entry_t entry = {
        .port_device_id = PCI_IRQ_ROUTING_NO_PARENT,
        .port_function_id = PCI_IRQ_ROUTING_NO_PARENT,
        .device_id = device_id,
    };
    for (uint32_t pin = 0; pin < PINS_PER_FUNCTION; pin++) {
      entry.pins[pin] = static_cast<uint8_t>(PCIE_INT_BASE + ((pin + device_id) % PCIE_INT_COUNT));
    }
    irq_routing_entries_[device_id] = entry;
  }

  return zx::ok();
}

zx_status_t QemuArm64Pciroot::PcirootGetBti(uint32_t bdf, uint32_t index, zx::bti* bti) {
  // Stub IOMMU: the QEMU virt board has no real IOMMU driver yet, so every
  // bus-mastering device shares a pass-through iommu keyed by BDF.
  zx::unowned_resource iommu_resource(get_iommu_resource(parent()));
  zx_iommu_desc_stub_t desc{};
  zx::iommu iommu;
  if (zx_status_t status =
          zx::iommu::create(*iommu_resource, ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc), &iommu);
      status != ZX_OK) {
    return status;
  }
  return zx::bti::create(iommu, /*options=*/0, bdf, bti);
}

zx_status_t QemuArm64Pciroot::PcirootGetPciPlatformInfo(pci_platform_info_t* info) {
  *info = context_.info;
  info->legacy_irqs_list = interrupts_.data();
  info->legacy_irqs_count = interrupts_.size();
  info->irq_routing_list = irq_routing_entries_.data();
  info->irq_routing_count = irq_routing_entries_.size();
  return ZX_OK;
}

zx_status_t QemuArm64::PciInit() {
  zx_status_t status = pci_root_host_.Mmio32().AddRegion(
      {.base = PCIE_MMIO_BASE_PHYS, .size = PCIE_MMIO_SIZE}, RegionAllocator::AllowOverlap::No);
  if (status != ZX_OK) {
    zxlogf(ERROR, "Failed to add MMIO region { %#lx - %#lx } to PCI root allocator: %s",
           PCIE_MMIO_BASE_PHYS, PCIE_MMIO_BASE_PHYS + PCIE_MMIO_SIZE, zx_status_get_string(status));
    return status;
  }

  status = pci_root_host_.Mmio64().AddRegion(
      {.base = PCIE_MMIO_HIGH_BASE_PHYS, .size = PCIE_MMIO_HIGH_SIZE},
      RegionAllocator::AllowOverlap::No);

  if (status != ZX_OK) {
    zxlogf(ERROR, "Failed to add MMIO region { %#lx - %#lx } to PCI root allocator: %s",
           PCIE_MMIO_HIGH_BASE_PHYS, PCIE_MMIO_HIGH_BASE_PHYS + PCIE_MMIO_HIGH_SIZE,
           zx_status_get_string(status));
    return status;
  }

  ralloc_region_t io = {.base = PCIE_PIO_BASE_PHYS, .size = PCIE_PIO_SIZE};
  if (status = pci_root_host_.Io().AddRegion(io, RegionAllocator::AllowOverlap::No);
      status != ZX_OK) {
    zxlogf(ERROR, "Failed to add IO region { %#lx - %#lx } to the PCI root allocator: %s",
           PCIE_PIO_BASE_PHYS, PCIE_PIO_BASE_PHYS + PCIE_PIO_SIZE, zx_status_get_string(status));
    return status;
  }

  McfgAllocation pci0_mcfg = {
      .address = PCIE_ECAM_BASE_PHYS,
      .pci_segment = 0,
      .start_bus_number = 0,
      .end_bus_number = (PCIE_ECAM_SIZE / pci::kEcamBytesPerBus) - 1,
  };

  pci_root_host_.mcfgs().push_back(pci0_mcfg);
  return ZX_OK;
}

zx_status_t QemuArm64::PciAdd() {
  McfgAllocation pci0_mcfg = {};
  zx_status_t status = pci_root_host_.GetSegmentMcfgAllocation(0, &pci0_mcfg);
  if (status != ZX_OK) {
    zxlogf(ERROR, "Couldn't retrieve the MMCFG for segment group %u: %s", 0,
           zx_status_get_string(status));
    return status;
  }

  // There's no dynamic configuration for this platform, so just grabbing the same mcfg
  // created in Init is adequate.
  std::array<char, 8> name = {"pci0"};
  QemuArm64Pciroot::Context ctx = {};
  ctx.info.start_bus_num = pci0_mcfg.start_bus_number;
  ctx.info.end_bus_num = pci0_mcfg.end_bus_number;
  ctx.info.segment_group = pci0_mcfg.pci_segment;
  memcpy(ctx.info.name, name.data(), name.size());

  zxlogf(DEBUG, "%s ecam { %#lx - %#lx }\n", name.data(), PCIE_ECAM_BASE_PHYS,
         PCIE_ECAM_BASE_PHYS + PCIE_ECAM_SIZE);
  const size_t vmo_size = fbl::round_up<size_t>(PCIE_ECAM_SIZE, zx_system_get_page_size());
  zx::vmo ecam_vmo = {};
  status = zx::vmo::create_physical(*zx::unowned_resource(get_mmio_resource(parent())),
                                    PCIE_ECAM_BASE_PHYS, vmo_size, &ecam_vmo);
  if (status != ZX_OK) {
    return status;
  }

  ctx.info.cam = {.vmo = ecam_vmo.release(), .is_extended = true};
  status = QemuArm64Pciroot::Create(&pci_root_host_, ctx, parent_, name.data());
  if (status != ZX_OK) {
    return status;
  }

  return ZX_OK;
}

}  // namespace board_qemu_arm64

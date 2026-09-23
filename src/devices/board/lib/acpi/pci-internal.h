// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BOARD_LIB_ACPI_PCI_INTERNAL_H_
#define SRC_DEVICES_BOARD_LIB_ACPI_PCI_INTERNAL_H_

#include <fuchsia/hardware/pciroot/cpp/banjo.h>
#include <lib/ddk/device.h>
#include <lib/ddk/driver.h>
#include <lib/pci/pciroot.h>
#include <lib/zx/resource.h>
#include <stdint.h>
#include <zircon/compiler.h>
#include <zircon/errors.h>

#include <unordered_map>

#include <acpica/acpi.h>
#include <acpica/actypes.h>
#include <ddktl/device.h>

#include "src/devices/board/lib/acpi/acpi.h"
#include "src/devices/board/lib/acpi/pci.h"

__BEGIN_CDECLS
// It would be nice to use the hwreg library here, but these structs should be kept
// simple so that it can be passed across process boundaries.
#define MB(n) (1024UL * 1024UL * (n))
#define PCI_BUS_MAX 255

// Base Address Allocation Structure, defined in PCI firmware spec v3.2 chapter 4.1.2
struct pci_ecam_baas {
  uint64_t base_address;
  uint16_t segment_group;
  uint8_t start_bus_num;
  uint8_t end_bus_num;
  uint32_t reserved0;
};

// A structure derived from ACPI _PRTs that represents a zx::interrupt to create and
// provide to the PCI bus driver.
struct acpi_legacy_irq {
  uint32_t vector;   // Hardware vector
  uint32_t options;  // Configuration for zx_interrupt_create
};

zx_status_t pci_report_current_resources(acpi::Acpi* acpi, zx_handle_t mmio_resource_handle);

class AcpiPciroot;
using AcpiPcirootType = ddk::Device<AcpiPciroot, ddk::GetProtocolable>;
class AcpiPciroot : ddk::PcirootProtocol<AcpiPciroot>, public AcpiPcirootType, public PcirootBase {
 public:
  struct Context {
    char name[ACPI_NAMESEG_SIZE + 1];
    ACPI_HANDLE acpi_object;
    acpi::UniquePtr<ACPI_DEVICE_INFO> acpi_device_info;
    zx_device_t* platform_bus;
    std::unordered_map<uint32_t, acpi_legacy_irq> irqs;
    std::vector<zx::resource> irq_resources;
    std::vector<pci_irq_routing_entry_t> routing;
    struct pci_platform_info info;
    iommu::IommuManagerInterface* iommu;
  };

  static zx_status_t Create(PciRootHost* root_host, AcpiPciroot::Context ctx, zx_device_t* parent,
                            const char* name, std::vector<pci_bdf_t> acpi_bdfs);
  using PcirootBase::PcirootAllocateMsi;
  using PcirootBase::PcirootDriverShouldProxyConfig;
  using PcirootBase::PcirootGetAddressSpace;
  using PcirootBase::PcirootGetMsiHandle;
  zx_status_t PcirootGetBti(uint32_t bdf, uint32_t index, zx::bti* bti);
  zx_status_t PcirootGetPciPlatformInfo(pci_platform_info_t* info);
  zx_status_t PcirootReadConfig8(const pci_bdf_t* address, uint16_t offset, uint8_t* value) final;
  zx_status_t PcirootReadConfig16(const pci_bdf_t* address, uint16_t offset, uint16_t* value) final;
  zx_status_t PcirootReadConfig32(const pci_bdf_t* address, uint16_t offset, uint32_t* value) final;
  zx_status_t PcirootWriteConfig8(const pci_bdf_t* address, uint16_t offset, uint8_t value) final;
  zx_status_t PcirootWriteConfig16(const pci_bdf_t* address, uint16_t offset, uint16_t value) final;
  zx_status_t PcirootWriteConfig32(const pci_bdf_t* address, uint16_t offset, uint32_t value) final;

  void DdkRelease() { delete this; }
  zx_status_t DdkGetProtocol(uint32_t proto_id, void* out) {
    if (proto_id != ZX_PROTOCOL_PCIROOT) {
      return ZX_ERR_NOT_SUPPORTED;
    }

    auto* proto = static_cast<ddk::AnyProtocol*>(out);
    proto->ops = &pciroot_protocol_ops_;
    proto->ctx = this;
    return ZX_OK;
  }

 private:
  Context context_;
  std::vector<pci_bdf_t> acpi_bdfs_;
  AcpiPciroot(PciRootHost* root_host, AcpiPciroot::Context ctx, zx_device_t* parent,
              const char* name, std::vector<pci_bdf_t> acpi_bdfs)
      : AcpiPcirootType(parent),
        PcirootBase(root_host),
        context_(std::move(ctx)),
        acpi_bdfs_(std::move(acpi_bdfs)) {}
  virtual ~AcpiPciroot() = default;
};

namespace acpi {

ACPI_STATUS GetPciRootIrqRouting(acpi::Acpi* acpi, ACPI_HANDLE root_obj,
                                 AcpiPciroot::Context* context);

}  // namespace acpi

__END_CDECLS

#endif  // SRC_DEVICES_BOARD_LIB_ACPI_PCI_INTERNAL_H_

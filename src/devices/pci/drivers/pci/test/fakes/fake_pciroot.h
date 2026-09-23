// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_DEVICES_PCI_DRIVERS_PCI_TEST_FAKES_FAKE_PCIROOT_H_
#define SRC_DEVICES_PCI_DRIVERS_PCI_TEST_FAKES_FAKE_PCIROOT_H_

#include <assert.h>
#include <fuchsia/hardware/pciroot/cpp/banjo.h>
#include <lib/fake-bti/bti.h>
#include <lib/fake-resource/resource.h>
#include <lib/zx/bti.h>
#include <lib/zx/channel.h>
#include <lib/zx/interrupt.h>
#include <lib/zx/msi.h>
#include <zircon/errors.h>
#include <zircon/syscalls/resource.h>

#include <vector>

#include <fbl/array.h>

#include "src/devices/pci/drivers/pci/test/fakes/fake_ecam.h"

// This FakePciroot class for the moment is a stub and test files
// will specialize the methods they need. Eventually when more tests
// are sorted out it may make sense to have pciroot tests be similar
// to the mock-i2c style fakes.
class FakePciroot : public ddk::PcirootProtocol<FakePciroot> {
 public:
  static constexpr uint64_t kDefaultHighMemoryAddress = (1llu << 32);
  static constexpr uint32_t kDefaultLowMemoryAddress = (1u << 10);
  static constexpr uint16_t kDefaultIoAddress = 0x10;
  // By default, pciroot won't populate an ecam unless it's called with Create().
  FakePciroot(uint8_t bus_start, uint8_t bus_cnt, bool is_extended, uint16_t segment_group = 0)
      : proto_({&pciroot_protocol_ops_, this}),
        ecam_(bus_start, bus_cnt, is_extended),
        info_{
            .name = "fakroot",
            .start_bus_num = bus_start,
            .end_bus_num = static_cast<uint8_t>(bus_start + bus_cnt),
            .segment_group = segment_group,
            .cam = {.vmo = ecam_.mmio().get_vmo()->get(), .is_extended = is_extended},
        } {
    ZX_ASSERT(fake_resource_create(ZX_RSRC_KIND_MMIO, mmio_resource_.reset_and_get_address()) ==
              ZX_OK);
    ZX_ASSERT(fake_resource_create(ZX_RSRC_KIND_IOPORT, ioport_resource_.reset_and_get_address()) ==
              ZX_OK);
    ZX_ASSERT(fake_bti_create(bti_.reset_and_get_address()) == ZX_OK);
  }
  FakePciroot() : FakePciroot(/*bus_start=*/0, /*bus_cnt=*/1, /*is_extended=*/false) {}

  FakePciroot(FakePciroot&&) = default;

  pciroot_protocol_t* proto() { return &proto_; }
  pci_platform_info_t info() {
    pci_platform_info_t info{
        .start_bus_num = info_.start_bus_num,
        .end_bus_num = info_.end_bus_num,
        .segment_group = info_.segment_group,
        .cam = info_.cam,
        .legacy_irqs_list = legacy_irqs_.data(),
        .legacy_irqs_count = legacy_irqs_.size(),
        .irq_routing_list = routing_entries_.data(),
        .irq_routing_count = routing_entries_.size(),
        .acpi_bdfs_list = acpi_devices_.data(),
        .acpi_bdfs_count = acpi_devices_.size(),
        .devicetree_bdfs_list = devicetree_devices_.data(),
        .devicetree_bdfs_count = devicetree_devices_.size(),
    };
    memcpy(info.name, info_.name, sizeof(info.name));
    return info;
  }
  FakeEcam& ecam() { return ecam_; }
  uint8_t bus_start() const { return info_.start_bus_num; }
  uint8_t bus_end() const { return info_.end_bus_num; }
  uint16_t segment_group() const { return info_.segment_group; }
  bool is_extended() const { return info_.cam.is_extended; }
  zx::bti& bti() { return bti_; }
  zx::resource& mmio_resource() { return mmio_resource_; }
  zx::resource& ioport_resource() { return ioport_resource_; }
  auto& legacy_irqs() { return legacy_irqs_; }
  auto& routing_entries() { return routing_entries_; }
  auto& allocation_eps() { return allocation_eps_; }
  auto& acpi_devices() { return acpi_devices_; }
  auto& devicetree_devices() { return devicetree_devices_; }

  // Protocol methods.
  zx_status_t PcirootGetBti(uint32_t bdf, uint32_t index, zx::bti* bti) {
    if (!enable_get_bti_) {
      return ZX_ERR_NOT_SUPPORTED;
    }

    return bti_.duplicate(ZX_RIGHT_SAME_RIGHTS, bti);
  }

  zx_status_t PcirootConnectSysmem(zx::channel connection) {
    if (!enable_connect_sysmem_) {
      return ZX_ERR_NOT_SUPPORTED;
    }

    return ZX_ERR_NOT_SUPPORTED;
  }

  zx_status_t PcirootGetPciPlatformInfo(pci_platform_info_t* out_info) {
    if (!enable_get_pci_platform_info_) {
      return ZX_ERR_NOT_SUPPORTED;
    }

    *out_info = info();
    return ZX_OK;
  }

  bool PcirootDriverShouldProxyConfig() { return enable_driver_should_proxy_config_; }
  zx_status_t PcirootReadConfig8(const pci_bdf_t* address, uint16_t offset, uint8_t* value) {
    if (!enable_config_read_) {
      return ZX_ERR_NOT_SUPPORTED;
    }

    if (address->bus_id < info_.start_bus_num || address->bus_id > info_.end_bus_num) {
      return ZX_ERR_NOT_SUPPORTED;
    }

    *value = ecam_.get_config_view(*address).Read8(offset);
    return ZX_OK;
  }
  zx_status_t PcirootReadConfig16(const pci_bdf_t* address, uint16_t offset, uint16_t* value) {
    if (!enable_config_read_) {
      return ZX_ERR_NOT_SUPPORTED;
    }

    if (address->bus_id < info_.start_bus_num || address->bus_id > info_.end_bus_num) {
      return ZX_ERR_NOT_SUPPORTED;
    }

    *value = ecam_.get_config_view(*address).Read16(offset);
    return ZX_OK;
  }
  zx_status_t PcirootReadConfig32(const pci_bdf_t* address, uint16_t offset, uint32_t* value) {
    if (!enable_config_read_) {
      return ZX_ERR_NOT_SUPPORTED;
    }

    if (address->bus_id < info_.start_bus_num || address->bus_id > info_.end_bus_num) {
      return ZX_ERR_NOT_SUPPORTED;
    }
    *value = ecam_.get_config_view(*address).Read32(offset);
    return ZX_OK;
  }
  zx_status_t PcirootWriteConfig8(const pci_bdf_t* address, uint16_t offset, uint8_t value) {
    if (!enable_config_write_) {
      return ZX_ERR_NOT_SUPPORTED;
    }

    if (address->bus_id < info_.start_bus_num || address->bus_id > info_.end_bus_num) {
      return ZX_ERR_NOT_SUPPORTED;
    }
    ecam_.get_config_view(*address).Write(value, offset);
    return ZX_OK;
  }
  zx_status_t PcirootWriteConfig16(const pci_bdf_t* address, uint16_t offset, uint16_t value) {
    if (!enable_config_write_) {
      return ZX_ERR_NOT_SUPPORTED;
    }

    if (address->bus_id < info_.start_bus_num || address->bus_id > info_.end_bus_num) {
      return ZX_ERR_NOT_SUPPORTED;
    }
    ecam_.get_config_view(*address).Write(value, offset);
    return ZX_OK;
  }
  zx_status_t PcirootWriteConfig32(const pci_bdf_t* address, uint16_t offset, uint32_t value) {
    if (!enable_config_write_) {
      return ZX_ERR_NOT_SUPPORTED;
    }

    if (address->bus_id < info_.start_bus_num || address->bus_id > info_.end_bus_num) {
      return ZX_ERR_NOT_SUPPORTED;
    }
    ecam_.get_config_view(*address).Write(value, offset);
    return ZX_OK;
  }

  zx_status_t PcirootAllocateMsi(uint32_t requested_irqs, bool can_target_64bit,
                                 zx::msi* out_allocation, msi_allocation_info_t* out_info) {
    if (!enable_allocate_msi_) {
      return ZX_ERR_NOT_SUPPORTED;
    }

    zx_status_t status =
        zx_msi_allocate(ZX_HANDLE_INVALID, requested_irqs, out_allocation->reset_and_get_address());
    if (status != ZX_OK) {
      return status;
    }
    out_info->target_addr = 0xCAFE;
    out_info->target_data = 0xC0FE;
    out_info->irq_count = requested_irqs;
    return ZX_OK;
  }

  zx_status_t PcirootGetMsiHandle(zx::msi allocation, uint32_t options, uint16_t msi_id,
                                  zx::vmo cfg_vmo, uint64_t cfg_offset,
                                  zx::interrupt* out_interrupt) {
    if (!enable_get_msi_handle_) {
      return ZX_ERR_NOT_SUPPORTED;
    }
    return zx::msi::create(allocation, options, msi_id, cfg_vmo, cfg_offset, out_interrupt);
  }

  zx_status_t PcirootGetAddressSpace(zx_paddr_t in_base, size_t size, pci_address_space_t type,
                                     bool low, uint64_t* out_base, zx::resource* resource,
                                     zx::eventpair* eventpair) {
    if (!enable_get_address_space_ || size == 0) {
      return ZX_ERR_NOT_SUPPORTED;
    }

    zx_rsrc_kind_t kind =
        (type == PCI_ADDRESS_SPACE_MEMORY) ? ZX_RSRC_KIND_MMIO : ZX_RSRC_KIND_IOPORT;
    if (in_base) {
      *out_base = in_base;
    } else {
      if (type == PCI_ADDRESS_SPACE_MEMORY) {
      }
      if (type == PCI_ADDRESS_SPACE_MEMORY) {
        if (low) {
          *out_base = kDefaultLowMemoryAddress;
        } else {
          *out_base = kDefaultHighMemoryAddress;
        }
      } else {
        *out_base = kDefaultIoAddress;
      }
    }

    const zx::resource& parent = (kind == ZX_RSRC_KIND_MMIO) ? mmio_resource_ : ioport_resource_;
    ZX_ASSERT(zx::resource::create(parent, kind, *out_base, size, "fake", 5, resource) == ZX_OK);
    zx::eventpair local_ep;
    zx::eventpair::create(0, &local_ep, eventpair);
    allocation_eps_.push_back(std::move(local_ep));
    return ZX_OK;
  }

  void enable_get_bti(bool enable) { enable_get_bti_ = enable; }
  void enable_connect_sysmem(bool enable) { enable_connect_sysmem_ = enable; }
  void enable_get_pci_platform_info(bool enable) { enable_get_pci_platform_info_ = enable; }
  void enable_driver_should_proxy_config(bool enable) {
    enable_driver_should_proxy_config_ = enable;
  }
  void enable_config_read(bool enable) { enable_config_read_ = enable; }
  void enable_config_write(bool enable) { enable_config_write_ = enable; }
  void enable_allocate_msi(bool enable) { enable_allocate_msi_ = enable; }
  void enable_get_address_space(bool enable) { enable_get_address_space_ = enable; }
  void enable_get_msi_handle(bool enable) { enable_get_msi_handle_ = enable; }

 private:
  pciroot_protocol_t proto_;
  FakeEcam ecam_;
  const pci_platform_info_t info_;
  std::vector<zx::eventpair> allocation_eps_;
  zx::bti bti_;
  zx::resource mmio_resource_;
  zx::resource ioport_resource_;
  std::vector<pci_legacy_irq_t> legacy_irqs_;
  std::vector<pci_irq_routing_entry_t> routing_entries_;
  std::vector<pci_bdf_t> acpi_devices_;
  std::vector<pci_bdf_t> devicetree_devices_;

  // Switches so tests can test error paths of PCIRoot usage.
  bool enable_get_bti_ = true;
  bool enable_connect_sysmem_ = true;
  bool enable_get_pci_platform_info_ = true;
  bool enable_driver_should_proxy_config_ = false;
  bool enable_config_read_ = true;
  bool enable_config_write_ = true;
  bool enable_allocate_msi_ = true;
  bool enable_get_address_space_ = true;
  bool enable_get_msi_handle_ = true;
};

#endif  // SRC_DEVICES_PCI_DRIVERS_PCI_TEST_FAKES_FAKE_PCIROOT_H_

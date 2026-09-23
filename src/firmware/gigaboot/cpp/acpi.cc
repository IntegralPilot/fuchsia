// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "acpi.h"

#include <lib/zbi-format/cpu.h>
#include <lib/zbi-format/driver-config.h>
#include <stdio.h>
#include <zircon/assert.h>

#include <algorithm>
#include <numeric>
#include <span>

#include <efi/types.h>
#include <phys/efi/main.h>

namespace gigaboot {

namespace {

struct AcpiRsdpRevisionDescription {
  uint8_t revision;
  AcpiTableSignature signature;
  size_t entry_size;
  const SdtHeader* (*accessor)(const AcpiRsdp&);
};

// Describes the invariants for different revisions of the SDT table.
// Note: newer revisions MUST add entries at the beginning of the array
//       so that when searching forward we find the NEWEST description
//       that is not greater than the RSDP we are using.
//
// Note: in C++20, it is possible to validate this invariant in a static assert.
//       Older language versions don't have sufficient support for constexpr to allow
//       the check statically.
constexpr AcpiRsdpRevisionDescription kRevisionTable[] = {

    {
        .revision = 1,
        .signature = kXsdtSignature,
        .entry_size = sizeof(AcpiRsdp::xsdt_address),
        .accessor =
            [](const auto& entry) {
              return reinterpret_cast<const SdtHeader*>(entry.xsdt_address);
            },
    },
    {
        .revision = 0,
        .signature = kRsdtSignature,
        .entry_size = sizeof(AcpiRsdp::rsdt_address),
        .accessor =
            [](const auto& entry) {
              return reinterpret_cast<const SdtHeader*>(
                  static_cast<efi_physical_addr>(entry.rsdt_address));
            },
    },
};

template <typename PtrNum>
const SdtHeader* TraverseEntries(const SdtHeader* table, AcpiTableSignature sig) {
  static_assert(std::numeric_limits<PtrNum>::is_integer);

  // The array of entry pointers is not guaranteed to be correctly aligned,
  // so we memcpy the entries into a local array.
  // 32 is an educated guess about the maximum number of headers in the array.
  size_t num_entries = (table->length - sizeof(*table)) / sizeof(PtrNum);
  std::array<PtrNum, 32> aligned_ptrs;
  std::span<PtrNum> aligned_ptr_span(aligned_ptrs.begin(),
                                     std::min(num_entries, aligned_ptrs.size()));
  memcpy(aligned_ptr_span.data(), reinterpret_cast<const uint8_t*>(table + 1),
         aligned_ptr_span.size_bytes());

  for (const auto addr : aligned_ptr_span) {
    const auto* entry = reinterpret_cast<const SdtHeader*>(addr);
    if (entry && entry->signature == sig) {
      return entry;
    }
  }
  return nullptr;
}

}  // namespace

std::optional<zbi_dcfg_arm_psci_driver_t> AcpiFadt::GetPsciDriver() const {
  if (!(arm_boot_arch & kPsciCompliant)) {
    return std::nullopt;
  }

  return zbi_dcfg_arm_psci_driver_t{
      .use_hvc = static_cast<uint8_t>(arm_boot_arch & kPsciUseHvc),
  };
}

zbi_dcfg_arm_generic_timer_driver_t AcpiGtdt::GetTimer() const {
  return {
      .irq_phys = nonsecure_el1_timer_gsiv,
      .irq_virt = virtual_el1_timer_gsiv,
  };
}

const SdtHeader* AcpiRsdp::LoadTableWithSignature(AcpiTableSignature sig) const {
  // Note the requirement for listing newer revisions first in the definition of kRevisionTable
  const auto lookup = std::find_if(std::cbegin(kRevisionTable), std::cend(kRevisionTable),
                                   [&](const auto& entry) { return revision >= entry.revision; });

  if (lookup == std::cend(kRevisionTable)) {
    printf("%s: invalid RSDP revision: %u\n", __func__, revision);
    return nullptr;
  }

  const SdtHeader* sdt_table = lookup->accessor(*this);
  if (sdt_table == nullptr) {
    printf("%s: null entry for SDT table\n", __func__);
    return nullptr;
  }

  if (sdt_table->signature != lookup->signature) {
    printf("%s: bad signature for SDT table with revision %u: %.*s\n", __func__, revision,
           static_cast<int>(sdt_table->signature.size()), sdt_table->signature.data());
    return nullptr;
  }

  if (lookup->entry_size == sizeof(uint64_t)) {
    return TraverseEntries<uint64_t>(sdt_table, sig);
  }

  if (lookup->entry_size == sizeof(uint32_t)) {
    return TraverseEntries<uint32_t>(sdt_table, sig);
  }

  ZX_ASSERT(false);
  return nullptr;
}

uint32_t AcpiSpcr::GetKdrv() const {
  // The SPCR table does not contain the granular subtype of the register
  // interface we need in revision 1, so return early in this case.
  if (hdr.revision < 2) {
    return 0;
  }

  // The SPCR types are documented in Table 3 on:
  // https://docs.microsoft.com/en-us/windows-hardware/drivers/bringup/acpi-debug-port-table
  // We currently only rely on PL011 devices to be initialized here.
  switch (interface_type) {
    case 0x0003:
      return ZBI_KERNEL_DRIVER_PL011_UART;
    default:
      printf("%s: unsupported serial interface type: 0x%x\n", __func__, interface_type);
      return 0;
  }
}

std::span<zbi_topology_node_t> AcpiMadt::GetTopology(std::span<zbi_topology_node_t> nodes) const {
  auto last_node = nodes.begin();

  for (const auto& controller : controllers()) {
    const auto* gicc = GetInterruptController<AcpiMadtGicInterface>(controller);
    if (gicc == nullptr) {
      continue;
    }

    if (last_node == nodes.end()) {
      return nodes;
    }

    size_t num_cpus = std::distance(nodes.begin(), last_node);
    *last_node = zbi_topology_node_t{
        .entity =
            {
                .discriminant = ZBI_TOPOLOGY_ENTITY_PROCESSOR,
                .processor =
                    {
                        .architecture_info =
                            {
                                .discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_ARM64,
                                .arm64 =
                                    {
                                        .cluster_1_id =
                                            static_cast<uint8_t>((gicc->mpidr >> 8) & 0xFF),
                                        .cluster_2_id =
                                            static_cast<uint8_t>((gicc->mpidr >> 16) & 0xFF),
                                        .cluster_3_id =
                                            static_cast<uint8_t>((gicc->mpidr >> 32) & 0xFF),
                                        .cpu_id = static_cast<uint8_t>(gicc->mpidr & 0xFF),
                                        .gic_id = static_cast<uint8_t>(gicc->cpu_interface_number),
                                    },
                            },
                        .flags = static_cast<zbi_topology_processor_flags_t>(
                            (num_cpus) ? ZBI_TOPOLOGY_PROCESSOR_FLAGS_PRIMARY : 0),
                        .logical_ids = {static_cast<uint16_t>(num_cpus)},
                        .logical_id_count = 1,
                    },
            },
        .parent_index = ZBI_TOPOLOGY_NO_PARENT,

    };
    ++last_node;
  }

  return std::span<zbi_topology_node_t>(nodes.begin(), last_node);
}

std::optional<AcpiMadt::GicDescriptor> AcpiMadt::GetGicDriver() const {
  const AcpiMadtGicInterface* gicc = nullptr;
  const AcpiMadtGicDistributor* gicd = nullptr;
  const AcpiGicMsi* gic_msi = nullptr;
  const AcpiMadtGicRedistributor* gicr = nullptr;

  for (const auto& controller : controllers()) {
    switch (controller.type) {
      case AcpiMadtGicInterface::kType:
        gicc = GetInterruptController<AcpiMadtGicInterface>(controller);
        break;
      case AcpiMadtGicDistributor::kType:
        gicd = GetInterruptController<AcpiMadtGicDistributor>(controller);
        break;
      case AcpiMadtGicRedistributor::kType:
        gicr = GetInterruptController<AcpiMadtGicRedistributor>(controller);
        break;
      case AcpiGicMsi::kType:
        gic_msi = GetInterruptController<AcpiGicMsi>(controller);
        break;
      default:
        printf("%s: unexpected interrupt controller type: 0x%x\n", __func__, controller.type);
        break;
    }
  }

  if (gicd == nullptr) {
    printf("%s: no valid gicd found\n", __func__);
    return std::nullopt;
  }
  uint64_t mmio_phys_addr = gicd->physical_base_address;
  switch (gicd->gic_version) {
    case 0x02:
      if (gicc == nullptr) {
        printf("%s: No gicc\n", __func__);
        return std::nullopt;
      }
      mmio_phys_addr = std::min(gicc->physical_base_address, mmio_phys_addr);
      return AcpiMadt::GicDescriptor{
          .driver =
              zbi_dcfg_arm_gic_v2_driver_t{
                  .mmio_phys = mmio_phys_addr,
                  .msi_frame_phys = gic_msi->physical_base_address,
                  .gicd_offset = gicd->physical_base_address - mmio_phys_addr,
                  .gicc_offset = gicc->physical_base_address - mmio_phys_addr,
                  .ipi_base = 0,
                  .optional = true,
                  .use_msi = gic_msi != nullptr,
              },
          .zbi_type = ZBI_KERNEL_DRIVER_ARM_GIC_V2,
      };
    case 0x03:
      if (gicr == nullptr) {
        printf("%s: No gicr\n", __func__);
        return std::nullopt;
      }
      mmio_phys_addr = std::min(gicr->discovery_range_base_address, mmio_phys_addr);
      return AcpiMadt::GicDescriptor{
          .driver =
              zbi_dcfg_arm_gic_v3_driver_t{
                  .mmio_phys = mmio_phys_addr,
                  .gicd_offset = gicd->physical_base_address - mmio_phys_addr,
                  .gicr_offset = gicr->discovery_range_base_address - mmio_phys_addr,
                  .gicr_stride = kGicv3rDefaultStride,
                  .ipi_base = 0,
                  .optional = true,
              },
          .zbi_type = ZBI_KERNEL_DRIVER_ARM_GIC_V3,
      };
    default:
      printf("%s: unexpected gic version: %u\n", __func__, gicd->gic_version);
      return std::nullopt;
  }
}

zbi_dcfg_simple_t AcpiSpcr::DeriveUartDriver() const {
  return {
      .mmio_phys = base_address.address,
      // IRQ is only valid if the lowest order bit of interrupt type is set.
      // Any other bit set to 1 in the interrupt type indicates that we should
      // use the Global System Interrupt (GSIV).
      .irq = (0x1 & interrupt_type) ? irq : gsiv,
  };
}

const AcpiRsdp* FindAcpiRsdp() {
  const AcpiRsdp* rsdp = nullptr;
  std::span<const efi_configuration_table> entries(gEfiSystemTable->ConfigurationTable,
                                                   gEfiSystemTable->NumberOfTableEntries);

  for (const efi_configuration_table& entry : entries) {
    // Check if this entry is an ACPI RSD PTR.
    const auto* vendor_table = reinterpret_cast<const std::array<uint8_t, 8>*>(entry.VendorTable);
    if ((entry.VendorGuid == kAcpiTableGuid || entry.VendorGuid == kAcpi20TableGuid) &&
        // Verify the signature of the ACPI RSD PTR.
        vendor_table && *vendor_table == kAcpiRsdpSignature) {
      rsdp = reinterpret_cast<const AcpiRsdp*>(entry.VendorTable);
      break;
    }
  }

  if (rsdp == nullptr) {
    printf("RSDP was not found\n");
    return nullptr;
  }

  // Verify the checksum of this table. Both V1 and V2 RSDPs should pass the
  // V1 checksum, which only covers the first 20 bytes of the table.
  // The checksum adds all the bytes and passes if the result is 0.
  std::span<const uint8_t> acpi_bytes(reinterpret_cast<const uint8_t*>(rsdp), kAcpiRsdpV1Size);
  if (uint8_t res = std::reduce(acpi_bytes.begin(), acpi_bytes.end(), 0ull) & 0xFF; res) {
    printf("RSDP V1 checksum failed\n");
    return nullptr;
  }

  // V2 RSDPs should additionally pass a checksum of the entire table.
  if (rsdp->revision > 0) {
    acpi_bytes = {acpi_bytes.begin(), rsdp->length};
    if (uint8_t res = static_cast<uint8_t>(std::reduce(acpi_bytes.begin(), acpi_bytes.end(), 0));
        res) {
      printf("RSDP V2 checksum failed\n");
      return nullptr;
    }
  }

  return rsdp;
}

}  // namespace gigaboot

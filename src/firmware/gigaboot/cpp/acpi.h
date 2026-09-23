// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_FIRMWARE_GIGABOOT_CPP_ACPI_H_
#define SRC_FIRMWARE_GIGABOOT_CPP_ACPI_H_

#include <lib/zbi-format/cpu.h>
#include <lib/zbi-format/driver-config.h>

#include <array>
#include <optional>
#include <span>
#include <variant>

#include <efi/types.h>

namespace gigaboot {

// ACPI signatures
using AcpiTableSignature = std::array<uint8_t, 4>;
constexpr AcpiTableSignature kXsdtSignature = {'X', 'S', 'D', 'T'};
constexpr AcpiTableSignature kRsdtSignature = {'R', 'S', 'D', 'T'};

// ACPI constants
constexpr uint8_t kPsciCompliant = 0x1;
constexpr uint8_t kPsciUseHvc = 0x2;

// The ARM GICv3 spec states that 0x20000 is the default GICR stride.
constexpr uint64_t kGicv3rDefaultStride = 0x20000;

// System Description Table.
// Parent structure for all SDT tables.
struct __attribute__((packed)) SdtHeader {
  AcpiTableSignature signature;
  uint32_t length;
  uint8_t revision;
  uint8_t checksum;
  uint8_t oem_id[6];
  uint8_t oem_table_id[8];
  uint32_t oem_revision;
  uint32_t creator_id;
  uint32_t creator_revision;
};
static_assert(sizeof(SdtHeader) == 36, "System Description Table Header is the wrong size");

struct __attribute__((packed)) InterruptControllerHeader {
  uint8_t type;
  uint8_t length;
};
static_assert(sizeof(InterruptControllerHeader) == 2,
              "Interrupt Controller Header is the wrong size");

// The Multiple APIC (Advanced Programmable Interrupt Controller) Description Table
// In memory, followed by a list of adjacent interrupt controllers.
struct __attribute__((packed)) AcpiMadt {
  constexpr static AcpiTableSignature kSig = {'A', 'P', 'I', 'C'};

  SdtHeader hdr;
  uint32_t local_ic_address;
  uint32_t flags;

  class Controllers {
   public:
    class Iterator {
     public:
      using value_type = InterruptControllerHeader;

      const value_type& operator*() const { return *m_ptr; }
      const value_type* operator->() const { return m_ptr; }
      Iterator operator++() {
        m_ptr = reinterpret_cast<const value_type*>(reinterpret_cast<const uint8_t*>(m_ptr) +
                                                    m_ptr->length);
        return *this;
      }
      Iterator operator++(int) {
        Iterator tmp = *this;
        ++(*this);
        return tmp;
      }

      friend bool operator==(const Iterator& a, const Iterator& b) { return a.m_ptr == b.m_ptr; }
      friend bool operator!=(const Iterator& a, const Iterator& b) { return !(a == b); }

     private:
      friend Controllers;

      explicit Iterator(const value_type* ptr) : m_ptr(ptr) {}

      const value_type* m_ptr;
    };

    Iterator begin() const { return Iterator(base_); }

    Iterator end() const {
      return Iterator(reinterpret_cast<const InterruptControllerHeader*>(
          reinterpret_cast<const uint8_t*>(base_) + length_));
    }

   private:
    friend AcpiMadt;

    Controllers(const InterruptControllerHeader* base, size_t length)
        : base_(base), length_(length) {}

    const InterruptControllerHeader* base_;
    size_t length_;
  };

  // Return an iterator over the followup interrupt controllers after the MADT in memory.
  Controllers controllers() const {
    return {reinterpret_cast<const InterruptControllerHeader*>(this + 1),
            hdr.length - sizeof(*this)};
  }

  // Given an out-parameter span of topology nodes,
  // returns a span of CPU topology nodes.
  // The 'nodes' param is the space provided for the return value.
  std::span<zbi_topology_node_t> GetTopology(std::span<zbi_topology_node_t> nodes) const;

  struct GicDescriptor {
    std::variant<zbi_dcfg_arm_gic_v2_driver_t, zbi_dcfg_arm_gic_v3_driver_t> driver;
    uint32_t zbi_type;
  };
  // Return a GIC driver config structure and associated ZBI type if available,
  // returns std::nullopt if there is no GIC driver available.
  std::optional<GicDescriptor> GetGicDriver() const;
};
static_assert(sizeof(AcpiMadt) == 44, "MADT is the wrong size");

// Generic Address Structure
// Used to define the position of registers.
struct __attribute__((packed)) AcpiGas {
  uint8_t address_space_id;
  uint8_t register_bit_width;
  uint8_t register_bit_offset;
  uint8_t access_size;
  uint64_t address;
};
static_assert(sizeof(AcpiGas) == 12, "GAS is the wrong size");

// The Fixed ACPI Description Table
// Contains information about fixed registers.
struct __attribute__((packed)) AcpiFadt {
  constexpr static AcpiTableSignature kSig = {'F', 'A', 'C', 'P'};

  SdtHeader hdr;
  uint32_t firmware_ctrl;
  uint32_t dsdt;
  uint8_t reserved;
  uint8_t preferred_pm_profile;
  uint16_t sci_int;
  uint32_t smi_cmd;
  uint8_t acpi_enable;
  uint8_t acpi_disable;
  uint8_t s4bios_req;
  uint8_t pstate_cnt;
  uint32_t pm1a_evt_blk;
  uint32_t pm1b_evt_blk;
  uint32_t pm1a_cnt_blk;
  uint32_t pm1b_cnt_blk;
  uint32_t pm2_cnt_blk;
  uint32_t pm_tmr_blk;
  uint32_t gpe0_blk;
  uint32_t gpe1_blk;
  uint8_t pm1_evt_len;
  uint8_t pm1_cnt_len;
  uint8_t pm2_cnt_len;
  uint8_t pm_tmr_len;
  uint8_t gpe0_blk_len;
  uint8_t gpe1_blk_len;
  uint8_t gpe1_base;
  uint8_t cst_cnt;
  uint16_t p_lvl2_lat;
  uint16_t p_lvl3_lat;
  uint16_t flush_size;
  uint16_t flush_stride;
  uint8_t duty_offset;
  uint8_t duty_width;
  uint8_t day_alrm;
  uint8_t mon_alrm;
  uint8_t century;
  uint16_t iapc_boot_arch;
  uint8_t reserved2;
  uint32_t flags;
  uint8_t reset_reg[12];
  uint8_t reset_value;
  uint16_t arm_boot_arch;
  uint8_t fadt_minor_version;
  uint64_t x_firmware_ctrl;
  uint64_t x_dsdt;
  AcpiGas x_pm1a_evt_blk;
  AcpiGas x_pm1b_evt_blk;
  AcpiGas x_pm1a_cnt_blk;
  AcpiGas x_pm1b_cnt_blk;
  AcpiGas x_pm2_cnt_blk;
  AcpiGas x_pm_tmr_blk;
  AcpiGas x_gpe0_blk;
  AcpiGas x_gpe1_blk;
  AcpiGas sleep_control_reg;
  AcpiGas sleep_status_reg;
  uint64_t hypervisor_vendory_identity;

  // If this table is Power State Coordination Interface (PSCI) compliant,
  // return a config structure for the PSCI driver.
  std::optional<zbi_dcfg_arm_psci_driver_t> GetPsciDriver() const;
};
static_assert(sizeof(AcpiFadt) == 276, "FADT is the wrong size");

// Generic Timer Description Table
struct __attribute__((packed)) AcpiGtdt {
  constexpr static AcpiTableSignature kSig = {'G', 'T', 'D', 'T'};

  SdtHeader hdr;
  uint64_t cnt_control_base;
  uint32_t reserved;
  uint32_t secure_el1_timer_gsiv;
  uint32_t secure_el1_timer_flags;
  uint32_t nonsecure_el1_timer_gsiv;
  uint32_t nonsecure_el1_timer_flags;
  uint32_t virtual_el1_timer_gsiv;
  uint32_t virtual_el1_timer_flags;
  uint32_t el2_timer_gsiv;
  uint32_t el2_timer_flags;
  uint64_t cnt_read_base;
  uint32_t platform_timer_count;
  uint32_t platform_timer_offset;
  uint32_t virtual_el2_timer_gsiv;
  uint32_t virtual_el2_timer_flags;

  // Return a config structure for the generic timer.
  zbi_dcfg_arm_generic_timer_driver_t GetTimer() const;
};
static_assert(sizeof(AcpiGtdt) == 104, "GTDT is the wrong size");

struct __attribute__((packed)) AcpiMadtGicInterface {
  static constexpr uint8_t kType = 0xb;

  InterruptControllerHeader hdr;
  uint16_t reserved;
  uint32_t cpu_interface_number;
  uint32_t acpi_processor_uid;
  uint32_t flags;
  uint32_t parking_protocol_version;
  uint32_t performance_interrupt_gsiv;
  uint64_t parked_address;
  uint64_t physical_base_address;
  uint64_t gicv;
  uint64_t gich;
  uint32_t vgic_maintenance_interrupt;
  uint64_t gicr_base_address;
  uint64_t mpidr;
  uint8_t processor_power_class;
  uint8_t reserved2;
  uint16_t spe_overflow_interrupt;
};
static_assert(sizeof(AcpiMadtGicInterface) == 80, "MADT GICC is the wrong size");

struct __attribute__((packed)) AcpiMadtGicDistributor {
  static constexpr uint8_t kType = 0xc;

  InterruptControllerHeader hdr;
  uint16_t reserved;
  uint32_t gic_id;
  uint64_t physical_base_address;
  uint32_t system_vector_base;
  uint8_t gic_version;
  uint8_t reserved2[3];
};
static_assert(sizeof(AcpiMadtGicDistributor) == 24, "MADT GICD is the wrong size");

// Gic Message Signaled Interrupts table
struct __attribute__((packed)) AcpiGicMsi {
  static constexpr uint8_t kType = 0xd;

  InterruptControllerHeader hdr;
  uint16_t reserved;
  uint32_t gic_msi_frame_id;
  uint64_t physical_base_address;
  uint32_t flags;
  uint16_t spi_count;
  uint16_t spi_base;
};
static_assert(sizeof(AcpiGicMsi) == 24, "MADT GIC MSI is the wrong size");

struct __attribute__((packed)) AcpiMadtGicRedistributor {
  static constexpr uint8_t kType = 0xe;

  InterruptControllerHeader hdr;
  uint16_t reserved;
  uint64_t discovery_range_base_address;
  uint32_t discovery_range_length;
};
static_assert(sizeof(AcpiMadtGicRedistributor) == 16, "MADT GICR is the wrong size");

// Serial Port Console Redirection table
struct __attribute__((packed)) AcpiSpcr {
  constexpr static AcpiTableSignature kSig = {'S', 'P', 'C', 'R'};

  SdtHeader hdr;
  uint8_t interface_type;
  uint8_t reserved[3];
  AcpiGas base_address;
  uint8_t interrupt_type;
  uint8_t irq;
  uint32_t gsiv;
  uint8_t baud_rate;
  uint8_t parity;
  uint8_t stop_bits;
  uint8_t flow_control;
  uint8_t terminal_type;
  uint8_t language;
  uint16_t pci_device_id;
  uint16_t pci_vendor_id;
  uint8_t pci_bus_number;
  uint8_t pci_device_number;
  uint8_t pci_function_number;
  uint32_t pci_flags;
  uint8_t pci_segment;
  uint32_t uart_clock_frequency;

  // Return a UART driver config structure.
  zbi_dcfg_simple_t DeriveUartDriver() const;
  // Returns a numeric constant describing the serial driver type.
  // Returns 0 if the interface type is unsupported.
  uint32_t GetKdrv() const;
};
static_assert(sizeof(AcpiSpcr) == 80, "SPCR is the wrong size");

// Given an interrupt controller header,
// check that its type field matches the type constant of IntCtrlType.
// If the field matches, reinterpret the header and (subsequent memory)
// as a controller of type IntCtrlType.
// Return nullptr if the type field does not match.
template <typename IntCtrlType>
const IntCtrlType* GetInterruptController(const InterruptControllerHeader& hdr) {
  static_assert(offsetof(IntCtrlType, hdr) == 0, "Nonzero offset of interrupt controller header");
  static_assert(std::is_same_v<decltype(IntCtrlType::hdr), InterruptControllerHeader>,
                "Invalid type for interrupt controller header");
  static_assert(std::is_same_v<decltype(IntCtrlType::kType), const uint8_t>,
                "Invalid type constant for controller");
  return (hdr.type == IntCtrlType::kType) ? reinterpret_cast<const IntCtrlType*>(&hdr) : nullptr;
}

constexpr std::array<uint8_t, 8> kAcpiRsdpSignature = {'R', 'S', 'D', ' ', 'P', 'T', 'R', ' '};
static_assert(sizeof(kAcpiRsdpSignature) == 8, "Unexpected size for RSDP signature constant");

// Root System Description Pointer
// If revision >= 1, check xsdt_address for the Extended System Description Table (XSDT).
// If revision == 0, check rsdt_address for the Root System Description Table (RSDT).
struct __attribute__((packed)) AcpiRsdp {
  std::array<uint8_t, 8> signature;
  uint8_t checksum;
  uint8_t oem_id[6];
  uint8_t revision;
  uint32_t rsdt_address;

  // Available in ACPI version 2.0.
  uint32_t length;
  uint64_t xsdt_address;
  uint8_t extended_checksum;
  uint8_t reserved[3];

  // Load the given table type, if available.
  template <typename Table>
  const Table* LoadTable() const {
    static_assert(offsetof(Table, hdr) == 0, "Nonzero offset of SDT header");
    static_assert(std::is_same_v<decltype(Table::hdr), SdtHeader>, "Invalid type for SDT header");
    return reinterpret_cast<const Table*>(LoadTableWithSignature(Table::kSig));
  }

 private:
  // Load the table with the given signature, if available.
  const SdtHeader* LoadTableWithSignature(AcpiTableSignature) const;
};
static_assert(sizeof(AcpiRsdp) == 36, "RSDP is the wrong size");

// Find and validate the pointer to the Root System Description Table.
const AcpiRsdp* FindAcpiRsdp();

constexpr size_t kAcpiRsdpV1Size = offsetof(AcpiRsdp, length);

constexpr efi_guid kAcpiTableGuid = ACPI_TABLE_GUID;
constexpr efi_guid kAcpi20TableGuid = ACPI_20_TABLE_GUID;

}  // namespace gigaboot

#endif  // SRC_FIRMWARE_GIGABOOT_CPP_ACPI_H_

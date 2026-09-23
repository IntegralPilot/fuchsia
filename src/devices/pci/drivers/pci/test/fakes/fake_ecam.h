// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_DEVICES_PCI_DRIVERS_PCI_TEST_FAKES_FAKE_ECAM_H_
#define SRC_DEVICES_PCI_DRIVERS_PCI_TEST_FAKES_FAKE_ECAM_H_

#include <fuchsia/hardware/pciroot/cpp/banjo.h>
#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/driver/mmio/cpp/mmio-view.h>
#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/driver/mmio/testing/cpp/test-helper.h>
#include <lib/pci/constants.h>
#include <lib/pci/hw.h>

#include <hwreg/bitfields.h>

#include "src/devices/pci/drivers/pci/config.h"

struct IoBaseAddress {
  uint32_t value;
  DEF_SUBBIT(value, 0, is_io_space);
  // bit 1 is reserved.
  DEF_SUBFIELD(value, 31, 2, address);
};
static_assert(sizeof(IoBaseAddress) == 4, "Bad size for IoBaseAddress");

struct Mmio32BaseAddress {
  uint32_t value;
  DEF_SUBBIT(value, 0, is_io_space);
  // bit 1 is reserved.
  DEF_SUBBIT(value, 2, is_64bit);
  DEF_SUBBIT(value, 3, is_prefetchable);
  DEF_SUBFIELD(value, 31, 4, address);
};
static_assert(sizeof(Mmio32BaseAddress) == 4, "Bad size for Mmio32BaseAddress");

struct FakeBaseAddress {
  union {
    IoBaseAddress io;
    Mmio32BaseAddress mmio32;
    uint32_t mmio64;
  };
};
static_assert(sizeof(FakeBaseAddress) == 4, "Bad size for FakeBaseAddress");

// Defines set_NAME and NAME() in addition to a private member to match the same
// chaining possible using the set_ methods DEF_SUBBIT generates.
//
// This macro has a pitfall if used following a private: declaration in the
// class/struct, so only use it in the public section.
#define DEF_WRAPPED_FIELD(TYPE, NAME) \
 private:                             \
  TYPE NAME##_;                       \
                                      \
 public:                              \
  TYPE NAME() { return NAME##_; }     \
  auto& set_##NAME(TYPE val) {        \
    NAME##_ = val;                    \
    return *this;                     \
  }                                   \
  static_assert(true)  // eat a ;

// A fake implementation of a PCI device configuration (Type 00h)
struct FakePciType0Config {
  DEF_WRAPPED_FIELD(uint16_t, vendor_id);
  DEF_WRAPPED_FIELD(uint16_t, device_id);
  DEF_WRAPPED_FIELD(uint16_t, command);
  DEF_SUBBIT(command_, 0, io_space_en);
  DEF_SUBBIT(command_, 1, mem_space_en);
  DEF_SUBBIT(command_, 2, bus_master_en);
  DEF_SUBBIT(command_, 3, special_cycles_en);
  DEF_SUBBIT(command_, 4, men_write_and_inval_en);
  DEF_SUBBIT(command_, 5, vga_palette_snoop_en);
  DEF_SUBBIT(command_, 6, parity_error_resp);
  // bit 7 is hardwired to 0.
  DEF_SUBBIT(command_, 8, serr_en);
  DEF_SUBBIT(command_, 9, fast_back_to_back_en);
  DEF_SUBBIT(command_, 10, interrupt_disable);
  DEF_WRAPPED_FIELD(uint16_t, status);
  // bits 2:0 are reserved.
  DEF_SUBBIT(status_, 3, int_status);
  DEF_SUBBIT(status_, 4, capabilities_list);
  DEF_SUBBIT(status_, 5, is_66mhz_capable);
  // bit 6 is reserved.
  DEF_SUBBIT(status_, 7, fast_back_to_back_capable);
  DEF_SUBBIT(status_, 8, master_data_parity_error);
  DEF_SUBFIELD(status_, 10, 9, devsel_timing);
  DEF_SUBBIT(status_, 11, signaled_target_abort);
  DEF_SUBBIT(status_, 12, received_target_abort);
  DEF_SUBBIT(status_, 13, received_master_abort);
  DEF_SUBBIT(status_, 14, signaled_system_error);
  DEF_SUBBIT(status_, 15, detected_parity_error);
  DEF_WRAPPED_FIELD(uint8_t, revision_id);
  DEF_WRAPPED_FIELD(uint8_t, program_interface);
  DEF_WRAPPED_FIELD(uint8_t, sub_class);
  DEF_WRAPPED_FIELD(uint8_t, base_class);
  DEF_WRAPPED_FIELD(uint8_t, cache_line_size);
  DEF_WRAPPED_FIELD(uint8_t, latency_timer);
  DEF_WRAPPED_FIELD(uint8_t, header_type);
  DEF_WRAPPED_FIELD(uint8_t, bist);
  DEF_SUBFIELD(bist_, 3, 0, completion_code);
  // bits 4-5 are reserved.
  DEF_SUBBIT(bist_, 6, start_bist);
  DEF_SUBBIT(bist_, 7, bist_capable);
  FakeBaseAddress base_address[6];
  DEF_WRAPPED_FIELD(uint32_t, cardbus_cis_ptr);
  DEF_WRAPPED_FIELD(uint16_t, subsystem_vendor_id);
  DEF_WRAPPED_FIELD(uint16_t, subsystem_id);
  DEF_WRAPPED_FIELD(uint32_t, expansion_rom_address);
  DEF_WRAPPED_FIELD(uint8_t, capabilities_ptr);

 private:
  [[maybe_unused]] uint8_t reserved_0[3];
  [[maybe_unused]] uint32_t reserved_1;

 public:
  DEF_WRAPPED_FIELD(uint8_t, interrupt_line);
  DEF_WRAPPED_FIELD(uint8_t, interrupt_pin);
  DEF_WRAPPED_FIELD(uint8_t, min_grant);
  DEF_WRAPPED_FIELD(uint8_t, max_latency);
};
static_assert(sizeof(FakePciType0Config) == 64, "Bad size for PciType0Config");

// A fake implementation of a PCI bridge configuration (Type 01h)
struct FakePciType1Config {
  DEF_WRAPPED_FIELD(uint16_t, vendor_id);
  DEF_WRAPPED_FIELD(uint16_t, device_id);
  DEF_WRAPPED_FIELD(uint16_t, command);
  DEF_SUBBIT(command_, 0, io_space_en);
  DEF_SUBBIT(command_, 1, mem_space_en);
  DEF_SUBBIT(command_, 2, bus_master_en);
  DEF_SUBBIT(command_, 3, special_cycles_en);
  DEF_SUBBIT(command_, 4, men_write_and_inval_en);
  DEF_SUBBIT(command_, 5, vga_palette_snoop_en);
  DEF_SUBBIT(command_, 6, parity_error_resp);
  // bit 7 is hardwired to 0.
  DEF_SUBBIT(command_, 8, serr_en);
  DEF_SUBBIT(command_, 9, fast_back_to_back_en);
  DEF_SUBBIT(command_, 10, interrupt_disable);
  DEF_WRAPPED_FIELD(uint16_t, status);
  // bits 2:0 are reserved.
  DEF_SUBBIT(status_, 3, int_status);
  DEF_SUBBIT(status_, 4, capabilities_list);
  DEF_SUBBIT(status_, 5, is_66mhz_capable);
  // bit 6 is reserved.
  DEF_SUBBIT(status_, 7, fast_back_to_back_capable);
  DEF_SUBBIT(status_, 8, master_data_parity_error);
  DEF_SUBFIELD(status_, 10, 9, devsel_timing);
  DEF_SUBBIT(status_, 11, signaled_target_abort);
  DEF_SUBBIT(status_, 12, received_target_abort);
  DEF_SUBBIT(status_, 13, received_master_abort);
  DEF_SUBBIT(status_, 14, signaled_system_error);
  DEF_SUBBIT(status_, 15, detected_parity_error);
  DEF_WRAPPED_FIELD(uint8_t, revision_id);
  DEF_WRAPPED_FIELD(uint8_t, program_interface);
  DEF_WRAPPED_FIELD(uint8_t, sub_class);
  DEF_WRAPPED_FIELD(uint8_t, base_class);
  DEF_WRAPPED_FIELD(uint8_t, cache_line_size);
  DEF_WRAPPED_FIELD(uint8_t, latency_timer);
  DEF_WRAPPED_FIELD(uint8_t, header_type);
  DEF_WRAPPED_FIELD(uint8_t, bist);
  DEF_SUBFIELD(bist_, 3, 0, completion_code);
  // bits 5:4 are reserved.
  DEF_SUBBIT(bist_, 6, start_bist);
  DEF_SUBBIT(bist_, 7, bist_capable);
  FakeBaseAddress base_address[2];
  DEF_WRAPPED_FIELD(uint8_t, primary_bus_number);
  DEF_WRAPPED_FIELD(uint8_t, secondary_bus_number);
  DEF_WRAPPED_FIELD(uint8_t, subordinate_bus_number);
  DEF_WRAPPED_FIELD(uint8_t, secondary_latency_timer);
  DEF_WRAPPED_FIELD(uint8_t, io_base);
  DEF_WRAPPED_FIELD(uint8_t, io_limit);
  DEF_WRAPPED_FIELD(uint16_t, secondary_status);
  // bits 4:0 are reserved.
  DEF_SUBBIT(secondary_status_, 5, secondary_is_66mhz_capable);
  // bit 6 is reserved.
  DEF_SUBBIT(secondary_status_, 7, secondary_fast_back_to_back_capable);
  DEF_SUBBIT(secondary_status_, 8, secondary_master_data_parity_error);
  DEF_SUBFIELD(secondary_status_, 10, 9, secondary_devsel_timing);
  DEF_SUBBIT(secondary_status_, 11, secondary_signaled_target_abort);
  DEF_SUBBIT(secondary_status_, 12, secondary_received_target_abort);
  DEF_SUBBIT(secondary_status_, 13, secondary_received_master_abort);
  DEF_SUBBIT(secondary_status_, 14, secondary_signaled_system_error);
  DEF_SUBBIT(secondary_status_, 15, secondary_detected_parity_error);
  DEF_WRAPPED_FIELD(uint16_t, memory_base);
  DEF_WRAPPED_FIELD(uint16_t, memory_limit);
  DEF_WRAPPED_FIELD(uint16_t, prefetchable_memory_base);
  DEF_WRAPPED_FIELD(uint16_t, prefetchable_memory_limit);
  DEF_WRAPPED_FIELD(uint32_t, prfetchable_memory_base_upper);
  DEF_WRAPPED_FIELD(uint32_t, prfetchable_memory_limit_upper);
  DEF_WRAPPED_FIELD(uint16_t, io_base_upper);
  DEF_WRAPPED_FIELD(uint16_t, io_limit_upper);
  DEF_WRAPPED_FIELD(uint8_t, capabilities_ptr);

 private:
  [[maybe_unused]] uint8_t reserved_0[3];

 public:
  DEF_WRAPPED_FIELD(uint32_t, expansion_rom_address);
  DEF_WRAPPED_FIELD(uint8_t, interrupt_line);
  DEF_WRAPPED_FIELD(uint8_t, interrupt_pin);
  DEF_WRAPPED_FIELD(uint16_t, bridge_control);
  DEF_SUBBIT(bridge_control_, 0, secondary_parity_error_resp);
  DEF_SUBBIT(bridge_control_, 1, secondary_serr_en);
  DEF_SUBBIT(bridge_control_, 2, isa_enable);
  DEF_SUBBIT(bridge_control_, 3, vga_enable);
  DEF_SUBBIT(bridge_control_, 4, vga_16bit_decode);
  DEF_SUBBIT(bridge_control_, 5, master_abort_mode);
  DEF_SUBBIT(bridge_control_, 6, seconday_bus_reset);
  DEF_SUBBIT(bridge_control_, 7, secondary_fast_back_to_back_en);
  DEF_SUBBIT(bridge_control_, 8, primary_discard_timer);
  DEF_SUBBIT(bridge_control_, 9, secondary_discard_timer);
  DEF_SUBBIT(bridge_control_, 10, discard_timer_status);
  DEF_SUBBIT(bridge_control_, 11, discard_timer_serr_en);
  // bits 15:12 are reserved.
};
static_assert(sizeof(FakePciType1Config) == 64, "Bad size for PciType1Config");
#undef DEF_WRAPPED_FIELD

// FakeEcam represents a contiguous block of PCI devices covering the bus range
// from |bus_start|:|bus_end|. This allows tests to create a virtual collection
// of buses that look like a real contiguous ecam with valid devices to scan
// and poke at by the PCI bus driver.
class FakeEcam {
 public:
  FakeEcam(uint8_t bus_start, uint8_t bus_cnt, bool is_extended)
      : bus_start_(bus_start),
        bus_cnt_(bus_cnt),
        is_extended_(is_extended),
        config_size_((is_extended) ? pci::kExtendedConfigSize : pci::kBaseConfigSize),
        mmio_(fdf_testing::CreateMmioBuffer(static_cast<size_t>(bus_cnt) *
                                            (pci::kMaxDevicesPerBus * pci::kMaxFunctionsPerDevice) *
                                            config_size_)) {
    reset();
  }

  fdf::MmioView get_config_view(pci_bdf_t address) {
    return mmio_.View(pci::GetConfigOffsetInCam(address, bus_start_, is_extended_), config_size_);
  }

  FakePciType0Config* get_device(pci_bdf_t address) {
    zx_vaddr_t offset = pci::GetConfigOffsetInCam(address, bus_start_, is_extended_);
    return reinterpret_cast<FakePciType0Config*>(reinterpret_cast<uintptr_t>(mmio_.get()) + offset);
  }

  FakePciType1Config* get_bridge(pci_bdf_t address) {
    zx_vaddr_t offset = pci::GetConfigOffsetInCam(address, bus_start_, is_extended_);
    return reinterpret_cast<FakePciType1Config*>(reinterpret_cast<uintptr_t>(mmio_.get()) + offset);
  }

  fdf::MmioView mmio() { return mmio_.View(0); }
  void reset() {
    for (zx_off_t offset = 0; offset < mmio_.get_size(); offset += sizeof(uint64_t)) {
      mmio_.Write64(0, offset);
      // If we're aligned to the start of a new device then set the two u16 registers for vendor and
      // device id to 0xFF since that tells PCI nothing is there.
      if (offset % config_size_ == 0) {
        mmio_.Write32(0xFFFFFFFF, offset);
      }
    }
  }

  uint8_t bus_start() const { return bus_start_; }
  uint8_t bus_cnt() const { return bus_cnt_; }
  uint8_t is_extended() const { return is_extended_; }
  size_t config_size() const { return config_size_; }
  std::unique_ptr<pci::Config> CreateMmioConfig(pci_bdf_t bdf) {
    return std::move(
        pci::MmioConfig::Create(bdf, mmio_, bus_start_, bus_start_ + bus_cnt_, is_extended_)
            .value());
  }

  zx_off_t GetConfigOffset(pci_bdf_t bdf) const {
    return pci::GetConfigOffsetInCam(bdf, bus_start_, is_extended_);
  }

 private:
  const uint8_t bus_start_;
  const uint8_t bus_cnt_;
  const bool is_extended_;
  const size_t config_size_;
  fdf::MmioBuffer mmio_;
};

#endif  // SRC_DEVICES_PCI_DRIVERS_PCI_TEST_FAKES_FAKE_ECAM_H_

// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_DEVICES_PCI_DRIVERS_PCI_CONFIG_H_
#define SRC_DEVICES_PCI_DRIVERS_PCI_CONFIG_H_

#include <endian.h>
#include <fidl/fuchsia.hardware.pci/cpp/wire.h>
#include <fuchsia/hardware/pciroot/cpp/banjo.h>
#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/pci/constants.h>
#include <lib/pci/hw.h>
#include <lib/zx/result.h>
#include <stdio.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <hwreg/bitfields.h>

namespace pci {
namespace config {

// Fields correspond to the name in the PCI Local Bus Spec section 6.2.
struct Command {
  uint16_t value;
  // 15-11 are reserved preserve.
  DEF_SUBBIT(value, 10, interrupt_disable);
  DEF_SUBBIT(value, 9, fast_back_to_back_enable);
  DEF_SUBBIT(value, 8, serr_enable);
  // 7 is reserved preserve.
  DEF_SUBBIT(value, 6, parity_error_response);
  DEF_SUBBIT(value, 5, vga_palette_snoop);
  DEF_SUBBIT(value, 4, memory_write_and_invalidate_enable);
  DEF_SUBBIT(value, 3, special_cycles);
  DEF_SUBBIT(value, 2, bus_master);
  DEF_SUBBIT(value, 1, memory_space);
  DEF_SUBBIT(value, 0, io_space);
};

struct Status {
  uint16_t value;
  DEF_SUBBIT(value, 15, detected_parity_error);
  DEF_SUBBIT(value, 14, signaled_system_error);
  DEF_SUBBIT(value, 13, received_master_abort);
  DEF_SUBBIT(value, 12, received_target_abort);
  DEF_SUBBIT(value, 11, signaled_target_abort);
  DEF_SUBFIELD(value, 10, 9, devsel_timing);
  DEF_SUBBIT(value, 8, master_data_parity_error);
  DEF_SUBBIT(value, 7, fast_b2b_capable);
  // 6 is reserved preserve.
  DEF_SUBBIT(value, 5, capable_66mhz);
  DEF_SUBBIT(value, 4, has_capabilities_list);
  DEF_SUBBIT(value, 3, interrupt_status);
  // 2-0 are reserved preserve.
};

// The layout of a Base Address Register changes based on its type.
// PCI Local Bus Spec section 6.2.5.1.
struct IoBaseAddress;
struct MmioBaseAddress;
struct BaseAddress : public hwreg::RegisterBase<BaseAddress, uint32_t> {
  DEF_RSVDZ_BIT(1);
  DEF_BIT(0, is_io_space);

  IoBaseAddress* io() { return reinterpret_cast<IoBaseAddress*>(this); }
  MmioBaseAddress* mmio() { return reinterpret_cast<MmioBaseAddress*>(this); }
  static auto Get() { return hwreg::RegisterAddr<BaseAddress>(0); }
};

struct IoBaseAddress : public BaseAddress {
  DEF_UNSHIFTED_FIELD(31, 2, base_address);
};

struct MmioBaseAddress : public BaseAddress {
  DEF_UNSHIFTED_FIELD(31, 4, base_address);
  DEF_BIT(3, is_prefetchable);
  DEF_BIT(2, is_64_bit);
};

}  // namespace config

// Per spec (quoting Linux DT host-generic bindings):
// For CAM, this 24-bit offset is:
//         cfg_offset(bus, device, function, register) =
//                    bus << 16 | device << 11 | function << 8 | register
// While ECAM extends this by 4 bits to accommodate 4k of function space:
//         cfg_offset(bus, device, function, register) =
//                    bus << 20 | device << 15 | function << 12 | register
struct CamOffset {
  zx_vaddr_t offset = 0;
  DEF_SUBFIELD(offset, 23, 16, bus);
  DEF_SUBFIELD(offset, 15, 11, device);
  DEF_SUBFIELD(offset, 10, 8, function);
};

struct EcamOffset {
  zx_vaddr_t offset = 0;
  DEF_SUBFIELD(offset, 27, 20, bus);
  DEF_SUBFIELD(offset, 19, 15, device);
  DEF_SUBFIELD(offset, 14, 12, function);
};

template <class CamType>
constexpr zx_vaddr_t GetConfigOffsetInCam(pci_bdf_t bdf, uint8_t start_bus) {
  zx_vaddr_t bus_offset = CamType().set_bus(start_bus).offset;
  zx_vaddr_t bdf_offset =
      CamType().set_bus(bdf.bus_id).set_device(bdf.device_id).set_function(bdf.function_id).offset;
  return bdf_offset - bus_offset;
}

// Find the offset into the cam region for the given bdf address. Every bus
// has 32 devices, every device has 8 functions, and each function has a
// configuration access region of 256 bytes base, 4096 if extended. The base
// address of the vmo provided to the bus driver corresponds to the
// start_bus_num, so offset the bdf address based on the bottom of our ecam.
constexpr zx_vaddr_t GetConfigOffsetInCam(pci_bdf_t bdf, uint8_t start_bus, bool is_extended) {
  if (is_extended) {
    return GetConfigOffsetInCam<EcamOffset>(bdf, start_bus);
  }
  return GetConfigOffsetInCam<CamOffset>(bdf, start_bus);
}

class PciReg8 {
 public:
  constexpr PciReg8() = default;
  constexpr explicit PciReg8(uint16_t offset) : offset_(offset) {}
  constexpr uint16_t offset() const { return offset_; }
  inline bool operator==(const PciReg8& other) const { return (offset() == other.offset()); }

 private:
  uint16_t offset_ = 0;
};

class PciReg16 {
 public:
  constexpr PciReg16() = default;
  constexpr explicit PciReg16(uint16_t offset) : offset_(offset) {}
  constexpr uint16_t offset() const { return offset_; }
  inline bool operator==(const PciReg8& other) const { return (offset() == other.offset()); }

 private:
  uint16_t offset_ = 0;
};

class PciReg32 {
 public:
  constexpr PciReg32() = default;
  constexpr explicit PciReg32(uint16_t offset) : offset_(offset) {}
  constexpr uint16_t offset() const { return offset_; }
  inline bool operator==(const PciReg8& other) const { return (offset() == other.offset()); }

 private:
  uint16_t offset_ = 0;
};

// Config supplies the factory for creating the appropriate pci config
// object based on the address space of the pci device.
class Config {
 public:
  // Standard PCI configuration space values. Offsets from PCI Firmware Spec ch 6.
  static constexpr PciReg16 kVendorId = PciReg16(pci::kConfigVendorId);
  static constexpr PciReg16 kDeviceId = PciReg16(pci::kConfigDeviceId);
  static constexpr PciReg16 kCommand = PciReg16(pci::kConfigCommand);
  static constexpr PciReg16 kStatus = PciReg16(pci::kConfigStatus);
  static constexpr PciReg8 kRevisionId = PciReg8(pci::kConfigRevisionId);
  static constexpr PciReg8 kProgramInterface = PciReg8(pci::kConfigClassCodeIntr);
  static constexpr PciReg8 kSubClass = PciReg8(pci::kConfigClassCodeSub);
  static constexpr PciReg8 kBaseClass = PciReg8(pci::kConfigClassCodeBase);
  static constexpr PciReg8 kCacheLineSize = PciReg8(pci::kConfigCacheLineSize);
  static constexpr PciReg8 kLatencyTimer = PciReg8(pci::kConfigLatencyTimer);
  static constexpr PciReg8 kHeaderType = PciReg8(pci::kConfigHeaderType);
  static constexpr PciReg8 kBist = PciReg8(pci::kConfigBist);
  // 0x10 is the address of the first BAR in config space
  // BAR rather than BaseAddress for space / sanity considerations
  static constexpr PciReg32 kBar(uint32_t bar) {
    ZX_ASSERT(bar < pci::kMaxBarCount);
    return PciReg32(static_cast<uint16_t>(pci::kConfigBaseAddresses + (bar * sizeof(uint32_t))));
  }
  static constexpr PciReg32 kCardbusCisPtr = PciReg32(pci::kConfigCardbusCisPtr);
  static constexpr PciReg16 kSubsystemVendorId = PciReg16(pci::kConfigSubsysVendorId);
  static constexpr PciReg16 kSubsystemId = PciReg16(pci::kConfigSubsysId);
  static constexpr PciReg32 kExpansionRomAddress = PciReg32(pci::kConfigExpRomAddress);
  static constexpr PciReg8 kCapabilitiesPtr = PciReg8(pci::kConfigCapabilities);
  // 0x35 through 0x3B is reserved
  static constexpr PciReg8 kInterruptLine = PciReg8(pci::kConfigInterruptLine);
  static constexpr PciReg8 kInterruptPin = PciReg8(pci::kConfigInterruptPin);
  static constexpr PciReg8 kMinGrant = PciReg8(pci::kConfigMinGrant);
  static constexpr PciReg8 kMaxLatency = PciReg8(pci::kConfigMaxLatency);
  static constexpr uint8_t kStdCfgEnd =
      static_cast<uint8_t>(kMaxLatency.offset() + sizeof(uint8_t));

  // pci to pci bridge config
  // Unlike a normal PCI header, a bridge only has two BARs, but the BAR offset in config space
  // is the same.
  static constexpr PciReg8 kPrimaryBusId = PciReg8(0x18);
  static constexpr PciReg8 kSecondaryBusId = PciReg8(0x19);
  static constexpr PciReg8 kSubordinateBusId = PciReg8(0x1A);
  static constexpr PciReg8 kSecondaryLatencyTimer = PciReg8(0x1B);
  static constexpr PciReg8 kIoBase = PciReg8(0x1C);
  static constexpr PciReg8 kIoLimit = PciReg8(0x1D);
  static constexpr PciReg16 kSecondaryStatus = PciReg16(0x1E);
  static constexpr PciReg16 kMemoryBase = PciReg16(0x20);
  static constexpr PciReg16 kMemoryLimit = PciReg16(0x22);
  static constexpr PciReg16 kPrefetchableMemoryBase = PciReg16(0x24);
  static constexpr PciReg16 kPrefetchableMemoryLimit = PciReg16(0x26);
  static constexpr PciReg32 kPrefetchableMemoryBaseUpper = PciReg32(0x28);
  static constexpr PciReg32 kPrefetchableMemoryLimitUpper = PciReg32(0x2C);
  static constexpr PciReg16 kIoBaseUpper = PciReg16(0x30);
  static constexpr PciReg16 kIoLimitUpper = PciReg16(0x32);
  // Capabilities Pointer for a bridge matches the standard 0x34 offset
  // 0x35 through 0x38 is reserved
  static constexpr PciReg32 kBridgeExpansionRomAddress = PciReg32(0x38);
  // interrupt line for a bridge matches the standard 0x3C offset
  // interrupt pin for a bridge matches the standard 0x3D offset
  static constexpr PciReg16 kBridgeControl = PciReg16(0x3E);

  inline const pci_bdf_t& bdf() const { return bdf_; }
  inline const char* addr() const { return addr_; }
  virtual const char* type() const = 0;
  // Return a copy of the MmioView backing the Config's MMIO space, if supported.
  virtual zx::result<fdf::MmioView> get_view() const { return zx::error(ZX_ERR_NOT_SUPPORTED); }

  // Virtuals
  void DumpConfig(uint16_t len) const;
  virtual uint8_t Read(PciReg8 addr) const = 0;
  virtual uint16_t Read(PciReg16 addr) const = 0;
  virtual uint32_t Read(PciReg32 addr) const = 0;
  virtual void Write(PciReg8 addr, uint8_t val) const = 0;
  virtual void Write(PciReg16 addr, uint16_t val) const = 0;
  virtual void Write(PciReg32 addr, uint32_t val) const = 0;
  virtual ~Config() = default;

 protected:
  explicit Config(pci_bdf_t bdf) : bdf_(bdf) {
    snprintf(addr_, sizeof(addr_), "%02x:%02x.%01x", bdf_.bus_id, bdf_.device_id, bdf_.function_id);
  }

 private:
  const pci_bdf_t bdf_;
  char addr_[8];
};

// MMIO config is the stardard method for accessing modern pci configuration space.
// A device's configuration space is mapped to a specific place in a given pci root's
// ecam and can be directly accessed with standard IO operations.t
class MmioConfig : public Config {
 public:
  static zx::result<std::unique_ptr<Config>> Create(pci_bdf_t bdf, const fdf::MmioBuffer& ecam_,
                                                    uint8_t start_bus, uint8_t end_bus,
                                                    bool is_extended);
  uint8_t Read(PciReg8 addr) const final;
  uint16_t Read(PciReg16 addr) const final;
  uint32_t Read(PciReg32 addr) const final;
  void Write(PciReg8 addr, uint8_t val) const final;
  void Write(PciReg16 addr, uint16_t val) const final;
  void Write(PciReg32 addr, uint32_t val) const override;
  const char* type() const override;
  zx::result<fdf::MmioView> get_view() const final { return zx::ok(fdf::MmioView(view_)); }

 private:
  friend class FakeMmioConfig;
  MmioConfig(pci_bdf_t bdf, const fdf::MmioView& view) : Config(bdf), view_(view) {}
  const fdf::MmioView view_;
};

// ProxyConfig is used with PCI buses that do not support MMIO config space,
// or require special controller configuration before config access. Examples
// of this are IO config on x64 due to needing to synchronize CF8/CFC with
// ACPI, and Designware on ARM where the controller needs to be configured to
// map a given device's configuration space in before access.
//
// For proxy configuration access all operations are passed to the pciroot
// protocol implementation hosted in the same devhost as the pci bus driver.
class ProxyConfig final : public Config {
 public:
  static zx::result<std::unique_ptr<Config>> Create(pci_bdf_t bdf,
                                                    ddk::PcirootProtocolClient* proto);
  uint8_t Read(PciReg8 addr) const final;
  uint16_t Read(PciReg16 addr) const final;
  uint32_t Read(PciReg32 addr) const final;
  void Write(PciReg8 addr, uint8_t val) const final;
  void Write(PciReg16 addr, uint16_t val) const final;
  void Write(PciReg32 addr, uint32_t val) const final;
  const char* type() const final;

 private:
  ProxyConfig(pci_bdf_t bdf, ddk::PcirootProtocolClient* proto) : Config(bdf), pciroot_(proto) {}
  // The bus driver outlives config objects.
  ddk::PcirootProtocolClient* const pciroot_;
};

}  // namespace pci

#endif  // SRC_DEVICES_PCI_DRIVERS_PCI_CONFIG_H_

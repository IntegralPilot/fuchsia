// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/pci/drivers/pci/config.h"

#include <assert.h>
#include <fuchsia/hardware/pciroot/cpp/banjo.h>
#include <inttypes.h>
#include <lib/ddk/debug.h>
#include <lib/pci/constants.h>

#include <optional>

#include <pretty/hexdump.h>

namespace pci {

// MMIO Config Implementation
zx::result<std::unique_ptr<Config>> MmioConfig::Create(pci_bdf_t bdf, const fdf::MmioBuffer& ecam,
                                                       uint8_t start_bus, uint8_t end_bus,
                                                       bool is_extended) {
  if (bdf.bus_id < start_bus || bdf.bus_id > end_bus || bdf.device_id >= pci::kMaxDevicesPerBus ||
      bdf.function_id >= pci::kMaxFunctionsPerDevice) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  zx_vaddr_t config_offset = GetConfigOffsetInCam(bdf, start_bus, /*is_extended=*/is_extended);
  zx_vaddr_t config_size = (is_extended) ? pci::kExtendedConfigSize : pci::kBaseConfigSize;

  return zx::ok(
      std::unique_ptr<MmioConfig>(new MmioConfig(bdf, ecam.View(config_offset, config_size))));
}

uint8_t MmioConfig::Read(const PciReg8 addr) const { return view_.Read<uint8_t>(addr.offset()); }

uint16_t MmioConfig::Read(const PciReg16 addr) const { return view_.Read<uint16_t>(addr.offset()); }

uint32_t MmioConfig::Read(const PciReg32 addr) const { return view_.Read<uint32_t>(addr.offset()); }

void MmioConfig::Write(PciReg8 addr, uint8_t val) const { view_.Write(val, addr.offset()); }

void MmioConfig::Write(PciReg16 addr, uint16_t val) const { view_.Write(val, addr.offset()); }

void MmioConfig::Write(PciReg32 addr, uint32_t val) const { view_.Write(val, addr.offset()); }

const char* MmioConfig::type() const { return "mmio"; }

// Proxy Config Implementation
zx::result<std::unique_ptr<Config>> ProxyConfig::Create(pci_bdf_t bdf,
                                                        ddk::PcirootProtocolClient* proto) {
  // Can't use std::make_unique because the constructor is private.
  return zx::ok(std::unique_ptr<ProxyConfig>(new ProxyConfig(bdf, proto)));
}

uint8_t ProxyConfig::Read(const PciReg8 addr) const {
  uint8_t tmp;
  ZX_ASSERT(pciroot_->ReadConfig8(&bdf(), addr.offset(), &tmp) == ZX_OK);
  return tmp;
}

uint16_t ProxyConfig::Read(const PciReg16 addr) const {
  uint16_t tmp;
  ZX_ASSERT(pciroot_->ReadConfig16(&bdf(), addr.offset(), &tmp) == ZX_OK);
  return tmp;
}

uint32_t ProxyConfig::Read(const PciReg32 addr) const {
  uint32_t tmp;
  ZX_ASSERT(pciroot_->ReadConfig32(&bdf(), addr.offset(), &tmp) == ZX_OK);
  return tmp;
}

void ProxyConfig::Write(PciReg8 addr, uint8_t val) const {
  ZX_ASSERT(pciroot_->WriteConfig8(&bdf(), addr.offset(), val) == ZX_OK);
}

void ProxyConfig::Write(PciReg16 addr, uint16_t val) const {
  ZX_ASSERT(pciroot_->WriteConfig16(&bdf(), addr.offset(), val) == ZX_OK);
}

void ProxyConfig::Write(PciReg32 addr, uint32_t val) const {
  ZX_ASSERT(pciroot_->WriteConfig32(&bdf(), addr.offset(), val) == ZX_OK);
}

const char* ProxyConfig::type() const { return "proxy"; }

void Config::DumpConfig(uint16_t len) const {
  printf("%u bytes of raw config (type: %s)\n", len, type());
  // PIO space can't be dumped directly so we read a row at a time
  constexpr uint8_t row_len = 16;
  uint32_t pos = 0;
  uint8_t buf[row_len];

  do {
    for (uint16_t i = 0; i < row_len; i++) {
      buf[i] = Read(PciReg8(static_cast<uint8_t>(pos + i)));
    }

    hexdump8_ex(buf, row_len, pos);
    pos += row_len;
  } while (pos < pci::kBaseConfigSize);
}

}  // namespace pci

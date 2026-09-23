// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/usb/drivers/aml-usb-phy/usb-phy3.h"

#include <lib/driver/logging/cpp/logger.h>

#include "src/devices/usb/drivers/aml-usb-phy/usb-phy-regs.h"
#include "src/devices/usb/drivers/aml-usb-phy/usb-phy3-regs.h"

namespace aml_usb_phy {

namespace {

void dump_phy3_regs(const fdf::MmioBuffer& mmio) {
  DUMP_REG(PHY3_R1, mmio)
  DUMP_REG(PHY3_R2, mmio)
  DUMP_REG(PHY3_R4, mmio)
  DUMP_REG(PHY3_R5, mmio)
}

}  // namespace

void UsbPhy3::dump_regs() const {
  fdf::info("    {}", name());
  dump_phy3_regs(mmio());
}

zx_status_t UsbPhy3::CrBusAddr(uint32_t addr) {
  auto phy3_r4 = PHY3_R4::Get().FromValue(0).set_phy_cr_data_in(addr);
  phy3_r4.WriteTo(&mmio());
  phy3_r4.set_phy_cr_cap_addr(0);
  phy3_r4.WriteTo(&mmio());
  phy3_r4.set_phy_cr_cap_addr(1);
  phy3_r4.WriteTo(&mmio());

  auto timeout = zx::deadline_after(zx::msec(1000));
  auto phy3_r5 = PHY3_R5::Get().FromValue(0);
  do {
    phy3_r5 = PHY3_R5::Get().ReadFrom(&mmio());
  } while (phy3_r5.phy_cr_ack() == 0 && timeout > zx::clock::get_monotonic());

  if (phy3_r5.phy_cr_ack() != 1) {
    fdf::warn("{}: Read set cap addr for addr {:#x} timed out", name(), addr);
  }

  phy3_r4.set_phy_cr_cap_addr(0);
  phy3_r4.WriteTo(&mmio());
  timeout = zx::deadline_after(zx::msec(1000));
  do {
    phy3_r5 = PHY3_R5::Get().ReadFrom(&mmio());
  } while (phy3_r5.phy_cr_ack() == 1 && timeout > zx::clock::get_monotonic());

  if (phy3_r5.phy_cr_ack() != 0) {
    fdf::warn("{}: Read cap addr for addr {:#x} timed out", name(), addr);
  }

  return ZX_OK;
}

uint32_t UsbPhy3::CrBusRead(uint32_t addr) {
  CrBusAddr(addr);

  auto phy3_r4 = PHY3_R4::Get().FromValue(0);
  phy3_r4.set_phy_cr_read(0);
  phy3_r4.WriteTo(&mmio());
  phy3_r4.set_phy_cr_read(1);
  phy3_r4.WriteTo(&mmio());

  auto timeout = zx::deadline_after(zx::msec(1000));
  auto phy3_r5 = PHY3_R5::Get().FromValue(0);
  do {
    phy3_r5 = PHY3_R5::Get().ReadFrom(&mmio());
  } while (phy3_r5.phy_cr_ack() == 0 && timeout > zx::clock::get_monotonic());

  if (phy3_r5.phy_cr_ack() != 1) {
    fdf::warn("{}: Read set for addr {:#x} timed out", name(), addr);
  }

  uint32_t data = phy3_r5.phy_cr_data_out();

  phy3_r4.set_phy_cr_read(0);
  phy3_r4.WriteTo(&mmio());
  timeout = zx::deadline_after(zx::msec(1000));
  do {
    phy3_r5 = PHY3_R5::Get().ReadFrom(&mmio());
  } while (phy3_r5.phy_cr_ack() == 1 && timeout > zx::clock::get_monotonic());

  if (phy3_r5.phy_cr_ack() != 0) {
    fdf::warn("{}: Read for addr {:#x} timed out", name(), addr);
  }

  return data;
}

zx_status_t UsbPhy3::CrBusWrite(uint32_t addr, uint32_t data) {
  CrBusAddr(addr);

  auto phy3_r4 = PHY3_R4::Get().FromValue(0);
  phy3_r4.set_phy_cr_data_in(data);
  phy3_r4.WriteTo(&mmio());

  phy3_r4.set_phy_cr_cap_data(0);
  phy3_r4.WriteTo(&mmio());
  phy3_r4.set_phy_cr_cap_data(1);
  phy3_r4.WriteTo(&mmio());

  auto timeout = zx::deadline_after(zx::msec(1000));
  auto phy3_r5 = PHY3_R5::Get().FromValue(0);
  do {
    phy3_r5 = PHY3_R5::Get().ReadFrom(&mmio());
  } while (phy3_r5.phy_cr_ack() == 0 && timeout > zx::clock::get_monotonic());

  if (phy3_r5.phy_cr_ack() != 1) {
    fdf::warn("{}: Write cap data for addr {:#x} timed out", name(), addr);
  }

  phy3_r4.set_phy_cr_cap_data(0);
  phy3_r4.WriteTo(&mmio());
  timeout = zx::deadline_after(zx::msec(1000));
  do {
    phy3_r5 = PHY3_R5::Get().ReadFrom(&mmio());
  } while (phy3_r5.phy_cr_ack() == 1 && timeout > zx::clock::get_monotonic());

  if (phy3_r5.phy_cr_ack() != 0) {
    fdf::warn("{}: Write cap data reset for addr {:#x} timed out", name(), addr);
  }

  phy3_r4.set_phy_cr_write(0);
  phy3_r4.WriteTo(&mmio());
  phy3_r4.set_phy_cr_write(1);
  phy3_r4.WriteTo(&mmio());
  timeout = zx::deadline_after(zx::msec(1000));
  do {
    phy3_r5 = PHY3_R5::Get().ReadFrom(&mmio());
  } while (phy3_r5.phy_cr_ack() == 0 && timeout > zx::clock::get_monotonic());

  if (phy3_r5.phy_cr_ack() != 1) {
    fdf::warn("{}: Write for addr {:#x} timed out", name(), addr);
  }

  phy3_r4.set_phy_cr_write(0);
  phy3_r4.WriteTo(&mmio());
  timeout = zx::deadline_after(zx::msec(1000));
  do {
    phy3_r5 = PHY3_R5::Get().ReadFrom(&mmio());
  } while (phy3_r5.phy_cr_ack() == 1 && timeout > zx::clock::get_monotonic());

  if (phy3_r5.phy_cr_ack() != 0) {
    fdf::warn("{}: Disable write for addr {:#x} timed out", name(), addr);
  }

  return ZX_OK;
}

zx_status_t UsbPhy3::Init(fdf::MmioBuffer& usbctrl_mmio) {
  auto reg = mmio().Read32(0);
  reg = reg | (3 << 5);
  mmio().Write32(reg, 0);

  zx::nanosleep(zx::deadline_after(zx::usec(100)));

  USB_R3_V2::Get()
      .ReadFrom(&usbctrl_mmio)
      .set_p30_ssc_en(1)
      .set_p30_ssc_range(2)
      .set_p30_ref_ssp_en(1)
      .WriteTo(&usbctrl_mmio);

  zx::nanosleep(zx::deadline_after(zx::usec(2)));

  USB_R2_V2::Get()
      .ReadFrom(&usbctrl_mmio)
      .set_p30_pcs_tx_deemph_3p5db(0x15)
      .set_p30_pcs_tx_deemph_6db(0x20)
      .WriteTo(&usbctrl_mmio);

  zx::nanosleep(zx::deadline_after(zx::usec(2)));

  USB_R1_V2::Get()
      .ReadFrom(&usbctrl_mmio)
      .set_u3h_host_port_power_control_present(1)
      .set_u3h_fladj_30mhz_reg(0x20)
      .set_p30_pcs_tx_swing_full(127)
      .WriteTo(&usbctrl_mmio);

  zx::nanosleep(zx::deadline_after(zx::usec(2)));

  auto phy3_r2 = PHY3_R2::Get().ReadFrom(&mmio());
  phy3_r2.set_phy_tx_vboost_lvl(0x4);
  phy3_r2.WriteTo(&mmio());
  zx::nanosleep(zx::deadline_after(zx::usec(2)));

  uint32_t data = CrBusRead(0x102d);
  data |= (1 << 7);
  CrBusWrite(0x102D, data);

  data = CrBusRead(0x1010);
  data &= ~0xff0;
  data |= 0x20;
  CrBusWrite(0x1010, data);

  data = CrBusRead(0x1006);
  data &= ~(1 << 6);
  data |= (1 << 7);
  data &= ~(0x7 << 8);
  data |= (0x3 << 8);
  data |= (0x1 << 11);
  CrBusWrite(0x1006, data);

  data = CrBusRead(0x1002);
  data &= ~0x3f80;
  data |= (0x16 << 7);
  data &= ~0x7f;
  data |= (0x7f | (1 << 14));
  CrBusWrite(0x1002, data);

  data = CrBusRead(0x30);
  data &= ~(0xf << 4);
  data |= (0x8 << 4);
  CrBusWrite(0x30, data);
  zx::nanosleep(zx::deadline_after(zx::usec(2)));

  auto phy3_r1 = PHY3_R1::Get().ReadFrom(&mmio());
  phy3_r1.set_phy_los_bias(0x4);
  phy3_r1.set_phy_los_level(0x9);
  phy3_r1.WriteTo(&mmio());

  return ZX_OK;
}

}  // namespace aml_usb_phy

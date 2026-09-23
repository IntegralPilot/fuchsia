// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/usb/drivers/aml-usb-phy/usb-phy2.h"

#include <lib/driver/logging/cpp/logger.h>

#include "src/devices/usb/drivers/aml-usb-phy/usb-phy-regs.h"
#include "src/devices/usb/drivers/aml-usb-phy/usb-phy2-regs.h"

namespace aml_usb_phy {

namespace {

class PHY2_R : public hwreg::RegisterBase<PHY2_R, uint32_t> {
 public:
  static auto Get(uint32_t reg, uint32_t i) { return hwreg::RegisterAddr<PHY2_R>(reg * 4); }

  static void dump(uint32_t reg, uint32_t i, const fdf::MmioBuffer& mmio, bool starred) {
    fdf::info("{}PHY2_R{}: {:x}", (starred) ? "*" : " ", reg,
              PHY2_R::Get(reg, i).ReadFrom(&mmio).reg_value());
  }
};

void dump_phy2_regs(uint32_t idx, const fdf::MmioBuffer& mmio) {
  uint32_t used_regs[] = {0, 1, 3, 4, 13, 14, 16, 17, 18, 20, 21};
  uint32_t cur = 0;
  for (uint32_t i = 0; i < 24; i++) {
    PHY2_R::dump(i, idx, mmio, used_regs[cur] == i);
    if (used_regs[cur] == i) {
      cur++;
    }
  }
}

}  // namespace

void UsbPhy2::dump_regs() const {
  fdf::info("    {}", name());
  dump_phy2_regs(idx(), mmio());
}

// Based on set_usb_pll() in phy-aml-new-usb2-v2.c
void UsbPhy2::InitPll(fuchsia_hardware_usb_phy::AmlogicPhyType type, bool needs_hack) {
  PHY2_R16::Get()
      .FromValue(0)
      .set_usb2_mppll_m(0x14)
      .set_usb2_mppll_n(1)
      .set_usb2_mppll_load(1)
      .set_usb2_mppll_lock_long(1)
      .set_usb2_mppll_fast_lock(1)
      .set_usb2_mppll_en(1)
      .set_usb2_mppll_reset(1)
      .WriteTo(&mmio());

  PHY2_R17::Get()
      .FromValue(0)
      .set_usb2_mppll_lambda1(7)
      .set_usb2_mppll_lambda0(7)
      .set_usb2_mppll_filter_pvt2(2)
      .set_usb2_mppll_filter_pvt1(9)
      .WriteTo(&mmio());

  PHY2_R18::Get()
      .FromValue(0)
      .set_usb2_mppll_lkw_sel(1)
      .set_usb2_mppll_lk_w(9)
      .set_usb2_mppll_lk_s(0x27)
      .set_usb2_mppll_dco_clk_sel(!needs_hack)
      .set_usb2_mppll_pfd_gain(1)
      .set_usb2_mppll_rou(7)
      .set_usb2_mppll_data_sel(3)
      .set_usb2_mppll_bias_adj(1)
      .set_usb2_mppll_alpha(3)
      .set_usb2_mppll_adj_ldo(1)
      .set_usb2_mppll_acg_range(1)
      .WriteTo(&mmio());

  zx::nanosleep(zx::deadline_after(zx::usec(100)));

  PHY2_R16::Get()
      .FromValue(0)
      .set_usb2_mppll_m(0x14)
      .set_usb2_mppll_n(1)
      .set_usb2_mppll_load(1)
      .set_usb2_mppll_lock_long(1)
      .set_usb2_mppll_fast_lock(1)
      .set_usb2_mppll_en(1)
      .set_usb2_mppll_reset(0)
      .WriteTo(&mmio());

  // Phy tuning for G12B
  PHY2_R20::Get()
      .FromValue(0)
      .set_usb2_otg_vbus_trim(4)
      .set_usb2_otg_vbusdet_en(1)
      .set_usb2_dmon_sel(0xf)
      .set_usb2_edgedrv_en(1)
      .set_usb2_edgedrv_trim(3)
      .WriteTo(&mmio());

  if (type == fuchsia_hardware_usb_phy::AmlogicPhyType::kG12A) {
    PHY2_R4::Get()
        .FromValue(0)
        .set_Calibration_code_Value(0xfff)
        .set_TEST_Bypass_mode_enable(!needs_hack)
        .WriteTo(&mmio());

    // Recovery state
    PHY2_R14::Get().FromValue(0).WriteTo(&mmio());
    PHY2_R13::Get()
        .FromValue(0)
        .set_Update_PMA_signals(1)
        .set_minimum_count_for_sync_detection(7)
        .WriteTo(&mmio());
  } else if (type == fuchsia_hardware_usb_phy::AmlogicPhyType::kG12B) {
    PHY2_R21::Get()
        .FromValue(0)
        .set_usb2_cal_ack_en(1)
        .set_usb2_tx_strg_pd(1)
        .set_usb2_otg_aca_trim(2)
        .WriteTo(&mmio());
    PHY2_R13::Get().FromValue(0).set_minimum_count_for_sync_detection(7).WriteTo(&mmio());
  }

  // Disconnect threshold
  PHY2_R3::Get()
      .FromValue(0)
      .set_disc_ref(3)
      .set_hsdic_ref(type == fuchsia_hardware_usb_phy::AmlogicPhyType::kG12A ? 3 : 2)
      .WriteTo(&mmio());
}

void UsbPhy2::SetModeInternal(fuchsia_hardware_usb_phy::Mode mode, fdf::MmioBuffer& usbctrl_mmio) {
  ZX_DEBUG_ASSERT(mode == fuchsia_hardware_usb_phy::Mode::kHost ||
                  mode == fuchsia_hardware_usb_phy::Mode::kPeripheral);
  if ((dr_mode() == fuchsia_hardware_usb_phy::Mode::kHost &&
       mode != fuchsia_hardware_usb_phy::Mode::kHost) ||
      (dr_mode() == fuchsia_hardware_usb_phy::Mode::kPeripheral &&
       mode != fuchsia_hardware_usb_phy::Mode::kPeripheral)) {
    fdf::error("If dr_mode_ is not USB_MODE_OTG, dr_mode_ must match requested mode.");
    return;
  }

  fdf::info("{}: Entering USB {} Mode", name(),
            mode == fuchsia_hardware_usb_phy::Mode::kHost ? "Host" : "Peripheral");

  if (mode == phy_mode())
    return;

  if (is_otg_capable()) {
    auto r0 = USB_R0_V2::Get().ReadFrom(&usbctrl_mmio);
    if (mode == fuchsia_hardware_usb_phy::Mode::kHost) {
      r0.set_u2d_act(0);
    } else {
      r0.set_u2d_act(1);
      r0.set_u2d_ss_scaledown_mode(0);
    }
    r0.WriteTo(&usbctrl_mmio);

    USB_R4_V2::Get()
        .ReadFrom(&usbctrl_mmio)
        .set_p21_sleepm0(mode == fuchsia_hardware_usb_phy::Mode::kPeripheral)
        .WriteTo(&usbctrl_mmio);
  }

  U2P_R0_V2::Get(idx())
      .ReadFrom(&usbctrl_mmio)
      .set_host_device(mode == fuchsia_hardware_usb_phy::Mode::kHost)
      .set_por(0)
      .WriteTo(&usbctrl_mmio);
}

}  // namespace aml_usb_phy

// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_DRIVERS_AML_USB_PHY_USB_PHY2_REGS_H_
#define SRC_DEVICES_USB_DRIVERS_AML_USB_PHY_USB_PHY2_REGS_H_

#include <lib/driver/mmio/cpp/mmio.h>
#include <zircon/types.h>

#include <hwreg/bitfields.h>

namespace aml_usb_phy {

class PHY2_R0 : public hwreg::RegisterBase<PHY2_R0, uint32_t> {
 public:
  DEF_BIT(0, PLL_Power_Down);
  DEF_BIT(1, HS_TX_Driver_Power_Down);
  DEF_BIT(2, FS_LS_Driver_Power_Down);
  DEF_BIT(3, HS_RX_Power_Down);
  DEF_BIT(4, FS_LS_RX_Power_Down);
  DEF_BIT(5, HS_Squelch_Power_Down);
  DEF_BIT(6, HS_Disconnect_Power_Down);
  DEF_BIT(7, Calibration_Power_Down);
  DEF_BIT(8, BGR_Power_Down);
  DEF_BIT(9, BIAS_Power_Down);
  DEF_BIT(12, reset_FS_LS_CDR);
  DEF_BIT(13, reset_HS_CDR);
  DEF_BIT(14, reset_FS_LS_Clock_Divider);
  DEF_FIELD(20, 15, refclk_multiplier);
  DEF_FIELD(26, 21, pll_clock_divide);
  DEF_BIT(27, PLL_Lock_over_ride);
  DEF_BIT(28, PLL_Bypass_Enable);
  DEF_FIELD(31, 29, PLL_Fine_Tuning);

  static auto Get() { return hwreg::RegisterAddr<PHY2_R0>(0x0); }
};

class PHY2_R1 : public hwreg::RegisterBase<PHY2_R1, uint32_t> {
 public:
  DEF_FIELD(1, 0, slew_control);
  DEF_FIELD(3, 2, bypass_en);
  DEF_BIT(8, spare);
  DEF_BIT(9, hs_en_mode);

  static auto Get() { return hwreg::RegisterAddr<PHY2_R1>(0x4); }
};

class PHY2_R2 : public hwreg::RegisterBase<PHY2_R2, uint32_t> {
 public:
  DEF_FIELD(23, 0, Calibration_code_Value);
  DEF_BIT(24, PRBS_Sync_Out);
  DEF_BIT(25, HS_Squelch_Status);
  DEF_BIT(26, HS_Disconnect_Status);
  DEF_BIT(30, cal_en_flag);
  DEF_BIT(31, Phy_status);

  static auto Get() { return hwreg::RegisterAddr<PHY2_R2>(0x8); }
};

class PHY2_R3 : public hwreg::RegisterBase<PHY2_R3, uint32_t> {
 public:
  DEF_FIELD(1, 0, squelch_ref);
  DEF_FIELD(3, 2, hsdic_ref);
  DEF_FIELD(7, 4, disc_ref);

  static auto Get() { return hwreg::RegisterAddr<PHY2_R3>(0xC); }
};

class PHY2_R4 : public hwreg::RegisterBase<PHY2_R4, uint32_t> {
 public:
  DEF_FIELD(23, 0, Calibration_code_Value);
  DEF_BIT(24, i_c2l_cal_en);
  DEF_BIT(25, i_c2l_reset_n);
  DEF_BIT(26, i_c2l_cal_done);
  DEF_BIT(27, TEST_Bypass_mode_enable);
  DEF_FIELD(31, 28, i_c2l_bias_trim);

  static auto Get() { return hwreg::RegisterAddr<PHY2_R4>(0x10); }
};

class PHY2_R5 : public hwreg::RegisterBase<PHY2_R5, uint32_t> {
 public:
  DEF_FIELD(21, 0, i_c2l_pll_perfcfg);
  DEF_FIELD(31, 24, i_c2l_obs);

  static auto Get() { return hwreg::RegisterAddr<PHY2_R5>(0x14); }
};

class PHY2_R6 : public hwreg::RegisterBase<PHY2_R6, uint32_t> {
 public:
  DEF_FIELD(7, 0, PCS_microsecond_timer_done_count_value_7_0);
  DEF_FIELD(11, 8, bypass_disc_cntr);
  DEF_FIELD(19, 16, fsls_farend_device_disconnect_micro_second_count_11_8);
  DEF_BIT(20, PCS_Reset_Receive_State_machine);
  DEF_BIT(21, PCS_Reset_Transmit_State_machine);
  DEF_BIT(23, Internal_loopback);
  DEF_FIELD(30, 24, cntr_timeout);
  DEF_BIT(31, hub_extra_bit_cntr);

  static auto Get() { return hwreg::RegisterAddr<PHY2_R6>(0x18); }
};

class PHY2_R7 : public hwreg::RegisterBase<PHY2_R7, uint32_t> {
 public:
  DEF_FIELD(3, 0, HS_CDR_internal_tap_select);
  DEF_FIELD(11, 4, cntr_done_value);
  DEF_FIELD(14, 12, fs_ls_minimum_count);
  DEF_BIT(15, host_tristate);
  DEF_BIT(16, acceptable_bit_drops);
  DEF_FIELD(20, 17, RX_ERROR_Turn_Around_Time_Count);
  DEF_FIELD(31, 24, Prbs_Error_count);

  static auto Get() { return hwreg::RegisterAddr<PHY2_R7>(0x1C); }
};

class PHY2_R8 : public hwreg::RegisterBase<PHY2_R8, uint32_t> {
 public:
  DEF_FIELD(2, 0, pattern);
  DEF_BIT(3, PRBS_Enable);
  DEF_BIT(4, PRBS_comparison_enable);
  DEF_BIT(5, PRBS_ERROR_Insert);
  DEF_BIT(6, reset_us_timer);
  DEF_BIT(7, Enable_RX_ERROR_Timeout_Mode);
  DEF_FIELD(15, 8, Custom_Pattern_0);
  DEF_FIELD(23, 16, Custom_Pattern_1);
  DEF_FIELD(31, 24, Custom_Pattern_2);

  static auto Get() { return hwreg::RegisterAddr<PHY2_R8>(0x20); }
};

class PHY2_R9 : public hwreg::RegisterBase<PHY2_R9, uint32_t> {
 public:
  DEF_FIELD(7, 0, Custom_Pattern_3);
  DEF_FIELD(15, 8, Custom_Pattern_4);
  DEF_FIELD(23, 16, Custom_Pattern_5);
  DEF_FIELD(31, 24, Custom_Pattern_6);

  static auto Get() { return hwreg::RegisterAddr<PHY2_R9>(0x24); }
};

class PHY2_R10 : public hwreg::RegisterBase<PHY2_R10, uint32_t> {
 public:
  DEF_FIELD(7, 0, Custom_Pattern_7);
  DEF_FIELD(15, 8, Custom_Pattern_8);
  DEF_FIELD(23, 16, Custom_Pattern_9);
  DEF_FIELD(31, 24, Custom_Pattern_10);

  static auto Get() { return hwreg::RegisterAddr<PHY2_R10>(0x28); }
};

class PHY2_R11 : public hwreg::RegisterBase<PHY2_R11, uint32_t> {
 public:
  DEF_FIELD(7, 0, Custom_Pattern_11);
  DEF_FIELD(15, 8, Custom_Pattern_12);
  DEF_FIELD(23, 16, Custom_Pattern_13);
  DEF_FIELD(31, 24, Custom_Pattern_14);

  static auto Get() { return hwreg::RegisterAddr<PHY2_R11>(0x2C); }
};

class PHY2_R12 : public hwreg::RegisterBase<PHY2_R12, uint32_t> {
 public:
  DEF_FIELD(7, 0, Custom_Pattern_15);
  DEF_FIELD(15, 8, Custom_Pattern_16);
  DEF_FIELD(23, 16, Custom_Pattern_17);
  DEF_FIELD(31, 24, Custom_Pattern_18);

  static auto Get() { return hwreg::RegisterAddr<PHY2_R12>(0x30); }
};

class PHY2_R13 : public hwreg::RegisterBase<PHY2_R13, uint32_t> {
 public:
  DEF_FIELD(7, 0, Custom_Pattern_19);
  DEF_BIT(14, load_stat);
  DEF_BIT(15, Update_PMA_signals);
  DEF_FIELD(20, 16, minimum_count_for_sync_detection);
  DEF_BIT(21, Clear_Hold_HS_disconnect);
  DEF_BIT(22, Bypass_Host_Disconnect_Value);
  DEF_BIT(23, Bypass_Host_Disconnect_Enable);
  DEF_BIT(24, i_c2l_hs_en);       // bypass_reg[0]
  DEF_BIT(25, i_c2l_fs_en);       // bypass_reg[1]
  DEF_BIT(26, i_c2l_ls_en);       // bypass_reg[2]
  DEF_BIT(27, i_c2l_hs_oe);       // bypass_reg[3]
  DEF_BIT(28, i_c2l_fs_oe);       // bypass_reg[4]
  DEF_BIT(29, i_c2l_hs_rx_en);    // bypass_reg[5]
  DEF_BIT(30, i_c2l_fsls_rx_en);  // bypass_reg[6]

  static auto Get() { return hwreg::RegisterAddr<PHY2_R13>(0x34); }
};

class PHY2_R14 : public hwreg::RegisterBase<PHY2_R14, uint32_t> {
 public:
  DEF_BIT(0, i_rpd_en);
  DEF_BIT(1, i_rpd_en);           // bypass_reg[8]
  DEF_FIELD(3, 2, i_rpu_sw2_en);  // bypass_reg[9]
  DEF_BIT(4, i_rpu_sw1_en);       // bypass_reg[11:10]
  DEF_BIT(5, pg_rstn);
  DEF_BIT(6, i_c2l_data_16_8);
  DEF_BIT(7, i_c2l_assert_single_enable_zero);
  // 23:16 -- Bypass_ctrl_7_0
  DEF_BIT(16, hs);           // bypass_reg[0]
  DEF_BIT(17, fs);           // bypass_reg[1]
  DEF_BIT(18, ls);           // bypass_reg[2]
  DEF_BIT(19, hs_out_en);    // bypass_reg[3]
  DEF_BIT(20, fsls_out_en);  // bypass_reg[4]
  DEF_BIT(21, hs_rx_en);     // bypass_reg[5]
  DEF_BIT(22, hls_rx_en);    // bypass_reg[6]
  // 31:24 -- Bypass_ctrl_15_8
  DEF_BIT(24, i_rpd_en);  // bypass_reg[8]
  // DEF_BIT(25, i_rpu_sw2_en);  // bypass_reg[9]
  // DEF_BIT(26, i_rpu_sw1_en);  // bypass_reg[10]

  static auto Get() { return hwreg::RegisterAddr<PHY2_R14>(0x38); }
};

class PHY2_R15 : public hwreg::RegisterBase<PHY2_R15, uint32_t> {
 public:
  DEF_FIELD(7, 0, se0_cntr);
  DEF_FIELD(15, 8, non_se0_cntr);
  DEF_FIELD(28, 16, ms_4_cntr);

  static auto Get() { return hwreg::RegisterAddr<PHY2_R15>(0x3C); }
};

class PHY2_R16 : public hwreg::RegisterBase<PHY2_R16, uint32_t> {
 public:
  DEF_FIELD(8, 0, usb2_mppll_m);
  DEF_FIELD(14, 10, usb2_mppll_n);
  DEF_BIT(20, usb2_mppll_tdc_mode);
  DEF_BIT(21, usb2_mppll_sdm_en);
  DEF_BIT(22, usb2_mppll_load);
  DEF_BIT(23, usb2_mppll_dco_sdm_en);
  DEF_FIELD(25, 24, usb2_mppll_lock_long);
  DEF_BIT(26, usb2_mppll_lock_f);
  DEF_BIT(27, usb2_mppll_fast_lock);
  DEF_BIT(28, usb2_mppll_en);
  DEF_BIT(29, usb2_mppll_reset);
  DEF_BIT(30, usb2_mppll_lock);
  DEF_BIT(31, usb2_mppll_lock_dig);

  static auto Get() { return hwreg::RegisterAddr<PHY2_R16>(0x40); }
};

class PHY2_R17 : public hwreg::RegisterBase<PHY2_R17, uint32_t> {
 public:
  DEF_FIELD(13, 0, usb2_mppll_frac_in);
  DEF_BIT(16, usb2_mppll_fix_en);
  DEF_FIELD(19, 17, usb2_mppll_lambda1);
  DEF_FIELD(22, 20, usb2_mppll_lambda0);
  DEF_BIT(23, usb2_mppll_filter_mode);
  DEF_FIELD(27, 24, usb2_mppll_filter_pvt2);
  DEF_FIELD(31, 28, usb2_mppll_filter_pvt1);

  static auto Get() { return hwreg::RegisterAddr<PHY2_R17>(0x44); }
};

class PHY2_R18 : public hwreg::RegisterBase<PHY2_R18, uint32_t> {
 public:
  DEF_FIELD(1, 0, usb2_mppll_lkw_sel);
  DEF_FIELD(5, 2, usb2_mppll_lk_w);
  DEF_FIELD(11, 6, usb2_mppll_lk_s);
  DEF_BIT(12, usb2_mppll_dco_m_en);
  DEF_BIT(13, usb2_mppll_dco_clk_sel);
  DEF_FIELD(15, 14, usb2_mppll_pfd_gain);
  DEF_FIELD(18, 16, usb2_mppll_rou);
  DEF_FIELD(21, 19, usb2_mppll_data_sel);
  DEF_FIELD(23, 22, usb2_mppll_bias_adj);
  DEF_FIELD(25, 24, usb2_mppll_bb_mode);
  DEF_FIELD(28, 26, usb2_mppll_alpha);
  DEF_FIELD(30, 29, usb2_mppll_adj_ldo);
  DEF_BIT(31, usb2_mppll_acg_range);

  static auto Get() { return hwreg::RegisterAddr<PHY2_R18>(0x48); }
};

class PHY2_R19 : public hwreg::RegisterBase<PHY2_R19, uint32_t> {
 public:
  DEF_FIELD(9, 0, usb2_mppll_reg_out);
  DEF_BIT(30, usb2_mppll_lock);
  DEF_BIT(31, usb2_mppll_lock_dig);

  static auto Get() { return hwreg::RegisterAddr<PHY2_R19>(0x4C); }
};

class PHY2_R20 : public hwreg::RegisterBase<PHY2_R20, uint32_t> {
 public:
  enum SquelchSel : uint8_t {
    kDebounce1 = 0b01,
    kDebounce2 = 0b10,

    // b00 and b11 are no debounce. Picking b00.
    kNoDeboune = 0b00,
  };

  DEF_BIT(0, usb2_otg_iddet_en);
  DEF_FIELD(3, 1, usb2_otg_vbus_trim);
  DEF_BIT(4, usb2_otg_vbusdet_en);
  DEF_BIT(5, usb2_amon_en);
  DEF_BIT(6, usb2_cal_code_r5);
  DEF_BIT(7, bypass_otg_det);
  DEF_BIT(8, usb2_dmon_en);
  DEF_FIELD(12, 9, usb2_dmon_sel);
  DEF_BIT(13, usb2_edgedrv_en);
  DEF_FIELD(15, 14, usb2_edgedrv_trim);
  DEF_FIELD(20, 16, usb2_bgr_adj);
  DEF_BIT(21, usb2_bgr_start);
  DEF_ENUM_FIELD(SquelchSel, 23, 22, squelch_sel);
  DEF_FIELD(28, 24, usb2_bgr_vref);
  DEF_FIELD(30, 29, usb2_bgr_dbg);
  DEF_BIT(31, bypass_cal_done_r5);

  static auto Get() { return hwreg::RegisterAddr<PHY2_R20>(0x50); }
};

class PHY2_R21 : public hwreg::RegisterBase<PHY2_R21, uint32_t> {
 public:
  DEF_BIT(0, usb2_bgr_force);
  DEF_BIT(1, usb2_cal_ack_en);
  DEF_BIT(2, usb2_otg_aca_en);
  DEF_BIT(3, usb2_tx_strg_pd);
  DEF_FIELD(5, 4, usb2_otg_aca_trim);
  DEF_BIT(7, hs_cdr_sel);
  DEF_FIELD(15, 8, hs_cdr_ctrl);
  // 19:16 -- bypass_utmi_cntr_19_16
  DEF_BIT(16, bypass_xcvr_select);  // bypass_utmi_cntr[16]
  DEF_BIT(17, bypass_term_select);  // bypass_utmi_cntr[17]
  DEF_BIT(18, bypass_suspend);      // bypass_utmi_cntr[18]
  DEF_BIT(19, bypass_opmode);       // bypass_utmi_cntr[19]
  // 25:20 -- bypass_utmi_reg_20_25
  DEF_FIELD(21, 20, xcvr_select_ctrl_reg);  // bypass_utmi_reg[21:20]
  DEF_BIT(22, term_select_ctrl_reg);        // bypass_utmi_reg[22]
  DEF_BIT(23, suspend_ctrl_reg);            // bypass_utmi_reg[23]
  DEF_FIELD(25, 24, opmode_ctrl_reg);       // bypass_utmi_reg[25:24]

  static auto Get() { return hwreg::RegisterAddr<PHY2_R21>(0x54); }
};

class PHY2_R22 : public hwreg::RegisterBase<PHY2_R22, uint32_t> {
 public:
  DEF_BIT(0, usb2_otg_id_dig);
  DEF_BIT(1, usb2_otg_sess_vld);
  DEF_BIT(2, usb2_otg_vbus_vld);
  DEF_FIELD(5, 3, usb2_otg_aca_iddig);
  DEF_FIELD(13, 6, hs_cdr_state);

  static auto Get() { return hwreg::RegisterAddr<PHY2_R22>(0x58); }
};

class PHY2_R23 : public hwreg::RegisterBase<PHY2_R23, uint32_t> {
 public:
  DEF_BIT(0, orw_usb2_bgr_en);
  DEF_FIELD(6, 1, orw_test_bus_sel);
  DEF_BIT(7, orw_test_bus_en);
  DEF_BIT(8, pcs_sel);
  DEF_BIT(9, sel_cdr);
  DEF_FIELD(11, 10, new_hs_disc_ctrl);
  DEF_BIT(12, ldo_en);
  DEF_FIELD(15, 13, ldo_trim);
  DEF_FIELD(31, 16, test_bus_data_int);

  static auto Get() { return hwreg::RegisterAddr<PHY2_R23>(0x5C); }
};

}  // namespace aml_usb_phy

#endif  // SRC_DEVICES_USB_DRIVERS_AML_USB_PHY_USB_PHY2_REGS_H_

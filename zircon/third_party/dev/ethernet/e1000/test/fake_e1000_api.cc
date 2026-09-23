// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include <zircon/compiler.h>

#include "zircon/third_party/dev/ethernet/e1000/e1000_api.h"

__BEGIN_CDECLS

u32 e1000_translate_register_82542(u32 reg) { return 0; }

s32 e1000_set_mac_type(struct e1000_hw *hw) {
  // Hardcode to match the type the tests expect
  hw->mac.type = e1000_pch_spt;
  return 0;
}

s32 e1000_read_phy_reg(struct e1000_hw *hw, u32 offset, u16 *data) { return 0; }

s32 e1000_write_phy_reg(struct e1000_hw *hw, u32 offset, u16 data) { return 0; }

bool e1000_enable_mng_pass_thru(struct e1000_hw *hw) { return false; }

s32 e1000_setup_init_funcs(struct e1000_hw *hw, bool init_device) { return 0; }

s32 e1000_get_bus_info(struct e1000_hw *hw) { return 0; }

u16 e1000_rxpbs_adjust_82580(u32 data) { return E1000_PBA_8K; }

s32 e1000_reset_hw(struct e1000_hw *hw) { return 0; }

s32 e1000_init_hw(struct e1000_hw *hw) { return 0; }

s32 e1000_get_phy_info(struct e1000_hw *hw) { return 0; }

s32 e1000_check_for_link(struct e1000_hw *hw) {
  // If the link status is link up then the flag should be false, if the link status is link down
  // the flag should remain where it was. This generally seems to match the actual
  // e1000_check_for_link behavior.
  if (E1000_READ_REG(hw, E1000_STATUS) & E1000_STATUS_LU) {
    hw->mac.get_link_status = false;
  }
  return 0;
}

s32 e1000_check_reset_block(struct e1000_hw *hw) { return 0; }

s32 e1000_validate_nvm_checksum(struct e1000_hw *hw) { return 0; }

s32 e1000_read_mac_addr(struct e1000_hw *hw) { return 0; }

void e1000_clear_hw_cntrs_base_generic(struct e1000_hw *hw) {}

void e1000_power_up_phy(struct e1000_hw *hw) {}

s32 e1000_disable_ulp_lpt_lp(struct e1000_hw *hw, bool force) { return 0; }

s32 e1000_cfg_on_link_up(struct e1000_hw *hw) { return 0; }

s32 e1000_get_speed_and_duplex(struct e1000_hw *hw, u16 *speed, u16 *duplex) {
  // Pretend we're at 1000 Mbps full duplex
  *speed = 1000;
  *duplex = FULL_DUPLEX;
  return 0;
}

bool e1000_get_laa_state_82571(struct e1000_hw *hw) { return false; }

int e1000_rar_set(struct e1000_hw *hw, u8 *addr, u32 index) { return 0; }

void e1000_update_mc_addr_list(struct e1000_hw *hw, u8 *mc_addr_list, u32 mc_addr_count) {}

s32 e1000_lv_jumbo_workaround_ich8lan(struct e1000_hw *hw, bool enable) { return 0; }

__END_CDECLS

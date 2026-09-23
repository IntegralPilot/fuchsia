/*-
 * SPDX-License-Identifier: BSD-2-Clause
 *
 * Copyright (c) 2016 Nicole Graziano <nicole@nextbsd.org>
 * All rights reserved.
 *
 * Redistribution and use in source and binary forms, with or without
 * modification, are permitted provided that the following conditions
 * are met:
 * 1. Redistributions of source code must retain the above copyright
 *    notice, this list of conditions and the following disclaimer.
 * 2. Redistributions in binary form must reproduce the above copyright
 *    notice, this list of conditions and the following disclaimer in the
 *    documentation and/or other materials provided with the distribution.
 *
 * THIS SOFTWARE IS PROVIDED BY THE AUTHOR AND CONTRIBUTORS ``AS IS'' AND
 * ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
 * IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
 * ARE DISCLAIMED.  IN NO EVENT SHALL THE AUTHOR OR CONTRIBUTORS BE LIABLE
 * FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
 * DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS
 * OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION)
 * HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT
 * LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY
 * OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF
 * SUCH DAMAGE.
 */

#include "support.h"

#include <lib/ddk/hw/arch_ops.h>
#include <net/ethernet.h>

#include <fbl/algorithm.h>

#include "adapter.h"
#include "e1000_api.h"

#define PCICFG_DESC_RING_STATUS 0xe4
#define FLUSH_DESC_REQUIRED 0x100
#define EM_FC_PAUSE_TIME 0x0680

#define PCIEM_LINK_CTL_ASPMC 0x0003
#define PCIER_LINK_CTL 0x10
#define PCIER_LINK_CAP 0xc
#define PCIEM_LINK_CAP_ASPM 0x00000c00

namespace e1000 {

/*********************************************************************
 * The 3 following flush routines are used as a workaround in the
 * I219 client parts and only for them.
 *
 * em_flush_tx_ring - remove all descriptors from the tx_ring
 *
 * We want to clear all pending descriptors from the TX ring.
 * zeroing happens when the HW reads the regs. We assign the ring itself as
 * the data of the next descriptor. We don't care about the data we are about
 * to reset the HW.
 **********************************************************************/
static void em_flush_tx_ring(struct adapter* adapter, TxRing<kTxDepth>& tx_ring) {
  struct e1000_hw* hw = &adapter->hw;
  u32 tctl, txd_lower = E1000_TXD_CMD_IFCS;
  u16 size = 512;

  tctl = E1000_READ_REG(hw, E1000_TCTL);
  E1000_WRITE_REG(hw, E1000_TCTL, tctl | E1000_TCTL_EN);

  e1000_tx_desc& txd = tx_ring.HeadDesc();

  /* Just use the ring as a dummy buffer addr */
  txd.buffer_addr = adapter->txd_phys;
  txd.lower.data = htole32(txd_lower | size);
  txd.upper.data = 0;

  /* flush descriptors to memory before notifying the HW */
  hw_wmb();

  E1000_WRITE_REG(hw, E1000_TDT(0), tx_ring.HeadIndex());
  hw_mb();
  usec_delay(250);
}

/*********************************************************************
 * em_flush_rx_ring - remove all descriptors from the rx_ring
 *
 * Mark all descriptors in the RX ring as consumed and disable the rx ring
 **********************************************************************/
static void em_flush_rx_ring(struct adapter* adapter) {
  struct e1000_hw* hw = &adapter->hw;
  u32 rctl, rxdctl;

  rctl = E1000_READ_REG(hw, E1000_RCTL);
  E1000_WRITE_REG(hw, E1000_RCTL, rctl & ~E1000_RCTL_EN);
  E1000_WRITE_FLUSH(hw);
  usec_delay(150);

  rxdctl = E1000_READ_REG(hw, E1000_RXDCTL(0));
  /* zero the lower 14 bits (prefetch and host thresholds) */
  rxdctl &= 0xffffc000;
  /*
   * update thresholds: prefetch threshold to 31, host threshold to 1
   * and make sure the granularity is "descriptors" and not "cache lines"
   */
  rxdctl |= (0x1F | (1 << 8) | E1000_RXDCTL_THRESH_UNIT_DESC);
  E1000_WRITE_REG(hw, E1000_RXDCTL(0), rxdctl);

  /* momentarily enable the RX ring for the changes to take effect */
  E1000_WRITE_REG(hw, E1000_RCTL, rctl | E1000_RCTL_EN);
  E1000_WRITE_FLUSH(hw);
  usec_delay(150);
  E1000_WRITE_REG(hw, E1000_RCTL, rctl & ~E1000_RCTL_EN);
}

/*********************************************************************
 * em_flush_desc_rings - remove all descriptors from the descriptor rings
 *
 * In I219, the descriptor rings must be emptied before resetting the HW
 * or before changing the device state to D3 during runtime (runtime PM).
 *
 * Failure to do this will cause the HW to enter a unit hang state which can
 * only be released by PCI reset on the device
 *
 **********************************************************************/
static void em_flush_desc_rings(struct adapter* adapter, TxRing<kTxDepth>& tx_ring) {
  struct e1000_hw* hw = &adapter->hw;
  u16 hang_state;
  u32 fext_nvm11, tdlen;

  /* First, disable MULR fix in FEXTNVM11 */
  fext_nvm11 = E1000_READ_REG(hw, E1000_FEXTNVM11);
  fext_nvm11 |= E1000_FEXTNVM11_DISABLE_MULR_FIX;
  E1000_WRITE_REG(hw, E1000_FEXTNVM11, fext_nvm11);

  /* do nothing if we're not in faulty state, or if the queue is empty */
  tdlen = E1000_READ_REG(hw, E1000_TDLEN(0));
  e1000_read_pci_cfg(hw, PCICFG_DESC_RING_STATUS, &hang_state);
  if (!(hang_state & FLUSH_DESC_REQUIRED) || !tdlen)
    return;
  em_flush_tx_ring(adapter, tx_ring);

  /* recheck, maybe the fault is caused by the rx ring */
  e1000_read_pci_cfg(hw, PCICFG_DESC_RING_STATUS, &hang_state);
  if (hang_state & FLUSH_DESC_REQUIRED)
    em_flush_rx_ring(adapter);
}

/*
 * em_get_hw_control sets the {CTRL_EXT|FWSM}:DRV_LOAD bit.
 * For ASF and Pass Through versions of f/w this means
 * that the driver is loaded. For AMT version type f/w
 * this means that the network i/f is open.
 */
void em_get_hw_control(struct adapter* adapter) {
  u32 ctrl_ext, swsm;

  if (adapter->hw.mac.type == e1000_82573) {
    swsm = E1000_READ_REG(&adapter->hw, E1000_SWSM);
    E1000_WRITE_REG(&adapter->hw, E1000_SWSM, swsm | E1000_SWSM_DRV_LOAD);
    return;
  }
  /* else */
  ctrl_ext = E1000_READ_REG(&adapter->hw, E1000_CTRL_EXT);
  E1000_WRITE_REG(&adapter->hw, E1000_CTRL_EXT, ctrl_ext | E1000_CTRL_EXT_DRV_LOAD);
}

/*
 * em_release_hw_control resets {CTRL_EXT|FWSM}:DRV_LOAD bit.
 * For ASF and Pass Through versions of f/w this means that
 * the driver is no longer loaded. For AMT versions of the
 * f/w this means that the network i/f is closed.
 */
void em_release_hw_control(struct adapter* adapter) {
  u32 ctrl_ext, swsm;

  if (!adapter->has_manage)
    return;

  if (adapter->hw.mac.type == e1000_82573) {
    swsm = E1000_READ_REG(&adapter->hw, E1000_SWSM);
    E1000_WRITE_REG(&adapter->hw, E1000_SWSM, swsm & ~E1000_SWSM_DRV_LOAD);
    return;
  }
  /* else */
  ctrl_ext = E1000_READ_REG(&adapter->hw, E1000_CTRL_EXT);
  E1000_WRITE_REG(&adapter->hw, E1000_CTRL_EXT, ctrl_ext & ~E1000_CTRL_EXT_DRV_LOAD);
}

/*
 * Disable the L0S and L1 LINK states
 */
static void em_disable_aspm(struct adapter* adapter) {
  u16 link_cap, link_ctrl;

  switch (adapter->hw.mac.type) {
    case e1000_82573:
    case e1000_82574:
    case e1000_82583:
      break;
    default:
      return;
  }
  if (e1000_read_pcie_cap_reg(&adapter->hw, PCIER_LINK_CAP, &link_cap) != ZX_OK) {
    return;
  }
  if ((link_cap & PCIEM_LINK_CAP_ASPM) == 0) {
    return;
  }
  if (e1000_read_pcie_cap_reg(&adapter->hw, PCIER_LINK_CTL, &link_ctrl) != ZX_OK) {
    return;
  }
  link_ctrl &= ~PCIEM_LINK_CTL_ASPMC;
  e1000_write_pcie_cap_reg(&adapter->hw, PCIER_LINK_CTL, &link_ctrl);
}

#define IGB_DMCTLX_DCFLUSH_DIS 0x80000000 /* Disable DMA Coalesce Flush */
#define IGB_TXPBSIZE 20408

/*********************************************************************
 *
 *  Initialize the DMA Coalescing feature
 *
 **********************************************************************/
static void igb_init_dmac(struct adapter* adapter, u32 pba) {
  struct e1000_hw* hw = &adapter->hw;
  u32 dmac, reg = ~E1000_DMACR_DMAC_EN;
  u16 hwm;
  u16 max_frame_size;

  if (hw->mac.type == e1000_i211)
    return;

  max_frame_size = kMaxBufferLength;
  if (hw->mac.type > e1000_82580) {
    if (adapter->dmac == 0) { /* Disabling it */
      E1000_WRITE_REG(hw, E1000_DMACR, reg);
      return;
    }

    /* Set starting threshold */
    E1000_WRITE_REG(hw, E1000_DMCTXTH, 0);

    hwm = static_cast<u16>(64 * pba - max_frame_size / 16);
    if (hwm < 64 * (pba - 6))
      hwm = static_cast<u16>(64 * (pba - 6));
    reg = E1000_READ_REG(hw, E1000_FCRTC);
    reg &= ~E1000_FCRTC_RTH_COAL_MASK;
    reg |= ((hwm << E1000_FCRTC_RTH_COAL_SHIFT) & E1000_FCRTC_RTH_COAL_MASK);
    E1000_WRITE_REG(hw, E1000_FCRTC, reg);

    dmac = pba - max_frame_size / 512;
    if (dmac < pba - 10)
      dmac = pba - 10;
    reg = E1000_READ_REG(hw, E1000_DMACR);
    reg &= ~E1000_DMACR_DMACTHR_MASK;
    reg |= ((dmac << E1000_DMACR_DMACTHR_SHIFT) & E1000_DMACR_DMACTHR_MASK);

    /* transition to L0x or L1 if available..*/
    reg |= (E1000_DMACR_DMAC_EN | E1000_DMACR_DMAC_LX_MASK);

    /* Check if status is 2.5Gb backplane connection
     * before configuration of watchdog timer, which is
     * in msec values in 12.8usec intervals
     * watchdog timer= msec values in 32usec intervals
     * for non 2.5Gb connection
     */
    if (hw->mac.type == e1000_i354) {
      int status = E1000_READ_REG(hw, E1000_STATUS);
      if ((status & E1000_STATUS_2P5_SKU) && (!(status & E1000_STATUS_2P5_SKU_OVER)))
        reg |= ((adapter->dmac * 5) >> 6);
      else
        reg |= (adapter->dmac >> 5);
    } else {
      reg |= (adapter->dmac >> 5);
    }

    E1000_WRITE_REG(hw, E1000_DMACR, reg);

    E1000_WRITE_REG(hw, E1000_DMCRTRH, 0);

    /* Set the interval before transition */
    reg = E1000_READ_REG(hw, E1000_DMCTLX);
    if (hw->mac.type == e1000_i350)
      reg |= IGB_DMCTLX_DCFLUSH_DIS;
    /*
    ** in 2.5Gb connection, TTLX unit is 0.4 usec
    ** which is 0x4*2 = 0xA. But delay is still 4 usec
    */
    if (hw->mac.type == e1000_i354) {
      int status = E1000_READ_REG(hw, E1000_STATUS);
      if ((status & E1000_STATUS_2P5_SKU) && (!(status & E1000_STATUS_2P5_SKU_OVER)))
        reg |= 0xA;
      else
        reg |= 0x4;
    } else {
      reg |= 0x4;
    }

    E1000_WRITE_REG(hw, E1000_DMCTLX, reg);

    /* free space in tx packet buffer to wake from DMA coal */
    E1000_WRITE_REG(hw, E1000_DMCTXTH, (IGB_TXPBSIZE - (2 * max_frame_size)) >> 6);

    /* make low power state decision controlled by DMA coal */
    reg = E1000_READ_REG(hw, E1000_PCIEMISC);
    reg &= ~E1000_PCIEMISC_LX_DECISION;
    E1000_WRITE_REG(hw, E1000_PCIEMISC, reg);

  } else if (hw->mac.type == e1000_82580) {
    u32 reg = E1000_READ_REG(hw, E1000_PCIEMISC);
    E1000_WRITE_REG(hw, E1000_PCIEMISC, reg & ~E1000_PCIEMISC_LX_DECISION);
    E1000_WRITE_REG(hw, E1000_DMACR, 0);
  }
}

/*********************************************************************
 *
 *  Initialize the hardware to a configuration as specified by the
 *  sc structure.
 *
 **********************************************************************/
void em_reset(struct adapter* adapter, TxRing<kTxDepth>& tx_ring) {
  struct e1000_hw* hw = &adapter->hw;
  u32 rx_buffer_size;
  u32 pba;

  /* Let the firmware know the OS is in control */
  em_get_hw_control(adapter);

  /* Set up smart power down as default off on newer adapters. */
  if ((hw->mac.type == e1000_82571 || hw->mac.type == e1000_82572)) {
    u16 phy_tmp = 0;

    /* Speed up time to link by disabling smart power down. */
    e1000_read_phy_reg(hw, IGP02E1000_PHY_POWER_MGMT, &phy_tmp);
    phy_tmp &= ~IGP02E1000_PM_SPD;
    e1000_write_phy_reg(hw, IGP02E1000_PHY_POWER_MGMT, phy_tmp);
  }

  /*
   * Packet Buffer Allocation (PBA)
   * Writing PBA sets the receive portion of the buffer
   * the remainder is used for the transmit buffer.
   */
  switch (hw->mac.type) {
    /* 82547: Total Packet Buffer is 40K */
    case e1000_82547:
    case e1000_82547_rev_2:
      if (hw->mac.max_frame_size > 8192)
        pba = E1000_PBA_22K; /* 22K for Rx, 18K for Tx */
      else
        pba = E1000_PBA_30K; /* 30K for Rx, 10K for Tx */
      break;
    /* 82571/82572/80003es2lan: Total Packet Buffer is 48K */
    case e1000_82571:
    case e1000_82572:
    case e1000_80003es2lan:
      pba = E1000_PBA_32K; /* 32K for Rx, 16K for Tx */
      break;
    /* 82573: Total Packet Buffer is 32K */
    case e1000_82573:
      pba = E1000_PBA_12K; /* 12K for Rx, 20K for Tx */
      break;
    case e1000_82574:
    case e1000_82583:
      pba = E1000_PBA_20K; /* 20K for Rx, 20K for Tx */
      break;
    case e1000_ich8lan:
      pba = E1000_PBA_8K;
      break;
    case e1000_ich9lan:
    case e1000_ich10lan:
      /* Boost Receive side for jumbo frames */
      if (hw->mac.max_frame_size > 4096)
        pba = E1000_PBA_14K;
      else
        pba = E1000_PBA_10K;
      break;
    case e1000_pchlan:
    case e1000_pch2lan:
    case e1000_pch_lpt:
    case e1000_pch_spt:
    case e1000_pch_cnp:
    case e1000_pch_tgp:
    case e1000_pch_adp:
    case e1000_pch_mtp:
    case e1000_pch_ptp:
      pba = E1000_PBA_26K;
      break;
    case e1000_82575:
      pba = E1000_PBA_32K;
      break;
    case e1000_82576:
    case e1000_vfadapt:
      pba = E1000_READ_REG(hw, E1000_RXPBS);
      pba &= E1000_RXPBS_SIZE_MASK_82576;
      break;
    case e1000_82580:
    case e1000_i350:
    case e1000_i354:
    case e1000_vfadapt_i350:
      pba = E1000_READ_REG(hw, E1000_RXPBS);
      pba = e1000_rxpbs_adjust_82580(pba);
      break;
    case e1000_i210:
    case e1000_i211:
      pba = E1000_PBA_34K;
      break;
    default:
      /* Remaining devices assumed to have a Packet Buffer of 64K. */
      if (hw->mac.max_frame_size > 8192)
        pba = E1000_PBA_40K; /* 40K for Rx, 24K for Tx */
      else
        pba = E1000_PBA_48K; /* 48K for Rx, 16K for Tx */
  }

  /* Special needs in case of Jumbo frames */
  if ((hw->mac.type == e1000_82575) && (kEthMtu > 1500)) {
    u32 tx_space, min_tx, min_rx;
    pba = E1000_READ_REG(hw, E1000_PBA);
    tx_space = pba >> 16;
    pba &= 0xffff;
    min_tx = (hw->mac.max_frame_size + sizeof(struct e1000_tx_desc) - ETHERNET_FCS_SIZE) * 2;
    min_tx = fbl::round_up(min_tx, 1024u);
    min_tx >>= 10;
    min_rx = hw->mac.max_frame_size;
    min_rx = fbl::round_up(min_rx, 1024u);
    min_rx >>= 10;
    if (tx_space < min_tx && ((min_tx - tx_space) < pba)) {
      pba = pba - (min_tx - tx_space);
      /*
       * if short on rx space, rx wins
       * and must trump tx adjustment
       */
      if (pba < min_rx)
        pba = min_rx;
    }
    E1000_WRITE_REG(hw, E1000_PBA, pba);
  }

  if (hw->mac.type < igb_mac_min)
    E1000_WRITE_REG(hw, E1000_PBA, pba);

  /*
   * These parameters control the automatic generation (Tx) and
   * response (Rx) to Ethernet PAUSE frames.
   * - High water mark should allow for at least two frames to be
   *   received after sending an XOFF.
   * - Low water mark works best when it is very near the high water mark.
   *   This allows the receiver to restart by sending XON when it has
   *   drained a bit. Here we use an arbitrary value of 1500 which will
   *   restart after one full frame is pulled from the buffer. There
   *   could be several smaller frames in the buffer and if so they will
   *   not trigger the XON until their total number reduces the buffer
   *   by 1500.
   * - The pause time is fairly large at 1000 x 512ns = 512 usec.
   */
  rx_buffer_size = (pba & 0xffff) << 10;
  hw->fc.high_water = rx_buffer_size - fbl::round_up(hw->mac.max_frame_size, 1024u);
  hw->fc.low_water = hw->fc.high_water - 1500;

  hw->fc.requested_mode = e1000_fc_full;

  if (hw->mac.type == e1000_80003es2lan)
    hw->fc.pause_time = 0xFFFF;
  else
    hw->fc.pause_time = EM_FC_PAUSE_TIME;

  hw->fc.send_xon = true;

  /* Device specific overrides/settings */
  switch (hw->mac.type) {
    case e1000_pchlan:
      /* Workaround: no TX flow ctrl for PCH */
      hw->fc.requested_mode = e1000_fc_rx_pause;
      hw->fc.pause_time = 0xFFFF; /* override */
      if (kEthMtu > 1500) {
        hw->fc.high_water = 0x3500;
        hw->fc.low_water = 0x1500;
      } else {
        hw->fc.high_water = 0x5000;
        hw->fc.low_water = 0x3000;
      }
      hw->fc.refresh_time = 0x1000;
      break;
    case e1000_pch2lan:
    case e1000_pch_lpt:
    case e1000_pch_spt:
    case e1000_pch_cnp:
    case e1000_pch_tgp:
    case e1000_pch_adp:
    case e1000_pch_mtp:
    case e1000_pch_ptp:
      hw->fc.high_water = 0x5C20;
      hw->fc.low_water = 0x5048;
      hw->fc.pause_time = 0x0650;
      hw->fc.refresh_time = 0x0400;
      /* Jumbos need adjusted PBA */
      if (kEthMtu > 1500)
        E1000_WRITE_REG(hw, E1000_PBA, 12);
      else
        E1000_WRITE_REG(hw, E1000_PBA, 26);
      break;
    case e1000_82575:
    case e1000_82576:
      /* 8-byte granularity */
      hw->fc.low_water = hw->fc.high_water - 8;
      break;
    case e1000_82580:
    case e1000_i350:
    case e1000_i354:
    case e1000_i210:
    case e1000_i211:
    case e1000_vfadapt:
    case e1000_vfadapt_i350:
      /* 16-byte granularity */
      hw->fc.low_water = hw->fc.high_water - 16;
      break;
    case e1000_ich9lan:
    case e1000_ich10lan:
      if (kEthMtu > 1500) {
        hw->fc.high_water = 0x2800;
        hw->fc.low_water = hw->fc.high_water - 8;
        break;
      }
      __FALLTHROUGH;
    default:
      if (hw->mac.type == e1000_80003es2lan)
        hw->fc.pause_time = 0xFFFF;
      break;
  }

  /* I219 needs some special flushing to avoid hangs */
  if (adapter->hw.mac.type >= e1000_pch_spt && adapter->hw.mac.type < igb_mac_min) {
    em_flush_desc_rings(adapter, tx_ring);
  }

  /* Issue a global reset */
  e1000_reset_hw(hw);
  if (hw->mac.type >= igb_mac_min) {
    E1000_WRITE_REG(hw, E1000_WUC, 0);
  } else {
    E1000_WRITE_REG(hw, E1000_WUFC, 0);
    em_disable_aspm(adapter);
  }
  if (adapter->flags & IGB_MEDIA_RESET) {
    e1000_setup_init_funcs(hw, true);
    e1000_get_bus_info(hw);
    adapter->flags &= ~IGB_MEDIA_RESET;
  }
  /* and a re-init */
  if (e1000_init_hw(hw) < 0) {
    return;
  }
  if (hw->mac.type >= igb_mac_min)
    igb_init_dmac(adapter, pba);

  E1000_WRITE_REG(hw, E1000_VET, ETHERTYPE_VLAN);
  e1000_get_phy_info(hw);
  e1000_check_for_link(hw);
}

}  // namespace e1000

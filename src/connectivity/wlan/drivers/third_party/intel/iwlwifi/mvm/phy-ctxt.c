/******************************************************************************
 *
 * Copyright(c) 2012 - 2014 Intel Corporation. All rights reserved.
 * Copyright(c) 2013 - 2014 Intel Mobile Communications GmbH
 * Copyright(c) 2018           Intel Corporation
 * All rights reserved.
 *
 * Redistribution and use in source and binary forms, with or without
 * modification, are permitted provided that the following conditions
 * are met:
 *
 *  * Redistributions of source code must retain the above copyright
 *    notice, this list of conditions and the following disclaimer.
 *  * Redistributions in binary form must reproduce the above copyright
 *    notice, this list of conditions and the following disclaimer in
 *    the documentation and/or other materials provided with the
 *    distribution.
 *  * Neither the name Intel Corporation nor the names of its
 *    contributors may be used to endorse or promote products derived
 *    from this software without specific prior written permission.
 *
 * THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
 * "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
 * LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
 * A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
 * OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
 * SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
 * LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
 * DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
 * THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
 * (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
 * OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
 *
 *****************************************************************************/

#include "third_party/iwlwifi/mvm/fw-api.h"
#include "third_party/iwlwifi/mvm/mvm.h"
#include "third_party/iwlwifi/platform/banjo/associnfo.h"

// A channel setting used as default value. In some cases, we need an arbitrary value for channel.
// For example, during initializing a PHY context in firmware, we need a (whatever) value to add
// an entry in firmware. This value usually will be changed later.
struct ieee80211_channel default_channel = {
	.band = WLAN_BAND_TWO_GHZ,
	.ch_num = 1,
	.hw_value = 1,
};
struct cfg80211_chan_def default_chandef = {
	.chan = &default_channel,
	.width = NL80211_CHAN_WIDTH_20_NOHT,
};

// Converts channel number to band type.
//
// Args:
//   chan_num: starts from 1.
//
// Returns:
//   the band ID.
wlan_band_t iwl_mvm_get_channel_band(uint8_t chan_num)
{
	return chan_num <= 14 ? WLAN_BAND_TWO_GHZ : WLAN_BAND_FIVE_GHZ;
}

/* Maps the driver specific channel width definition to the fw values */
uint8_t iwl_mvm_get_channel_width(struct cfg80211_chan_def *chandef)
{
	switch (chandef->width) {
	case CHANNEL_BANDWIDTH_CBW20:
		return PHY_VHT_CHANNEL_MODE20;
	case CHANNEL_BANDWIDTH_CBW40:
	case CHANNEL_BANDWIDTH_CBW40BELOW: // fall-thru
		return PHY_VHT_CHANNEL_MODE40;
	case CHANNEL_BANDWIDTH_CBW80:
		return PHY_VHT_CHANNEL_MODE80;
	case CHANNEL_BANDWIDTH_CBW160:
		return PHY_VHT_CHANNEL_MODE160;
	default:
		WARN(1, "Invalid channel width=%u", chandef->width);
		return PHY_VHT_CHANNEL_MODE20;
	}
}

/*
 * Maps the driver specific control channel position (relative to the center
 * freq) definitions to the the fw values.
 *
 * Here is the channel list: https://en.wikipedia.org/wiki/List_of_WLAN_channels
 */
u8 iwl_mvm_get_ctrl_pos(struct cfg80211_chan_def *chandef)
{
	uint8_t primary = chandef->chan->hw_value;
	channel_bandwidth_t cbw =
		convert_nl80211_chan_width_to_channel_bandwidth(chandef->width);
	uint8_t base;

	if (cbw == CHANNEL_BANDWIDTH_CBW20) {
		// 20Mhz always uses the default value.
		return PHY_VHT_CTRL_POS_1_BELOW;
	}

	// TODO(fxbug.dev/29830): move out the center freq calculation into a shared lib.

	if (36 <= primary && primary <= 64) {
		base = 36;
	} else if (100 <= primary && primary <= 128) {
		base = 100;
	} else if (132 <= primary && primary <= 144) {
		if (cbw ==
		    CHANNEL_BANDWIDTH_CBW160) { // This group doesn't support 160MHz primary
			// channels. Use default value.
			return PHY_VHT_CTRL_POS_1_BELOW;
		}
		base = 132;
	} else if (149 <= primary && primary <= 161) {
		if (cbw ==
		    CHANNEL_BANDWIDTH_CBW160) { // This group doesn't support 160MHz primary
			// channels. Use default value.
			return PHY_VHT_CTRL_POS_1_BELOW;
		}
		base = 149;
	} else {
		// 2.4GHz band or invalid channel index. Use default value.
		return PHY_VHT_CTRL_POS_1_BELOW;
	}

	uint8_t offset_to_base = primary - base;
	uint8_t mask; // used to mask the chan_index.
		// # of 1's means the bandwidth.
	bool has_bit2 = offset_to_base & 0x4; // for HT40+/- checking
	switch (cbw) {
	case CHANNEL_BANDWIDTH_CBW40: // The secondary channel is above the primary.
		mask = 0x7; // Keep 3 bits.
		if (has_bit2) { // Channel 40, 48, 56 ... doesn't allow HT40+.
			return PHY_VHT_CTRL_POS_1_BELOW;
		}
		break;
	case CHANNEL_BANDWIDTH_CBW40BELOW: // The secondary channel is below the primary.
		mask = 0x7; // Keep 3 bits.
		if (!has_bit2) { // Channel 36, 44, 52 ... doesn't allow HT40-.
			return PHY_VHT_CTRL_POS_1_BELOW;
		}
		break;
	case CHANNEL_BANDWIDTH_CBW80:
		mask = 0xf; // Keep 4 bits.
		break;
	case CHANNEL_BANDWIDTH_CBW160:
		mask = 0x1f; // Keep 5 bits.
		break;
	/*
     * The FW is expected to check the control channel position only
     * when in HT/VHT and the channel width is not 20MHz. Return
     * this value as the default one.
     */
	default:
		IWL_WARN(chandef,
			 "Invalid channel bandwidth (primary=%u cbw=%u)\n",
			 chandef->chan->hw_value, cbw);
		return PHY_VHT_CTRL_POS_1_BELOW;
	}

	// Now, calculate offset from the primary channel index to the center.
	//
	// Take primary channel 48 @80MHz channel as example:
	//
	//            primary          | freq |           | mask=0xf |  half
	//            channel          |offset| chan num  | (CBW80)  |bandwidth=8 (40MHz)
	//  ==============================================================================
	//
	//   36  -------------------->     0    base           ^          ^
	//        base index                                   |          |
	//   40                           20    base + 4       |          |
	//                                      -------------- | -------- | <----------- center freq/index
	//   44                           40    base + 8       |     ^    v    | 10Mhz
	//                                                     |     |
	//   48  -------------------->    60    base + 12      |     v --- This is offset_to_center.
	//        offset_to_base = 12                          |
	//                                80                   v
	//
	// We can get:
	//
	//   offset_to_base = 48 - 36       # = 12 indexes
	//   mask = 0xf                     # 80MHz
	//   half_bandwidth = 8             # 40MHz
	//   center_index = 8 - 2 = 6       # 30MHz (offset to the base index)
	//   offset_to_center = 12 - 6 = 6  # 60MHz - 30MHz = +30MHz
	//
	uint8_t half_bandwith =
		(mask + 1) /
		2; // # of channel indexes within the half bandwidth.
	uint8_t center_index =
		half_bandwith - 2; // 2 indexes = 2 * 5MHz = 10Mhz.
	int offset_to_center = (offset_to_base & mask) - center_index;

	const int mhz_per_index = 5; // 5 MHz for each channel index.
	switch (offset_to_center * mhz_per_index) {
	case -70:
		return PHY_VHT_CTRL_POS_4_BELOW;
	case -50:
		return PHY_VHT_CTRL_POS_3_BELOW;
	case -30:
		return PHY_VHT_CTRL_POS_2_BELOW;
	case -10:
		return PHY_VHT_CTRL_POS_1_BELOW;
	case 10:
		return PHY_VHT_CTRL_POS_1_ABOVE;
	case 30:
		return PHY_VHT_CTRL_POS_2_ABOVE;
	case 50:
		return PHY_VHT_CTRL_POS_3_ABOVE;
	case 70:
		return PHY_VHT_CTRL_POS_4_ABOVE;
	default:
		IWL_WARN(chandef,
			 "Invalid channel definition (primary=%u cbw=%u)\n",
			 chandef->chan->hw_value, cbw);
		return PHY_VHT_CTRL_POS_1_BELOW;
	}
}

/*
 * Construct the generic fields of the PHY context command
 */
static void iwl_mvm_phy_ctxt_cmd_hdr(struct iwl_mvm_phy_ctxt *ctxt,
				     struct iwl_phy_context_cmd *cmd,
				     u32 action)
{
	cmd->id_and_color =
		cpu_to_le32(FW_CMD_ID_AND_COLOR(ctxt->id, ctxt->color));
	cmd->action = cpu_to_le32(action);
}

static void iwl_mvm_phy_ctxt_set_rxchain(struct iwl_mvm *mvm,
					 struct iwl_mvm_phy_ctxt *ctxt,
					 __le32 *rxchain_info, u8 chains_static,
					 u8 chains_dynamic)
{
	u8 active_cnt, idle_cnt;

	/* Set rx the chains */
	idle_cnt = chains_static;
	active_cnt = chains_dynamic;

	/* In scenarios where we only ever use a single-stream rates,
	 * i.e. legacy 11b/g/a associations, single-stream APs or even
	 * static SMPS, enable both chains to get diversity, improving
	 * the case where we're far enough from the AP that attenuation
	 * between the two antennas is sufficiently different to impact
	 * performance.
	 */
	if (active_cnt == 1 && iwl_mvm_rx_diversity_allowed(mvm, ctxt)) {
		idle_cnt = 2;
		active_cnt = 2;
	}

	/*
	 * If the firmware requested it, then we know that it supports
	 * getting zero for the values to indicate "use one, but pick
	 * which one yourself", which means it can dynamically pick one
	 * that e.g. has better RSSI.
	 */
	if (mvm->fw_static_smps_request && active_cnt == 1 && idle_cnt == 1) {
		idle_cnt = 0;
		active_cnt = 0;
	}

	*rxchain_info = cpu_to_le32(iwl_mvm_get_valid_rx_ant(mvm)
				    << PHY_RX_CHAIN_VALID_POS);
	*rxchain_info |= cpu_to_le32(idle_cnt << PHY_RX_CHAIN_CNT_POS);
	*rxchain_info |= cpu_to_le32(active_cnt << PHY_RX_CHAIN_MIMO_CNT_POS);
#ifdef CONFIG_IWLWIFI_DEBUGFS
	if (unlikely(mvm->dbgfs_rx_phyinfo))
		*rxchain_info = cpu_to_le32(mvm->dbgfs_rx_phyinfo);
#endif
}

/*
 * Construct the generic fields of the PHY context command
 */
static void iwl_mvm_phy_ctxt_cmd_data_v1(struct iwl_mvm *mvm,
					 struct iwl_mvm_phy_ctxt *ctxt,
					 struct iwl_phy_context_cmd_v1 *cmd,
					 struct cfg80211_chan_def *chandef,
					 u8 chains_static, u8 chains_dynamic)
{
	struct iwl_phy_context_cmd_tail *tail =
		iwl_mvm_chan_info_cmd_tail(mvm, &cmd->ci);

	/* Set the channel info data */
	iwl_mvm_set_chan_info_chandef(mvm, &cmd->ci, chandef);

// For 'rxchain_info' and 'rx_chain_info', to make compiler happy.
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Waddress-of-packed-member"

	iwl_mvm_phy_ctxt_set_rxchain(mvm, ctxt, &tail->rxchain_info,
				     chains_static, chains_dynamic);

	tail->txchain_info = cpu_to_le32(iwl_mvm_get_valid_tx_ant(mvm));
}

/*
 * Add the phy configuration to the PHY context command
 */
static void iwl_mvm_phy_ctxt_cmd_data(struct iwl_mvm *mvm,
				      struct iwl_mvm_phy_ctxt *ctxt,
				      struct iwl_phy_context_cmd *cmd,
				      struct cfg80211_chan_def *chandef,
				      u8 chains_static, u8 chains_dynamic)
{
	cmd->lmac_id =
		cpu_to_le32(iwl_mvm_get_lmac_id(mvm->fw, chandef->chan->band));

	/* Set the channel info data */
	iwl_mvm_set_chan_info_chandef(mvm, &cmd->ci, chandef);

	/* we only support RLC command version 2 */
	if (iwl_fw_lookup_cmd_ver(
		    mvm->fw, WIDE_ID(DATA_PATH_GROUP, RLC_CONFIG_CMD), 0) < 2)
		iwl_mvm_phy_ctxt_set_rxchain(mvm, ctxt, &cmd->rxchain_info,
					     chains_static, chains_dynamic);
}

static int iwl_mvm_phy_send_rlc(struct iwl_mvm *mvm,
				struct iwl_mvm_phy_ctxt *ctxt, u8 chains_static,
				u8 chains_dynamic)
{
	struct iwl_rlc_config_cmd cmd = {
		.phy_id = cpu_to_le32(ctxt->id),
	};

	if (iwl_fw_lookup_cmd_ver(
		    mvm->fw, WIDE_ID(DATA_PATH_GROUP, RLC_CONFIG_CMD), 0) < 2)
		return 0;

	BUILD_BUG_ON(IWL_RLC_CHAIN_INFO_DRIVER_FORCE !=
		     PHY_RX_CHAIN_DRIVER_FORCE_MSK);
	BUILD_BUG_ON(IWL_RLC_CHAIN_INFO_VALID != PHY_RX_CHAIN_VALID_MSK);
	BUILD_BUG_ON(IWL_RLC_CHAIN_INFO_FORCE != PHY_RX_CHAIN_FORCE_SEL_MSK);
	BUILD_BUG_ON(IWL_RLC_CHAIN_INFO_FORCE_MIMO !=
		     PHY_RX_CHAIN_FORCE_MIMO_SEL_MSK);
	BUILD_BUG_ON(IWL_RLC_CHAIN_INFO_COUNT != PHY_RX_CHAIN_CNT_MSK);
	BUILD_BUG_ON(IWL_RLC_CHAIN_INFO_MIMO_COUNT !=
		     PHY_RX_CHAIN_MIMO_CNT_MSK);

	iwl_mvm_phy_ctxt_set_rxchain(mvm, ctxt, &cmd.rlc.rx_chain_info,
				     chains_static, chains_dynamic);

	return iwl_mvm_send_cmd_pdu(
		mvm, iwl_cmd_id(RLC_CONFIG_CMD, DATA_PATH_GROUP, 2), 0,
		sizeof(cmd), &cmd);
}

#pragma GCC diagnostic pop

/*
 * Send a command to apply the current phy configuration. The command is send
 * only if something in the configuration changed: in case that this is the
 * first time that the phy configuration is applied or in case that the phy
 * configuration changed from the previous apply.
 */
static zx_status_t iwl_mvm_phy_ctxt_apply(struct iwl_mvm *mvm,
					  struct iwl_mvm_phy_ctxt *ctxt,
					  struct cfg80211_chan_def *chandef,
					  u8 chains_static, u8 chains_dynamic,
					  u32 action)
{
	int ret;
	int ver = iwl_fw_lookup_cmd_ver(mvm->fw, PHY_CONTEXT_CMD, 1);

	if (ver == 3 || ver == 4) {
		struct iwl_phy_context_cmd cmd = {};

		/* Set the command header fields */
		iwl_mvm_phy_ctxt_cmd_hdr(ctxt, &cmd, action);

		/* Set the command data */
		iwl_mvm_phy_ctxt_cmd_data(mvm, ctxt, &cmd, chandef,
					  chains_static, chains_dynamic);

		ret = iwl_mvm_send_cmd_pdu(mvm, PHY_CONTEXT_CMD, 0, sizeof(cmd),
					   &cmd);
	} else if (ver < 3) {
		struct iwl_phy_context_cmd_v1 cmd = {};
		u16 len = sizeof(cmd) - iwl_mvm_chan_info_padding(mvm);

		/* Set the command header fields */
		iwl_mvm_phy_ctxt_cmd_hdr(
			ctxt, (struct iwl_phy_context_cmd *)&cmd, action);

		/* Set the command data */
		iwl_mvm_phy_ctxt_cmd_data_v1(mvm, ctxt, &cmd, chandef,
					     chains_static, chains_dynamic);
		ret = iwl_mvm_send_cmd_pdu(mvm, PHY_CONTEXT_CMD, 0, len, &cmd);
	} else {
		IWL_ERR(mvm, "PHY ctxt cmd error ver %d not supported\n", ver);
		return ZX_ERR_NOT_SUPPORTED;
	}

	if (ret) {
		IWL_ERR(mvm, "PHY ctxt cmd error. ret=%d\n", ret);
		return ret;
	}

	if (action != FW_CTXT_ACTION_REMOVE)
		return iwl_mvm_phy_send_rlc(mvm, ctxt, chains_static,
					    chains_dynamic);

	return ZX_OK;
}

/*
 * Send a command to add a PHY context based on the current HW configuration.
 */
zx_status_t iwl_mvm_phy_ctxt_add(struct iwl_mvm *mvm,
				 struct iwl_mvm_phy_ctxt *ctxt,
				 struct cfg80211_chan_def *chandef,
				 uint8_t chains_static, uint8_t chains_dynamic)
{
	WARN_ON(!test_bit(IWL_MVM_STATUS_IN_HW_RESTART, &mvm->status) &&
		ctxt->ref);
	iwl_assert_lock_held(&mvm->mutex);

	*ctxt->channel = *chandef->chan; // Copy the channel info to avoid
		// the lifecycle issue.
	ctxt->width = chandef->width;
	ctxt->center_freq1 = chandef->chan->center_freq;

	return iwl_mvm_phy_ctxt_apply(mvm, ctxt, chandef, chains_static,
				      chains_dynamic, FW_CTXT_ACTION_ADD);
}

/*
 * Update the number of references to the given PHY context. This is valid only
 * in case the PHY context was already created, i.e., its reference count > 0.
 */
void iwl_mvm_phy_ctxt_ref(struct iwl_mvm *mvm, struct iwl_mvm_phy_ctxt *ctxt)
{
	iwl_assert_lock_held(&mvm->mutex);
	ctxt->ref++;
}

/*
 * Send a command to modify the PHY context based on the current HW
 * configuration. Note that the function does not check that the configuration
 * changed.
 */
zx_status_t iwl_mvm_phy_ctxt_changed(struct iwl_mvm *mvm,
				     struct iwl_mvm_phy_ctxt *ctxt,
				     struct cfg80211_chan_def *chandef,
				     uint8_t chains_static,
				     uint8_t chains_dynamic)
{
	enum iwl_ctxt_action action = FW_CTXT_ACTION_MODIFY;

	iwl_assert_lock_held(&mvm->mutex);

	if (fw_has_capa(&mvm->fw->ucode_capa,
			IWL_UCODE_TLV_CAPA_BINDING_CDB_SUPPORT) &&
	    iwl_mvm_get_channel_band(chandef->chan->hw_value) !=
		    iwl_mvm_get_channel_band(ctxt->channel->hw_value)) {
		zx_status_t ret;

		/* ... remove it here ...*/
		ret = iwl_mvm_phy_ctxt_apply(mvm, ctxt, chandef, chains_static,
					     chains_dynamic,
					     FW_CTXT_ACTION_REMOVE);
		if (ret != ZX_OK) {
			return ret;
		}

		/* ... and proceed to add it again */
		action = FW_CTXT_ACTION_ADD;
	}

	*ctxt->channel = *chandef->chan; // Copy the channel info to avoid
		// the lifecycle issue.
	ctxt->width = chandef->width;
	ctxt->center_freq1 = chandef->chan->center_freq;
	return iwl_mvm_phy_ctxt_apply(mvm, ctxt, chandef, chains_static,
				      chains_dynamic, action);
}

zx_status_t iwl_mvm_phy_ctxt_unref(struct iwl_mvm *mvm,
				   struct iwl_mvm_phy_ctxt *ctxt)
{
	iwl_assert_lock_held(&mvm->mutex);

	if (!ctxt) {
		return ZX_ERR_INVALID_ARGS;
	}

	if (ctxt->ref == 0) {
		return ZX_ERR_BAD_STATE;
	}

	ctxt->ref--;

	/*
   * Move unused phy's to a default channel. When the phy is moved the,
   * fw will cleanup immediate quiet bit if it was previously set,
   * otherwise we might not be able to reuse this phy.
   */
	if (ctxt->ref == 0) {
		// TODO(45353): support MIMO Rx.
		iwl_mvm_phy_ctxt_changed(mvm, ctxt, &default_chandef, 1, 1);
	}

	return ZX_OK;
}

#if 0 // NEEDS_PORTING
static void iwl_mvm_binding_iterator(void* _data, uint8_t* mac, struct ieee80211_vif* vif) {
  unsigned long* data = _data;
  struct iwl_mvm_vif* mvmvif = iwl_mvm_vif_from_mac80211(vif);

  if (!mvmvif->phy_ctxt) {
    return;
  }

  if (vif->type == NL80211_IFTYPE_STATION || vif->type == NL80211_IFTYPE_AP) {
    __set_bit(mvmvif->phy_ctxt->id, data);
  }
}

int iwl_mvm_phy_ctx_count(struct iwl_mvm* mvm) {
  unsigned long phy_ctxt_counter = 0;

  ieee80211_iterate_active_interfaces_atomic(mvm->hw, IEEE80211_IFACE_ITER_NORMAL,
                                             iwl_mvm_binding_iterator, &phy_ctxt_counter);

  return hweight8(phy_ctxt_counter);
}
#endif // NEEDS_PORTING

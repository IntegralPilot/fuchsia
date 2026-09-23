/******************************************************************************
 *
 * Copyright (C) 2018 Intel Corporation
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
#ifndef SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_INTEL_IWLWIFI_IWL_DBG_TLV_H_
#define SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_INTEL_IWLWIFI_IWL_DBG_TLV_H_

#include <stdint.h>

#include "third_party/iwlwifi/platform/kernel.h"

/**
 * struct iwl_apply_point_data
 * @data: start address of this apply point data
 * @size total size of the data
 * @offset: current offset of the copied data
 */
struct iwl_apply_point_data {
	void *data;
	int size;
	int offset;
};

/**
 * struct iwl_dbg_tlv_node - debug TLV node
 * @list: list of &struct iwl_dbg_tlv_node
 * @tlv: debug TLV
 */
struct iwl_dbg_tlv_node {
	struct list_head list;
	struct iwl_ucode_tlv tlv;
};

/**
 * union iwl_dbg_tlv_tp_data - data that is given in a time point
 * @fw_pkt: a packet received from the FW
 */
union iwl_dbg_tlv_tp_data {
	struct iwl_rx_packet *fw_pkt;
};

/**
 * struct iwl_dbg_tlv_time_point_data
 * @trig_list: list of triggers
 * @active_trig_list: list of active triggers
 * @hcmd_list: list of host commands
 * @config_list: list of configuration
 */
struct iwl_dbg_tlv_time_point_data {
	struct list_head trig_list;
	struct list_head active_trig_list;
	struct list_head hcmd_list;
	struct list_head config_list;
};

struct iwl_trans;
struct iwl_fw_runtime;
struct iwl_ucode_tlv;
void iwl_load_fw_dbg_tlv(struct device *dev, struct iwl_trans *trans);
void iwl_fw_dbg_free(struct iwl_trans *trans);
void iwl_fw_dbg_copy_tlv(struct iwl_trans *trans, struct iwl_ucode_tlv *tlv,
			 bool ext);
void iwl_alloc_dbg_tlv(struct iwl_trans *trans, size_t len, const uint8_t *data,
		       bool ext);

void _iwl_dbg_tlv_time_point(struct iwl_fw_runtime *fwrt,
			     enum iwl_fw_ini_time_point tp_id,
			     union iwl_dbg_tlv_tp_data *tp_data, bool sync);

static inline void iwl_dbg_tlv_time_point(struct iwl_fw_runtime *fwrt,
					  enum iwl_fw_ini_time_point tp_id,
					  union iwl_dbg_tlv_tp_data *tp_data)
{
#if 0 // NEEDS_PORTING
  _iwl_dbg_tlv_time_point(fwrt, tp_id, tp_data, false);
#endif // NEEDS_PORTING
}

#endif // SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_INTEL_IWLWIFI_IWL_DBG_TLV_H_

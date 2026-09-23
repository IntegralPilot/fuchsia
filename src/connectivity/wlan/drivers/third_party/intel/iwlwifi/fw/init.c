/******************************************************************************
 *
 * Copyright(c) 2017 Intel Deutschland GmbH
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
#include "third_party/iwlwifi/fw/api/datapath.h"
#include "third_party/iwlwifi/fw/api/rx.h"
#include "third_party/iwlwifi/fw/api/system.h"
#include "third_party/iwlwifi/fw/dbg.h"
#include "third_party/iwlwifi/fw/debugfs.h"
#include "third_party/iwlwifi/fw/runtime.h"
#include "third_party/iwlwifi/iwl-drv.h"
#include "third_party/iwlwifi/platform/task.h"

void iwl_fw_runtime_init(struct iwl_fw_runtime *fwrt, struct iwl_trans *trans,
			 const struct iwl_fw *fw,
			 const struct iwl_fw_runtime_ops *ops, void *ops_ctx,
			 struct dentry *dbgfs_dir)
{
	memset(fwrt, 0, sizeof(*fwrt));
	fwrt->trans = trans;
	fwrt->fw = fw;
	fwrt->dev = trans->dev;
	fwrt->dump.conf = FW_DBG_INVALID;
	fwrt->ops = ops;
	fwrt->ops_ctx = ops_ctx;
	iwl_task_create(trans->dev, &iwl_fw_error_dump_wk, fwrt,
			&fwrt->dump.wk);
	iwl_fwrt_dbgfs_register(fwrt, dbgfs_dir);
}

void iwl_fw_runtime_free(struct iwl_fw_runtime *fwrt)
{
	iwl_task_release_sync(fwrt->dump.wk);
	fwrt->dump.wk = NULL;
	kfree(fwrt->dump.d3_debug_data);
	fwrt->dump.d3_debug_data = NULL;
}

void iwl_fw_runtime_suspend(struct iwl_fw_runtime *fwrt)
{
	iwl_fw_suspend_timestamp(fwrt);
}

void iwl_fw_runtime_resume(struct iwl_fw_runtime *fwrt)
{
	iwl_fw_resume_timestamp(fwrt);
}

/* set device type and latency */
zx_status_t iwl_set_soc_latency(struct iwl_fw_runtime *fwrt)
{
	struct iwl_soc_configuration_cmd cmd = {};
	struct iwl_host_cmd hcmd = {
		.id = WIDE_ID(SYSTEM_GROUP, SOC_CONFIGURATION_CMD),
		.data[0] = &cmd,
		.len[0] = sizeof(cmd),
	};
	zx_status_t ret;

	/*
	 * In VER_1 of this command, the discrete value is considered
	 * an integer; In VER_2, it's a bitmask.  Since we have only 2
	 * values in VER_1, this is backwards-compatible with VER_2,
	 * as long as we don't set any other bits.
	 */
	if (!fwrt->trans->trans_cfg->integrated)
		cmd.flags = cpu_to_le32(SOC_CONFIG_CMD_FLAGS_DISCRETE);

	BUILD_BUG_ON(IWL_CFG_TRANS_LTR_DELAY_NONE !=
		     SOC_FLAGS_LTR_APPLY_DELAY_NONE);
	BUILD_BUG_ON(IWL_CFG_TRANS_LTR_DELAY_200US !=
		     SOC_FLAGS_LTR_APPLY_DELAY_200);
	BUILD_BUG_ON(IWL_CFG_TRANS_LTR_DELAY_2500US !=
		     SOC_FLAGS_LTR_APPLY_DELAY_2500);
	BUILD_BUG_ON(IWL_CFG_TRANS_LTR_DELAY_1820US !=
		     SOC_FLAGS_LTR_APPLY_DELAY_1820);

	if (fwrt->trans->trans_cfg->ltr_delay != IWL_CFG_TRANS_LTR_DELAY_NONE &&
	    !WARN_ON(!fwrt->trans->trans_cfg->integrated))
		cmd.flags |= le32_encode_bits(fwrt->trans->trans_cfg->ltr_delay,
					      SOC_FLAGS_LTR_APPLY_DELAY_MASK);

	if (iwl_fw_lookup_cmd_ver(fwrt->fw, SCAN_REQ_UMAC,
				  IWL_FW_CMD_VER_UNKNOWN) >= 2 &&
	    fwrt->trans->trans_cfg->low_latency_xtal)
		cmd.flags |= cpu_to_le32(SOC_CONFIG_CMD_FLAGS_LOW_LATENCY);

	cmd.latency = cpu_to_le32(fwrt->trans->trans_cfg->xtal_latency);

	ret = iwl_trans_send_cmd(fwrt->trans, &hcmd);
	if (ret != ZX_OK)
		IWL_ERR(fwrt, "Failed to set soc latency: %d\n", ret);
	return ret;
}

zx_status_t iwl_configure_rxq(struct iwl_fw_runtime *fwrt)
{
	uint8_t i, num_queues;
	uint16_t size;
	zx_status_t ret;
	struct iwl_rfh_queue_config *cmd;
	struct iwl_host_cmd hcmd = {
		.id = WIDE_ID(DATA_PATH_GROUP, RFH_QUEUE_CONFIG_CMD),
		.dataflags[0] = IWL_HCMD_DFL_NOCOPY,
	};

	/*
	 * The default queue is configured via context info, so if we
	 * have a single queue, there's nothing to do here.
	 */
	if (fwrt->trans->num_rx_queues == 1)
		return ZX_OK;

	/* Any chip lower than 22K only supports a single Rx queue. Ignore the request. */
	if (fwrt->trans->trans_cfg->device_family < IWL_DEVICE_FAMILY_22000)
		return ZX_OK;

	/* skip the default queue */
	num_queues = fwrt->trans->num_rx_queues - 1;

	size = struct_size(cmd, data, num_queues);

	cmd = calloc(1, size);
	if (!cmd)
		return ZX_ERR_NO_MEMORY;

	cmd->num_queues = num_queues;

	for (i = 0; i < num_queues; i++) {
		struct iwl_trans_rxq_dma_data data;

		cmd->data[i].q_num = i + 1;
		iwl_trans_get_rxq_dma_data(fwrt->trans, i + 1, &data);

		cmd->data[i].fr_bd_cb = cpu_to_le64(data.fr_bd_cb);
		cmd->data[i].urbd_stts_wrptr =
			cpu_to_le64(data.urbd_stts_wrptr);
		cmd->data[i].ur_bd_cb = cpu_to_le64(data.ur_bd_cb);
		cmd->data[i].fr_bd_wid = cpu_to_le32(data.fr_bd_wid);
	}

	hcmd.data[0] = cmd;
	hcmd.len[0] = size;

	ret = iwl_trans_send_cmd(fwrt->trans, &hcmd);

	kfree(cmd);

	if (ret != ZX_OK)
		IWL_ERR(fwrt, "Failed to configure RX queues: %d\n", ret);

	return ret;
}

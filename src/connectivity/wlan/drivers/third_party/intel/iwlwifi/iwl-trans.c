/******************************************************************************
 *
 * Copyright(c) 2015 Intel Mobile Communications GmbH
 * Copyright(c) 2016 - 2017 Intel Deutschland GmbH
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

#include "third_party/iwlwifi/iwl-trans.h"

#include "third_party/iwlwifi/fw/api/tx.h"
#include "third_party/iwlwifi/fw/api/commands.h"
#include "third_party/iwlwifi/iwl-constants.h"
#include "third_party/iwlwifi/iwl-drv.h"
#include "third_party/iwlwifi/iwl-fh.h"
#include "third_party/iwlwifi/queue/tx.h"

struct iwl_trans *iwl_trans_alloc(unsigned int priv_size, struct device *dev,
				  struct iwl_trans_ops *ops,
				  const struct iwl_cfg_trans_params *cfg_trans)
{
	struct iwl_trans *trans;

	trans = calloc(1, sizeof(*trans) + priv_size);
	if (!trans) {
		IWL_ERR(trans, "Failed to allocate transport\n");
		return NULL;
	}

	trans->trans_cfg = cfg_trans;

	trans->dev = dev;
	trans->ops = ops;
	trans->num_rx_queues = 1;

	WARN_ON(!ops->wait_txq_empty && !ops->wait_tx_queues_empty);

	if (trans->trans_cfg->use_tfh) {
		trans->txqs.tfd.addr_size = 64;
		trans->txqs.tfd.max_tbs = IWL_TFH_NUM_TBS;
		trans->txqs.tfd.size = sizeof(struct iwl_tfh_tfd);
	} else {
		trans->txqs.tfd.addr_size = 36;
		trans->txqs.tfd.max_tbs = IWL_NUM_OF_TBS;
		trans->txqs.tfd.size = sizeof(struct iwl_tfd);
	}
	trans->max_skb_frags = IWL_TRANS_MAX_FRAGS(trans);

	return trans;
}

zx_status_t iwl_trans_init(struct iwl_trans *trans)
{
	int txcmd_size, txcmd_align;

	if (!trans->trans_cfg->gen2) {
		txcmd_size = sizeof(struct iwl_tx_cmd);
		txcmd_align = sizeof(void *);
	} else if (trans->trans_cfg->device_family < IWL_DEVICE_FAMILY_AX210) {
		txcmd_size = sizeof(struct iwl_tx_cmd_gen2);
		txcmd_align = 64;
	} else {
		txcmd_size = sizeof(struct iwl_tx_cmd_gen3);
		txcmd_align = 128;
	}

	txcmd_size += sizeof(struct iwl_cmd_header);
	txcmd_size += 36; /* biggest possible 802.11 header */

	/* Ensure device TX cmd cannot reach/cross a page boundary in gen2 */
	if (WARN_ON(trans->trans_cfg->gen2 && txcmd_size >= txcmd_align))
		return ZX_ERR_INVALID_ARGS;

	if (trans->trans_cfg->device_family >= IWL_DEVICE_FAMILY_BZ)
		trans->txqs.bc_tbl_size = sizeof(struct iwl_gen3_bc_tbl_entry) *
					  TFD_QUEUE_BC_SIZE_GEN3_BZ;
	else if (trans->trans_cfg->device_family >= IWL_DEVICE_FAMILY_AX210)
		trans->txqs.bc_tbl_size = sizeof(struct iwl_gen3_bc_tbl_entry) *
					  TFD_QUEUE_BC_SIZE_GEN3_AX210;
	else
		trans->txqs.bc_tbl_size = sizeof(struct iwlagn_scd_bc_tbl);
#if 0 // NEEDS_PORTING
	/*
	 * For gen2 devices, we use a single allocation for each byte-count
	 * table, but they're pretty small (1k) so use a DMA pool that we
	 * allocate here.
	 */
	if (trans->trans_cfg->gen2) {
		trans->txqs.bc_pool = dmam_pool_create("iwlwifi:bc", trans->dev,
						       trans->txqs.bc_tbl_size,
						       256, 0);
		if (!trans->txqs.bc_pool)
			return -ENOMEM;
	}

	/* Some things must not change even if the config does */
	WARN_ON(trans->txqs.tfd.addr_size !=
		(trans->trans_cfg->use_tfh ? 64 : 36));

	snprintf(trans->dev_cmd_pool_name, sizeof(trans->dev_cmd_pool_name),
		 "iwl_cmd_pool:%s", dev_name(trans->dev));
	trans->dev_cmd_pool =
		kmem_cache_create(trans->dev_cmd_pool_name,
				  txcmd_size, txcmd_align,
				  SLAB_HWCACHE_ALIGN, NULL);
	if (!trans->dev_cmd_pool)
		return -ENOMEM;

	trans->txqs.tso_hdr_page = alloc_percpu(struct iwl_tso_hdr_page);
	if (!trans->txqs.tso_hdr_page) {
		kmem_cache_destroy(trans->dev_cmd_pool);
		return -ENOMEM;
	}

	/* Initialize the wait queue for commands */
	init_waitqueue_head(&trans->wait_command_queue);
#endif // NEEDS_PORTING

	return ZX_OK;
}

void iwl_trans_free(struct iwl_trans *trans)
{
	free(trans);
}

zx_status_t iwl_trans_send_cmd(struct iwl_trans *trans,
			       struct iwl_host_cmd *cmd)
{
	zx_status_t ret;

	if (unlikely(!(cmd->flags & CMD_SEND_IN_RFKILL) &&
		     test_bit(STATUS_RFKILL_OPMODE, &trans->status))) {
		return ZX_ERR_BAD_STATE;
	}

	/*
   * We can't test IWL_MVM_STATUS_IN_D3 in mvm->status because this
   * bit is set early in the D3 flow, before we send all the commands
   * that configure the firmware for D3 operation (power, patterns, ...)
   * and we don't want to flag all those with CMD_SEND_IN_D3.
   * So use the system_pm_mode instead. The only command sent after
   * we set system_pm_mode is D3_CONFIG_CMD, which we now flag with
   * CMD_SEND_IN_D3.
   */
	if (unlikely(trans->system_pm_mode == IWL_PLAT_PM_MODE_D3 &&
		     !(cmd->flags & CMD_SEND_IN_D3)))
		return ZX_ERR_UNAVAILABLE;

	if (unlikely(trans->state != IWL_TRANS_FW_ALIVE)) {
		IWL_ERR(trans, "%s bad state = %d\n", __func__, trans->state);
		return ZX_ERR_IO;
	}

	if (WARN_ON((cmd->flags & CMD_WANT_ASYNC_CALLBACK) &&
		    !(cmd->flags & CMD_ASYNC))) {
		return ZX_ERR_IO_INVALID;
	}

#if 0 // NEEDS_PORTING
  if (!(cmd->flags & CMD_ASYNC))
    lock_map_acquire_read(&trans->sync_cmd_lockdep_map);
#endif // NEEDS_PORTING

	if (trans->wide_cmd_header && !iwl_cmd_groupid(cmd->id)) {
		if (cmd->id != REPLY_ERROR)
			cmd->id = DEF_ID(cmd->id);
	}

	ret = iwl_trans_txq_send_hcmd(trans, cmd);

#if 0 // NEEDS_PORTING
  if (!(cmd->flags & CMD_ASYNC))
    lock_map_release(&trans->sync_cmd_lockdep_map);
#endif // NEEDS_PORTING

	if ((cmd->flags & CMD_WANT_SKB) && !ret && !cmd->resp_pkt) {
		IWL_ERR(trans,
			"%s(): WANT_SKB required but no resp pkt attached.\n",
			__func__);
		return ZX_ERR_IO;
	}

	return ret;
}

/* Comparator for struct iwl_hcmd_names.
 * Used in the binary search over a list of host commands.
 *
 * @key: command_id that we're looking for.
 * @elt: struct iwl_hcmd_names candidate for match.
 *
 * @return 0 iff equal.
 */
static int iwl_hcmd_names_cmp(const void *key, const void *elt)
{
	const struct iwl_hcmd_names *name = elt;
	uint8_t cmd1 = *(uint8_t *)key;
	uint8_t cmd2 = name->cmd_id;

	return (cmd1 - cmd2);
}

const char *iwl_get_cmd_string(struct iwl_trans *trans, uint32_t id)
{
	uint8_t grp, cmd;
	struct iwl_hcmd_names *ret;
	const struct iwl_hcmd_arr *arr;
	size_t size = sizeof(struct iwl_hcmd_names);

	grp = iwl_cmd_groupid(id);
	cmd = iwl_cmd_opcode(id);

	if (!trans->command_groups || grp >= trans->command_groups_size ||
	    !trans->command_groups[grp].arr) {
		return "UNKNOWN";
	}

	arr = &trans->command_groups[grp];
	ret = bsearch(&cmd, arr->arr, arr->size, size, iwl_hcmd_names_cmp);
	if (!ret) {
		return "UNKNOWN";
	}
	return ret->cmd_name;
}

int iwl_cmd_groups_verify_sorted(const struct iwl_trans_config *trans)
{
	int i, j;
	const struct iwl_hcmd_arr *arr;

	for (i = 0; i < trans->command_groups_size; i++) {
		arr = &trans->command_groups[i];
		if (!arr->arr) {
			continue;
		}
		for (j = 0; j < arr->size - 1; j++) {
			if (arr->arr[j].cmd_id > arr->arr[j + 1].cmd_id) {
				return -1;
			}
		}
	}
	return 0;
}

void iwl_trans_ref(struct iwl_trans *trans)
{
	if (trans->ops->ref) {
		trans->ops->ref(trans);
	}
}

void iwl_trans_unref(struct iwl_trans *trans)
{
	if (trans->ops->unref) {
		trans->ops->unref(trans);
	}
}

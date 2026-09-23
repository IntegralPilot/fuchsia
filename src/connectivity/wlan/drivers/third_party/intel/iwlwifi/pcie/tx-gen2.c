/******************************************************************************
 *
 * Copyright(c) 2017 Intel Deutschland GmbH
 * Copyright(c) 2018        Intel Corporation
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
#include "third_party/iwlwifi/fw/api/tx.h"
#include "third_party/iwlwifi/iwl-csr.h"
#include "third_party/iwlwifi/iwl-debug.h"
#include "third_party/iwlwifi/iwl-io.h"
#include "third_party/iwlwifi/pcie/internal.h"
#include "third_party/iwlwifi/queue/tx.h"

/*************** HOST COMMAND QUEUE FUNCTIONS   *****/

/*
 * iwl_pcie_gen2_enqueue_hcmd - enqueue a uCode command
 * @priv: device private data point
 * @cmd: a pointer to the ucode command structure
 *
 * The function returns ZX_ERR_* values to indicate the operation failed. On success, opposite to
 * the original driver, the index (>= 0) of command in the command queue is NOT returned (since the
 * caller doesn't use it anyway).
 */
zx_status_t iwl_pcie_gen2_enqueue_hcmd(struct iwl_trans *trans,
				       struct iwl_host_cmd *cmd)
{
	struct iwl_trans_pcie *trans_pcie = IWL_TRANS_GET_PCIE_TRANS(trans);
	struct iwl_txq *txq = trans->txqs.txq[trans->txqs.cmd.q_id];
	struct iwl_device_cmd *out_cmd;
	struct iwl_cmd_meta *out_meta;
	void *dup_buf = NULL;
	dma_addr_t phys_addr;
	int i, cmd_pos, idx;
	u16 copy_size, cmd_size, tb0_size;
	bool had_nocopy = false;
	u8 group_id = iwl_cmd_groupid(cmd->id);
	const u8 *cmddata[IWL_MAX_CMD_TBS_PER_TFD];
	u16 cmdlen[IWL_MAX_CMD_TBS_PER_TFD];
	struct iwl_tfh_tfd *tfd;
	__UNUSED unsigned long flags;
	zx_status_t ret = ZX_OK;

	copy_size = sizeof(struct iwl_cmd_header_wide);
	cmd_size = sizeof(struct iwl_cmd_header_wide);

	for (i = 0; i < IWL_MAX_CMD_TBS_PER_TFD; i++) {
		cmddata[i] = cmd->data[i];
		cmdlen[i] = cmd->len[i];

		if (!cmd->len[i])
			continue;

		/* need at least IWL_FIRST_TB_SIZE copied */
		if (copy_size < IWL_FIRST_TB_SIZE) {
			int copy = IWL_FIRST_TB_SIZE - copy_size;

			if (copy > cmdlen[i])
				copy = cmdlen[i];
			cmdlen[i] -= copy;
			cmddata[i] += copy;
			copy_size += copy;
		}

		if (cmd->dataflags[i] & IWL_HCMD_DFL_NOCOPY) {
			had_nocopy = true;
			if (cmd->dataflags[i] & IWL_HCMD_DFL_DUP) {
				IWL_WARN(
					trans,
					"hcmd contains DFL_DUP but DFL_NOCOPY is asserted too.\n");
				ret = ZX_ERR_INVALID_ARGS;
				goto free_dup_buf;
			}
		} else if (cmd->dataflags[i] & IWL_HCMD_DFL_DUP) {
			/*
			 * This is also a chunk that isn't copied
			 * to the static buffer so set had_nocopy.
			 */
			had_nocopy = true;

			/* only allowed once */
			if (dup_buf) {
				IWL_WARN(trans,
					 "Only allowed once: dup_buf=%p\n",
					 dup_buf);
				ret = ZX_ERR_INVALID_ARGS;
				goto free_dup_buf;
			}

			dup_buf = calloc(1, cmdlen[i]);
			if (!dup_buf)
				return ZX_ERR_NO_MEMORY;
			memcpy(dup_buf, cmddata[i], cmdlen[i]);
		} else {
			/* NOCOPY must not be followed by normal! */
			if (had_nocopy) {
				IWL_WARN(
					trans,
					"NOCOPY must not be followed by normal!\n");
				ret = ZX_ERR_INVALID_ARGS;
				goto free_dup_buf;
			}
			copy_size += cmdlen[i];
		}
		cmd_size += cmd->len[i];
	}

	/*
	 * If any of the command structures end up being larger than the
	 * TFD_MAX_PAYLOAD_SIZE and they aren't dynamically allocated into
	 * separate TFDs, then we will need to increase the size of the buffers
	 */
	if (copy_size > TFD_MAX_PAYLOAD_SIZE) {
		IWL_WARN(trans, "Command %s (%#x) is too large (%d bytes)\n",
			 iwl_get_cmd_string(trans, cmd->id), cmd->id,
			 copy_size);
		ret = ZX_ERR_INVALID_ARGS;
		goto free_dup_buf;
	}

	spin_lock_irqsave(&txq->lock, flags);

	idx = iwl_txq_get_cmd_index(txq, txq->write_ptr);
	tfd = iwl_txq_get_tfd(trans, txq, txq->write_ptr);
	memset(tfd, 0, sizeof(*tfd));

	if (iwl_txq_space(trans, txq) < ((cmd->flags & CMD_ASYNC) ? 2 : 1)) {
		spin_unlock_irqrestore(&txq->lock, flags);

		IWL_ERR(trans, "No space in command queue\n");
		iwl_op_mode_cmd_queue_full(trans->op_mode);
		ret = ZX_ERR_NO_SPACE;
		goto free_dup_buf;
	}

	out_cmd = iwl_iobuf_virtual(txq->entries[idx].cmd);
	out_meta = &txq->entries[idx].meta;

	/* re-initialize to NULL */
	memset(out_meta, 0, sizeof(*out_meta));
	if (cmd->flags & CMD_WANT_SKB)
		out_meta->source = cmd;

	/* set up the header */
	out_cmd->hdr_wide.cmd = iwl_cmd_opcode(cmd->id);
	out_cmd->hdr_wide.group_id = group_id;
	out_cmd->hdr_wide.version = iwl_cmd_version(cmd->id);
	out_cmd->hdr_wide.length =
		cpu_to_le16(cmd_size - sizeof(struct iwl_cmd_header_wide));
	out_cmd->hdr_wide.reserved = 0;
	out_cmd->hdr_wide.sequence =
		cpu_to_le16(QUEUE_TO_SEQ(trans->txqs.cmd.q_id) |
			    INDEX_TO_SEQ(txq->write_ptr));

	cmd_pos = sizeof(struct iwl_cmd_header_wide);
	copy_size = sizeof(struct iwl_cmd_header_wide);

	/* and copy the data that needs to be copied */
	for (i = 0; i < IWL_MAX_CMD_TBS_PER_TFD; i++) {
		int copy;

		if (!cmd->len[i])
			continue;

		/* copy everything if not nocopy/dup */
		if (!(cmd->dataflags[i] &
		      (IWL_HCMD_DFL_NOCOPY | IWL_HCMD_DFL_DUP))) {
			copy = cmd->len[i];

			memcpy((u8 *)out_cmd + cmd_pos, cmd->data[i], copy);
			cmd_pos += copy;
			copy_size += copy;
			continue;
		}

		/*
		 * Otherwise we need at least IWL_FIRST_TB_SIZE copied
		 * in total (for bi-directional DMA), but copy up to what
		 * we can fit into the payload for debug dump purposes.
		 */
		copy = min_t(int, TFD_MAX_PAYLOAD_SIZE - cmd_pos, cmd->len[i]);

		memcpy((u8 *)out_cmd + cmd_pos, cmd->data[i], copy);
		cmd_pos += copy;

		/* However, treat copy_size the proper way, we need it below */
		if (copy_size < IWL_FIRST_TB_SIZE) {
			copy = IWL_FIRST_TB_SIZE - copy_size;

			if (copy > cmd->len[i])
				copy = cmd->len[i];
			copy_size += copy;
		}
	}

	IWL_DEBUG_HC(
		trans,
		"Sending command %s (%.2x.%.2x), seq: 0x%04X, %d bytes at %d[%d]:%d\n",
		iwl_get_cmd_string(trans, cmd->id), group_id, out_cmd->hdr.cmd,
		le16_to_cpu(out_cmd->hdr.sequence), cmd_size, txq->write_ptr,
		idx, trans->txqs.cmd.q_id);

	/* start the TFD with the minimum copy bytes */
	struct iwl_pcie_first_tb_buf *tb_bufs =
		iwl_iobuf_virtual(txq->first_tb_bufs);
	tb0_size = min_t(int, copy_size, IWL_FIRST_TB_SIZE);
	memcpy(&tb_bufs[idx], out_cmd, tb0_size);
	dma_addr_t first_tb_dma = iwl_txq_get_first_tb_dma(txq, idx);
	iwl_txq_gen2_set_tb(trans, tfd, first_tb_dma, tb0_size, NULL);
	iwl_iobuf_cache_flush(txq->first_tb_bufs, 0, IWL_FIRST_TB_SIZE);

	/* map first command fragment, if any remains */
	if (copy_size > tb0_size) {
		phys_addr =
			iwl_iobuf_physical(txq->entries[idx].cmd) + tb0_size;
		iwl_txq_gen2_set_tb(trans, tfd, phys_addr, copy_size - tb0_size,
				    NULL);
		iwl_iobuf_cache_flush(txq->entries[idx].cmd, 0,
				      copy_size - tb0_size);
	}

	/* map the remaining (adjusted) nocopy/dup fragments */
	for (i = 0; i < IWL_MAX_CMD_TBS_PER_TFD; i++) {
		void *data = (void *)(uintptr_t)cmddata[i];

		if (!cmdlen[i])
			continue;
		if (!(cmd->dataflags[i] &
		      (IWL_HCMD_DFL_NOCOPY | IWL_HCMD_DFL_DUP)))
			continue;
		if (cmd->dataflags[i] & IWL_HCMD_DFL_DUP)
			data = dup_buf;

		// Leverage the 'dup_io_buf' for the DMA address used for 'iwl_txq_gen2_set_tb()'.
		// Check more comments in 'iwl_pcie_enqueue_hcmd()' in pcie/tx.c.
		struct iwl_iobuf *dup_io_buf = txq->entries[idx].dup_io_buf;
		ZX_ASSERT(dup_io_buf == NULL);
		uint16_t dup_len = cmdlen[i];
		iwl_iobuf_allocate_contiguous(&trans_pcie->pci_dev->dev,
					      dup_len, &dup_io_buf);
		void *virt_addr = iwl_iobuf_virtual(dup_io_buf);
		memcpy(virt_addr, data, dup_len);
		phys_addr = iwl_iobuf_physical(dup_io_buf);
		iwl_txq_gen2_set_tb(trans, tfd, phys_addr, dup_len, NULL);
		iwl_iobuf_cache_flush(dup_io_buf, 0, dup_len);
		txq->entries[idx].dup_io_buf = dup_io_buf;
	}

	BUILD_BUG_ON(IWL_TFH_NUM_TBS > sizeof(out_meta->tbs) * BITS_PER_BYTE);
	out_meta->flags = cmd->flags;
#if 0 // NEEDS_PORTING
	if (WARN_ON_ONCE(txq->entries[idx].free_buf))
		kfree_sensitive(txq->entries[idx].free_buf);
	txq->entries[idx].free_buf = dup_buf;

	trace_iwlwifi_dev_hcmd(trans->dev, cmd, cmd_size, &out_cmd->hdr_wide);
#endif // NEEDS_PORTING

	/* start timer if queue currently empty */
	if (txq->read_ptr == txq->write_ptr && txq->wd_timeout) {
		iwl_irq_timer_stop(txq->stuck_timer);
		iwl_irq_timer_start(txq->stuck_timer, txq->wd_timeout);
	}

	spin_lock(&trans_pcie->reg_lock);
	/* Increment and update queue's write index */
	txq->write_ptr = iwl_txq_inc_wrap(trans, txq->write_ptr);
	iwl_txq_inc_wr_ptr(trans, txq);
	spin_unlock(&trans_pcie->reg_lock);

	spin_unlock_irqrestore(&txq->lock, flags);
free_dup_buf:
	free(dup_buf);
	return ret;
}

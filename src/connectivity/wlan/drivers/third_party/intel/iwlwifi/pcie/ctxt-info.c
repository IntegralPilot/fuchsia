// Use of this code is governed by a BSD-style license that can be found in the LICENSE file.
/*
 * Copyright (C) 2017 Intel Deutschland GmbH
 * Copyright (C) 2018-2021 Intel Corporation
 *
 * Comments from Intel about this file:
 *
 *   In a nutshell - the context info is an area in the DRAM which the IML (image loader) reads from
 *   in order to init the FW.
 *
 *   For older devices (ThP2 and below) the driver was loading each chunk to the device and then
 *   kicking the ROM to run the init sequence.
 *
 *   For newer devices (HrP2 and newer) we introduced the context info. The driver will place
 *   fw images (upper and lower macs) on the DRAM, will assign the context info in the pointers,
 *   and then would kick the image loader to started making these DMA transactions and start the
 *   upper and lower MACs.
 */
#include "third_party/iwlwifi/iwl-trans.h"
#include "third_party/iwlwifi/iwl-fh.h"
#include "third_party/iwlwifi/iwl-context-info.h"
#include "third_party/iwlwifi/pcie/internal.h"
#include "third_party/iwlwifi/iwl-prph.h"
#include "third_party/iwlwifi/platform/memory.h"
#include "third_party/iwlwifi/queue/tx.h"

static zx_status_t
_iwl_pcie_ctxt_info_dma_alloc_coherent(struct iwl_trans *trans, size_t size,
				       struct iwl_iobuf **out_iobuf, int depth)
{
	zx_status_t result;

	ZX_ASSERT(out_iobuf);

	if (depth > 2) {
		IWL_WARN(
			trans,
			"failed to allocate DMA memory not crossing 2^32 boundary. depth=%d\n",
			depth);
		return ZX_ERR_INVALID_ARGS;
	}

	result = iwl_iobuf_allocate_contiguous(trans->dev, size, out_iobuf);

	if (result != ZX_OK)
		return result;

	dma_addr_t phys = iwl_iobuf_physical(*out_iobuf);
	if (unlikely(iwl_txq_crosses_4g_boundary(phys, size))) {
		struct iwl_iobuf *old = *out_iobuf;

		result = _iwl_pcie_ctxt_info_dma_alloc_coherent(
			trans, size, out_iobuf, depth + 1);
		iwl_iobuf_release(old);
	}

	return result;
}

static zx_status_t
iwl_pcie_ctxt_info_dma_alloc_coherent(struct iwl_trans *trans, size_t size,
				      struct iwl_iobuf **out_iobuf)
{
	// This is a workaround for the case that ctxt_info section could be zero.
	// Since Fuchsia io_buf doesn't accept size 0, we change it to allocate 1 byte instead.
	if (size == 0) {
		size = 1;
	}

	return _iwl_pcie_ctxt_info_dma_alloc_coherent(trans, size, out_iobuf,
						      0);
}

zx_status_t iwl_pcie_ctxt_info_alloc_dma(struct iwl_trans *trans,
					 const void *data, u32 len,
					 struct iwl_dram_data *dram)
{
	zx_status_t result;
	result =
		iwl_pcie_ctxt_info_dma_alloc_coherent(trans, len, &dram->block);
	if (result != ZX_OK)
		return result;
	dram->physical = iwl_iobuf_physical(dram->block);

	dram->size = len;
	void *virt = iwl_iobuf_virtual(dram->block);
	memcpy(virt, data, len);

	return ZX_OK;
}

void iwl_pcie_ctxt_info_free_paging(struct iwl_trans *trans)
{
	struct iwl_self_init_dram *dram = &trans->init_dram;
	int i;

	if (!dram->paging) {
		if (dram->paging_cnt) {
			IWL_WARN(
				trans,
				"No paging allocated but the paging count is non-zero (%d).\n",
				dram->paging_cnt);
		}
		return;
	}

	/* free firmware sections */
	for (i = 0; i < dram->fw_cnt; i++) {
		iwl_iobuf_release(dram->fw[i].block);
	}

	/* free paging*/
	for (i = 0; i < dram->paging_cnt; i++) {
		iwl_iobuf_release(dram->paging[i].block);
	}

	free(dram->fw);
	dram->fw_cnt = 0;
	dram->fw = NULL;
	free(dram->paging);
	dram->paging_cnt = 0;
	dram->paging = NULL;
}

zx_status_t iwl_pcie_init_fw_sec(struct iwl_trans *trans,
				 const struct fw_img *fw,
				 struct iwl_context_info_dram *ctxt_dram)
{
	struct iwl_self_init_dram *dram = &trans->init_dram;
	int i, ret, lmac_cnt, umac_cnt, paging_cnt;

	if (dram->paging) {
		IWL_WARN(trans,
			 "paging shouldn't already be initialized (%d pages)\n",
			 dram->paging_cnt);
		iwl_pcie_ctxt_info_free_paging(trans);
	}

	lmac_cnt = iwl_pcie_get_num_sections(fw, 0);
	/* add 1 due to separator */
	umac_cnt = iwl_pcie_get_num_sections(fw, lmac_cnt + 1);
	/* add 2 due to separators */
	paging_cnt = iwl_pcie_get_num_sections(fw, lmac_cnt + umac_cnt + 2);

	dram->fw = calloc(umac_cnt + lmac_cnt, sizeof(*dram->fw));
	if (!dram->fw)
		return ZX_ERR_NO_MEMORY;
	dram->paging = calloc(paging_cnt, sizeof(*dram->paging));
	if (!dram->paging)
		return ZX_ERR_NO_MEMORY;

	/* initialize lmac sections */
	for (i = 0; i < lmac_cnt; i++) {
		ret = iwl_pcie_ctxt_info_alloc_dma(trans, fw->sec[i].data,
						   fw->sec[i].len,
						   &dram->fw[dram->fw_cnt]);
		if (ret != ZX_OK)
			return ret;
		ctxt_dram->lmac_img[i] =
			cpu_to_le64(dram->fw[dram->fw_cnt].physical);
		dram->fw_cnt++;
	}

	/* initialize umac sections */
	for (i = 0; i < umac_cnt; i++) {
		/* access FW with +1 to make up for lmac separator */
		ret = iwl_pcie_ctxt_info_alloc_dma(
			trans, fw->sec[dram->fw_cnt + 1].data,
			fw->sec[dram->fw_cnt + 1].len, &dram->fw[dram->fw_cnt]);
		if (ret != ZX_OK)
			return ret;
		ctxt_dram->umac_img[i] =
			cpu_to_le64(dram->fw[dram->fw_cnt].physical);
		dram->fw_cnt++;
	}

	/*
	 * Initialize paging.
	 * Paging memory isn't stored in dram->fw as the umac and lmac - it is
	 * stored separately.
	 * This is since the timing of its release is different -
	 * while fw memory can be released on alive, the paging memory can be
	 * freed only when the device goes down.
	 * Given that, the logic here in accessing the fw image is a bit
	 * different - fw_cnt isn't changing so loop counter is added to it.
	 */
	for (i = 0; i < paging_cnt; i++) {
		/* access FW with +2 to make up for lmac & umac separators */
		int fw_idx = dram->fw_cnt + i + 2;

		ret = iwl_pcie_ctxt_info_alloc_dma(trans, fw->sec[fw_idx].data,
						   fw->sec[fw_idx].len,
						   &dram->paging[i]);
		if (ret != ZX_OK)
			return ret;

		ctxt_dram->virtual_img[i] =
			cpu_to_le64(dram->paging[i].physical);
		dram->paging_cnt++;
	}

	return ZX_OK;
}

zx_status_t iwl_pcie_ctxt_info_init(struct iwl_trans *trans,
				    const struct fw_img *fw)
{
	struct iwl_trans_pcie *trans_pcie = IWL_TRANS_GET_PCIE_TRANS(trans);
	struct iwl_context_info *ctxt_info;
	struct iwl_context_info_rbd_cfg *rx_cfg;
	u32 control_flags = 0, rb_size;
	dma_addr_t phys;
	struct iwl_iobuf *ctxt_info_iobuf;
	zx_status_t ret;

	memset(&trans->init_dram, 0, sizeof(trans->init_dram));

	ret = iwl_pcie_ctxt_info_dma_alloc_coherent(trans, sizeof(*ctxt_info),
						    &ctxt_info_iobuf);
	if (ret != ZX_OK)
		return ret;
	ctxt_info = iwl_iobuf_virtual(ctxt_info_iobuf);
	phys = iwl_iobuf_physical(ctxt_info_iobuf);

	trans_pcie->ctxt_info_dma_addr = phys;

	ctxt_info->version.version = 0;
	ctxt_info->version.mac_id =
		cpu_to_le16((u16)iwl_read32(trans, CSR_HW_REV));
	/* size is in DWs */
	ctxt_info->version.size = cpu_to_le16(sizeof(*ctxt_info) / 4);

	switch (trans_pcie->rx_buf_size) {
	case IWL_AMSDU_2K:
		rb_size = IWL_CTXT_INFO_RB_SIZE_2K;
		break;
	case IWL_AMSDU_4K:
		rb_size = IWL_CTXT_INFO_RB_SIZE_4K;
		break;
	case IWL_AMSDU_8K:
		rb_size = IWL_CTXT_INFO_RB_SIZE_8K;
		break;
	case IWL_AMSDU_12K:
		rb_size = IWL_CTXT_INFO_RB_SIZE_16K;
		break;
	default:
		WARN_ON(1);
		rb_size = IWL_CTXT_INFO_RB_SIZE_4K;
	}

	WARN_ON(RX_QUEUE_CB_SIZE(trans->cfg->num_rbds) > 12);
	control_flags = IWL_CTXT_INFO_TFD_FORMAT_LONG;
	control_flags |= u32_encode_bits(RX_QUEUE_CB_SIZE(trans->cfg->num_rbds),
					 IWL_CTXT_INFO_RB_CB_SIZE);
	control_flags |= u32_encode_bits(rb_size, IWL_CTXT_INFO_RB_SIZE);
	ctxt_info->control.control_flags = cpu_to_le32(control_flags);

	/* initialize RX default queue */
	rx_cfg = &ctxt_info->rbd_cfg;
	rx_cfg->free_rbd_addr =
		iwl_iobuf_physical(trans_pcie->rxq->descriptors);
	rx_cfg->used_rbd_addr =
		iwl_iobuf_physical(trans_pcie->rxq->used_descriptors);
	rx_cfg->status_wr_ptr = iwl_iobuf_physical(trans_pcie->rxq->rb_status);

	/* initialize TX command queue */
	ctxt_info->hcmd_cfg.cmd_queue_addr =
		cpu_to_le64(trans->txqs.txq[trans->txqs.cmd.q_id]->dma_addr);
	ctxt_info->hcmd_cfg.cmd_queue_size =
		TFD_QUEUE_CB_SIZE(IWL_CMD_QUEUE_SIZE);

	/* allocate ucode sections in dram and set addresses */
	ret = iwl_pcie_init_fw_sec(trans, fw, &ctxt_info->dram);
	if (ret != ZX_OK) {
		iwl_iobuf_release(ctxt_info_iobuf);
		return ret;
	}

	trans_pcie->ctxt_info = ctxt_info;
	trans_pcie->ctxt_info_iobuf = ctxt_info_iobuf;

	iwl_enable_fw_load_int_ctx_info(trans);

	/* Configure debug, if exists */
	if (iwl_pcie_dbg_on(trans))
		iwl_pcie_apply_destination(trans);

	/* kick FW self load */
	iwl_write64(trans, CSR_CTXT_INFO_BA, trans_pcie->ctxt_info_dma_addr);

	/* Context info will be released upon alive or failure to get one */

	return ZX_OK;
}

void iwl_pcie_ctxt_info_free(struct iwl_trans *trans)
{
	struct iwl_trans_pcie *trans_pcie = IWL_TRANS_GET_PCIE_TRANS(trans);

	if (!trans_pcie->ctxt_info)
		return;

	iwl_iobuf_release(trans_pcie->ctxt_info_iobuf);
	trans_pcie->ctxt_info_iobuf = NULL;
	trans_pcie->ctxt_info_dma_addr = 0;
	trans_pcie->ctxt_info = NULL;

	iwl_pcie_ctxt_info_free_fw_img(trans);
}

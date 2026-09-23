/******************************************************************************
 *
 * Copyright(c) 2003 - 2015 Intel Corporation. All rights reserved.
 * Copyright(c) 2013 - 2015 Intel Mobile Communications GmbH
 * Copyright(c) 2016 - 2017 Intel Deutschland GmbH
 * Copyright(c) 2018 Intel Corporation
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
#ifndef SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_INTEL_IWLWIFI_PCIE_INTERNAL_H_
#define SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_INTEL_IWLWIFI_PCIE_INTERNAL_H_

#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/sync/completion.h>
#include <lib/mmio-ptr/mmio-ptr.h>
#include <threads.h>
#include <zircon/listnode.h>

#include "third_party/iwlwifi/iwl-csr.h"
#include "third_party/iwlwifi/iwl-debug.h"
#include "third_party/iwlwifi/iwl-drv.h"
#include "third_party/iwlwifi/iwl-fh.h"
#include "third_party/iwlwifi/iwl-io.h"
#include "third_party/iwlwifi/iwl-op-mode.h"
#include "third_party/iwlwifi/iwl-trans.h"
#include "third_party/iwlwifi/platform/banjo/pci.h"
#include "third_party/iwlwifi/platform/compiler.h"
#include "third_party/iwlwifi/platform/ieee80211.h"
#include "third_party/iwlwifi/platform/irq.h"
#include "third_party/iwlwifi/platform/kernel.h"
#include "third_party/iwlwifi/platform/memory.h"
#include "zircon/compiler.h"

__BEGIN_CDECLS

/* We need 2 entries for the TX command and header, and another one might
 * be needed for potential data in the SKB's head. The remaining ones can
 * be used for frags.
 */
#define IWL_PCIE_MAX_FRAGS(x) (x->max_tbs - 3)

/*
 * RX related structures and functions
 */
#define RX_NUM_QUEUES 1
#define RX_POST_REQ_ALLOC 2
#define RX_CLAIM_REQ_ALLOC 8
#define RX_PENDING_WATERMARK 16
#define FIRST_RX_QUEUE 512

struct iwl_host_cmd;

/*This file includes the declaration that are internal to the
 * trans_pcie layer */

/**
 * struct iwl_rx_mem_buffer
 * @io_buf: driver's handle to the rxb memory
 * @list: list entry for the membuffer
 * @invalid: rxb is in driver ownership - not owned by HW
 * @vid: index of this rxb in the global table
 * @offset: indicates which offset of the page (in bytes)
 *	this buffer uses (if multiple RBs fit into one page)
 */
struct iwl_rx_mem_buffer {
	struct iwl_iobuf *io_buf;
	list_node_t list;
	u32 offset;
	u16 vid;
	bool invalid;
};

/**
 * struct isr_statistics - interrupt statistics
 *
 */
struct isr_statistics {
	u32 hw;
	u32 sw;
	u32 err_code;
	u32 sch;
	u32 alive;
	u32 rfkill;
	u32 ctkill;
	u32 wakeup;
	u32 rx;
	u32 tx;
	u32 unhandled;
};

/**
 * struct iwl_rx_transfer_desc - transfer descriptor
 * @addr: ptr to free buffer start address
 * @rbid: unique tag of the buffer
 * @reserved: reserved
 */
struct iwl_rx_transfer_desc {
	__le16 rbid;
	__le16 reserved[3];
	__le64 addr;
} __packed;

#define IWL_RX_CD_SIZE 0xffffff00
#define IWL_RX_CD_FLAGS_FRAGMENTED BIT(0)

/**
 * struct iwl_rx_completion_desc - completion descriptor
 * @reserved1: reserved
 * @rbid: unique tag of the received buffer
 * @flags: flags (0: fragmented, all others: reserved)
 * @reserved2: reserved
 */
struct iwl_rx_completion_desc {
	__le32 reserved1;
	__le16 rbid;
	u8 flags;
	u8 reserved2[25];
} __packed;

/**
 * struct iwl_rx_completion_desc_bz - Bz completion descriptor
 * @rbid: unique tag of the received buffer
 * @flags: flags (0: fragmented, all others: reserved)
 * @reserved: reserved
 */
struct iwl_rx_completion_desc_bz {
	__le16 rbid;
	u8 flags;
	u8 reserved[1];
} __packed;

/**
 * struct iwl_rxq - Rx queue
 * @id: queue index
 * @descriptors: IO buffer of receive buffer descriptors (rbd).
 *  Address size is 32 bit in pre-9000 devices and 64 bit in 9000 devices.
 *  In 22560 devices it is a pointer to a list of iwl_rx_transfer_desc's
 *  This replaces the former 'bd' and 'bd_dma' fields.
 * @used_descriptors: driver's pointer to buffer of used receive buffer descriptors (rbd).
 *  This replaces the former 'used_bd_dma' field. Its virtual address 'used_bd' is also unioned
 *  with the 'bd_32' field (for 9000 and lower chipsets) and the 'cd' field (for 22000 chipsets).
 * @read: Shared index to newest available Rx buffer
 * @write: Shared index to oldest written Rx packet
 * @free_count: Number of pre-allocated buffers in rx_free
 * @used_count: Number of RBDs handled to allocator to use for allocation
 * @write_actual:
 * @rx_free: list of RBDs with allocated RB ready for use
 * @rx_used: list of RBDs with no RB attached
 * @need_update: flag to indicate we need to update read/write index
 * @rb_status: driver's pointer to receive buffer status
 *  This replaces the former 'rb_stts' and 'rb_stts_dma' fields.
 * @lock:
 * @queue: actual rx queue. Not used for multi-rx queue.
 * @next_rb_is_fragment: indicates that the previous RB that we handled set
 *	the fragmented flag, so the next one is still another fragment
 *
 * NOTE:  rx_free and rx_used are used as a FIFO for iwl_rx_mem_buffers
 */
struct iwl_rxq {
	int id;
	struct iwl_iobuf *descriptors;
	struct iwl_iobuf *used_descriptors;

	//  These fields are only used for multi-rx queue devices.
	union {
		void *used_bd; // the virtual address of 'used_descriptors'.
		__le32 *bd_32;
		struct iwl_rx_completion_desc *cd;
	};

	u32 read;
	u32 write;
	u32 free_count;
	u32 used_count;
	u32 write_actual;
	u32 queue_size;
	list_node_t rx_free;
	list_node_t rx_used;
	bool need_update, next_rb_is_fragment;
	struct iwl_iobuf *rb_status;
	mtx_t lock;
	struct napi_struct
		napi; // TODO(43218): replace with something like mvmvif so that when
	//              packet is received we know where to dispatch.
	struct iwl_rx_mem_buffer *queue[RX_QUEUE_SIZE];
};

/**
 * iwl_queue_inc_wrap - increment queue index, wrap back to beginning
 * @index -- current index
 */
static inline int iwl_queue_inc_wrap(struct iwl_trans *trans, int index)
{
	return ++index &
	       (trans->trans_cfg->base_params->max_tfd_queue_size - 1);
}

/**
 * iwl_get_closed_rb_stts - get closed rb stts from different structs
 * @rxq - the rxq to get the rb stts from
 */
static inline __le16 iwl_get_closed_rb_stts(struct iwl_trans *trans,
					    struct iwl_rxq *rxq)
{
	if (trans->trans_cfg->device_family >= IWL_DEVICE_FAMILY_AX210) {
		__le16 *rb_status = (__le16 *)iwl_iobuf_virtual(rxq->rb_status);

		return READ_ONCE(*rb_status);
	} else {
		struct iwl_rb_status *rb_status =
			(struct iwl_rb_status *)iwl_iobuf_virtual(
				rxq->rb_status);

		return READ_ONCE(rb_status->closed_rb_num);
	}
}

/**
 * iwl_queue_dec_wrap - decrement queue index, wrap back to end
 * @index -- current index
 */
static inline int iwl_queue_dec_wrap(struct iwl_trans *trans, int index)
{
	return --index &
	       (trans->trans_cfg->base_params->max_tfd_queue_size - 1);
}

#define TFD_TX_CMD_SLOTS 256
#define TFD_CMD_SLOTS 32

static inline dma_addr_t iwl_pcie_get_first_tb_dma(struct iwl_txq *txq, int idx)
{
	return iwl_iobuf_physical(txq->first_tb_bufs) +
	       (sizeof(struct iwl_pcie_first_tb_buf) * idx);
}

#ifdef CONFIG_IWLWIFI_DEBUGFS
/**
 * enum iwl_fw_mon_dbgfs_state - the different states of the monitor_data
 * debugfs file
 *
 * @IWL_FW_MON_DBGFS_STATE_CLOSED: the file is closed.
 * @IWL_FW_MON_DBGFS_STATE_OPEN: the file is open.
 * @IWL_FW_MON_DBGFS_STATE_DISABLED: the file is disabled, once this state is
 *  set the file can no longer be used.
 */
enum iwl_fw_mon_dbgfs_state {
	IWL_FW_MON_DBGFS_STATE_CLOSED,
	IWL_FW_MON_DBGFS_STATE_OPEN,
	IWL_FW_MON_DBGFS_STATE_DISABLED,
};
#endif

/**
 * enum iwl_shared_irq_flags - level of sharing for irq
 * @IWL_SHARED_IRQ_NON_RX: interrupt vector serves non rx causes.
 * @IWL_SHARED_IRQ_FIRST_RSS: interrupt vector serves first RSS queue.
 */
enum iwl_shared_irq_flags {
	IWL_SHARED_IRQ_NON_RX = BIT(0),
	IWL_SHARED_IRQ_FIRST_RSS = BIT(1),
};

/**
 * enum iwl_image_response_code - image response values
 * @IWL_IMAGE_RESP_DEF: the default value of the register
 * @IWL_IMAGE_RESP_SUCCESS: iml was read successfully
 * @IWL_IMAGE_RESP_FAIL: iml reading failed
 */
enum iwl_image_response_code {
	IWL_IMAGE_RESP_DEF = 0,
	IWL_IMAGE_RESP_SUCCESS = 1,
	IWL_IMAGE_RESP_FAIL = 2,
};

/**
 * struct cont_rec: continuous recording data structure
 * @prev_wr_ptr: the last address that was read in monitor_data
 *  debugfs file
 * @prev_wrap_cnt: the wrap count that was used during the last read in
 *  monitor_data debugfs file
 * @state: the state of monitor_data debugfs file as described
 *  in &iwl_fw_mon_dbgfs_state enum
 * @mutex: locked while reading from monitor_data debugfs file
 */
#ifdef CONFIG_IWLWIFI_DEBUGFS
struct cont_rec {
	u32 prev_wr_ptr;
	u32 prev_wrap_cnt;
	u8 state;
	/* Used to sync monitor_data debugfs file with driver unload flow */
	struct mutex mutex;
};
#endif

enum iwl_pcie_fw_reset_state {
	FW_RESET_IDLE,
	FW_RESET_REQUESTED,
	FW_RESET_OK,
	FW_RESET_ERROR,
};

/**
 * enum wl_pcie_imr_status - imr dma transfer state
 * @IMR_D2S_IDLE: default value of the dma transfer
 * @IMR_D2S_REQUESTED: dma transfer requested
 * @IMR_D2S_COMPLETED: dma transfer completed
 * @IMR_D2S_ERROR: dma transfer error
 */
enum iwl_pcie_imr_status {
	IMR_D2S_IDLE,
	IMR_D2S_REQUESTED,
	IMR_D2S_COMPLETED,
	IMR_D2S_ERROR,
};

/**
 * struct iwl_trans_pcie - PCIe transport specific data
 * @rxq: all the RX queue data
 * @rx_pool: initial pool of iwl_rx_mem_buffer for all the queues
 * @global_table: table mapping received VID from hw to rxb
 * @rba: allocator for RX replenishing
 * @ctxt_info: context information for FW self init
 * @ctxt_info_gen3: context information for gen3 devices
 * @prph_info: prph info for self init
 * @prph_scratch: prph scratch for self init
 * @ctxt_info_dma_addr: dma addr of context information
 * @prph_info_dma_addr: dma addr of prph info
 * @prph_scratch_dma_addr: dma addr of prph scratch
 * @ctxt_info_dma_addr: dma addr of context information
 * @init_dram: DRAM data of firmware image (including paging).
 *  Context information addresses will be taken from here.
 *  This is driver's local copy for keeping track of size and
 *  count for allocating and freeing the memory.
 * @iml: image loader image virtual address
 * @trans: pointer to the generic transport area
 * @scd_base_addr: scheduler sram base address in SRAM
 * @scd_bc_tbls: pointer to the byte count table of the scheduler
 * @kw: keep warm address
 * @pnvm_dram: DRAM area that contains the PNVM data
 * @pci_dev: basic pci-network driver stuff
 * @pci: PCI protocol
 * @mmio: PCI memory mapped IO
 * @ucode_write_complete: indicates that the ucode has been copied.
 * @ucode_write_waitq: wait queue for uCode load
 * @cmd_queue - command queue number
 * @def_rx_queue - default rx queue number
 * @rx_buf_size: Rx buffer size
 * @scd_set_active: should the transport configure the SCD for HCMD queue
 * @rx_page_order: page order for receive buffer size
 * @reg_lock: protect hw register access
 * @mutex: to protect stop_device / start_fw / start_hw
#ifdef CPTCFG_IWLWIFI_DEBUGFS
 * @fw_mon_data: fw continuous recording data
#endif
 * @msix_entries: array of MSI-X entries
 * @msix_enabled: true if managed to enable MSI-X
 * @shared_vec_mask: the type of causes the shared vector handles
 *  (see iwl_shared_irq_flags).
 * @alloc_vecs: the number of interrupt vectors allocated by the OS
 * @def_irq: default irq for non rx causes
 * @fh_init_mask: initial unmasked fh causes
 * @hw_init_mask: initial unmasked hw causes
 * @fh_mask: current unmasked fh causes
 * @hw_mask: current unmasked hw causes
 * @in_rescan: true if we have triggered a device rescan
 * @supported_dma_mask: DMA mask to validate the actual address against,
 *	will be DMA_BIT_MASK(11) or DMA_BIT_MASK(12) depending on the device
 */
struct iwl_trans_pcie {
	struct iwl_rxq *rxq;
	struct iwl_rx_mem_buffer *rx_pool;
	struct iwl_rx_mem_buffer **global_table;
	union {
		struct iwl_context_info *
			ctxt_info; // the virtual address of 'ctxt_info_iobuf' below.
		struct iwl_context_info_gen3 *ctxt_info_gen3;
	};
	struct iwl_iobuf *ctxt_info_iobuf;
	struct iwl_prph_info *prph_info;
	struct iwl_prph_scratch *prph_scratch;
	void *iml;
	dma_addr_t ctxt_info_dma_addr;
	dma_addr_t prph_info_dma_addr;
	dma_addr_t prph_scratch_dma_addr;
	dma_addr_t iml_dma_addr;
	struct iwl_self_init_dram init_dram;
	struct iwl_trans *trans;

	/* INT ICT Table */
	struct iwl_iobuf *ict_tbl;
	int ict_index;
	bool use_ict;
	bool is_down, opmode_down;
	s8 debug_rfkill;
	struct isr_statistics isr_stats;

	zx_handle_t irq_handle;
	thrd_t irq_thread;
	mtx_t irq_lock;
	mtx_t mutex;
	uint32_t inta_mask;
	uint32_t scd_base_addr;
	struct iwl_dma_ptr
		scd_bc_tbls; // TODO(b/299302635): remove me after uprev
	struct iwl_dma_ptr kw;

	struct iwl_dram_data pnvm_dram;
	struct iwl_dram_data reduce_power_dram;

	struct iwl_txq *txq_memory;
	// TODO(b/299302635): remove below queue fields since iwl_trans->txqs has them.
	struct iwl_txq *txq[IWL_MAX_TVQM_QUEUES];
	unsigned long queue_used[BITS_TO_LONGS(IWL_MAX_TVQM_QUEUES)];
	unsigned long queue_stopped[BITS_TO_LONGS(IWL_MAX_TVQM_QUEUES)];

	/* PCI bus related data */
	struct iwl_pci_dev *pci_dev;
	struct iwl_pci_fidl *pci;
	pci_interrupt_mode_t irq_mode;
	MMIO_PTR void *mmio_vaddr;

	bool ucode_write_complete;
	bool sx_complete;
	sync_completion_t ucode_write_waitq;
#if 0 // NEEDS_PORTING
    wait_queue_head_t d0i3_waitq;
#endif // NEEDS_PORTING

	uint8_t page_offs, dev_cmd_offs;
	sync_completion_t sx_waitq;

	uint8_t cmd_queue; // TODO(b/299302635): remove me
	uint8_t def_rx_queue;
	uint8_t cmd_fifo;
	unsigned int cmd_q_wdg_timeout;
	uint8_t n_no_reclaim_cmds;
	uint8_t no_reclaim_cmds[MAX_NO_RECLAIM_CMDS];
	uint8_t max_tbs;
	uint16_t tfd_size;
	u16 num_rx_bufs;

	enum iwl_amsdu_size rx_buf_size;
	bool scd_set_active;
	bool pcie_dbg_dumped_once;
	u32 rx_page_order;
	u32 rx_buf_bytes;
	u32 supported_dma_mask;

	/*protect hw register */
	mtx_t reg_lock;
	bool cmd_hold_nic_awake;

#ifdef CONFIG_IWLWIFI_DEBUGFS
	struct cont_rec fw_mon_data;
#endif

#if 0 // NEEDS_PORTING
    struct msix_entry msix_entries[IWL_MAX_RX_HW_QUEUES];
#endif // NEEDS_PORTING
	bool msix_enabled;
	u8 shared_vec_mask;
	u32 alloc_vecs;
	u32 def_irq;
	u32 fh_init_mask;
	u32 hw_init_mask;
	u32 fh_mask;
	u32 hw_mask;
#if 0 // NEEDS_PORTING
    cpumask_t affinity_mask[IWL_MAX_RX_HW_QUEUES];
#endif // NEEDS_PORTING
	bool in_rescan;

	struct iwl_iobuf *base_rb_stts;

	bool fw_reset_handshake;
	enum iwl_pcie_fw_reset_state fw_reset_state;
	sync_completion_t fw_reset_waitq;
	enum iwl_pcie_imr_status imr_status;
	sync_completion_t imr_waitq;
	char rf_name[32];
};

static inline struct iwl_trans_pcie *
IWL_TRANS_GET_PCIE_TRANS(struct iwl_trans *trans)
{
	return (struct iwl_trans_pcie *)trans->trans_specific;
}

static inline void iwl_pcie_clear_irq(struct iwl_trans *trans, int queue)
{
	/*
     * Before sending the interrupt the HW disables it to prevent
     * a nested interrupt. This is done by writing 1 to the corresponding
     * bit in the mask register. After handling the interrupt, it should be
     * re-enabled by clearing this bit. This register is defined as
     * write 1 clear (W1C) register, meaning that it's being clear
     * by writing 1 to the bit.
     */
	iwl_write32(trans, CSR_MSIX_AUTOMASK_ST_AD, (uint32_t)BIT(queue));
}

static inline struct iwl_trans *
iwl_trans_pcie_get_trans(struct iwl_trans_pcie *trans_pcie)
{
	return container_of((void *)trans_pcie, struct iwl_trans,
			    trans_specific);
}

/*
 * Convention: trans API functions: iwl_trans_pcie_XXX
 *  Other functions: iwl_pcie_XXX
 */
struct iwl_trans *
iwl_trans_pcie_alloc(struct iwl_pci_dev *pdev,
		     const struct iwl_pci_device_id *ent,
		     const struct iwl_cfg_trans_params *cfg_trans);
void iwl_trans_pcie_free(struct iwl_trans *trans);

bool __iwl_trans_pcie_grab_nic_access(struct iwl_trans *trans);
#define _iwl_trans_pcie_grab_nic_access(trans) \
	(__iwl_trans_pcie_grab_nic_access(trans))

/*****************************************************
 * RX
 ******************************************************/
zx_status_t iwl_pcie_rx_init(struct iwl_trans *trans);
int iwl_pcie_gen2_rx_init(struct iwl_trans *trans);
int iwl_pcie_irq_handler(void *arg);
zx_status_t iwl_pcie_isr(struct iwl_trans *trans);
#if 0 // NEEDS_PORTING
irqreturn_t iwl_pcie_msix_isr(int irq, void* data);
irqreturn_t iwl_pcie_irq_msix_handler(int irq, void* dev_id);
irqreturn_t iwl_pcie_irq_rx_msix_handler(int irq, void* dev_id);
#endif // NEEDS_PORTING
int iwl_pcie_rx_stop(struct iwl_trans *trans);
void iwl_pcie_rx_free(struct iwl_trans *trans);
void iwl_pcie_free_rbs_pool(struct iwl_trans *trans);
void iwl_pcie_rx_init_rxb_lists(struct iwl_rxq *rxq);
#if 0 // NEEDS_PORTING
void iwl_pcie_rxq_alloc_rbs(struct iwl_trans *trans, gfp_t priority,
			    struct iwl_rxq *rxq);
#endif // NEEDS_PORTING

/*****************************************************
 * ICT - interrupt handling
 ******************************************************/
zx_status_t iwl_pcie_alloc_ict(struct iwl_trans *trans);
void iwl_pcie_free_ict(struct iwl_trans *trans);
void iwl_pcie_reset_ict(struct iwl_trans *trans);
void iwl_pcie_disable_ict(struct iwl_trans *trans);

// Exposed for tests only.
uint32_t iwl_pcie_int_cause_ict(struct iwl_trans *trans);

/*****************************************************
 * TX / HCMD
 ******************************************************/
zx_status_t iwl_pcie_tx_init(struct iwl_trans *trans);
int iwl_pcie_gen2_tx_init(struct iwl_trans *trans, int txq_id, int queue_size);
void iwl_pcie_tx_start(struct iwl_trans *trans, uint32_t scd_base_addr);
zx_status_t iwl_pcie_tx_stop(struct iwl_trans *trans);
void iwl_pcie_tx_free(struct iwl_trans *trans);
bool iwl_trans_pcie_txq_enable(struct iwl_trans *trans, int queue, uint16_t ssn,
			       const struct iwl_trans_txq_scd_cfg *cfg,
			       zx_duration_t wdg_timeout);
void iwl_trans_pcie_txq_disable(struct iwl_trans *trans, int queue,
				bool configure_scd);
void iwl_trans_pcie_txq_set_shared_mode(struct iwl_trans *trans,
					uint32_t txq_id, bool shared_mode);
zx_status_t iwl_trans_pcie_tx(struct iwl_trans *trans,
			      struct ieee80211_mac_packet *pkt,
			      struct iwl_device_tx_cmd *dev_cmd, int txq_id);
void iwl_pcie_txq_check_wrptrs(struct iwl_trans *trans);
zx_status_t iwl_trans_pcie_send_hcmd(struct iwl_trans *trans,
				     struct iwl_host_cmd *cmd);
zx_status_t iwl_pcie_cmdq_reclaim(struct iwl_trans *trans, int txq_id,
				  uint32_t idx);
void iwl_pcie_gen2_txq_inc_wr_ptr(struct iwl_trans *trans, struct iwl_txq *txq);
void iwl_pcie_hcmd_complete(struct iwl_trans *trans,
			    struct iwl_rx_cmd_buffer *rxb);
void iwl_trans_pcie_reclaim(struct iwl_trans *trans, int txq_id, int ssn);
void iwl_trans_pcie_tx_reset(struct iwl_trans *trans);

static inline uint16_t iwl_pcie_tfd_tb_get_len(struct iwl_trans *trans,
					       void *_tfd, uint8_t idx)
{
	if (trans->trans_cfg->use_tfh) {
		struct iwl_tfh_tfd *tfd = (struct iwl_tfh_tfd *)_tfd;
		struct iwl_tfh_tb *tb = &tfd->tbs[idx];

		return le16_to_cpu(tb->tb_len);
	} else {
		struct iwl_tfd *tfd = (struct iwl_tfd *)_tfd;
		struct iwl_tfd_tb *tb = &tfd->tbs[idx];

		return le16_to_cpu(tb->hi_n_len) >> 4;
	}
}

/*****************************************************
 * Error handling
 ******************************************************/
void iwl_pcie_dump_csr(struct iwl_trans *trans);

/*****************************************************
 * Helpers
 ******************************************************/
static inline void _iwl_disable_interrupts(struct iwl_trans *trans)
{
	struct iwl_trans_pcie *trans_pcie = IWL_TRANS_GET_PCIE_TRANS(trans);

	clear_bit(STATUS_INT_ENABLED, &trans->status);
	if (!trans_pcie->msix_enabled) {
		/* disable interrupts from uCode/NIC to host */
		iwl_write32(trans, CSR_INT_MASK, 0x00000000);

		/* acknowledge/clear/reset any interrupts still pending
     * from uCode or flow handler (Rx/Tx DMA) */
		iwl_write32(trans, CSR_INT, 0xffffffff);
		iwl_write32(trans, CSR_FH_INT_STATUS, 0xffffffff);
	} else {
		/* disable all the interrupt we might use */
		iwl_write32(trans, CSR_MSIX_FH_INT_MASK_AD,
			    trans_pcie->fh_init_mask);
		iwl_write32(trans, CSR_MSIX_HW_INT_MASK_AD,
			    trans_pcie->hw_init_mask);
	}
	IWL_DEBUG_ISR(trans, "Disabled interrupts\n");
}

static inline int iwl_pcie_get_num_sections(const struct fw_img *fw, int start)
{
	int i = 0;

	while (start < fw->num_sec &&
	       fw->sec[start].offset != CPU1_CPU2_SEPARATOR_SECTION &&
	       fw->sec[start].offset != PAGING_SEPARATOR_SECTION) {
		start++;
		i++;
	}

	return i;
}

static inline void iwl_pcie_ctxt_info_free_fw_img(struct iwl_trans *trans)
{
	struct iwl_trans_pcie *trans_pcie = IWL_TRANS_GET_PCIE_TRANS(trans);
	struct iwl_self_init_dram *dram = &trans_pcie->init_dram;
	int i;

	if (!dram->fw) {
		if (dram->fw_cnt) {
			IWL_WARN(
				trans,
				"dram->fw not allocated, but dram->fw_cnt is non-zero (%d)\n",
				dram->fw_cnt);
		}
		return;
	}

	for (i = 0; i < dram->fw_cnt; i++) {
		iwl_iobuf_release(dram->fw[i].block);
	}

	kfree(dram->fw);
	dram->fw_cnt = 0;
	dram->fw = NULL;
}

static inline void iwl_disable_interrupts(struct iwl_trans *trans)
{
	struct iwl_trans_pcie *trans_pcie = IWL_TRANS_GET_PCIE_TRANS(trans);

	mtx_lock(&trans_pcie->irq_lock);
	_iwl_disable_interrupts(trans);
	mtx_unlock(&trans_pcie->irq_lock);
}

static inline void _iwl_enable_interrupts(struct iwl_trans *trans)
{
	struct iwl_trans_pcie *trans_pcie = IWL_TRANS_GET_PCIE_TRANS(trans);

	IWL_DEBUG_ISR(trans, "Enabling interrupts\n");
	set_bit(STATUS_INT_ENABLED, &trans->status);
	if (!trans_pcie->msix_enabled) {
		trans_pcie->inta_mask = CSR_INI_SET_MASK;
		iwl_write32(trans, CSR_INT_MASK, trans_pcie->inta_mask);
	} else {
		/*
     * fh/hw_mask keeps all the unmasked causes.
     * Unlike msi, in msix cause is enabled when it is unset.
     */
		trans_pcie->hw_mask = trans_pcie->hw_init_mask;
		trans_pcie->fh_mask = trans_pcie->fh_init_mask;
		iwl_write32(trans, CSR_MSIX_FH_INT_MASK_AD,
			    ~trans_pcie->fh_mask);
		iwl_write32(trans, CSR_MSIX_HW_INT_MASK_AD,
			    ~trans_pcie->hw_mask);
	}
}

static inline void iwl_enable_interrupts(struct iwl_trans *trans)
{
	struct iwl_trans_pcie *trans_pcie = IWL_TRANS_GET_PCIE_TRANS(trans);

	mtx_lock(&trans_pcie->irq_lock);
	_iwl_enable_interrupts(trans);
	mtx_unlock(&trans_pcie->irq_lock);
}

static inline void iwl_enable_hw_int_msk_msix(struct iwl_trans *trans,
					      uint32_t msk)
{
	struct iwl_trans_pcie *trans_pcie = IWL_TRANS_GET_PCIE_TRANS(trans);

	iwl_write32(trans, CSR_MSIX_HW_INT_MASK_AD, ~msk);
	trans_pcie->hw_mask = msk;
}

static inline void iwl_enable_fh_int_msk_msix(struct iwl_trans *trans,
					      uint32_t msk)
{
	struct iwl_trans_pcie *trans_pcie = IWL_TRANS_GET_PCIE_TRANS(trans);

	iwl_write32(trans, CSR_MSIX_FH_INT_MASK_AD, ~msk);
	trans_pcie->fh_mask = msk;
}

static inline void iwl_enable_fw_load_int(struct iwl_trans *trans)
{
	struct iwl_trans_pcie *trans_pcie = IWL_TRANS_GET_PCIE_TRANS(trans);

	IWL_DEBUG_ISR(trans, "Enabling FW load interrupt\n");
	if (!trans_pcie->msix_enabled) {
		trans_pcie->inta_mask = CSR_INT_BIT_FH_TX;
		iwl_write32(trans, CSR_INT_MASK, trans_pcie->inta_mask);
	} else {
		iwl_write32(trans, CSR_MSIX_HW_INT_MASK_AD,
			    trans_pcie->hw_init_mask);
		iwl_enable_fh_int_msk_msix(trans,
					   MSIX_FH_INT_CAUSES_D2S_CH0_NUM);
	}
}

static inline void iwl_enable_fw_load_int_ctx_info(struct iwl_trans *trans)
{
	struct iwl_trans_pcie *trans_pcie = IWL_TRANS_GET_PCIE_TRANS(trans);

	IWL_DEBUG_ISR(trans, "Enabling ALIVE interrupt only\n");

	if (!trans_pcie->msix_enabled) {
		/*
		 * When we'll receive the ALIVE interrupt, the ISR will call
		 * iwl_enable_fw_load_int_ctx_info again to set the ALIVE
		 * interrupt (which is not really needed anymore) but also the
		 * RX interrupt which will allow us to receive the ALIVE
		 * notification (which is Rx) and continue the flow.
		 */
		trans_pcie->inta_mask = CSR_INT_BIT_ALIVE | CSR_INT_BIT_FH_RX;
		iwl_write32(trans, CSR_INT_MASK, trans_pcie->inta_mask);
	} else {
		IWL_WARN(trans, "MSIX is NOT supported for ctx info\n");
#if 0 // NEEDS_PORTING
		iwl_enable_hw_int_msk_msix(trans,
					   MSIX_HW_INT_CAUSES_REG_ALIVE);
		/*
		 * Leave all the FH causes enabled to get the ALIVE
		 * notification.
		 */
		iwl_enable_fh_int_msk_msix(trans, trans_pcie->fh_init_mask);
#endif // NEEDS_PORTING
	}
}

static inline uint16_t iwl_pcie_get_cmd_index(const struct iwl_txq *q,
					      uint32_t index)
{
	return (uint16_t)(index & (q->n_window - 1));
}

static inline void *iwl_pcie_get_tfd(struct iwl_trans *trans,
				     struct iwl_txq *txq, int idx)
{
	struct iwl_trans_pcie *trans_pcie = IWL_TRANS_GET_PCIE_TRANS(trans);

	if (trans->trans_cfg->use_tfh) {
		idx = iwl_pcie_get_cmd_index(txq, idx);
	}

	char *ptr = (char *)iwl_iobuf_virtual(txq->tfds);
	return ptr + trans_pcie->tfd_size * idx;
}

#if 0 // NEEDS_PORTING
static inline const char* queue_name(struct device* dev, struct iwl_trans_pcie* trans_p, int i) {
    if (trans_p->shared_vec_mask) {
        int vec = trans_p->shared_vec_mask & IWL_SHARED_IRQ_FIRST_RSS ? 1 : 0;

        if (i == 0) {
            return DRV_NAME ": shared IRQ";
        }

        return devm_kasprintf(dev, GFP_KERNEL, DRV_NAME ": queue %d", i + vec);
    }
    if (i == 0) {
        return DRV_NAME ": default queue";
    }

    if (i == trans_p->alloc_vecs - 1) {
        return DRV_NAME ": exception";
    }

    return devm_kasprintf(dev, GFP_KERNEL, DRV_NAME ": queue %d", i);
}
#endif // NEEDS_PORTING

static inline void iwl_enable_rfkill_int(struct iwl_trans *trans)
{
	struct iwl_trans_pcie *trans_pcie = IWL_TRANS_GET_PCIE_TRANS(trans);

	IWL_DEBUG_ISR(trans, "Enabling rfkill interrupt\n");
	if (!trans_pcie->msix_enabled) {
		trans_pcie->inta_mask = CSR_INT_BIT_RF_KILL;
		iwl_write32(trans, CSR_INT_MASK, trans_pcie->inta_mask);
	} else {
		iwl_write32(trans, CSR_MSIX_FH_INT_MASK_AD,
			    trans_pcie->fh_init_mask);
		iwl_enable_hw_int_msk_msix(trans,
					   MSIX_HW_INT_CAUSES_REG_RF_KILL);
	}

	if (trans->trans_cfg->device_family == IWL_DEVICE_FAMILY_9000) {
		/*
     * On 9000-series devices this bit isn't enabled by default, so
     * when we power down the device we need set the bit to allow it
     * to wake up the PCI-E bus for RF-kill interrupts.
     */
		iwl_set_bit(trans, CSR_GP_CNTRL,
			    CSR_GP_CNTRL_REG_FLAG_RFKILL_WAKE_L1A_EN);
	}
}

void iwl_pcie_handle_rfkill_irq(struct iwl_trans *trans);

// Hardware Tx queue is full, stop queueing packets from MLME.
static inline void iwl_stop_queue(struct iwl_trans *trans, struct iwl_txq *txq)
{
	struct iwl_trans_pcie *trans_pcie = IWL_TRANS_GET_PCIE_TRANS(trans);

	if (!test_and_set_bit(txq->id, trans_pcie->queue_stopped)) {
		iwl_op_mode_queue_full(trans->op_mode, txq->id);
		IWL_DEBUG_TX_QUEUES(trans, "Stop hwq %d\n", txq->id);
	} else {
		IWL_DEBUG_TX_QUEUES(trans, "hwq %d already stopped\n", txq->id);
	}
}

static inline bool iwl_queue_used(const struct iwl_txq *q, int i)
{
	int index = iwl_pcie_get_cmd_index(q, i);
	int r = iwl_pcie_get_cmd_index(q, q->read_ptr);
	int w = iwl_pcie_get_cmd_index(q, q->write_ptr);

	return w >= r ? (index >= r && index < w) : !(index < r && index >= w);
}

static inline bool iwl_is_rfkill_set(struct iwl_trans *trans)
{
	struct iwl_trans_pcie *trans_pcie = IWL_TRANS_GET_PCIE_TRANS(trans);

	if (trans_pcie->debug_rfkill) {
		return true;
	}

	return !(iwl_read32(trans, CSR_GP_CNTRL) &
		 CSR_GP_CNTRL_REG_FLAG_HW_RF_KILL_SW);
}

static inline void __iwl_trans_pcie_set_bits_mask(struct iwl_trans *trans,
						  u32 reg, u32 mask, u32 value)
{
	u32 v;

#ifdef CONFIG_IWLWIFI_DEBUG
	WARN_ON_ONCE(value & ~mask);
#endif

	v = iwl_read32(trans, reg);
	v &= ~mask;
	v |= value;
	iwl_write32(trans, reg, v);
}

static inline void __iwl_trans_pcie_clear_bit(struct iwl_trans *trans, u32 reg,
					      u32 mask)
{
	__iwl_trans_pcie_set_bits_mask(trans, reg, mask, 0);
}

static inline void __iwl_trans_pcie_set_bit(struct iwl_trans *trans, u32 reg,
					    u32 mask)
{
	__iwl_trans_pcie_set_bits_mask(trans, reg, mask, mask);
}

static inline bool iwl_pcie_dbg_on(struct iwl_trans *trans)
{
	return (trans->dbg_dest_tlv || trans->ini_valid);
}

void iwl_trans_pcie_rf_kill(struct iwl_trans *trans, bool state);
void iwl_trans_pcie_dump_regs(struct iwl_trans *trans);

#ifdef CONFIG_IWLWIFI_DEBUGFS
void iwl_trans_pcie_dbgfs_register(struct iwl_trans *trans);
#else
static inline int iwl_trans_pcie_dbgfs_register(struct iwl_trans *trans)
{
	return 0;
}
#endif

void iwl_pcie_rx_allocator_work(struct work_struct *data);

/* common functions that are used by gen2 transport */
int iwl_pcie_gen2_apm_init(struct iwl_trans *trans);
void iwl_pcie_apm_config(struct iwl_trans *trans);
int iwl_pcie_prepare_card_hw(struct iwl_trans *trans);
void iwl_pcie_synchronize_irqs(struct iwl_trans *trans);
bool iwl_pcie_check_hw_rf_kill(struct iwl_trans *trans);
void iwl_trans_pcie_handle_stop_rfkill(struct iwl_trans *trans,
				       bool was_in_rfkill);
void iwl_pcie_txq_free_tfd(struct iwl_trans *trans, struct iwl_txq *txq);
int iwl_queue_space(struct iwl_trans *trans, const struct iwl_txq *q);
void iwl_pcie_apm_stop_master(struct iwl_trans *trans);
void iwl_pcie_conf_msix_hw(struct iwl_trans_pcie *trans_pcie);
zx_status_t iwl_pcie_txq_init(struct iwl_trans *trans, struct iwl_txq *txq,
			      uint16_t slots_num, bool cmd_queue);
zx_status_t iwl_pcie_txq_alloc(struct iwl_trans *trans, struct iwl_txq *txq,
			       uint16_t slots_num, bool cmd_queue);
void iwl_pcie_apply_destination(struct iwl_trans *trans);
void iwl_pcie_txq_unmap(struct iwl_trans *trans, int txq_id); // for testing

/* common functions that are used by gen3 transport */
void iwl_pcie_alloc_fw_monitor(struct iwl_trans *trans, uint8_t max_power);

/* This function is exposed for unit testing */
zx_status_t iwl_pcie_txq_build_tfd(struct iwl_trans *trans, struct iwl_txq *txq,
				   zx_paddr_t addr, uint16_t len, bool reset,
				   uint32_t *num_tbs);

/* transport gen 2 exported functions */
int iwl_trans_pcie_gen2_start_fw(struct iwl_trans *trans,
				 const struct fw_img *fw, bool run_in_rfkill);
void iwl_trans_pcie_gen2_fw_alive(struct iwl_trans *trans, u32 scd_addr);
int iwl_trans_pcie_gen2_send_hcmd(struct iwl_trans *trans,
				  struct iwl_host_cmd *cmd);
void iwl_trans_pcie_gen2_stop_device(struct iwl_trans *trans);
void _iwl_trans_pcie_gen2_stop_device(struct iwl_trans *trans);
void iwl_pcie_d3_complete_suspend(struct iwl_trans *trans, bool test,
				  bool reset);
int iwl_pcie_gen2_enqueue_hcmd(struct iwl_trans *trans,
			       struct iwl_host_cmd *cmd);
void iwl_trans_pcie_copy_imr_fh(struct iwl_trans *trans, u32 dst_addr,
				u64 src_addr, u32 byte_cnt);
int iwl_trans_pcie_copy_imr(struct iwl_trans *trans, u32 dst_addr, u64 src_addr,
			    u32 byte_cnt);

__END_CDECLS

#endif // SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_INTEL_IWLWIFI_PCIE_INTERNAL_H_

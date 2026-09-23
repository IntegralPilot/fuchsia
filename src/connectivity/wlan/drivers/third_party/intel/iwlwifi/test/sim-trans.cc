// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This file is to fake the PCIe transportation layer and the firmware
// in order to test upper layers of the iwlwifi driver.
//
// The simulated behaviors are implemented in the 'trans_ops_sim_trans',
// which is a 'struct iwl_trans_ops'.
//
// This file also calls device_add() to register 'wlan_phy_impl_protocol_ops_t'
// so that we can simulate the MLME (the user of this softmac driver) to test
// the iface functions.
//
// After iwl_sim_trans_transport_alloc() is called, memory containing iwl_trans +
// sim_trans_priv is returned. To access the simulated-transportation-specific
// variable, use IWL_TRANS_GET_SIM_TRANS(trans) to get it.

#include "third_party/iwlwifi/test/sim-trans.h"

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/fake-bti/bti.h>
#include <zircon/status.h>

#include "third_party/iwlwifi/platform/banjo/softmac.h"
#include "third_party/iwlwifi/platform/banjo/wlanphyimpl.h"

extern "C" {
#include "third_party/iwlwifi/fw/api/alive.h"
#include "third_party/iwlwifi/fw/api/commands.h"
#include "third_party/iwlwifi/iwl-config.h"
#include "third_party/iwlwifi/iwl-drv.h"
#include "third_party/iwlwifi/iwl-trans.h"
}

#include "third_party/iwlwifi/platform/mvm-mlme.h"
#include "third_party/iwlwifi/platform/rcu-manager.h"

using wlan::testing::IWL_TRANS_GET_SIM_TRANS;
using wlan::testing::sim_trans_priv;
using wlan::testing::SimMvm;

namespace wlan::iwlwifi {

SimTransIwlwifiDriver::SimTransIwlwifiDriver(iwl_trans* drvdata, async_dispatcher_t* dispatcher)
    : WlanPhyDevice(), drvdata_(drvdata) {
  // Get namespace entries.
  std::vector<fuchsia_component_runner::ComponentNamespaceEntry> entries;
  zx::result open_result = component::OpenServiceRoot();
  ZX_ASSERT_MSG(open_result.is_ok(), "Failed to open service root: %d", open_result.status_value());

  ::fidl::ClientEnd<::fuchsia_io::Directory> svc = std::move(*open_result);
  entries.emplace_back(fuchsia_component_runner::ComponentNamespaceEntry{{
      .path = "/svc",
      .directory = std::move(svc),
  }});

  // Create Namespace object from the entries.
  auto ns = fdf::Namespace::Create(entries);
  ZX_ASSERT_MSG(!ns.is_error(), "Create namespace failed: %d", ns.status_value());

  // Create driver::Logger with dispatcher and namespace.
  auto logger = fdf::Logger::Create2(*ns, fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                     "SimTransIwlwifiDriver loop", FUCHSIA_LOG_INFO);
  logger_ = std::move(logger);

  // Initialize the log instance with driver::Logger.
  wlan::drivers::log::Instance::Init(0, logger_.get());
}

SimTransIwlwifiDriver::~SimTransIwlwifiDriver() {
  struct iwl_trans* trans = drvdata_;
  if (trans->drv) {
    iwl_drv_stop(trans->drv);
  }
  free(trans);
  wlan::drivers::log::Instance::Reset();
}

iwl_trans* SimTransIwlwifiDriver::drvdata() { return drvdata_; }
const iwl_trans* SimTransIwlwifiDriver::drvdata() const { return drvdata_; }

zx_status_t SimTransIwlwifiDriver::AddWlansoftmacDevice(uint16_t iface_id,
                                                        struct iwl_mvm_vif* mvmvif) {
  softmac_device_count_++;
  mvmvif_ptrs_[iface_id] = mvmvif;
  return ZX_OK;
}

zx_status_t SimTransIwlwifiDriver::RemoveWlansoftmacDevice(uint16_t iface_id) {
  softmac_device_count_--;
  // Clean up this bit to enable subsequent iface manipulations.
  iwl_trans_get_mvm(drvdata_)->if_delete_in_progress = false;
  // In real case, this pointer is freed in mac_release() which is called from the destructor of
  // WlanSoftmacDevice. This test doesn't spawn real WlanSoftmacDevice instances
  free(mvmvif_ptrs_[iface_id]);
  return ZX_OK;
}

size_t SimTransIwlwifiDriver::DeviceCount() { return softmac_device_count_; }

}  // namespace wlan::iwlwifi

// Send a fake packet from FW to unblock one wait in mvm->notif_wait.
static void rx_fw_notification(struct iwl_trans* trans, uint8_t cmd, const void* data,
                               size_t size) {
  struct iwl_mvm* mvm = IWL_OP_MODE_GET_MVM(trans->op_mode);
  iwl_rx_packet pkt = {};
  pkt.len_n_flags = (size & FH_RSCSR_FRAME_SIZE_MSK) + sizeof(pkt.hdr);
  pkt.hdr.cmd = cmd;

  std::string buffer(sizeof(pkt) + size, '\0');
  std::memcpy(buffer.data(), &pkt, sizeof(pkt));
  std::memcpy(buffer.data() + sizeof(pkt), data, size);
  iwl_notification_wait_notify(&mvm->notif_wait, reinterpret_cast<iwl_rx_packet*>(buffer.data()));
}

// Notify the mvm->notif_wait to unblock the waiting.
static void unblock_notif_wait(struct iwl_trans* trans) {
  struct iwl_mvm* mvm = IWL_OP_MODE_GET_MVM(trans->op_mode);
  iwl_notification_notify(&mvm->notif_wait);
}

static zx_status_t iwl_sim_trans_start_hw(struct iwl_trans* iwl_trans, bool low_power) {
  return ZX_OK;
}

static void iwl_sim_trans_op_mode_leave(struct iwl_trans* iwl_trans) {}

static zx_status_t iwl_sim_trans_start_fw(struct iwl_trans* trans, const struct fw_img* fw,
                                          bool run_in_rfkill) {
  // Kick off the firmware.
  //
  // Since we don't have a real firmware to load, there will be no notification from the firmware.
  // Fake a RX packet's behavior so that we won't get blocked in the iwl_mvm_mac_start().
  mvm_alive_resp resp = {};
  resp.status = htole16(IWL_ALIVE_STATUS_OK);
  rx_fw_notification(trans, iwl_legacy_cmds::MVM_ALIVE, &resp, sizeof(resp));

  return ZX_OK;
}

static void iwl_sim_trans_fw_alive(struct iwl_trans* trans, uint32_t scd_addr) {}

static void iwl_sim_trans_stop_device(struct iwl_trans* trans) {}

static zx_status_t iwl_sim_trans_send_cmd(struct iwl_trans* trans, struct iwl_host_cmd* cmd) {
  bool notify_wait;
  zx_status_t ret = IWL_TRANS_GET_SIM_TRANS(trans)->fw->SendCmd(trans, cmd, &notify_wait);

  // On real hardware, some particular commands would reply a packet to unblock the wait.
  // However, in the simulated firmware, we don't generate the packet. We unblock it directly.
  if (notify_wait) {
    unblock_notif_wait(trans);
  }

  if (!(cmd->flags & CMD_ASYNC)) {
    // On real hardware, a SYNC command will be unblocked by iwl_pcie_hcmd_complete(), which is
    // called in RX ISR.  However, in the simulated environment, we don't generate interrupt
    // Thus, we need to unblock the SYNC command here.
    //
    // TODO(b/299301406): remove this code once the simulated interrupt is supported.
    sync_completion_signal(&trans->wait_command_queue);
    clear_bit(STATUS_SYNC_HCMD_ACTIVE, &trans->status);
  }

  return ret;
}

static zx_status_t iwl_sim_trans_tx(struct iwl_trans* trans, ieee80211_mac_packet* pkt,
                                    struct iwl_device_tx_cmd* dev_cmd, int queue) {
  return ZX_ERR_NOT_SUPPORTED;
}

static void iwl_sim_trans_reclaim(struct iwl_trans* trans, int queue, int ssn) {}

static bool iwl_sim_trans_txq_enable(struct iwl_trans* trans, int queue, uint16_t ssn,
                                     const struct iwl_trans_txq_scd_cfg* cfg,
                                     zx_duration_t queue_wdg_timeout) {
  return false;
}

static void iwl_sim_trans_txq_disable(struct iwl_trans* trans, int queue, bool configure_scd) {}

static void iwl_sim_trans_write8(struct iwl_trans* trans, uint32_t ofs, uint8_t val) {}

static void iwl_sim_trans_write32(struct iwl_trans* trans, uint32_t ofs, uint32_t val) {}

static uint32_t iwl_sim_trans_read32(struct iwl_trans* trans, uint32_t ofs) {
  return __INT32_MAX__;
}

static uint32_t iwl_sim_trans_read_prph(struct iwl_trans* trans, uint32_t ofs) {
  return __INT32_MAX__;
}

static void iwl_sim_trans_write_prph(struct iwl_trans* trans, uint32_t ofs, uint32_t val) {}

static zx_status_t iwl_sim_trans_read_mem(struct iwl_trans* trans, uint32_t addr, void* buf,
                                          size_t dwords) {
  return ZX_ERR_NOT_SUPPORTED;
}

static zx_status_t iwl_sim_trans_write_mem(struct iwl_trans* trans, uint32_t addr, const void* buf,
                                           size_t dwords) {
  return ZX_ERR_NOT_SUPPORTED;
}

static void iwl_sim_trans_configure(struct iwl_trans* trans,
                                    const struct iwl_trans_config* trans_cfg) {
  // Copy from iwl_trans_pcie_configure().
  trans->txqs.cmd.q_id = trans_cfg->cmd_queue;
  trans->txqs.cmd.fifo = trans_cfg->cmd_fifo;
  trans->txqs.cmd.wdg_timeout = trans_cfg->cmd_q_wdg_timeout;
  trans->txqs.page_offs = trans_cfg->cb_data_offs;
  trans->txqs.dev_cmd_offs = trans_cfg->cb_data_offs + sizeof(void*);
  trans->txqs.queue_alloc_cmd_ver = trans_cfg->queue_alloc_cmd_ver;

  trans->txqs.bc_table_dword = trans_cfg->bc_table_dword;

  trans->command_groups = trans_cfg->command_groups;
  trans->command_groups_size = trans_cfg->command_groups_size;
}

static void iwl_sim_trans_set_pmi(struct iwl_trans* trans, bool state) {}

static zx_status_t iwl_sim_trans_sw_reset(struct iwl_trans* trans, bool retake_ownership) {
  return ZX_OK;
}

static bool iwl_sim_trans_grab_nic_access(struct iwl_trans* trans) { return false; }

static void iwl_sim_trans_release_nic_access(struct iwl_trans* trans, unsigned long* flags) {}

static void iwl_sim_trans_set_bits_mask(struct iwl_trans* trans, uint32_t reg, uint32_t mask,
                                        uint32_t value) {}

static void iwl_sim_trans_ref(struct iwl_trans* trans) {}

static void iwl_sim_trans_unref(struct iwl_trans* trans) {}

static zx_status_t iwl_sim_trans_suspend(struct iwl_trans* trans) { return ZX_ERR_NOT_SUPPORTED; }

static void iwl_sim_trans_resume(struct iwl_trans* trans) {}

static zx_status_t iwl_sim_trans_wait_tx_queues_empty(struct iwl_trans* trans, uint32_t txq_bm) {
  return ZX_OK;
}

static zx_status_t iwl_sim_trans_wait_txq_empty(struct iwl_trans* trans, int queue) {
  return ZX_OK;
}

static struct iwl_trans_ops trans_ops_sim_trans = {
    .start_hw = iwl_sim_trans_start_hw,
    .op_mode_leave = iwl_sim_trans_op_mode_leave,
    .start_fw = iwl_sim_trans_start_fw,
    .fw_alive = iwl_sim_trans_fw_alive,
    .stop_device = iwl_sim_trans_stop_device,
    .send_cmd = iwl_sim_trans_send_cmd,
    .tx = iwl_sim_trans_tx,
    .reclaim = iwl_sim_trans_reclaim,
    .txq_enable = iwl_sim_trans_txq_enable,
    .txq_disable = iwl_sim_trans_txq_disable,
    .wait_tx_queues_empty = iwl_sim_trans_wait_tx_queues_empty,
    .wait_txq_empty = iwl_sim_trans_wait_txq_empty,
    .write8 = iwl_sim_trans_write8,
    .write32 = iwl_sim_trans_write32,
    .read32 = iwl_sim_trans_read32,
    .read_prph = iwl_sim_trans_read_prph,
    .write_prph = iwl_sim_trans_write_prph,
    .read_mem = iwl_sim_trans_read_mem,
    .write_mem = iwl_sim_trans_write_mem,
    .configure = iwl_sim_trans_configure,
    .set_pmi = iwl_sim_trans_set_pmi,
    .sw_reset = iwl_sim_trans_sw_reset,
    .grab_nic_access = iwl_sim_trans_grab_nic_access,
    .release_nic_access = iwl_sim_trans_release_nic_access,
    .set_bits_mask = iwl_sim_trans_set_bits_mask,
    .ref = iwl_sim_trans_ref,
    .unref = iwl_sim_trans_unref,
    .suspend = iwl_sim_trans_suspend,
    .resume = iwl_sim_trans_resume,

#if 0   // NEEDS_PORTING
    void (*d3_suspend)(struct iwl_trans* trans, bool test, bool reset);
    int (*d3_resume)(struct iwl_trans* trans, enum iwl_d3_status* status, bool test, bool reset);

    /* 22000 functions */
    int (*txq_alloc)(struct iwl_trans* trans, __le16 flags, uint8_t sta_id, uint8_t tid, int cmd_id,
                     int size, unsigned int queue_wdg_timeout);
    void (*txq_free)(struct iwl_trans* trans, int queue);
    int (*rxq_dma_data)(struct iwl_trans* trans, int queue, struct iwl_trans_rxq_dma_data* data);

    void (*txq_set_shared_mode)(struct iwl_trans* trans, uint32_t txq_id, bool shared);

    void (*freeze_txq_timer)(struct iwl_trans* trans, unsigned long txqs, bool freeze);
    void (*block_txq_ptrs)(struct iwl_trans* trans, bool block);

    struct iwl_trans_dump_data* (*dump_data)(struct iwl_trans* trans, uint32_t dump_mask);
    void (*debugfs_cleanup)(struct iwl_trans* trans);
#endif  // NEEDS_PORTING
};

// iwl_trans_alloc() will allocate memory containing iwl_trans + sim_trans_priv.
static struct iwl_trans* iwl_sim_trans_transport_alloc(struct device* dev,
                                                       const struct iwl_cfg_trans_params* trans_cfg,
                                                       SimMvm* fw) {
  struct iwl_trans* iwl_trans =
      iwl_trans_alloc(sizeof(struct sim_trans_priv), dev, &trans_ops_sim_trans, trans_cfg);

  IWL_TRANS_GET_SIM_TRANS(iwl_trans)->fw = fw;

  return iwl_trans;
}

// This function intends to be like this because we want to mimic the driver initialization process
// in PcieIwlwifiDriver::Init(). The goal is to allocate transportation layer resources and bind
// them together.
//
// We also determine the chip we want to simulate in this function. Currently we choose 7265 series
// chips.
//
// A 'struct iwl_trans' instance will returned in 'out_trans'.
//
static zx_status_t sim_transport_bind(
    SimMvm* fw, struct device* dev, struct iwl_trans** out_iwl_trans,
    std::unique_ptr<wlan::iwlwifi::SimTransIwlwifiDriver>* out_device,
    async_dispatcher_t* driver_dispatcher) {
  zx_status_t status = ZX_OK;
  const struct iwl_cfg* cfg = &iwl7265_2ac_cfg;
  const struct iwl_cfg_trans_params* trans_cfg = (const struct iwl_cfg_trans_params*)cfg;
  // We can force the cast above because the 'trans_cfg' is not used when 'cfg' is assigned.

  struct iwl_trans* iwl_trans = iwl_sim_trans_transport_alloc(dev, trans_cfg, fw);
  if (!iwl_trans) {
    return ZX_ERR_INTERNAL;
  }
  iwl_trans->cfg = cfg;  // Assign the lagecy 'cfg' pointer. The production code assigns this value
                         // iwl_pci_probe(). However, the unittest code doesn't have that code path.
  ZX_ASSERT(out_iwl_trans);

  std::unique_ptr<wlan::iwlwifi::SimTransIwlwifiDriver> device;
  libsync::Completion create_device;
  async::PostTask(driver_dispatcher, [&]() {
    device = std::make_unique<wlan::iwlwifi::SimTransIwlwifiDriver>(iwl_trans, driver_dispatcher);
    create_device.Signal();
  });
  create_device.Wait();

  status = iwl_drv_init();
  ZX_ASSERT_MSG(status == ZX_OK, "Failed to init driver: %s", zx_status_get_string(status));

  status = iwl_drv_start(iwl_trans, &iwl_trans->drv);
  ZX_ASSERT_MSG(status == ZX_OK, "Failed to start driver: %s", zx_status_get_string(status));

  status = iwl_mvm_mac_start(IWL_OP_MODE_GET_MVM(iwl_trans->op_mode));
  ZX_ASSERT_MSG(status == ZX_OK, "Failed to start mac: %s", zx_status_get_string(status));

  *out_iwl_trans = iwl_trans;
  *out_device = std::move(device);

  return ZX_OK;
}

namespace wlan::testing {

zx_status_t sim_load_firmware_callback_entry(void* ctx, const char* name, zx_handle_t* vmo,
                                             size_t* size) {
  return static_cast<SimTransport*>(ctx)->LoadFirmware(name, vmo, size);
}

SimTransport::SimTransport() : device_{}, iwl_trans_(nullptr) {
  task_loop_ = std::make_unique<::async::Loop>(&kAsyncLoopConfigNoAttachToCurrentThread);
  task_loop_->StartThread("iwlwifi-test-task-worker", nullptr);
  irq_loop_ = std::make_unique<::async::Loop>(&kAsyncLoopConfigNoAttachToCurrentThread);
  irq_loop_->StartThread("iwlwifi-test-irq-worker", nullptr);
  rcu_manager_ =
      std::make_unique<::wlan::iwlwifi::RcuManager>(sim_driver_dispatcher_->async_dispatcher());

  device_.load_firmware_ctx = (void*)this;
  device_.load_firmware_callback = &sim_load_firmware_callback_entry;
  device_.task_dispatcher = task_loop_->dispatcher();
  device_.irq_dispatcher = irq_loop_->dispatcher();
  device_.rcu_manager = static_cast<struct rcu_manager*>(rcu_manager_.get());
  fake_bti_create(&device_.bti);
}

SimTransport::~SimTransport() {
  irq_loop_->Shutdown();
  task_loop_->Shutdown();
  zx_handle_close(device_.bti);
  libsync::Completion destruct_driver;
  async::PostTask(sim_driver_dispatcher_->async_dispatcher(), [&]() {
    sim_driver_.reset();
    destruct_driver.Signal();
  });
  destruct_driver.Wait();
}

zx_status_t SimTransport::Init() {
  return sim_transport_bind(this, &device_, &iwl_trans_, &sim_driver_,
                            sim_driver_dispatcher_->async_dispatcher());
}

void SimTransport::SetFirmware(std::string firmware) {
  fake_firmware_ = std::vector<uint8_t>(firmware.begin(), firmware.end());
}

zx_status_t SimTransport::LoadFirmware(const char* name, zx_handle_t* fw, size_t* size) {
  zx_status_t status = ZX_OK;
  zx_handle_t vmo = ZX_HANDLE_INVALID;
  if ((status = zx_vmo_create(fake_firmware_.size(), 0, &vmo)) != ZX_OK) {
    return status;
  }
  if ((status = zx_vmo_write(vmo, fake_firmware_.data(), 0, fake_firmware_.size())) != ZX_OK) {
    return status;
  }

  *fw = vmo;
  *size = fake_firmware_.size();
  return ZX_OK;
}

struct iwl_trans* SimTransport::iwl_trans() { return iwl_trans_; }

const struct iwl_trans* SimTransport::iwl_trans() const { return iwl_trans_; }

::wlan::iwlwifi::SimTransIwlwifiDriver* SimTransport::sim_driver() { return sim_driver_.get(); }

const ::wlan::iwlwifi::SimTransIwlwifiDriver* SimTransport::sim_driver() const {
  return sim_driver_.get();
}

fdf::UnownedSynchronizedDispatcher SimTransport::get_unowned_synchronized_dispatcher() {
  return runtime_.StartBackgroundDispatcher();
}

async_dispatcher_t* SimTransport::async_driver_dispatcher() {
  return sim_driver_dispatcher_->async_dispatcher();
}

fdf_dispatcher_t* SimTransport::fdf_driver_dispatcher() { return sim_driver_dispatcher_->get(); }

}  // namespace wlan::testing

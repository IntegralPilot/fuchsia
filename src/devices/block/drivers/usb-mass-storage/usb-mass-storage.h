// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_USB_MASS_STORAGE_USB_MASS_STORAGE_H_
#define SRC_DEVICES_BLOCK_DRIVERS_USB_MASS_STORAGE_USB_MASS_STORAGE_H_

#include <fuchsia/hardware/usb/c/banjo.h>
#include <inttypes.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/fit/function.h>
#include <lib/fzl/vmo-mapper.h>
#include <lib/scsi/block-device.h>
#include <lib/scsi/controller.h>
#include <lib/sync/completion.h>
#include <lib/sync/cpp/completion.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <zircon/assert.h>

#include <atomic>
#include <deque>
#include <memory>
#include <mutex>
#include <span>

#include <fbl/array.h>
#include <fbl/condition_variable.h>
#include <fbl/ref_counted.h>
#include <fbl/ref_ptr.h>
#include <usb/ums.h>
#include <usb/usb-request.h>
#include <usb/usb.h>

namespace ums {

// Abstract waiter class for waiting on a sync_completion_t.
// This is necessary to allow injection of a timer by a test
// into the UsbMassStorageDevice class, allowing for a simulated clock.
class WaiterInterface : public fbl::RefCounted<WaiterInterface> {
 public:
  virtual zx_status_t Wait(sync_completion_t* completion, zx_duration_t duration) = 0;
  virtual ~WaiterInterface() = default;
};

// struct representing a pending block request for a logical unit
struct Transaction {
  scsi::ScsiRequest request;
  uint8_t lun;
};

struct UsbRequestContext {
  usb_request_complete_callback_t completion;
};

class UsbMassStorageDevice : public fdf::DriverBase2, public scsi::Controller {
 public:
  static constexpr char kDriverName[] = "ums";

  explicit UsbMassStorageDevice() : fdf::DriverBase2(kDriverName) {}
  ~UsbMassStorageDevice() override = default;

  zx::result<> Start(fdf::DriverContext context) override;

  void Stop(fdf::StopCompleter completer) override;

  // scsi::Controller
  fidl::WireSyncClient<fuchsia_driver_framework::Node>& root_node() override { return root_node_; }
  std::string_view driver_name() const override { return name(); }
  const std::shared_ptr<fdf::Namespace>& driver_incoming() const override { return incoming_; }
  std::shared_ptr<fdf::OutgoingDirectory>& driver_outgoing() override { return outgoing(); }
  const std::optional<std::string>& driver_node_name() const override { return node_name_; }
  fdf::Logger& driver_logger() override { return logger(); }
  zx_status_t ExecuteCommandSync(uint8_t target, uint16_t lun, iovec cdb, bool is_write,
                                 iovec data) override;
  void ExecuteCommandsAsync(uint8_t target, uint16_t lun,
                            std::span<scsi::ScsiRequest> batch) override;

  // Performs the object initialization.
  zx_status_t Init();

  // Visible for testing.
  const std::vector<std::unique_ptr<scsi::BlockDevice>>& block_devs() const
      TA_NO_THREAD_SAFETY_ANALYSIS {
    return block_devs_;
  }
  size_t queued_txns_count() {
    std::lock_guard<std::mutex> l(queue_lock_);
    return queued_txns_.size();
  }
  bool is_dead() const { return dead_.load(); }
  zx_status_t CheckLunsReady();
  void set_on_pre_shutdown(fit::function<void(uint8_t lun)> on_pre_shutdown) {
    on_pre_shutdown_ = std::move(on_pre_shutdown);
  }

  DISALLOW_COPY_ASSIGN_AND_MOVE(UsbMassStorageDevice);

 protected:
  const std::shared_ptr<fdf::Namespace>& incoming() const { return incoming_; }
  void set_waiter(fbl::RefPtr<WaiterInterface> waiter) { waiter_ = std::move(waiter); }

 private:
  zx_status_t Reset();

  // Sends a Command Block Wrapper (command portion of request)
  // to a USB mass storage device.
  zx_status_t SendCbw(uint8_t lun, uint32_t transfer_length, uint8_t flags, uint8_t command_len,
                      const void* command);

  // Reads a Command Status Wrapper from a USB mass storage device
  // and validates that the command index in the response matches the index
  // in the previous request.
  zx_status_t ReadCsw(uint32_t* out_residue, bool retry = false);

  // Validates the command index and signature of a command status wrapper.
  csw_status_t VerifyCsw(usb_request_t* csw_request, uint32_t* out_residue);

  zx_status_t ReadSync(size_t transfer_length);

  zx_status_t DataTransfer(zx_handle_t vmo_handle, zx_off_t offset, size_t length,
                           uint8_t ep_address);

  zx_status_t DoTransaction(scsi::ScsiRequest& req, uint8_t lun, std::string_view action);

  void WorkerLoop();

  void RequestQueue(usb_request_t* request, const usb_request_complete_callback_t* completion);

  zx::result<> AllocatePages(zx::vmo& vmo, fzl::VmoMapper& mapper, size_t size);

  usb::UsbDevice usb_;

  uint32_t tag_send_;  // next tag to send in CBW

  uint32_t tag_receive_;  // next tag we expect to receive in CSW

  uint8_t max_lun_;  // index of last logical unit

  uint32_t max_transfer_bytes_;  // maximum transfer size reported by usb_get_max_transfer_size()

  uint8_t interface_number_;

  // Dispatcher for processing queued block requests.
  fdf::Dispatcher worker_dispatcher_;
  // Signaled when worker_dispatcher_ is shut down.
  libsync::Completion worker_shutdown_completion_;

  uint8_t bulk_in_addr_;

  uint8_t bulk_out_addr_;

  size_t bulk_in_max_packet_;

  size_t bulk_out_max_packet_;

  usb_request_t* cbw_req_;

  usb_request_t* data_req_;

  usb_request_t* csw_req_;

  usb_request_t* data_transfer_req_;  // for use in DataTransfer

  size_t parent_req_size_;

  std::atomic_size_t pending_requests_ = 0;

  fbl::RefPtr<WaiterInterface> waiter_;

  std::atomic_bool dead_ = false;

  // list of queued transactions
  std::deque<Transaction> queued_txns_ TA_GUARDED(queue_lock_);

  // Per-LUN flag indicating whether new requests for this LUN should be failed immediately
  // (e.g. while the LUN is shutting down or being torn down).
  std::vector<bool> fail_new_requests_ TA_GUARDED(queue_lock_);

  sync_completion_t txn_completion_;  // signals WorkerLoop when new txns are available
                                      // and when device is dead
  std::mutex queue_lock_;
  std::mutex txn_lock_;   // Synchronizes RequestQueue completion.
  std::mutex luns_lock_;  // Synchronizes LUN lifecycle and guards block_devs_ against WorkerLoop.

  fit::function<void(uint8_t lun)> on_pre_shutdown_;

  std::shared_ptr<fdf::Namespace> incoming_;
  std::optional<std::string> node_name_;
  fidl::WireSyncClient<fuchsia_driver_framework::Node> parent_node_;
  fidl::WireSyncClient<fuchsia_driver_framework::Node> root_node_;
  fidl::WireSyncClient<fuchsia_driver_framework::NodeController> node_controller_;

  std::vector<std::unique_ptr<scsi::BlockDevice>> block_devs_ TA_GUARDED(luns_lock_);
};

}  // namespace ums

#endif  // SRC_DEVICES_BLOCK_DRIVERS_USB_MASS_STORAGE_USB_MASS_STORAGE_H_

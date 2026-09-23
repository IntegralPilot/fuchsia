// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_VIRTIO_SCSI_H_
#define SRC_DEVICES_BLOCK_DRIVERS_VIRTIO_SCSI_H_

#include <lib/dma-buffer/buffer.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/fit/function.h>
#include <lib/fzl/vmo-mapper.h>
#include <lib/scsi/block-device.h>
#include <lib/scsi/controller.h>
#include <lib/sync/completion.h>
#include <lib/virtio/backends/backend.h>
#include <lib/virtio/device.h>
#include <lib/virtio/ring.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <stdlib.h>
#include <sys/uio.h>
#include <zircon/compiler.h>

#include <atomic>
#include <memory>
#include <optional>
#include <vector>

#include <fbl/auto_lock.h>
#include <fbl/condition_variable.h>
#include <virtio/scsi.h>

namespace virtio {

constexpr int MAX_IOS = 16;

class ScsiDriver;

class ScsiDevice : public virtio::Device {
 public:
  enum Queue {
    CONTROL = 0,
    EVENT = 1,
    REQUEST = 2,
  };

  ScsiDevice(ScsiDriver* scsi_driver, zx::bti bti, std::unique_ptr<Backend> backend)
      : virtio::Device(std::move(bti), std::move(backend)), scsi_driver_(scsi_driver) {}
  ~ScsiDevice() override;

  // virtio::Device overrides
  zx_status_t Init() override;
  void Release() override;
  // Invoked for most device interrupts.
  void IrqRingUpdate() override;
  // Invoked on config change interrupts.
  void IrqConfigChange() override {}
  const char* tag() const override { return "virtio-scsi"; }

  static constexpr uint32_t kMaxXferSectors = 1024;  // 512K clamp

  static void FillLUNStructure(struct virtio_scsi_req_cmd* req, uint8_t target, uint16_t lun);

  void QueueCommand(uint8_t target, uint16_t lun, iovec cdb, bool is_write,
                    zx::unowned_vmo data_vmo, zx_off_t vmo_offset_bytes, size_t transfer_bytes,
                    fit::callback<void(zx_status_t)> cb, void* data, bool vmar_mapped,
                    std::optional<zx::vmo> trim_data_vmo = std::nullopt);

  zx::result<> AllocatePages(zx::vmo& vmo, fzl::VmoMapper& mapper, size_t size);

  zx_status_t ProbeLuns();

  uint32_t active_ios() {
    fbl::AutoLock lock(&lock_);
    return active_ios_;
  }
  void* GetIOForTesting();

 private:
  ScsiDriver* const scsi_driver_;

  // Latched copy of virtio-scsi device configuration.
  struct virtio_scsi_config config_ TA_GUARDED(lock_) = {};

  struct scsi_io_slot {
    zx::unowned_vmo data_vmo;
    zx_off_t vmo_offset_bytes;
    size_t transfer_bytes;
    bool is_write;
    void* data;
    bool vmar_mapped;
    std::unique_ptr<dma_buffer::ContiguousBuffer> request_buffer;
    bool avail;
    vring_desc* tail_desc;
    fit::callback<void(zx_status_t)> callback;
    void* data_in_region;
    dma_buffer::ContiguousBuffer* request_buffers;
    struct virtio_scsi_resp_cmd* response;
    // Sustains the lifetime of the trim data while it is being used.
    std::optional<zx::vmo> trim_data_vmo;
  };
  scsi_io_slot* GetIO() TA_REQ(lock_);
  void FreeIO(scsi_io_slot* io_slot) TA_REQ(lock_);
  std::vector<fit::callback<void(zx_status_t)>> CleanUpPendingTxns() TA_REQ(lock_);
  bool irq_thread_started_ TA_GUARDED(lock_) = false;
  bool released_ TA_GUARDED(lock_) = false;
  size_t request_buffers_size_;
  scsi_io_slot scsi_io_slot_table_[MAX_IOS] TA_GUARDED(lock_) = {};

  Ring control_ring_ TA_GUARDED(lock_){this};
  Ring request_queue_{this};

  // Synchronizes virtio rings and worker thread control.
  fbl::Mutex lock_;

  // We use the condvar to control the number of IO's in flight
  // as well as to wait for descs to become available.
  fbl::ConditionVariable ioslot_cv_ __TA_GUARDED(lock_);
  fbl::ConditionVariable desc_cv_ __TA_GUARDED(lock_);
  uint32_t active_ios_ __TA_GUARDED(lock_);
  uint64_t scsi_transport_tag_ __TA_GUARDED(lock_);
};

class ScsiDriver : public fdf::DriverBase2, public scsi::Controller {
 public:
  static constexpr char kDriverName[] = "virtio-scsi";

  explicit ScsiDriver() : fdf::DriverBase2(kDriverName) {}

  zx::result<> Start(fdf::DriverContext context) override;

  void Stop(fdf::StopCompleter completer) override;

  // scsi::Controller overrides
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

 protected:
  void set_scsi_device(std::unique_ptr<ScsiDevice> device) { scsi_device_ = std::move(device); }
  ScsiDevice* scsi_device() { return scsi_device_.get(); }

 private:
  std::unique_ptr<ScsiDevice> scsi_device_;

  std::shared_ptr<fdf::Namespace> incoming_;
  std::optional<std::string> node_name_;

  fidl::WireSyncClient<fuchsia_driver_framework::Node> parent_node_;
  fidl::WireSyncClient<fuchsia_driver_framework::Node> root_node_;
  fidl::WireSyncClient<fuchsia_driver_framework::NodeController> node_controller_;
};

}  // namespace virtio

#endif  // SRC_DEVICES_BLOCK_DRIVERS_VIRTIO_SCSI_H_

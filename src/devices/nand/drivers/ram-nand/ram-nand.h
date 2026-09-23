// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_NAND_DRIVERS_RAM_NAND_RAM_NAND_H_
#define SRC_DEVICES_NAND_DRIVERS_RAM_NAND_RAM_NAND_H_

#include <fidl/fuchsia.boot.metadata/cpp/fidl.h>
#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <fidl/fuchsia.hardware.nand/cpp/fidl.h>
#include <fuchsia/hardware/nand/c/banjo.h>
#include <fuchsia/hardware/nand/cpp/banjo.h>
#include <lib/async/cpp/wait.h>
#include <lib/driver/compat/cpp/banjo_server.h>
#include <lib/driver/compat/cpp/device_server.h>
#include <lib/driver/devfs/cpp/connector.h>
#include <lib/driver/metadata/cpp/metadata_server.h>
#include <lib/fzl/vmo-mapper.h>
#include <lib/sync/completion.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <lib/zx/vmo.h>
#include <limits.h>
#include <threads.h>
#include <zircon/listnode.h>
#include <zircon/types.h>

#include <optional>

#include <fbl/array.h>
#include <fbl/macros.h>
#include <fbl/mutex.h>

namespace ram_nand {

// Wrapper for nand_info_t. It simplifies initialization of NandDevice.
struct NandParams : public nand_info_t {
  NandParams() : NandParams(0, 0, 0, 0, 0) {}

  NandParams(uint32_t page_size, uint32_t pages_per_block, uint32_t num_blocks, uint32_t ecc_bits,
             uint32_t oob_size)
      : NandParams(nand_info_t{page_size,
                               pages_per_block,
                               num_blocks,
                               ecc_bits,
                               oob_size,
                               static_cast<nand_class_t>(fuchsia_hardware_nand::wire::Class::kFtl),
                               {}}) {}

  explicit NandParams(const nand_info_t& base) {
    // NandParams has no data members.
    *this = *reinterpret_cast<const NandParams*>(&base);
  }

  uint64_t GetSize() const { return static_cast<uint64_t>(page_size + oob_size) * NumPages(); }

  uint32_t NumPages() const { return pages_per_block * num_blocks; }
};

// Provides the bulk of the functionality for a ram-backed NAND device.
class NandDevice : public ddk::NandProtocol<NandDevice>,
                   public fidl::WireServer<fuchsia_hardware_nand::RamNand> {
 public:
  using Id = size_t;

  // Called after responding to an Unlink FIDL request.
  using OnUnlink = fit::callback<void()>;

  explicit NandDevice(const NandParams& params, async_dispatcher_t* dispatcher, Id id,
                      OnUnlink on_unlink)
      : params_(params),
        on_unlink_(std::move(on_unlink)),
        device_name_(std::format("ram-nand-{}", id)),
        banjo_server_(ZX_PROTOCOL_NAND, this, &nand_protocol_ops_),
        dispatcher_(dispatcher) {}
  ~NandDevice() override;

  // Perform object initialization, and return a unique name of this device.
  // If `vmo` is not provided or is not valid, the device will create its own buffer, and
  // initialize it to be empty (all 1s).
  zx::result<std::string> Init(fuchsia_hardware_nand::wire::RamNandInfo& info,
                               fidl::UnownedClientEnd<fuchsia_driver_framework::Node> parent,
                               const std::shared_ptr<fdf::Namespace>& incoming,
                               const std::shared_ptr<fdf::OutgoingDirectory>& outgoing,
                               const std::optional<std::string>& node_name);

  // fidl::WireServer<fuchsia_hardware_nand::RamNand> implementation.
  void Unlink(UnlinkCompleter::Sync& completer) override;

  // NAND protocol implementation.
  void NandQuery(nand_info_t* info_out, size_t* nand_op_size_out);
  void NandQueue(nand_operation_t* operation, nand_queue_callback completion_cb, void* cookie);
  zx_status_t NandGetFactoryBadBlockList(uint32_t* bad_blocks, size_t bad_block_len,
                                         size_t* num_bad_blocks);

 private:
  struct RamNandOp {
    nand_operation_t* op;
    nand_queue_callback completion_cb;
    void* cookie;

    // ftl::NandOperation::CreateOperation() requires that the size of the nand operation be at
    // least `sizeof(nand_operation_t)`.
    uint8_t padding_[24];
  };
  static_assert(sizeof(RamNandOp) >= sizeof(nand_operation_t));

  void OnChildNodeClosed(async_dispatcher_t* dispatcher, async::WaitBase* wait, zx_status_t status,
                         const zx_packet_signal_t* signal);
  void DevfsConnect(fidl::ServerEnd<fuchsia_hardware_nand::RamNand> server);

  zx::result<> AddPendingOperation(nand_operation_t* operation, nand_queue_callback completion_cb,
                                   void* cookie);
  void PerformOperations();
  std::pair<bool, std::vector<RamNandOp>> TakePendingOperations();
  void PerformOperation(RamNandOp& operation);
  uint64_t MainDataSize() const {
    return static_cast<uint64_t>(params_.NumPages()) * params_.page_size;
  }

  // Implementation of the actual commands.
  zx_status_t ReadWriteData(nand_operation_t* operation, bool bytes);
  zx_status_t ReadWriteOob(nand_operation_t* operation);
  zx_status_t Erase(nand_operation_t* operation);

  uintptr_t mapped_addr_ = 0;
  zx::vmo vmo_;

  // The mapping for the wear info vmo. May be empty, check for nullptr before use.
  fzl::VmoMapper wear_info_;

  NandParams params_;

  fbl::Mutex lock_;

  // Use `AddPendingOperation()` and `TakePendingOperation()` in order to manipulate
  // `pending_operations_`.
  std::vector<RamNandOp> pending_operations_ TA_GUARDED(lock_);

  bool dead_ TA_GUARDED(lock_) = false;

  // Triggered when operations have been added to `pending_operations_` or when `dead_` is set to
  // true.
  sync_completion_t operations_pending_;

  std::optional<std::thread> operation_performer_;

  // If non-zero, the driver will fail writes once the write-count reaches this value.
  uint64_t fail_after_ = 0;

  // The number of bytes written.
  uint64_t write_count_ = 0;

  // Called after responding to an Unlink FIDL request.
  OnUnlink on_unlink_;

  // Name of the ram-nand device.
  std::string device_name_;

  compat::BanjoServer banjo_server_;
  compat::SyncInitializedDeviceServer compat_server_;
  driver_devfs::Connector<fuchsia_hardware_nand::RamNand> devfs_connector_{
      fit::bind_member<&NandDevice::DevfsConnect>(this)};
  fidl::ClientEnd<fuchsia_driver_framework::NodeController> child_;
  async::WaitMethod<NandDevice, &NandDevice::OnChildNodeClosed> child_node_wait_{this};
  std::optional<UnlinkCompleter::Async> unlink_completer_;

  async_dispatcher_t* dispatcher_;

  fidl::ServerBindingGroup<fuchsia_hardware_nand::RamNand> bindings_;
  fdf_metadata::MetadataServer<fuchsia_boot_metadata::PartitionMap> partition_map_metadata_server_{
      device_name_};

  DISALLOW_COPY_ASSIGN_AND_MOVE(NandDevice);
};

}  // namespace ram_nand

#endif  // SRC_DEVICES_NAND_DRIVERS_RAM_NAND_RAM_NAND_H_

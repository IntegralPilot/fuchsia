// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_NVME_NVME_H_
#define SRC_DEVICES_BLOCK_DRIVERS_NVME_NVME_H_

#include <lib/device-protocol/pci.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/inspect/component/cpp/component.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/sync/completion.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <threads.h>
#include <zircon/listnode.h>
#include <zircon/types.h>

#include <mutex>

#include "src/devices/block/drivers/nvme/commands.h"
#include "src/devices/block/drivers/nvme/io-command.h"
#include "src/devices/block/drivers/nvme/namespace.h"
#include "src/devices/block/drivers/nvme/queue-pair.h"
#include "src/devices/block/drivers/nvme/registers.h"

namespace fake_nvme {
class FakeController;
}

namespace nvme {

class Nvme : public fdf::DriverBase2 {
 public:
  static constexpr char kDriverName[] = "nvme";

  explicit Nvme() : fdf::DriverBase2(kDriverName) {}
  ~Nvme() override;

  zx::result<> Start(fdf::DriverContext context) override;

  void Stop(fdf::StopCompleter completer) override;

  // Perform an admin command synchronously (i.e., blocks for the command to complete or timeout).
  // Returns the command completion.
  zx::result<Completion> DoAdminCommandSync(
      Submission& submission, std::optional<zx::unowned_vmo> admin_data = std::nullopt);

  // Queue an IO command to be performed asynchronously.
  void QueueIoCommand(IoCommand* io_cmd);

  inspect::Inspector& inspect() { return component_inspector_->inspector(); }
  inspect::Node& inspect_node() { return inspect_node_; }

  QueuePair* io_queue() const { return io_queue_.get(); }
  uint32_t max_data_transfer_bytes() const { return max_data_transfer_bytes_; }
  bool volatile_write_cache_enabled() const { return volatile_write_cache_enabled_; }
  uint16_t atomic_write_unit_normal() const { return atomic_write_unit_normal_; }
  uint16_t atomic_write_unit_power_fail() const { return atomic_write_unit_power_fail_; }

  std::vector<std::unique_ptr<nvme::Namespace>>& namespaces() { return namespaces_; }

  // Called by children device of this controller for invoking AddChild() or instantiating
  // compat::DeviceServer.
  fidl::WireSyncClient<fuchsia_driver_framework::Node>& root_node() { return root_node_; }
  std::string_view driver_name() const { return name(); }
  const std::shared_ptr<fdf::Namespace>& driver_incoming() const { return incoming_; }
  std::shared_ptr<fdf::OutgoingDirectory>& driver_outgoing() { return outgoing(); }
  const std::optional<std::string>& driver_node_name() const { return node_name_; }

  zx::event node_token() const {
    zx::event copy;
    if (node_token_.is_valid()) {
      node_token_.duplicate(ZX_RIGHT_SAME_RIGHTS, &copy);
    }
    return copy;
  }

 protected:
  // Returns a function for releasing the initialized resources. Override to inject dependency for
  // unit testing.
  virtual zx::result<fit::function<void()>> InitResources();

  ddk::Pci pci_;
  std::optional<fdf::MmioBuffer> mmio_;
  fuchsia_hardware_pci::InterruptMode irq_mode_;
  zx::interrupt irq_;
  zx::bti bti_;

 private:
  friend class fake_nvme::FakeController;

  int IrqLoop();
  int IoLoop();

  // Main driver initialization.
  zx_status_t Init();

  // Process pending IO commands. Called in the IoLoop().
  void ProcessIoSubmissions();
  // Process pending IO completions. Called in the IoLoop().
  void ProcessIoCompletions();
  void PerformTeardown();

  std::shared_ptr<fdf::Namespace> incoming_;
  std::optional<inspect::ComponentInspector> component_inspector_;
  std::optional<std::string> node_name_;
  zx::event node_token_;
  inspect::Node inspect_node_;

  std::mutex commands_lock_;
  // The pending list consists of commands that have been received via QueueIoCommand() and are
  // waiting for IO to start.
  list_node_t pending_commands_ TA_GUARDED(commands_lock_) = LIST_INITIAL_VALUE(pending_commands_);

  // Admin submission and completion queues.
  std::unique_ptr<QueuePair> admin_queue_;
  std::mutex admin_lock_;  // Used to serialize admin transactions.

  // IO submission and completion queues.
  std::unique_ptr<QueuePair> io_queue_;
  // Notifies IoThread() that it has work to do. Signaled from QueueIoCommand() or the IRQ handler.
  sync_completion_t io_signal_;

  thrd_t irq_thread_;
  thrd_t io_thread_;
  bool irq_thread_started_ = false;
  bool io_thread_started_ = false;

  uint32_t max_data_transfer_bytes_;
  // This flag indicates whether the volatile write cache of the device is enabled. It can only be
  // enabled if the volatile write cache is supported.
  bool volatile_write_cache_enabled_ = false;
  bool driver_shutdown_ = false;

  uint16_t atomic_write_unit_normal_;
  uint16_t atomic_write_unit_power_fail_;

  std::vector<std::unique_ptr<nvme::Namespace>> namespaces_;

  fidl::WireSyncClient<fuchsia_driver_framework::Node> parent_node_;
  fidl::WireSyncClient<fuchsia_driver_framework::Node> root_node_;
  fidl::WireSyncClient<fuchsia_driver_framework::NodeController> node_controller_;
};

}  // namespace nvme

#endif  // SRC_DEVICES_BLOCK_DRIVERS_NVME_NVME_H_

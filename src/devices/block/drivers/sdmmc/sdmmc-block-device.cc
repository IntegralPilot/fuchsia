// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "sdmmc-block-device.h"

#include <endian.h>
#include <fidl/fuchsia.hardware.cqhci/cpp/fidl.h>
#include <fidl/fuchsia.hardware.power/cpp/fidl.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <fidl/fuchsia.power.system/cpp/fidl.h>
#include <fuchsia/hardware/sdmmc/cpp/banjo.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/logging/cpp/structured_logger.h>
#include <lib/driver/power/cpp/element-description-builder.h>
#include <lib/fit/defer.h>
#include <lib/fzl/vmo-mapper.h>
#include <lib/trace/event.h>
#include <threads.h>
#include <zircon/hw/gpt.h>
#include <zircon/process.h>
#include <zircon/status.h>
#include <zircon/threads.h>

#include <algorithm>
#include <atomic>
#include <memory>

#include <fbl/alloc_checker.h>
#include <safemath/safe_conversions.h>

#include "sdmmc-partition-device.h"
#include "sdmmc-root-device.h"
#include "sdmmc-rpmb-device.h"
#include "src/devices/block/lib/common/include/common.h"
#include "tools/power_config/lib/cpp/power_config.h"

namespace sdmmc {
namespace {

constexpr size_t kTranMaxAttempts = 10;

// Boot and RPMB partition sizes are in units of 128 KiB/KB.
constexpr uint32_t kBootSizeMultiplier = 128 * 1024;

// Populates and returns a fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion using the supplied
// arguments.
zx::result<fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion> GetBufferRegion(zx_handle_t vmo,
                                                                            uint64_t offset,
                                                                            uint64_t size) {
  zx::vmo dup;
  zx_status_t status = zx_handle_duplicate(vmo, ZX_RIGHT_SAME_RIGHTS, dup.reset_and_get_address());
  if (status != ZX_OK) {
    fdf::error("Failed to duplicate vmo: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion buffer_region;
  buffer_region.buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmo(std::move(dup));
  buffer_region.offset = offset;
  buffer_region.size = size;
  return zx::ok(std::move(buffer_region));
}

}  // namespace

zx_status_t SdmmcBlockDevice::Create(SdmmcRootDevice* parent, std::unique_ptr<SdmmcDevice> sdmmc,
                                     std::unique_ptr<SdmmcBlockDevice>* out_dev) {
  fbl::AllocChecker ac;
  out_dev->reset(new (&ac) SdmmcBlockDevice(parent, std::move(sdmmc)));
  if (!ac.check()) {
    fdf::error("failed to allocate device memory");
    return ZX_ERR_NO_MEMORY;
  }

  return ZX_OK;
}

zx_status_t SdmmcBlockDevice::AddDevice() {
  // Device must be in TRAN state at this point
  zx_status_t st = WaitForTran();
  if (st != ZX_OK) {
    fdf::error("waiting for TRAN state failed, retcode = {}", zx_status_get_string(st));
    return ZX_ERR_TIMED_OUT;
  }

  root_ = parent_->driver_inspector().root().CreateChild("sdmmc_core");
  properties_.io_errors_ = root_.CreateUint("io_errors", 0);
  properties_.io_retries_ = root_.CreateUint("io_retries", 0);

  fbl::AutoLock worker_lock(&worker_lock_);
  fbl::AutoLock lock(&queue_lock_);

  if (!is_sd_) {
    MmcSetInspectProperties();
  }

  auto dispatcher = fdf::SynchronizedDispatcher::Create(
      fdf::SynchronizedDispatcher::Options::kAllowSyncCalls, "sdmmc-block-worker",
      [&](fdf_dispatcher_t*) { worker_shutdown_completion_.Signal(); },
      "fuchsia.devices.block.drivers.sdmmc.worker");
  if (dispatcher.is_error()) {
    fdf::error("Failed to create dispatcher: {}", zx_status_get_string(dispatcher.status_value()));
    return dispatcher.status_value();
  }
  worker_dispatcher_ = *std::move(dispatcher);

  st = async::PostTask(worker_dispatcher_.async_dispatcher(), [this] { WorkerLoop(); });
  if (st != ZX_OK) {
    fdf::error("Failed to start worker thread: {}", zx_status_get_string(st));
    return st;
  }

  const bool cq_enabled = (sdmmc_->host_info().caps & SDMMC_HOST_CAP_COMMAND_QUEUEING &&
                           parent_->config().command_queueing_enabled());
  suspend_supported_ = !is_sd_ && parent_->config().storage_power_management_enabled();

  // If command queuing is enabled, then the command queuing driver is responsible for setting the
  // CPU element dependency.  The framework ensures that the command queuing driver has a dependency
  // on this driver.
  if (suspend_supported_ && !cq_enabled) {
    zx::result result = ConfigurePowerManagement();

    if (result.is_ok()) {
      fdf::info("Configured power management successfully (sdmmc).");
    } else if (parent_->config().enable_suspend()) {
      // Only log and return error on a failed power configuration if enable_suspend and
      // storage_power_management_enabled were true.
      fdf::error("Failed to configure power management: {}", result.status_string());
      return result.status_value();
    }
  }

  auto [controller_client_end, controller_server_end] =
      fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();
  auto [node_client_end, node_server_end] =
      fidl::Endpoints<fuchsia_driver_framework::Node>::Create();

  controller_.Bind(std::move(controller_client_end));
  block_node_.Bind(std::move(node_client_end));

  fidl::Arena arena;
  const auto args =
      fuchsia_driver_framework::wire::NodeAddArgs::Builder(arena).name(arena, block_name()).Build();

  auto result = parent_->root_node()->AddChild(args, std::move(controller_server_end),
                                               std::move(node_server_end));
  if (!result.ok()) {
    fdf::error("Failed to add child block device: {}", result.status_string());
    return result.status();
  }

  auto remove_device_on_error =
      fit::defer([&]() { [[maybe_unused]] auto result = controller_->Remove(); });

  fbl::AllocChecker ac;
  if (!is_sd_ && raw_ext_csd_[MMC_EXT_CSD_RPMB_SIZE_MULT] > 0) {
    if (!cq_enabled) {
      std::unique_ptr<RpmbDevice> rpmb_device(new (&ac) RpmbDevice(this, raw_cid_, raw_ext_csd_));
      if (!ac.check()) {
        fdf::error("failed to allocate device memory");
        return ZX_ERR_NO_MEMORY;
      }

      // Only publish the RPMB service if command queueing is disabled.  When command queueing is
      // enabled, we instead expose this RPMB device as a driver FIDL service to be consumed by the
      // CQHCI driver.
      fdf::info("Adding rpmb device");
      if ((st = rpmb_device->AddDevice()) != ZX_OK) {
        fdf::error("failed to add rpmb device: {}", zx_status_get_string(st));
        return st;
      }
      child_rpmb_device_ = std::move(rpmb_device);
    } else {
      std::unique_ptr<DriverRpmbDevice> rpmb_device(
          new (&ac) DriverRpmbDevice(this, raw_cid_, raw_ext_csd_));
      if (!ac.check()) {
        fdf::error("failed to allocate device memory");
        return ZX_ERR_NO_MEMORY;
      }

      driver_rpmb_device_ = std::move(rpmb_device);
    }
  }

  auto inline_crypto_client =
      parent_->driver_incoming()->Connect<fuchsia_hardware_sdmmc::SdmmcService::InlineCrypto>();
  if (inline_crypto_client.is_ok()) {
    inline_encryption_client_ = fdf::WireSyncClient(std::move(*inline_crypto_client));
  }

  if (cq_enabled) {
    // With command queueing, the child CQHCI driver handles fuchsia.storage.block.*.

    remove_device_on_error.cancel();
    return AddCqhciDevice();
  }
  // Otherwise, bind PartitionDevices for each partition to handle fuchsia.storage.block.*.

  std::unique_ptr<PartitionDevice> user_partition(
      new (&ac) PartitionDevice(this, block_info_, USER_DATA_PARTITION));
  if (!ac.check()) {
    fdf::error("failed to allocate device memory");
    return ZX_ERR_NO_MEMORY;
  }

  if ((st = user_partition->AddDevice()) != ZX_OK) {
    fdf::error("failed to add user partition device: {}", zx_status_get_string(st));
    return st;
  }

  child_partition_devices_.push_back(std::move(user_partition));

  if (!is_sd_) {
    const uint32_t boot_size = raw_ext_csd_[MMC_EXT_CSD_BOOT_SIZE_MULT] * kBootSizeMultiplier;
    const bool boot_enabled =
        raw_ext_csd_[MMC_EXT_CSD_PARTITION_CONFIG] & MMC_EXT_CSD_BOOT_PARTITION_ENABLE_MASK;
    if (boot_size > 0 && boot_enabled) {
      const uint64_t boot_partition_block_count = boot_size / block_info_.block_size;
      const fuchsia_storage_block::wire::BlockInfo boot_info = {
          .block_count = boot_partition_block_count,
          .block_size = block_info_.block_size,
          .max_transfer_size = block_info_.max_transfer_size,
          .flags = static_cast<fuchsia_storage_block::wire::DeviceFlag>(block_info_.flags),
      };

      std::unique_ptr<PartitionDevice> boot_partition_1(
          new (&ac) PartitionDevice(this, boot_info, BOOT_PARTITION_1));
      if (!ac.check()) {
        fdf::error("failed to allocate device memory");
        return ZX_ERR_NO_MEMORY;
      }

      std::unique_ptr<PartitionDevice> boot_partition_2(
          new (&ac) PartitionDevice(this, boot_info, BOOT_PARTITION_2));
      if (!ac.check()) {
        fdf::error("failed to allocate device memory");
        return ZX_ERR_NO_MEMORY;
      }

      if ((st = boot_partition_1->AddDevice()) != ZX_OK) {
        fdf::error("failed to add boot partition device: {}", zx_status_get_string(st));
        return st;
      }

      child_partition_devices_.push_back(std::move(boot_partition_1));

      if ((st = boot_partition_2->AddDevice()) != ZX_OK) {
        fdf::error("failed to add boot partition device: {}", zx_status_get_string(st));
        return st;
      }

      child_partition_devices_.push_back(std::move(boot_partition_2));
    }
  }

  remove_device_on_error.cancel();

  return ZX_OK;
}

zx_status_t SdmmcBlockDevice::AddCqhciDevice() {
  ZX_DEBUG_ASSERT(!cqhci_host_info_);
  fuchsia_hardware_cqhci::CqhciHostInfo info = {};
  fuchsia_hardware_sdmmc::SdmmcHostInfo host_info = {};
  host_info.caps(fuchsia_hardware_sdmmc::SdmmcHostCap{sdmmc_->host_info().caps});
  host_info.max_transfer_size(sdmmc_->host_info().max_transfer_size);
  host_info.max_buffer_regions(sdmmc_->host_info().max_buffer_regions);
  info.sdmmc_host_info(std::move(host_info));
  std::vector<uint8_t> ext_csd(std::begin(raw_ext_csd_), std::end(raw_ext_csd_));
  info.ext_csd(std::move(ext_csd));
  info.rca(sdmmc_->Rca());
  info.partitions().emplace_back(fuchsia_hardware_cqhci::EmmcPartitionId::kUserDataPartition,
                                 block_info_.block_count, block_info_.block_size);
  const uint32_t boot_size = raw_ext_csd_[MMC_EXT_CSD_BOOT_SIZE_MULT] * kBootSizeMultiplier;
  const bool boot_enabled =
      raw_ext_csd_[MMC_EXT_CSD_PARTITION_CONFIG] & MMC_EXT_CSD_BOOT_PARTITION_ENABLE_MASK;
  if (boot_size > 0 && boot_enabled) {
    const uint64_t boot_partition_block_count = boot_size / block_info_.block_size;
    info.partitions().emplace_back(fuchsia_hardware_cqhci::EmmcPartitionId::kBootPartition1,
                                   boot_partition_block_count, block_info_.block_size);
    info.partitions().emplace_back(fuchsia_hardware_cqhci::EmmcPartitionId::kBootPartition2,
                                   boot_partition_block_count, block_info_.block_size);
  }
  fuchsia_hardware_cqhci::Service::InstanceHandler handler({
      .cqhci = cqhci_bindings_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->get(),
                                             fidl::kIgnoreBindingClosure),
      .rpmb = driver_rpmb_bindings_.CreateHandler(driver_rpmb_device_.get(),
                                                  fdf::Dispatcher::GetCurrent()->get(),
                                                  fidl::kIgnoreBindingClosure),
  });
  if (inline_encryption_client_.is_valid()) {
    auto result = handler.add_inline_crypto(
        [this](fdf::ServerEnd<fuchsia_hardware_inlineencryption::DriverDevice> server_end) {
          auto result = parent_->driver_incoming()
                            ->Connect<fuchsia_hardware_sdmmc::SdmmcService::InlineCrypto>(
                                std::move(server_end));
          if (result.is_error()) {
            FDF_LOGL(ERROR, logger(), "Failed to connect to inline crypto: %s",
                     result.status_string());
          }
        });
    ZX_ASSERT(result.is_ok());
  }
  cqhci_host_info_ = std::move(info);

  if (zx::result<> result = parent_->driver_outgoing()->AddService<fuchsia_hardware_cqhci::Service>(
          std::move(handler));
      result.is_error()) {
    fdf::error("Failed to add cqhci service: {}", result);
    return result.status_value();
  }

  auto [controller_client_end, controller_server_end] =
      fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();
  cqhci_controller_.Bind(std::move(controller_client_end));

  fidl::Arena arena;
  std::vector<fuchsia_driver_framework::wire::Offer> offers{
      fdf::MakeOffer2<fuchsia_hardware_cqhci::Service>(arena),
  };

  const auto args = fuchsia_driver_framework::wire::NodeAddArgs::Builder(arena)
                        .name(arena, "cqhci")
                        .offers2(arena, std::move(offers))
                        .Build();

  auto result = block_node()->AddChild(args, std::move(controller_server_end), {});
  if (!result.ok()) {
    fdf::error("Failed to add child partition device: {}", result.status_string());
    return result.status();
  }
  return ZX_OK;
}

zx::result<> SdmmcBlockDevice::ConfigurePowerManagement() {
  if (!parent_->power_element_token().is_valid()) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  zx::result connect_to_cpu_element_manager =
      parent_->driver_incoming()->Connect<fuchsia_power_system::CpuElementManager>();
  if (connect_to_cpu_element_manager.is_error()) {
    fdf::error("CpuElementManager unavailable: {}",
               zx_status_get_string(connect_to_cpu_element_manager.error_value()));
    return connect_to_cpu_element_manager.take_error();
  }
  fidl::SyncClient<fuchsia_power_system::CpuElementManager> cpu_element_manager(
      std::move(connect_to_cpu_element_manager.value()));
  zx::event dependency_token;
  zx_status_t status =
      parent_->power_element_token().duplicate(ZX_RIGHT_SAME_RIGHTS, &dependency_token);
  if (status != ZX_OK) {
    fdf::error("Failed to duplicate power token: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  fidl::Result<fuchsia_power_system::CpuElementManager::AddExecutionStateDependency> result =
      cpu_element_manager->AddExecutionStateDependency(
          {{.dependency_token = std::move(dependency_token), .power_level = 1}});
  if (result.is_error()) {
    fdf::error("CpuElementManager token registration failed: {}",
               result.error_value().FormatDescription());
    if (result.error_value().is_framework_error()) {
      return zx::error(result.error_value().framework_error().status());
    }

    switch (result.error_value().domain_error()) {
      case fuchsia_power_system::AddExecutionStateDependencyError::kInvalidArgs:
        return zx::error(ZX_ERR_INVALID_ARGS);
      case fuchsia_power_system::AddExecutionStateDependencyError::kBadState:
        return zx::error(ZX_ERR_BAD_STATE);
      default:
        return zx::error(ZX_ERR_INTERNAL);
    }
  }

  return zx::success();
}

void SdmmcBlockDevice::Suspend(fdf_power::SuspendCompleter completer) {
  if (suspend_supported_) {
    SetLevel(kPowerLevelOff);
  }
  completer();
}

void SdmmcBlockDevice::Resume(fdf_power::ResumeCompleter completer) {
  if (suspend_supported_) {
    SetLevel(kPowerLevelOn);
  }
  completer();
}

void SdmmcBlockDevice::SetLevel(uint8_t level) {
  switch (level) {
    case kPowerLevelOn: {
      const zx::time start = zx::clock::get_monotonic();

      fbl::AutoLock lock(&worker_lock_);
      // Actually raise the hardware's power level.
      zx_status_t status = ResumePower();
      if (status != ZX_OK) {
        const zx::duration duration = zx::clock::get_monotonic() - start;
        fdf::error("Failed to resume power after {} us: {}", duration.to_usecs(),
                   zx_status_get_string(status));
        return;
      }

      // Communicate to Power Broker that the hardware power level has been raised.
      worker_condition_.Broadcast();
      break;
    }
    case kPowerLevelOff: {
      fbl::AutoLock lock(&worker_lock_);
      // Actually lower the hardware's power level.
      zx_status_t status = SuspendPower();
      if (status != ZX_OK) {
        fdf::error("Failed to suspend power: {}", zx_status_get_string(status));
        return;
      }
      // Communicate to Power Broker that the hardware power level has been lowered.
      break;
    }
    default:
      fdf::error("Unexpected power level for hardware power element: {}", level);
      return;
  }
}

void SdmmcBlockDevice::HostInfo(fdf::Arena& arena, HostInfoCompleter::Sync& completer) {
  completer.buffer(arena).ReplySuccess(fidl::ToWire(arena, cqhci_host_info_.value()));
}

void SdmmcBlockDevice::InitializeCommandQueueing(
    InitializeCommandQueueingRequestView request, fdf::Arena& arena,
    InitializeCommandQueueingCompleter::Sync& completer) {
  zx::result resources = sdmmc_->InitializeCommandQueueing(
      std::move(request->virtual_interrupt), std::move(request->virtual_interrupt_lifeline));
  if (resources.is_error()) {
    completer.buffer(arena).ReplyError(resources.status_value());
    return;
  }

  completer.buffer(arena).ReplySuccess(
      std::move(resources->cqhci_mmio), resources->cqhci_mmio_offset,
      std::move(resources->sdhci_mmio), resources->sdhci_mmio_offset, std::move(resources->bti),
      std::move(resources->interrupt));
}

void SdmmcBlockDevice::EnableCqhci(fdf::Arena& arena, EnableCqhciCompleter::Sync& completer) {
  {
    fbl::AutoLock worker_lock(&worker_lock_);
    if (!(raw_ext_csd_[MMC_EXT_CSD_CMDQ_MODE_EN] & MMC_EXT_CSD_CMDQ_MODE_ENABLED)) {
      // Before yielding to the command queueing driver, make sure the user data partition is
      // selected.  Enabling command queuing whilst the RPMB partition is selected can result in
      // errors.
      if (zx_status_t status = SetPartition(USER_DATA_PARTITION); status != ZX_OK) {
        completer.buffer(arena).ReplyError(status);
        return;
      }

      zx_status_t status = MmcDoSwitch(MMC_EXT_CSD_CMDQ_MODE_EN, MMC_EXT_CSD_CMDQ_MODE_ENABLED);
      if (status != ZX_OK) {
        completer.buffer(arena).ReplyError(status);
        return;
      }
    }
  }
  completer.buffer(arena).Reply(sdmmc_->EnableCqhci());
}

void SdmmcBlockDevice::DisableCqhci(fdf::Arena& arena, DisableCqhciCompleter::Sync& completer) {
  zx::result result = sdmmc_->DisableCqhci();
  if (result.is_error()) {
    completer.buffer(arena).ReplyError(result.status_value());
    return;
  }
  fbl::AutoLock worker_lock(&worker_lock_);

  // Assume nothing regarding the current partition; the command queueing driver can change the
  // partition.
  current_partition_ = UNKNOWN;

  zx_status_t status = MmcDoSwitch(MMC_EXT_CSD_CMDQ_MODE_EN, 0);
  if (status != ZX_OK) {
    completer.buffer(arena).ReplyError(status);
    return;
  }
  if (status = sdmmc_->MmcSendExtCsd(raw_ext_csd_); status != ZX_OK) {
    fdf::error("MMC_SEND_EXT_CSD failed: {}", zx_status_get_string(status));
  }
  completer.buffer(arena).ReplySuccess();
}

void SdmmcBlockDevice::StopWorkerDispatcher(std::optional<fdf::StopCompleter> completer) {
  if (worker_dispatcher_.get()) {
    {
      fbl::AutoLock worker_lock(&worker_lock_);
      shutdown_ = true;
    }
    worker_condition_.Broadcast();
    worker_event_.Signal();

    worker_dispatcher_.ShutdownAsync();
    worker_shutdown_completion_.Wait();
  }

  // error out all pending requests
  fbl::AutoLock lock(&queue_lock_);

  for (auto& request : rpmb_list_) {
    request.callback(ZX_ERR_CANCELED);
  }
  rpmb_list_.clear();

  if (child_partition_devices_.empty()) {
    if (completer.has_value()) {
      completer.value()(zx::ok());
    }
    return;
  }

  struct Context {
    Context(size_t count, std::optional<fdf::StopCompleter> comp)
        : pending_count(count), completer(std::move(comp)) {}
    std::atomic<size_t> pending_count;
    std::optional<fdf::StopCompleter> completer;
  };

  auto ctx = std::make_shared<Context>(child_partition_devices_.size(), std::move(completer));

  for (auto& device : child_partition_devices_) {
    device->StopBlockServer([ctx]() {
      if (ctx->pending_count.fetch_sub(1) == 1) {
        if (ctx->completer.has_value()) {
          ctx->completer.value()(zx::ok());
        }
      }
    });
  }
}

void SdmmcBlockDevice::SendPowerOffNotification() {
  fdf::info("SendPowerOffNotification");
  if (is_sd_ || !parent_->config().storage_power_management_enabled()) {
    return;
  }

  fbl::AutoLock worker_lock(&worker_lock_);
  if (power_suspended_ && vccq_off_with_controller_off_) {
    // We don't need to send a power off notification if the device is already off.
    return;
  }

  // Move the device back to TRAN so we can send the power off notification.
  if (power_suspended_ && ResumePower() != ZX_OK) {
    return;
  }

  zx_status_t status = MmcDoSwitch(MMC_EXT_CSD_POWER_OFF_NOTIFICATION, MMC_EXT_CSD_POWER_OFF_LONG);
  if (status != ZX_OK) {
    fdf::error("Failed to send power off notification: {}", zx_status_get_string(status));
  }
}

zx_status_t SdmmcBlockDevice::ReadWriteWithRetries(std::vector<block_server::Request>& requests,
                                                   EmmcPartition partition) {
  zx_status_t st = SetPartition(partition);
  if (st != ZX_OK) {
    return st;
  }

  uint32_t attempts = 0;
  while (true) {
    attempts++;
    const bool last_attempt = attempts >= sdmmc_->kTryAttempts;

    st = ReadWriteAttempt(requests, !last_attempt);

    if (st == ZX_OK || last_attempt) {
      break;
    }
  }

  properties_.io_retries_.Add(attempts - 1);
  if (st != ZX_OK) {
    fdf::error("do_txn error: {}", zx_status_get_string(st));
    properties_.io_errors_.Add(1);
  }

  fdf::debug("do_txn complete");
  return st;
}

zx_status_t SdmmcBlockDevice::ReadWriteAttempt(std::vector<block_server::Request>& requests,
                                               bool suppress_error_messages) {
  // For single-block transfers, we could get higher performance by using SDMMC_READ_BLOCK/
  // SDMMC_WRITE_BLOCK without the need to SDMMC_SET_BLOCK_COUNT or SDMMC_STOP_TRANSMISSION.
  // However, we always do multiple-block transfers for simplicity.
  ZX_DEBUG_ASSERT(requests.size() >= 1);
  const block_server::Request& first_request = requests[0];
  const bool is_read = first_request.operation.tag == block_server::Operation::Tag::Read;
  const bool command_packing = requests.size() > 1;
  const uint32_t cmd_idx = is_read ? SDMMC_READ_MULTIPLE_BLOCK : SDMMC_WRITE_MULTIPLE_BLOCK;
  const uint32_t cmd_flags =
      is_read ? SDMMC_READ_MULTIPLE_BLOCK_FLAGS : SDMMC_WRITE_MULTIPLE_BLOCK_FLAGS;
  uint32_t total_data_transfer_blocks = 0;
  for (const auto& request : requests) {
    total_data_transfer_blocks += request.operation.read.block_count;
  }

  fdf::debug(
      "sdmmc: do_txn blockop {} offset_vmo 0x{:x}"
      " length 0x{:x} packing_count {} blocksize 0x{:x}"
      " max_transfer_size 0x{:x}",
      is_read ? 'R' : 'W', first_request.operation.read.vmo_offset, total_data_transfer_blocks,
      requests.size(), block_info_.block_size, block_info_.max_transfer_size);

  fdf::Arena arena('SDMC');
  fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq> reqs;
  if (!command_packing) {
    // TODO(https://fxbug.dev/42076962): Consider using SDMMC_CMD_AUTO23, which is likely to enhance
    // performance.
    reqs.Allocate(arena, 2);

    auto& set_block_count = reqs[0];
    set_block_count.cmd_idx = SDMMC_SET_BLOCK_COUNT;
    set_block_count.cmd_flags = SDMMC_SET_BLOCK_COUNT_FLAGS;
    set_block_count.arg = total_data_transfer_blocks;

    auto& rw_multiple_block = reqs[1];
    rw_multiple_block.cmd_idx = cmd_idx;
    rw_multiple_block.cmd_flags = cmd_flags;
    rw_multiple_block.arg = static_cast<uint32_t>(first_request.operation.read.device_block_offset);
    rw_multiple_block.blocksize = block_info_.block_size;
    rw_multiple_block.use_inline_crypto =
        first_request.operation.read.options.inline_crypto.is_enabled;
    rw_multiple_block.slot = first_request.operation.read.options.inline_crypto.slot;
    if (first_request.operation.read.options.inline_crypto.is_enabled &&
        first_request.operation.read.options.inline_crypto.dun >
            std::numeric_limits<uint32_t>::max()) {
      return ZX_ERR_OUT_OF_RANGE;
    }
    rw_multiple_block.dun =
        static_cast<uint32_t>(first_request.operation.read.options.inline_crypto.dun);

    rw_multiple_block.buffers.Allocate(arena, 1);
    auto buffer_region =
        GetBufferRegion(first_request.vmo->get(), first_request.operation.read.vmo_offset,
                        first_request.operation.read.block_count * block_info_.block_size);
    if (buffer_region.is_error()) {
      return buffer_region.status_value();
    }
    rw_multiple_block.buffers[0] = *std::move(buffer_region);
  } else {
    // Form packed command header (section 6.6.29.1, eMMC standard 5.1)
    memset(readwrite_metadata_.packed_command_header_data, 0,
           sizeof(*readwrite_metadata_.packed_command_header_data));
    readwrite_metadata_.packed_command_header_data->version = 1;
    readwrite_metadata_.packed_command_header_data->rw = is_read ? 1 : 2;
    // Safe because requests.size() <= kMaxPackedCommandsFor512ByteBlockSize.
    readwrite_metadata_.packed_command_header_data->num_entries =
        safemath::checked_cast<uint8_t>(requests.size());

    // Packed write: SET_BLOCK_COUNT (header+data) -> WRITE_MULTIPLE_BLOCK (header+data)
    // Packed read: SET_BLOCK_COUNT (header) -> WRITE_MULTIPLE_BLOCK (header) ->
    //              SET_BLOCK_COUNT (data) -> READ_MULTIPLE_BLOCK (data)
    fuchsia_hardware_sdmmc::wire::SdmmcReq* set_block_count{};
    fuchsia_hardware_sdmmc::wire::SdmmcReq* rw_multiple_block{};
    fuchsia_hardware_sdmmc::wire::SdmmcReq* send_ext_csd{};
    if (!is_read) {
      reqs.Allocate(arena, 3);
      set_block_count = &reqs.data()[0];
      rw_multiple_block = &reqs.data()[1];
      send_ext_csd = &reqs.data()[2];
    } else {
      reqs.Allocate(arena, 5);
      set_block_count = &reqs.data()[2];
      rw_multiple_block = &reqs.data()[3];
      send_ext_csd = &reqs.data()[4];

      auto& write_set_block_count = reqs[0];
      write_set_block_count.cmd_idx = SDMMC_SET_BLOCK_COUNT;
      write_set_block_count.cmd_flags = SDMMC_SET_BLOCK_COUNT_FLAGS;
      write_set_block_count.arg = MMC_SET_BLOCK_COUNT_PACKED | 1;  // 1 header block.

      auto& write_multiple_block = reqs[1];
      write_multiple_block.cmd_idx = SDMMC_WRITE_MULTIPLE_BLOCK;
      write_multiple_block.cmd_flags = SDMMC_WRITE_MULTIPLE_BLOCK_FLAGS;
      write_multiple_block.arg =
          static_cast<uint32_t>(first_request.operation.read.device_block_offset);
      write_multiple_block.blocksize = block_info_.block_size;
      write_multiple_block.buffers.Allocate(arena, 1);  // 1 header block.
      write_multiple_block.buffers[0] = {
          .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmoId(kPackedCommandVmoId),
          .offset = 0,
          .size = block_info_.block_size,
      };
    }

    set_block_count->cmd_idx = SDMMC_SET_BLOCK_COUNT;
    set_block_count->cmd_flags = SDMMC_SET_BLOCK_COUNT_FLAGS;
    set_block_count->arg = MMC_SET_BLOCK_COUNT_PACKED |
                           (is_read ? total_data_transfer_blocks
                                    : (total_data_transfer_blocks + 1));  // +1 for header block->

    rw_multiple_block->cmd_idx = cmd_idx;
    rw_multiple_block->cmd_flags = cmd_flags;
    rw_multiple_block->arg =
        static_cast<uint32_t>(first_request.operation.read.device_block_offset);
    rw_multiple_block->blocksize = block_info_.block_size;

    int buffer_index_offset;
    if (is_read) {
      buffer_index_offset = 0;
      rw_multiple_block->buffers.Allocate(arena, requests.size());
    } else {
      buffer_index_offset = 1;
      rw_multiple_block->buffers.Allocate(arena, requests.size() + 1);  // +1 for header block.
      rw_multiple_block->buffers[0] = {
          .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmoId(kPackedCommandVmoId),
          .offset = 0,
          .size = block_info_.block_size,
      };
    }

    // The following buffer regions point to the data.
    for (size_t i = 0; i < requests.size(); i++) {
      const block_server::Request& request = requests[i];
      // TODO(https://fxbug.dev/436663316): Support multiple packed commands with inline crypto.
      ZX_ASSERT(!request.operation.read.options.inline_crypto.is_enabled);
      readwrite_metadata_.packed_command_header_data->arg[i].cmd23_arg =
          request.operation.read.block_count;
      readwrite_metadata_.packed_command_header_data->arg[i].cmdXX_arg =
          static_cast<uint32_t>(request.operation.read.device_block_offset);

      auto buffer_region =
          GetBufferRegion(request.vmo->get(), request.operation.read.vmo_offset,
                          request.operation.read.block_count * block_info_.block_size);
      if (buffer_region.is_error()) {
        return buffer_region.status_value();
      }
      rw_multiple_block->buffers[buffer_index_offset + i] = *std::move(buffer_region);
    }

    zx_cache_flush(readwrite_metadata_.packed_command_header_data, block_info_.block_size,
                   ZX_CACHE_FLUSH_DATA);

    // Packed command errors are reported in EXT_CSD.
    send_ext_csd->cmd_idx = MMC_SEND_EXT_CSD;
    send_ext_csd->cmd_flags = MMC_SEND_EXT_CSD_FLAGS;
    send_ext_csd->arg = 0;
    send_ext_csd->blocksize = MMC_EXT_CSD_SIZE;
    send_ext_csd->buffers.Allocate(arena, 1);
    send_ext_csd->buffers[0] = {
        .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmoId(kPackedCommandVmoId),
        // Write EXT_CSD to the VMO immediately following the packed command header.
        .offset = readwrite_metadata_.ext_csd_offset(),
        .size = readwrite_metadata_.ext_csd.size(),
    };
  }

  for (auto& req : reqs) {
    req.suppress_error_messages = suppress_error_messages;
  }

  zx_status_t status =
      sdmmc_->SdmmcIoRequest(std::move(arena), reqs, readwrite_metadata_.buffer_regions.get());
  if (!command_packing) {
    return status;
  }

  zx_cache_flush(readwrite_metadata_.ext_csd.data(), readwrite_metadata_.ext_csd.size(),
                 ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE);

  if (readwrite_metadata_.ext_csd[MMC_EXT_CSD_PACKED_COMMAND_STATUS] != 0) {
    if (!suppress_error_messages) {
      fdf::error("Packed command status: 0x{:02x} Packed failure index: {}",
                 readwrite_metadata_.ext_csd[MMC_EXT_CSD_PACKED_COMMAND_STATUS],
                 readwrite_metadata_.ext_csd[MMC_EXT_CSD_PACKED_FAILURE_INDEX]);
    }
    return ZX_ERR_IO;
  }
  return ZX_OK;
}

zx_status_t SdmmcBlockDevice::Flush() {
  if (!cache_enabled_) {
    return ZX_OK;
  }

  // TODO(https://fxbug.dev/42075502): Enable the cache and add flush support for SD.
  ZX_ASSERT(!is_sd_);

  zx_status_t st = MmcDoSwitch(MMC_EXT_CSD_FLUSH_CACHE, MMC_EXT_CSD_FLUSH_MASK);
  if (st != ZX_OK) {
    fdf::error("Failed to flush the cache: {}", zx_status_get_string(st));
  }
  return st;
}

zx_status_t SdmmcBlockDevice::Barrier() {
  // If BARRIER_SUPPORT is unset, the block server library should never send PRE_BARRIER requests.
  ZX_ASSERT((block_info_.flags & fuchsia_storage_block::wire::DeviceFlag::kBarrierSupport) !=
            fuchsia_storage_block::wire::DeviceFlag{0});

  if (!cache_enabled_) {
    return ZX_OK;
  }

  // TODO(https://fxbug.dev/42075502): Enable the cache and add barrier support for SD.
  ZX_ASSERT(!is_sd_);

  // If the device uses a FIFO cache policy, barriers do nothing so return early.
  if (cache_flush_fifo_) {
    return ZX_OK;
  }

  zx_status_t st = MmcDoSwitch(MMC_EXT_CSD_FLUSH_CACHE, MMC_EXT_CSD_BARRIER_MASK);
  if (st != ZX_OK) {
    fdf::error("Failed to set a barrier: {}", zx_status_get_string(st));
  }
  return st;
}

zx_status_t SdmmcBlockDevice::Trim(uint64_t offset_dev, uint64_t length,
                                   const EmmcPartition partition) {
  // TODO(b/312236221): Add trim support for SD.
  if (is_sd_) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  if ((block_info_.flags & fuchsia_storage_block::wire::DeviceFlag::kTrimSupport) ==
      fuchsia_storage_block::wire::DeviceFlag{0}) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  zx_status_t status = SetPartition(partition);
  if (status != ZX_OK) {
    return status;
  }

  constexpr uint32_t kEraseErrorFlags =
      MMC_STATUS_ADDR_OUT_OF_RANGE | MMC_STATUS_ERASE_SEQ_ERR | MMC_STATUS_ERASE_PARAM;

  const sdmmc_req_t trim_start = {
      .cmd_idx = MMC_ERASE_GROUP_START,
      .cmd_flags = MMC_ERASE_GROUP_START_FLAGS,
      .arg = static_cast<uint32_t>(offset_dev),
  };
  uint32_t response[4] = {};
  if ((status = sdmmc_->Request(&trim_start, response)) != ZX_OK) {
    fdf::error("failed to set trim group start: {}", zx_status_get_string(status));
    properties_.io_errors_.Add(1);
    return status;
  }
  if (response[0] & kEraseErrorFlags) {
    fdf::error("card reported trim group start error: 0x{:08x}", response[0]);
    properties_.io_errors_.Add(1);
    return ZX_ERR_IO;
  }

  const sdmmc_req_t trim_end = {
      .cmd_idx = MMC_ERASE_GROUP_END,
      .cmd_flags = MMC_ERASE_GROUP_END_FLAGS,
      .arg = static_cast<uint32_t>(offset_dev + length - 1),
  };
  if ((status = sdmmc_->Request(&trim_end, response)) != ZX_OK) {
    fdf::error("failed to set trim group end: {}", zx_status_get_string(status));
    properties_.io_errors_.Add(1);
    return status;
  }
  if (response[0] & kEraseErrorFlags) {
    fdf::error("card reported trim group end error: 0x{:08x}", response[0]);
    properties_.io_errors_.Add(1);
    return ZX_ERR_IO;
  }

  const sdmmc_req_t trim = {
      .cmd_idx = SDMMC_ERASE,
      .cmd_flags = SDMMC_ERASE_FLAGS,
      .arg = MMC_ERASE_TRIM_ARG,
  };
  if ((status = sdmmc_->Request(&trim, response)) != ZX_OK) {
    fdf::error("trim failed: {}", zx_status_get_string(status));
    properties_.io_errors_.Add(1);
    return status;
  }
  if (response[0] & kEraseErrorFlags) {
    fdf::error("card reported trim error: 0x{:08x}", response[0]);
    properties_.io_errors_.Add(1);
    return ZX_ERR_IO;
  }

  return ZX_OK;
}

zx::result<uint16_t> SdmmcBlockDevice::GetRpmbRequestType(const RpmbRequestInfo& request) const {
  constexpr size_t kRequestTypeOffset = 510;
  constexpr uint16_t kRequestTypeMask = 0xfff;

  if (request.tx_frames.size < sizeof(fuchsia_hardware_rpmb::wire::kFrameSize)) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  uint16_t request_type_be{};
  if (zx_status_t status = request.tx_frames.vmo.read(
          &request_type_be, request.tx_frames.offset + kRequestTypeOffset, sizeof(request_type_be));
      status != ZX_OK) {
    fdf::error("Failed to read RPMB TX frame: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  const uint16_t request_type = be16toh(request_type_be);
  const uint16_t masked_request_type = request_type & kRequestTypeMask;

  // Mask off the RPMB region number if appropriate.
  if (masked_request_type >= kRpmbRequestProgramKey &&
      masked_request_type <= kRpmbRequestReadResult) {
    return zx::success(masked_request_type);
  }
  return zx::success(request_type);
}

zx_status_t SdmmcBlockDevice::RpmbRequest(const RpmbRequestInfo& request) {
  const bool supports_32_frame_writes =
      raw_ext_csd_[MMC_EXT_CSD_WR_REL_PARAM] & MMC_EXT_CSD_EN_RPMB_REL_WR_MASK;

  // TODO(https://fxbug.dev/42166356): Find out if RPMB requests can be retried.
  using fuchsia_hardware_rpmb::wire::kFrameSize;

  const uint64_t tx_frame_count = request.tx_frames.size / kFrameSize;
  const uint64_t rx_frame_count =
      request.rx_frames.vmo.is_valid() ? (request.rx_frames.size / kFrameSize) : 0;
  const bool read_needed = rx_frame_count > 0;

  const zx::result<uint16_t> request_type = GetRpmbRequestType(request);
  if (request_type.is_error()) {
    return request_type.status_value();
  }

  zx_status_t status = SetPartition(RPMB_PARTITION);
  if (status != ZX_OK) {
    return status;
  }

  uint32_t unused_response[4];
  for (uint64_t frames_sent = 0; frames_sent < tx_frame_count;) {
    const uint64_t remaining_frames = tx_frame_count - frames_sent;

    // Writes must either be one or two frames, or 32 frames if supported. Intermediate values are
    // not allowed.
    const uint64_t frames_to_send =
        (supports_32_frame_writes && remaining_frames >= 32) ? 32 : std::min(remaining_frames, 2UL);

    sdmmc_req_t set_tx_block_count = {
        .cmd_idx = SDMMC_SET_BLOCK_COUNT,
        .cmd_flags = SDMMC_SET_BLOCK_COUNT_FLAGS,
        .arg = static_cast<uint32_t>(frames_to_send),
    };

    switch (*request_type) {
      case kRpmbRequestReadWriteCounter:
      case kRpmbRequestReadData:
      case kRpmbRequestReadResult:
      case kRpmbRequestReadConfiguration:
        break;
      default:
        // Use reliable writes unless the request is one of the four known read request types.
        set_tx_block_count.arg |= MMC_SET_BLOCK_COUNT_RELIABLE_WRITE;
        break;
    }

    if ((status = sdmmc_->Request(&set_tx_block_count, unused_response)) != ZX_OK) {
      fdf::error("failed to set block count for RPMB request: {}", zx_status_get_string(status));
      properties_.io_errors_.Add(1);
      return status;
    }

    const sdmmc_buffer_region_t write_region = {
        .buffer = {.vmo = request.tx_frames.vmo.get()},
        .type = SDMMC_BUFFER_TYPE_VMO_HANDLE,
        .offset = request.tx_frames.offset + (frames_sent * kFrameSize),
        .size = frames_to_send * kFrameSize,
    };
    const sdmmc_req_t write_tx_frames = {
        .cmd_idx = SDMMC_WRITE_MULTIPLE_BLOCK,
        .cmd_flags = SDMMC_WRITE_MULTIPLE_BLOCK_FLAGS,
        .arg = 0,  // Ignored by the card.
        .blocksize = kFrameSize,
        .buffers_list = &write_region,
        .buffers_count = 1,
    };
    if ((status = sdmmc_->Request(&write_tx_frames, unused_response)) != ZX_OK) {
      fdf::error("failed to write RPMB frames: {}", zx_status_get_string(status));
      properties_.io_errors_.Add(1);
      return status;
    }

    frames_sent += frames_to_send;
  }

  if (!read_needed) {
    return ZX_OK;
  }

  const sdmmc_req_t set_rx_block_count = {
      .cmd_idx = SDMMC_SET_BLOCK_COUNT,
      .cmd_flags = SDMMC_SET_BLOCK_COUNT_FLAGS,
      .arg = static_cast<uint32_t>(rx_frame_count),
  };
  if ((status = sdmmc_->Request(&set_rx_block_count, unused_response)) != ZX_OK) {
    fdf::error("failed to set block count for RPMB request: {}", zx_status_get_string(status));
    properties_.io_errors_.Add(1);
    return status;
  }

  const sdmmc_buffer_region_t read_region = {
      .buffer = {.vmo = request.rx_frames.vmo.get()},
      .type = SDMMC_BUFFER_TYPE_VMO_HANDLE,
      .offset = request.rx_frames.offset,
      .size = rx_frame_count * kFrameSize,
  };
  const sdmmc_req_t read_rx_frames = {
      .cmd_idx = SDMMC_READ_MULTIPLE_BLOCK,
      .cmd_flags = SDMMC_READ_MULTIPLE_BLOCK_FLAGS,
      .arg = 0,
      .blocksize = kFrameSize,
      .buffers_list = &read_region,
      .buffers_count = 1,
  };
  if ((status = sdmmc_->Request(&read_rx_frames, unused_response)) != ZX_OK) {
    fdf::error("failed to read RPMB frames: {}", zx_status_get_string(status));
    properties_.io_errors_.Add(1);
    return status;
  }

  return ZX_OK;
}

zx_status_t SdmmcBlockDevice::SetPartition(const EmmcPartition partition) {
  if (is_sd_ || partition == current_partition_) {
    return ZX_OK;
  }

  const uint8_t partition_config_value =
      (raw_ext_csd_[MMC_EXT_CSD_PARTITION_CONFIG] & MMC_EXT_CSD_PARTITION_ACCESS_MASK) | partition;

  zx_status_t status = MmcDoSwitch(MMC_EXT_CSD_PARTITION_CONFIG, partition_config_value);
  if (status != ZX_OK) {
    fdf::error("failed to switch to partition {}", static_cast<uint32_t>(partition));
    properties_.io_errors_.Add(1);
    return status;
  }

  current_partition_ = partition;
  return ZX_OK;
}

void SdmmcBlockDevice::RpmbQueue(RpmbRequestInfo info) {
  using fuchsia_hardware_rpmb::wire::kFrameSize;

  if (info.tx_frames.size % kFrameSize != 0) {
    fdf::error("tx frame buffer size not a multiple of {}", kFrameSize);
    info.callback(ZX_ERR_INVALID_ARGS);
    return;
  }

  // Checking against SDMMC_SET_BLOCK_COUNT_MAX_BLOCKS is sufficient for casting to uint16_t.
  static_assert(SDMMC_SET_BLOCK_COUNT_MAX_BLOCKS <= UINT16_MAX);

  const uint64_t tx_frame_count = info.tx_frames.size / kFrameSize;
  if (tx_frame_count == 0) {
    info.callback(ZX_OK);
    return;
  }

  if (tx_frame_count > SDMMC_SET_BLOCK_COUNT_MAX_BLOCKS) {
    fdf::error("received {} tx frames, maximum is {}", tx_frame_count,
               static_cast<uint32_t>(SDMMC_SET_BLOCK_COUNT_MAX_BLOCKS));
    info.callback(ZX_ERR_OUT_OF_RANGE);
    return;
  }

  if (info.rx_frames.vmo.is_valid()) {
    if (info.rx_frames.size % kFrameSize != 0) {
      fdf::error("rx frame buffer size is not a multiple of {}", kFrameSize);
      info.callback(ZX_ERR_INVALID_ARGS);
      return;
    }

    const uint64_t rx_frame_count = info.rx_frames.size / kFrameSize;
    if (rx_frame_count > SDMMC_SET_BLOCK_COUNT_MAX_BLOCKS) {
      fdf::error("received {} rx frames, maximum is {}", rx_frame_count,
                 static_cast<uint32_t>(SDMMC_SET_BLOCK_COUNT_MAX_BLOCKS));
      info.callback(ZX_ERR_OUT_OF_RANGE);
      return;
    }
  }

  fbl::AutoLock lock(&queue_lock_);
  if (rpmb_list_.size() >= kMaxOutstandingRpmbRequests) {
    info.callback(ZX_ERR_SHOULD_WAIT);
  } else {
    rpmb_list_.push_back(std::move(info));
    lock.release();
    worker_event_.Signal();
  }
}

void SdmmcBlockDevice::HandleRpmbRequests(std::deque<RpmbRequestInfo>& rpmb_list) {
  for (size_t i = 0; i < kRoundRobinRequestCount && !rpmb_list.empty(); i++) {
    RpmbRequestInfo& request = *rpmb_list.begin();
    zx_status_t status = RpmbRequest(request);
    if (status == ZX_OK) {
      request.callback(ZX_OK);
    } else {
      request.callback(status);
    }

    rpmb_list.pop_front();
  }
}

void SdmmcBlockDevice::WorkerLoop() {
  std::deque<RpmbRequestInfo> rpmb_list;

  for (;;) {
    fbl::AutoLock worker_lock(&worker_lock_);
    while (power_suspended_ && !shutdown_)
      worker_condition_.Wait(&worker_lock_);
    if (shutdown_)
      break;

    {
      fbl::AutoLock lock(&queue_lock_);
      if (rpmb_list_.empty()) {
        worker_event_.Reset();
        lock.release();
        worker_lock.release();
        worker_event_.Wait();
        continue;
      }

      rpmb_list.swap(rpmb_list_);
    }

    TRACE_DURATION("sdmmc", "work loop");

    while (!rpmb_list.empty()) {
      HandleRpmbRequests(rpmb_list);
    }
  }

  fdf::debug("worker thread terminated successfully");
}

zx_status_t SdmmcBlockDevice::SuspendPower() {
  if (power_suspended_ == true) {
    return ZX_OK;
  }

  if (zx_status_t status = Flush(); status != ZX_OK) {
    fdf::error("Failed to flush: {}", zx_status_get_string(status));
    return status;
  }

  if (vccq_off_with_controller_off_) {
    if (zx_status_t status =
            MmcDoSwitch(MMC_EXT_CSD_POWER_OFF_NOTIFICATION, MMC_EXT_CSD_POWER_OFF_SHORT);
        status != ZX_OK) {
      fdf::error("Failed to send power off notification: {}", zx_status_get_string(status));
      return status;
    }
  } else {
    if (zx_status_t status = sdmmc_->MmcSelectCard(/*select=*/false); status != ZX_OK) {
      fdf::error("Failed to (de-)SelectCard before sleep: {}", zx_status_get_string(status));
      return status;
    }

    if (zx_status_t status = sdmmc_->MmcSleepOrAwake(/*sleep=*/true); status != ZX_OK) {
      fdf::error("Failed to sleep: {}", zx_status_get_string(status));
      return status;
    }
  }

  trace_async_id_ = TRACE_NONCE();
  TRACE_ASYNC_BEGIN("sdmmc", "suspend", trace_async_id_);
  power_suspended_ = true;
  properties_.power_suspended_.Set(power_suspended_);
  fdf::info("Power suspended.");
  return ZX_OK;
}

zx_status_t SdmmcBlockDevice::ResumePower() {
  if (power_suspended_ == false) {
    return ZX_OK;
  }

  if (vccq_off_with_controller_off_) {
    // The device was turned off and is now back in the pre-idle state. Change the bus settings to
    // match the device state so they stay in sync.
    if (zx_status_t status = sdmmc_->SetBusWidth(SDMMC_BUS_WIDTH_ONE); status != ZX_OK) {
      return status;
    }
    if (zx_status_t status = sdmmc_->SetTiming(SDMMC_TIMING_LEGACY); status != ZX_OK) {
      return status;
    }
    if (zx_status_t status = sdmmc_->SetBusFreq(kInitializationFrequencyHz); status != ZX_OK) {
      return status;
    }

    // Probe the device again now that power has been restored.
    sdmmc_->ClearRca();
    if (zx_status_t status = sdmmc_->SdmmcGoIdle(); status != ZX_OK) {
      return status;
    }

    ZX_DEBUG_ASSERT(!is_sd_);
    if (zx_status_t status = ProbeMmcLocked(); status != ZX_OK) {
      return status;
    }
  } else {
    if (zx_status_t status = sdmmc_->MmcSleepOrAwake(/*sleep=*/false); status != ZX_OK) {
      fdf::error("Failed to awake: {}", zx_status_get_string(status));
      return status;
    }

    if (zx_status_t status = sdmmc_->MmcSelectCard(/*select=*/true); status != ZX_OK) {
      fdf::error("Failed to SelectCard after awake: {}", zx_status_get_string(status));
      return status;
    }
  }

  TRACE_ASYNC_END("sdmmc", "suspend", trace_async_id_);
  power_suspended_ = false;
  properties_.power_suspended_.Set(power_suspended_);

  fdf::info("Power resumed.");
  return ZX_OK;
}

zx_status_t SdmmcBlockDevice::WaitForIdle() { return WaitForState(0); }

zx_status_t SdmmcBlockDevice::WaitForTran() { return WaitForState(MMC_STATUS_CURRENT_STATE_TRAN); }

zx_status_t SdmmcBlockDevice::WaitForState(uint32_t state) {
  uint32_t current_state;
  size_t attempt = 0;
  for (; attempt <= kTranMaxAttempts; attempt++) {
    uint32_t response;
    zx_status_t st = sdmmc_->SdmmcSendStatus(&response);
    if (st != ZX_OK) {
      fdf::error("SDMMC_SEND_STATUS error, retcode = {}", zx_status_get_string(st));
      return st;
    }

    current_state = MMC_STATUS_CURRENT_STATE(response);
    if (current_state == MMC_STATUS_CURRENT_STATE_RECV) {
      st = sdmmc_->SdmmcStopTransmission();
      continue;
    } else if (current_state == state) {
      break;
    }

    zx::nanosleep(zx::deadline_after(zx::msec(10)));
  }

  if (attempt == kTranMaxAttempts) {
    // Too many retries, fail.
    return ZX_ERR_TIMED_OUT;
  } else {
    return ZX_OK;
  }
}

void SdmmcBlockDevice::SetBlockInfo(uint32_t block_size, uint64_t block_count) {
  block_info_.block_size = block_size;
  block_info_.block_count = block_count;
}

const inspect::Inspector& SdmmcBlockDevice::inspect() const {
  return parent_->driver_inspector().inspector();
}

fdf::Logger& SdmmcBlockDevice::logger() const { return parent_->logger(); }

void SdmmcBlockDevice::OnRequests(PartitionDevice& partition,
                                  cpp20::span<block_server::Request> requests) {
  fbl::AutoLock lock(&worker_lock_);
  while (power_suspended_ && !shutdown_)
    worker_condition_.Wait(&worker_lock_);
  if (shutdown_) {
    for (auto& request : requests) {
      partition.SendReply(request.request_id, zx::error(ZX_ERR_CANCELED));
    }
    return;
  }

  class Packer {
   public:
    Packer(SdmmcBlockDevice* device, PartitionDevice& partition, size_t max_requests, int max_bytes,
           uint32_t block_size)
        : device_(*device),
          partition_(partition),
          max_requests_(max_requests),
          max_bytes_(max_bytes),
          block_size_(block_size) {}

    void Push(block_server::Request& request) {
      uint64_t bytes = request.operation.read.block_count * block_size_;

      if (bytes == 0)
        return;

      size_t max_requests = max_requests_;

      // If inline encryption is enabled, we don't pack requests since we need a way to adjust the
      // DUN for a non-contiguous set of reads/writes.
      // TODO(https://fxbug.dev/436663316): Support packed commands with inline crypto.
      if (request.operation.read.options.inline_crypto.is_enabled) {
        [[maybe_unused]] auto result = Flush();
        max_requests = 1;
      }

      if (max_bytes_ > 0) {
        for (;;) {
          const uint64_t space = max_bytes_ - total_bytes_;
          if (bytes <= space) {
            break;
          }

          // Split this request if there is at least one block of free space. If not, just proceed
          // to flush the outstanding requests and pick this one up on the next iteration.
          const uint64_t split_amount_blocks = space / block_size_;
          if (split_amount_blocks > 0) {
            requests_.push_back(block_server::SplitRequest(
                request, static_cast<uint32_t>(split_amount_blocks), block_size_));
            bytes -= split_amount_blocks * block_size_;
          }

          if (auto result = Flush(split_amount_blocks > 0); result.is_error()) {
            // The partial request failed which means we ignore the rest of the request.
            return;
          }
        }
      }

      if (requests_.empty()) {
        // We need an extra buffer for the packed command header when there are multiple requests.
        total_bytes_ = zx_system_get_page_size();
      }
      requests_.push_back(request);
      total_bytes_ += bytes;
      if (requests_.size() >= max_requests || (max_bytes_ > 0 && total_bytes_ >= max_bytes_)) {
        [[maybe_unused]] auto result = Flush();
      }
    }

    // Unfortunately, there's no way for us to tell the compiler that we hold `worker_lock_` here,
    // so we have to skip thread safety analysis.  If `split_last` is true, the last request is a
    // partial request and so the response is not sent if successful since the caller will want to
    // finish the request in the next batch.  If there is a failure, the response is sent and the
    // caller is expected to discard the remaining request.
    zx::result<> Flush(bool split_last = false) TA_NO_THREAD_SAFETY_ANALYSIS {
      if (requests_.empty())
        return zx::ok();
      TRACE_DURATION(
          "sdmmc",
          requests_[0].operation.tag == block_server::Operation::Tag::Read ? "read" : "write",
          "count", requests_.size(),
          // The total number of bytes that will be read or written excluding the packed command
          // header.
          "size", total_bytes_ - zx_system_get_page_size());
      zx::result<> result =
          zx::make_result(device_.ReadWriteWithRetries(requests_, partition_.partition()));
      if (split_last && result.is_ok())
        requests_.pop_back();
      for (const block_server::Request& request : requests_) {
        partition_.SendReply(request.request_id, result);
      }
      requests_.clear();
      total_bytes_ = 0;
      return result;
    }

   private:
    SdmmcBlockDevice& device_;
    PartitionDevice& partition_;
    const size_t max_requests_;
    const uint64_t max_bytes_;
    const uint32_t block_size_;
    std::vector<block_server::Request> requests_;
    uint64_t total_bytes_ = 0;
  };

  zx_status_t status;
  EmmcPartition part = partition.partition();
  size_t max_reads = part == USER_DATA_PARTITION ? max_packed_reads_effective_ : 1;
  size_t max_writes = part == USER_DATA_PARTITION ? max_packed_writes_effective_ : 1;
  Packer read_packer(this, partition, max_reads, block_info_.max_transfer_size,
                     block_info_.block_size);
  Packer write_packer(this, partition, max_writes, block_info_.max_transfer_size,
                      block_info_.block_size);

  [[maybe_unused]] zx::result<> unused_result;

  for (block_server::Request& request : requests) {
    if (zx_status_t status =
            block_server::CheckIoRange(request, partition.block_info().block_count);
        status != ZX_OK) {
      fdf::warn("Invalid request range.");
      partition.SendReply(request.request_id, zx::make_result(status));
      continue;
    }
    bool crypto_out_of_range = false;
    if (request.operation.tag == block_server::Operation::Tag::Read) {
      crypto_out_of_range =
          request.operation.read.options.inline_crypto.is_enabled &&
          request.operation.read.options.inline_crypto.dun > std::numeric_limits<uint32_t>::max();
    } else if (request.operation.tag == block_server::Operation::Tag::Write) {
      crypto_out_of_range =
          request.operation.write.options.inline_crypto.is_enabled &&
          request.operation.write.options.inline_crypto.dun > std::numeric_limits<uint32_t>::max();
    }
    if (crypto_out_of_range) {
      fdf::warn("Inline crypto DUN out of 32-bit range.");
      partition.SendReply(request.request_id, zx::error(ZX_ERR_OUT_OF_RANGE));
      continue;
    }
    switch (request.operation.tag) {
      case block_server::Operation::Tag::Read:
        read_packer.Push(request);
        break;
      case block_server::Operation::Tag::Write:
        if (request.operation.write.options.flags.is_pre_barrier()) {
          TRACE_DURATION_BEGIN("sdmmc", "barrier");

          if (status = Barrier(); status != ZX_OK) {
            partition.SendReply(request.request_id, zx::make_result(status));
          }
          TRACE_DURATION_END("sdmmc", "barrier", "opcode",
                             TA_INT32(static_cast<int32_t>(request.operation.tag)), "txn_status",
                             TA_INT32(status));
          if (status != ZX_OK) {
            break;
          }
        }
        write_packer.Push(request);
        break;

      case block_server::Operation::Tag::Flush:
        TRACE_DURATION_BEGIN("sdmmc", "flush");

        // Technically, we might not need to do this, because there's no guarantee regarding the
        // ordering of requests, but it's arguably safer for us to flush preceding write requests
        // before issuing the flush command.
        unused_result = write_packer.Flush();

        status = Flush();

        partition.SendReply(request.request_id, zx::make_result(status));

        TRACE_DURATION_END("sdmmc", "flush", "opcode",
                           TA_INT32(static_cast<int32_t>(request.operation.tag)), "txn_status",
                           TA_INT32(status));
        break;

      case block_server::Operation::Tag::Trim:
        TRACE_DURATION_BEGIN("sdmmc", "trim");

        status = Trim(request.operation.trim.device_block_offset,
                      request.operation.trim.block_count, partition.partition());
        partition.SendReply(request.request_id, zx::make_result(status));

        TRACE_DURATION_END(
            "sdmmc", "trim", "opcode", TA_INT32(static_cast<int32_t>(request.operation.tag)),
            "length", TA_INT32(request.operation.trim.block_count), "offset_dev",
            TA_INT64(request.operation.trim.device_block_offset), "txn_status", TA_INT32(status));
        break;

      case block_server::Operation::Tag::CloseVmo:
      case block_server::Operation::Tag::StartDecompressedRead:
      case block_server::Operation::Tag::ContinueDecompressedRead:
        __UNREACHABLE;
    }
  }

  unused_result = read_packer.Flush();
  unused_result = write_packer.Flush();
}

void SdmmcBlockDevice::ProgramKey(ProgramKeyRequestView request,
                                  ProgramKeyCompleter::Sync& completer) {
  fdf::Arena arena(kArenaTag);
  auto result = inline_encryption_client_.buffer(arena)->ProgramKey(request->wrapped_key,
                                                                    request->data_unit_size);
  if (!result.ok()) {
    fdf::error("Transport error when sending ProgramKey: {}", result.status_string());
    completer.ReplyError(result.status());
    return;
  }
  if (result->is_error()) {
    fdf::error("ProgramKey failed: {}", zx_status_get_string(result->error_value()));
    completer.ReplyError(result->error_value());
    return;
  }
  completer.ReplySuccess(result->value()->slot);
}
void SdmmcBlockDevice::DeriveRawSecret(DeriveRawSecretRequestView request,
                                       DeriveRawSecretCompleter::Sync& completer) {
  fdf::Arena arena(kArenaTag);
  auto result = inline_encryption_client_.buffer(arena)->DeriveRawSecret(request->wrapped_key);
  if (!result.ok()) {
    fdf::error("Transport error when sending DeriveRawSecret: {}", result.status_string());
    completer.ReplyError(result.status());
    return;
  }
  if (result->is_error()) {
    fdf::error("DeriveRawSecret failed: {}", zx_status_get_string(result->error_value()));
    completer.ReplyError(result->error_value());
    return;
  }
  completer.ReplySuccess(result->value()->secret);
}

}  // namespace sdmmc

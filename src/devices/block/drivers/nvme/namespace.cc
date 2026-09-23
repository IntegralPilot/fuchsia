// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/block/drivers/nvme/namespace.h"

#include <lib/driver/logging/cpp/logger.h>
#include <lib/fzl/vmo-mapper.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>
#include <zircon/types.h>

#include <hwreg/bitfields.h>

#include "src/devices/block/drivers/nvme/commands/identify.h"
#include "src/devices/block/drivers/nvme/io-command.h"
#include "src/devices/block/drivers/nvme/nvme.h"
#include "src/devices/block/drivers/nvme/queue-pair.h"

namespace nvme {

zx_status_t Namespace::AddNamespace() {
  auto handlers = fuchsia_hardware_block_volume::Service::InstanceHandler({
      .volume =
          [this](fidl::ServerEnd<fuchsia_storage_block::Block> server_end) {
            ServeRequests(std::move(server_end));
          },
      .token =
          [this](fidl::ServerEnd<fuchsia_driver_token::NodeToken> server_end) {
            fidl::BindServer(fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                             std::move(server_end), this);
          },
  });

  auto result = controller_->driver_outgoing()->AddService<fuchsia_hardware_block_volume::Service>(
      std::move(handlers), NamespaceName().c_str());
  if (result.is_error()) {
    fdf::error("Failed to add volume service instance: {}", result.status_string());
    return result.status_value();
  }

  auto [controller_client_end, controller_server_end] =
      fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();

  node_controller_.Bind(std::move(controller_client_end));

  fidl::Arena arena;

  const auto args = fuchsia_driver_framework::wire::NodeAddArgs::Builder(arena)
                        .name(arena, NamespaceName())
                        .Build();

  auto add_child_result =
      controller_->root_node()->AddChild(args, std::move(controller_server_end), {});
  if (!add_child_result.ok()) {
    fdf::error("Failed to add child Namespace: {}", add_child_result.status_string());
    return add_child_result.status();
  }
  return ZX_OK;
}

zx::result<std::unique_ptr<Namespace>> Namespace::Bind(Nvme* controller, uint32_t namespace_id) {
  if (namespace_id == 0 || namespace_id == ~0u) {
    fdf::error("Attempted to create namespace with invalid id {}.", namespace_id);
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  fbl::AllocChecker ac;
  auto ns = fbl::make_unique_checked<Namespace>(&ac, controller, namespace_id);
  if (!ac.check()) {
    fdf::error("Failed to allocate memory for namespace {}.", namespace_id);
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  zx_status_t status = ns->Init();
  if (status != ZX_OK) {
    return zx::error(status);
  }

  status = ns->AddNamespace();
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(std::move(ns));
}

namespace {

void PopulateNamespaceInspect(const IdentifyNvmeNamespace& ns, const fbl::String& namespace_name,
                              uint16_t atomic_write_unit_normal,
                              uint16_t atomic_write_unit_power_fail, uint32_t max_transfer_bytes,
                              uint32_t block_size_bytes, inspect::Node* inspect_node,
                              inspect::Inspector* inspector) {
  auto inspect_ns = inspect_node->CreateChild(namespace_name);
  uint16_t nawun = ns.ns_atomics() ? ns.n_aw_un + 1 : atomic_write_unit_normal;
  uint16_t nawupf = ns.ns_atomics() ? ns.n_aw_u_pf + 1 : atomic_write_unit_power_fail;
  inspect_ns.RecordInt("atomic_write_unit_normal_blocks", nawun);
  inspect_ns.RecordInt("atomic_write_unit_power_fail_blocks", nawupf);
  inspect_ns.RecordInt("namespace_atomic_boundary_size_normal_blocks", ns.n_abs_n);
  inspect_ns.RecordInt("namespace_atomic_boundary_offset_blocks", ns.n_ab_o);
  inspect_ns.RecordInt("namespace_atomic_boundary_size_power_fail_blocks", ns.n_abs_pf);
  inspect_ns.RecordInt("namespace_optimal_io_boundary_blocks", ns.n_oio_b);
  // table of block formats
  for (int i = 0; i < ns.n_lba_f; i++) {
    if (ns.lba_formats[i].value) {
      auto& fmt = ns.lba_formats[i];
      inspect_ns.RecordInt(fbl::StringPrintf("lba_format_%u_block_size_bytes", i),
                           fmt.lba_data_size_bytes());
      inspect_ns.RecordInt(fbl::StringPrintf("lba_format_%u_relative_performance", i),
                           fmt.relative_performance());
      inspect_ns.RecordInt(fbl::StringPrintf("lba_format_%u_metadata_size_bytes", i),
                           fmt.metadata_size_bytes());
    }
  }
  inspect_ns.RecordInt("active_lba_format_index", ns.lba_format_index());
  inspect_ns.RecordInt("data_protection_caps", ns.dpc & 0x3F);
  inspect_ns.RecordInt("data_protection_set", ns.dps & 3);
  inspect_ns.RecordInt("namespace_size_blocks", static_cast<int64_t>(ns.n_sze));
  inspect_ns.RecordInt("namespace_cap_blocks", static_cast<int64_t>(ns.n_cap));
  inspect_ns.RecordInt("namespace_util_blocks", static_cast<int64_t>(ns.n_use));
  inspect_ns.RecordInt("max_transfer_bytes", max_transfer_bytes);
  inspect_ns.RecordInt("block_size_bytes", block_size_bytes);
  inspector->emplace(std::move(inspect_ns));
}

}  // namespace

zx_status_t Namespace::Init() {
  zx::vmo admin_data;
  const uint32_t kPageSize = zx_system_get_page_size();
  zx_status_t status = zx::vmo::create(kPageSize, 0, &admin_data);
  if (status != ZX_OK) {
    fdf::error("Failed to create vmo: {}", zx_status_get_string(status));
    return status;
  }

  fzl::VmoMapper mapper;
  status = mapper.Map(admin_data);
  if (status != ZX_OK) {
    fdf::error("Failed to map vmo: {}", zx_status_get_string(status));
    return status;
  }

  // Identify namespace.
  IdentifySubmission identify_ns;
  identify_ns.namespace_id = namespace_id_;
  identify_ns.set_structure(IdentifySubmission::IdentifyCns::kIdentifyNamespace);
  zx::result<Completion> completion =
      controller_->DoAdminCommandSync(identify_ns, admin_data.borrow());
  if (completion.is_error()) {
    fdf::error("Failed to identify namespace {}: {}", namespace_id_, completion.status_string());
    return completion.status_value();
  }

  auto ns = static_cast<IdentifyNvmeNamespace*>(mapper.start());

  block_info_.device_flags |=
      static_cast<uint32_t>(fuchsia_storage_block::wire::DeviceFlag::kFuaSupport);
  block_info_.block_count = ns->n_sze;
  auto& fmt = ns->lba_formats[ns->lba_format_index()];
  block_info_.block_size = fmt.lba_data_size_bytes();

  if (fmt.metadata_size_bytes()) {
    fdf::error("NVMe drive uses LBA format with metadata ({} bytes), which we do not support.",
               fmt.metadata_size_bytes());
    return ZX_ERR_NOT_SUPPORTED;
  }
  // The NVMe spec only mentions a lower bound. The upper bound may be a false requirement.
  if ((block_info_.block_size < 512) || (block_info_.block_size > 32768)) {
    fdf::error("Cannot handle LBA size of {}.", block_info_.block_size);
    return ZX_ERR_NOT_SUPPORTED;
  }

  // NVME r/w commands operate in block units, maximum of 64K blocks.
  const uint32_t max_bytes_per_cmd = block_info_.block_size * 65536;
  uint32_t max_transfer_bytes = controller_->max_data_transfer_bytes();
  if (max_transfer_bytes == 0) {
    max_transfer_bytes = max_bytes_per_cmd;
  } else {
    max_transfer_bytes = std::min(max_transfer_bytes, max_bytes_per_cmd);
  }

  // Limit maximum transfer size to 1MB which fits comfortably within our single PRP page per
  // QueuePair setup.
  const uint32_t prp_restricted_transfer_bytes = QueuePair::kMaxTransferPages * kPageSize;
  max_transfer_bytes = std::min(max_transfer_bytes, prp_restricted_transfer_bytes);

  block_info_.max_transfer_size = max_transfer_bytes;

  // Convert to block units.
  max_transfer_blocks_ = max_transfer_bytes / block_info_.block_size;

  PopulateNamespaceInspect(*ns, NamespaceName(), controller_->atomic_write_unit_normal(),
                           controller_->atomic_write_unit_power_fail(), max_transfer_bytes,
                           block_info_.block_size, &controller_->inspect_node(),
                           &controller_->inspect());

  {
    fbl::AutoLock lock(&lock_);
    block_server_.emplace(block_info_, this);
  }

  return ZX_OK;
}

zx::result<std::reference_wrapper<IoCommand>> Namespace::AllocateIoCommand() {
  while (!shutdown_ && io_command_bitmap_.all()) {
    pool_cond_.Wait(&lock_);
  }
  if (shutdown_) {
    return zx::error(ZX_ERR_CANCELED);
  }
  for (size_t i = 0; i < kMaxRequests; i++) {
    if (!io_command_bitmap_.test(i)) {
      io_command_bitmap_.set(i);
      return zx::ok(std::ref(io_command_pool_[i]));
    }
  }
  return zx::error(ZX_ERR_NO_RESOURCES);
}

void Namespace::FreeIoCommand(IoCommand* io_cmd) {
  ZX_ASSERT(io_cmd >= io_command_pool_.data());
  size_t idx = io_cmd - io_command_pool_.data();
  ZX_ASSERT(idx < kMaxRequests);
  io_command_bitmap_.reset(idx);
  pool_cond_.Signal();
}

void Namespace::StopBlockServer(fit::callback<void()> callback) {
  fbl::AutoLock lock(&lock_);
  shutdown_ = true;
  pool_cond_.Broadcast();
  if (block_server_) {
    block_server_->DestroyAsync([this, callback = std::move(callback)]() mutable {
      fbl::AutoLock lock(&lock_);
      block_server_.reset();
      callback();
    });
  } else {
    callback();
  }
}

Namespace::~Namespace() {
  if (controller_ && controller_->driver_outgoing()) {
    (void)controller_->driver_outgoing()->RemoveService<fuchsia_hardware_block_volume::Service>(
        NamespaceName().c_str());
  }
  fbl::AutoLock lock(&lock_);
  if (block_server_) {
    fdf::warn("Namespace destroyed with active block server connection.");
  }
}

void Namespace::OnRequests(std::span<block_server::Request> requests) {
  for (const auto& request : requests) {
    fbl::AutoLock lock(&lock_);
    zx::result<std::reference_wrapper<IoCommand>> alloc_result = AllocateIoCommand();
    if (alloc_result.is_error()) {
      block_server_->SendReply(request.request_id, alloc_result.take_error());
      continue;
    }
    IoCommand& io_cmd = alloc_result.value().get();
    io_cmd.request_id = request.request_id;
    io_cmd.ns = this;
    io_cmd.namespace_id = namespace_id_;
    io_cmd.block_size_bytes = block_info_.block_size;
    io_cmd.operation = request.operation;
    io_cmd.vmo = request.vmo;

    io_cmd.completion_cb = [this, &io_cmd](zx_status_t status) {
      CompleteIoCommand(&io_cmd, status);
    };

    uint64_t length = 0;
    uint64_t offset_dev = 0;
    uint64_t offset_vmo = 0;

    switch (request.operation.tag) {
      case block_server::Operation::Tag::Read:
        length = request.operation.read.block_count;
        offset_dev = request.operation.read.device_block_offset;
        offset_vmo = request.operation.read.vmo_offset / block_info_.block_size;
        fdf::trace("Read {} blocks at {}", length, offset_dev);
        break;
      case block_server::Operation::Tag::Write:
        length = request.operation.write.block_count;
        offset_dev = request.operation.write.device_block_offset;
        offset_vmo = request.operation.write.vmo_offset / block_info_.block_size;
        fdf::trace("Write {} blocks at {}", length, offset_dev);
        break;
      case block_server::Operation::Tag::Flush:
        fdf::trace("Flush");
        break;
      default:
        io_cmd.Complete(ZX_ERR_NOT_SUPPORTED);
        continue;
    }

    if (length > 0) {
      if (zx_status_t status = block_server::CheckIoRange(request, block_info_.block_count);
          status != ZX_OK) {
        io_cmd.Complete(status);
        continue;
      }
      if (length > max_transfer_blocks_) {
        fdf::error("Io request size {} is larger than max transfer size {}", length,
                   max_transfer_blocks_);
        io_cmd.Complete(ZX_ERR_INVALID_ARGS);
        continue;
      }
    }

    controller_->QueueIoCommand(&io_cmd);
  }
}

fdf::Logger& Namespace::logger() const { return controller_->logger(); }

void Namespace::CompleteIoCommand(IoCommand* io_cmd, zx_status_t status) {
  fbl::AutoLock lock(&lock_);
  block_server_->SendReply(*io_cmd->request_id, zx::make_result(status));
  FreeIoCommand(io_cmd);
}

void Namespace::ServeRequests(fidl::ServerEnd<fuchsia_storage_block::Block> server_end) {
  fbl::AutoLock lock(&lock_);
  if (shutdown_) {
    return;
  }
  block_server_->Serve(std::move(server_end));
}

void Namespace::Get(GetCompleter::Sync& completer) {
  zx::event token = controller_->node_token();
  if (token.is_valid()) {
    completer.Reply(zx::ok(std::move(token)));
  } else {
    completer.Reply(zx::error(ZX_ERR_NOT_FOUND));
  }
}

}  // namespace nvme

// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/i2c/drivers/i2c/i2c.h"

#include <fidl/fuchsia.hardware.i2c/cpp/fidl.h>
#include <fidl/fuchsia.hardware.i2cimpl/cpp/fidl.h>
#include <fidl/fuchsia.scheduler/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_export.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/metadata/cpp/metadata.h>
#include <lib/trace/event.h>

#include "src/devices/i2c/drivers/i2c/i2c_parser.h"

namespace i2c {

namespace {
fuchsia_hardware_i2c_businfo::I2CBusMetadata ConvertMetadata(i2c_metadata::I2cMetadata generic) {
  std::vector<fuchsia_hardware_i2c_businfo::I2CChannel> channels;
  for (auto& c : generic.channels) {
    fuchsia_hardware_i2c_businfo::I2CChannel channel;
    channel.address(c.address);
    if (c.id) {
      channel.global_id(*c.id);
    }
    if (c.name) {
      channel.name(std::move(*c.name));
    }
    channels.push_back(std::move(channel));
  }
  return {{.channels = std::move(channels), .bus_id = generic.bus_id}};
}
}  // namespace

zx::result<> I2cDriver::Start(fdf::DriverContext context) {
  auto incoming = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  auto i2cimpl_result = incoming->Connect<fuchsia_hardware_i2cimpl::Service::Device>();
  if (i2cimpl_result.is_error()) {
    fdf::error("Failed to connect to fuchsia.hardware.i2cimpl service: {}", i2cimpl_result);
    return i2cimpl_result.take_error();
  }
  i2c_.Bind(std::move(*i2cimpl_result), fdf::Dispatcher::GetCurrent()->get());

  fidl::Arena arena;
  std::optional<fuchsia_hardware_i2c_businfo::I2CBusMetadata> metadata;

  // Try to get generic metadata first
  zx::result generic_res =
      fdf_metadata::GetMetadataFromFidlServiceIfExists<fuchsia_driver_metadata::Dictionary>(
          incoming->svc_dir(), "fuchsia.hardware.i2c.businfo.I2CBusMetadata");
  if (generic_res.is_ok() && generic_res.value().has_value()) {
    auto parsed = i2c_metadata::I2cMetadata::Parse(*generic_res.value());
    if (parsed) {
      metadata = ConvertMetadata(std::move(*parsed));
    } else {
      fdf::error("Failed to parse generic I2C metadata");
    }
  }

  if (!metadata.has_value()) {
    // Fall back to old metadata
    zx::result i2c_bus_metadata_res =
        fdf_metadata::GetMetadata<fuchsia_hardware_i2c_businfo::I2CBusMetadata>(incoming);
    if (i2c_bus_metadata_res.is_error()) {
      fdf::error("Failed to get i2c_bus_metadata  {}", i2c_bus_metadata_res);
      return i2c_bus_metadata_res.take_error();
    }
    metadata = std::move(*i2c_bus_metadata_res);
  }

  fuchsia_hardware_i2c_businfo::I2CBusMetadata i2c_bus_metadata = std::move(*metadata);

  if (!i2c_bus_metadata.channels().has_value()) {
    fdf::error("No channels supplied from the metadata");
    return zx::error(ZX_ERR_NO_RESOURCES);
  }

  fdf::Arena i2c_arena('I2CI');
  fdf::WireUnownedResult max_transfer_size = i2c_.sync().buffer(i2c_arena)->GetMaxTransferSize();
  if (!max_transfer_size.ok()) {
    fdf::error("Failed to send GetMaxTransferSize request: {}", max_transfer_size.status_string());
    return zx::error(max_transfer_size.status());
  }
  if (max_transfer_size->is_error()) {
    fdf::error("Failed to get max transfer size: {}",
               zx_status_get_string(max_transfer_size->error_value()));
    return zx::error(max_transfer_size->error_value());
  }
  max_transfer_ = max_transfer_size->value()->size;

  // Add owned i2c node.
  zx::result child = AddOwnedChild("i2c");
  if (child.is_error()) {
    fdf::error("failed to add i2c child node: {}", child);
    return child.take_error();
  }

  i2c_node_ = std::move(child->node_);

  const auto config = context.take_config<i2c_config::Config>();
  if (zx::result<> result = AddI2cChildren(i2c_bus_metadata, config, context.node_name(), incoming);
      result.is_error()) {
    return result;
  }

  zx_status_t status =
      fdf_dispatcher_seal(driver_dispatcher()->get(), FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS);
  if (status != ZX_OK) {
    fdf::error("Failed to sync seal dispatcher: {}", zx_status_get_string(status));
    return zx::error(status);
  }
  return zx::ok();
}

void I2cDriver::Stop(fdf::StopCompleter completer) {
  shutdown_ = true;
  completer(zx::ok());
}

I2cDriver::~I2cDriver() {
  // The dispatcher has been stopped, meaning the current request must have already been completed.
  ZX_DEBUG_ASSERT(!current_request_.has_value());
  std::ranges::for_each(pending_requests_.begin(), pending_requests_.end(),
                        [](Request& request) { request.Complete(ZX_ERR_CANCELED); });
}

zx::result<> I2cDriver::AddI2cChildren(const fuchsia_hardware_i2c_businfo::I2CBusMetadata& metadata,
                                       i2c_config::Config config,
                                       const std::optional<std::string>& node_name,
                                       const std::shared_ptr<fdf::Namespace>& incoming) {
  if (!metadata.channels()) {
    fdf::error("Failed to find number of channels in metadata: {}",
               zx_status_get_string(ZX_ERR_NOT_FOUND));
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  fdf::debug("Number of i2c channels supplied: {}", metadata.channels()->size());
  const uint32_t bus_id = metadata.bus_id().value_or(0);
  for (const auto& channel : metadata.channels().value()) {
    // Add an i2c child to the owned i2c node.
    auto i2c_child_server = I2cChildServer::CreateAndAddChild(
        fit::bind_member(this, &I2cDriver::Transact), i2c_node_, logger(), bus_id, channel,
        incoming, outgoing(), node_name, config);
    if (i2c_child_server.is_error()) {
      fdf::error("Failed to create child server: {}",
                 zx_status_get_string(i2c_child_server.error_value()));
      return i2c_child_server.take_error();
    }
    child_servers_.push_back(std::move(i2c_child_server.value()));
  }

  return zx::ok();
}

void I2cDriver::Transact(uint16_t address, TransferRequestView request,
                         TransferCompleter::Sync& completer) {
  TRACE_DURATION("i2c", "I2cDevice Process Queued Transacts");

  if (shutdown_) {
    completer.ReplyError(ZX_ERR_CANCELED);
    return;
  }

  if (request->transactions.size() < 1) {
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }
  if (request->transactions.size() > fuchsia_hardware_i2c::wire::kMaxCountTransactions) {
    completer.ReplyError(ZX_ERR_OUT_OF_RANGE);
    return;
  }

  RequestConverter converter(request, address, max_transfer_);

  if (current_request_.has_value()) {
    if (pending_requests_.size() >= kMaxTransactionsPerChild * child_servers_.size()) {
      fdf::error("Queue is full, dropping request");
      completer.ReplyError(ZX_ERR_SHOULD_WAIT);
      return;
    }

    // An I2C request is already pending. Save this request and push it to the queue to be completed
    // later.
    zx_status_t status = pending_requests_.emplace_back(completer.ToAsync()).SaveRequest(converter);
    if (status != ZX_OK) {
      pending_requests_.back().Complete(status);
      pending_requests_.pop_back();
    }
    return;
  }

  // If no request is pending, we can immediately call to the i2cimpl driver without persisting the
  // request data.

  impl_ops_.clear();
  impl_ops_.insert(impl_ops_.cend(), request->transactions.size(), {});
  zx_status_t status = converter.Convert(
      [](fidl::VectorView<uint8_t>& write_vector) {
        return fidl::ObjectView<fidl::VectorView<uint8_t>>::FromExternal(&write_vector);
      },
      impl_ops_);
  if (status == ZX_OK) {
    current_request_.emplace(
        fidl::VectorView<fuchsia_hardware_i2cimpl::wire::I2cImplOp>::FromExternal(impl_ops_),
        completer.ToAsync());
    StartCurrentRequest();
  } else {
    completer.ReplyError(status);
  }
  impl_ops_.clear();
}

void I2cDriver::StartCurrentRequest() {
  ZX_DEBUG_ASSERT(current_request_.has_value());
  RequestStorage& request = *current_request_;
  i2c_.buffer(request.arena())
      ->Transact(request->ops())
      .ThenExactlyOnce(fit::bind_member<&I2cDriver::CompleteRequest>(this));
}

void I2cDriver::CompleteRequest(
    fdf::WireUnownedResult<fuchsia_hardware_i2cimpl::Device::Transact>& result) {
  TRACE_DURATION("i2c", "I2cDevice Complete Transacts");

  ZX_DEBUG_ASSERT(current_request_.has_value());

  if (RequestStorage& request = current_request_.value(); !result.ok()) {
    if (!shutdown_ || !result.error().is_dispatcher_shutdown()) {
      fdf::error("Failed to send Transfer request: {}", result.status_string());
    }
    request->Complete(result.status());
  } else if (result->is_error()) {
    // Don't log at ERROR severity here, as some I2C devices intentionally NACK to indicate that
    // they are busy.
    fdf::debug("Failed to perform transfer: {}", zx_status_get_string(result->error_value()));
    request->Complete(result->error_value());
  } else {
    read_vectors_.clear();
    for (const auto& read : result.value()->read) {
      read_vectors_.emplace_back(read.data);
    }
    request->Complete(fidl::VectorView<fidl::VectorView<uint8_t>>::FromExternal(read_vectors_));
    read_vectors_.clear();
  }

  current_request_.reset();
  // Break on shutdown to prevent recursive calls to cancel every request in the queue.
  if (!pending_requests_.empty() && !shutdown_) {
    current_request_.emplace(&pending_requests_.front(), GetCurrentRequestDeleter());
    StartCurrentRequest();
  }
}

}  // namespace i2c

FUCHSIA_DRIVER_EXPORT2(i2c::I2cDriver);

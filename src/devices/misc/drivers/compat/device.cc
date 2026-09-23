// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/misc/drivers/compat/device.h"

#include <fidl/fuchsia.driver.compat/cpp/wire.h>
#include <fidl/fuchsia.driver.framework/cpp/wire_types.h>
#include <lib/async/cpp/task.h>
#include <lib/ddk/binding_priv.h>
#include <lib/driver/compat/cpp/symbols.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/fdf/cpp/protocol.h>
#include <lib/fit/defer.h>
#include <lib/fpromise/bridge.h>
#include <lib/stdcompat/span.h>
#include <zircon/compiler.h>
#include <zircon/errors.h>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/platform/cpp/bind.h>

#include "driver.h"
#include "src/devices/misc/drivers/compat/composite_node_spec_util.h"

namespace fdf {
using namespace fuchsia_driver_framework;
}
namespace fcd = fuchsia_component_decl;
namespace fdm = fuchsia_system_state;

namespace {

struct ProtocolInfo {
  std::string_view name;
  uint32_t id;
  uint32_t flags;
};

static constexpr ProtocolInfo kProtocolInfos[] = {
#define DDK_PROTOCOL_DEF(tag, val, name, flags) {name, val, flags},
#include <lib/ddk/protodefs.h>
};

// TODO(https://fxbug.dev/42077603): we pass a bad URL to |NodeController::RequestBind|
// to unbind the driver of a node but not rebind it. This is a temporary
// workaround to pass the fshost tests in DFv2.
static constexpr std::string_view kKnownBadDriverUrl = "not-a-real-driver-url-see-fxb-126978";

fidl::StringView ProtocolIdToClassName(uint32_t protocol_id) {
  for (const ProtocolInfo& info : kProtocolInfos) {
    if (info.id != protocol_id) {
      continue;
    }
    if (info.flags & PF_NOPUB) {
      return {};
    }
    return fidl::StringView::FromExternal(info.name);
  }
  return {};
}

template <typename T>
bool HasOp(const zx_protocol_device_t* ops, T member) {
  return ops != nullptr && ops->*member != nullptr;
}

std::vector<std::string> MakeZirconServiceOffers(device_add_args_t* zx_args) {
  std::vector<std::string> offers;
  for (const auto& offer :
       cpp20::span(zx_args->fidl_service_offers, zx_args->fidl_service_offer_count)) {
    offers.push_back(std::string(offer));
  }
  return offers;
}

std::vector<std::string> MakeDriverServiceOffers(device_add_args_t* zx_args) {
  std::vector<std::string> offers;
  for (const auto& offer :
       cpp20::span(zx_args->runtime_service_offers, zx_args->runtime_service_offer_count)) {
    offers.push_back(std::string(offer));
  }
  return offers;
}

uint8_t PowerStateToSuspendReason(fdm::SystemPowerState power_state) {
  switch (power_state) {
    case fdm::SystemPowerState::kReboot:
      return DEVICE_SUSPEND_REASON_REBOOT;
    case fdm::SystemPowerState::kRebootRecovery:
      return DEVICE_SUSPEND_REASON_REBOOT_RECOVERY;
    case fdm::SystemPowerState::kRebootBootloader:
      return DEVICE_SUSPEND_REASON_REBOOT_BOOTLOADER;
    case fdm::SystemPowerState::kMexec:
      return DEVICE_SUSPEND_REASON_MEXEC;
    case fdm::SystemPowerState::kPoweroff:
      return DEVICE_SUSPEND_REASON_POWEROFF;
    case fdm::SystemPowerState::kSuspendRam:
      return DEVICE_SUSPEND_REASON_SUSPEND_RAM;
    case fdm::SystemPowerState::kRebootKernelInitiated:
      return DEVICE_SUSPEND_REASON_REBOOT_KERNEL_INITIATED;
    default:
      return DEVICE_SUSPEND_REASON_SELECTIVE_SUSPEND;
  }
}

}  // namespace

namespace compat {

std::vector<fuchsia_driver_framework::wire::NodeProperty2> CreateProperties(
    fidl::AnyArena& arena, fdf::Logger& logger, device_add_args_t* zx_args) {
  std::vector<fuchsia_driver_framework::wire::NodeProperty2> properties;
  properties.reserve(zx_args->str_prop_count + zx_args->fidl_service_offer_count + 1);
  bool has_protocol = false;
  for (auto [key, value] : cpp20::span(zx_args->str_props, zx_args->str_prop_count)) {
    if (key == std::string_view(bind_fuchsia::PROTOCOL)) {
      has_protocol = true;
    }
    switch (value.data_type) {
      case ZX_DEVICE_PROPERTY_VALUE_BOOL:
        properties.emplace_back(fdf::MakeProperty2(arena, key, value.data.bool_val));
        break;
      case ZX_DEVICE_PROPERTY_VALUE_STRING:
        properties.emplace_back(fdf::MakeProperty2(arena, key, value.data.str_val));
        break;
      case ZX_DEVICE_PROPERTY_VALUE_INT:
        properties.emplace_back(fdf::MakeProperty2(arena, key, value.data.int_val));
        break;
      case ZX_DEVICE_PROPERTY_VALUE_ENUM:
        properties.emplace_back(fdf::MakeProperty2(arena, key, value.data.enum_val));
        break;
      default:
        logger.log(fdf::ERROR, "Unsupported property type, key: {}", key);
        break;
    }
  }

  // Some DFv1 devices expect to be able to set their own protocol, without specifying proto_id.
  // If we see a BIND_PROTOCOL property, don't add our own.
  if (!has_protocol) {
    // If we do not have a protocol id, set it to MISC to match DFv1 behavior.
    uint32_t proto_id = zx_args->proto_id == 0 ? ZX_PROTOCOL_MISC : zx_args->proto_id;
    properties.emplace_back(fdf::MakeProperty2(arena, bind_fuchsia::PROTOCOL, proto_id));
  }
  return properties;
}

Device::DelayedReleaseOp::DelayedReleaseOp(std::shared_ptr<Device> device) {
  memcpy(&compat_symbol, &device->compat_symbol_, sizeof(compat_symbol));
  memcpy(&ops, &device->ops_, sizeof(ops));
}

Device::DelayedReleaseOp::~DelayedReleaseOp() {
  // We shouldn't need to call the parent's pre-release hook here,
  // as we should have only delayed the release hook if the device
  // was the last device of the driver.
  if (HasOp(ops, &zx_protocol_device_t::release)) {
    ops->release(compat_symbol.context);
  }
}

Device::Device(device_t device, const zx_protocol_device_t* ops, Driver* driver,
               std::optional<Device*> parent, std::shared_ptr<fdf::Logger> logger,
               async_dispatcher_t* dispatcher)
    : devfs_connector_([this](fidl::ServerEnd<fuchsia_device::Controller> controller) {
        devfs_server_.ServeDeviceFidl(controller.TakeChannel());
      }),
      devfs_controller_connector_([this](fidl::ServerEnd<fuchsia_device::Controller> server_end) {
        dev_controller_bindings_.AddBinding(dispatcher_, std::move(server_end), this,
                                            fidl::kIgnoreBindingClosure);
      }),
      devfs_server_(*this, dispatcher),
      name_(device.name),
      logger_(std::move(logger)),
      dispatcher_(dispatcher),
      driver_(driver),
      compat_symbol_(device),
      ops_(ops),
      parent_(parent),
      executor_(dispatcher) {}

Device::~Device() {
  if (!children_.empty()) {
    logger_->log(fdf::WARN, "{}: Destructing device, but still had {} children", Name(),
                 children_.size());
    // Ensure we do not get use-after-free from calling child_pre_release
    // on a destructed parent device.
    children_.clear();
  }

  if (ShouldCallRelease()) {
    // Call the parent's pre-release.
    if (HasOp((*parent_)->ops_, &zx_protocol_device_t::child_pre_release)) {
      (*parent_)->ops_->child_pre_release((*parent_)->compat_symbol_.context,
                                          compat_symbol_.context);
    }

    if (!release_after_dispatcher_shutdown_ && HasOp(ops_, &zx_protocol_device_t::release)) {
      ops_->release(compat_symbol_.context);
    }
  }

  for (auto& completer : remove_completers_) {
    completer.complete_ok();
  }
}

zx_device_t* Device::ZxDevice() { return static_cast<zx_device_t*>(this); }

void Device::Bind(fidl::WireSharedClient<fdf::Node> node) { node_ = std::move(node); }

void Device::Unbind() {
  // This closes the client-end of the node to signal to the driver framework
  // that node should be removed.
  //
  // `fidl::WireClient` does not provide a direct way to unbind a client, so we
  // assign a default client to unbind the existing client.
  node_ = {};
}

fpromise::promise<void> Device::HandleStopSignal() {
  if (system_power_state() == fdm::SystemPowerState::kFullyOn) {
    // kFullyOn means that power manager hasn't initiated a system power state transition. As a
    // result, we can assume our stop request came as a result of our parent node
    // disappearing.
    return UnbindOp();
  }
  return SuspendOp();
}

fpromise::promise<void> Device::UnbindOp() {
  ZX_ASSERT_MSG(!unbind_completer_, "Cannot call UnbindOp twice");
  fpromise::bridge<void> finished_bridge;
  unbind_completer_ = std::move(finished_bridge.completer);

  // If we are being unbound we have to remove all of our children first.
  return RemoveChildren().then(
      [this, bridge = std::move(finished_bridge)](fpromise::result<>& result) mutable {
        // We don't call unbind on the root parent device because it belongs to another driver.
        // We find the root parent device because it does not have parent_ set.
        if (parent_.has_value() && HasOp(ops_, &zx_protocol_device_t::unbind)) {
          // CompleteUnbind will be called from |device_unbind_reply|.
          ops_->unbind(compat_symbol_.context);
        } else {
          CompleteUnbind();
        }
        return bridge.consumer.promise();
      });
}

fpromise::promise<void> Device::SuspendOp() {
  ZX_ASSERT_MSG(!suspend_completer_, "Cannot call HandleStopRequest twice");
  fpromise::bridge<void> finished_bridge;
  suspend_completer_ = std::move(finished_bridge.completer);

  // If we are being suspended we have to suspend all of our children first.
  return SuspendChildren()
      .then([this, bridge = std::move(finished_bridge)](fpromise::result<>& result) mutable {
        // We don't call unbind on the root parent device because it belongs to another driver.
        // We find the root parent device because it does not have parent_ set.
        if (parent_.has_value() && HasOp(ops_, &zx_protocol_device_t::suspend)) {
          // CompleteSuspend will be called from |device_suspend_reply|.
          ops_->suspend(compat_symbol_.context, DEV_POWER_STATE_D3COLD, false,
                        PowerStateToSuspendReason(system_power_state()));
        } else {
          CompleteSuspend();
        }
        return bridge.consumer.promise();
      })
      .wrap_with(scope_);
}

void Device::CompleteUnbind() {
  auto task = fpromise::make_ok_promise()
                  .and_then([this]() mutable {
                    // Remove ourself from devfs.
                    devfs_connector_.reset();
                    dev_controller_bindings_.CloseAll(ZX_OK);
                    // Our unbind is finished, so close all outstanding connections to devfs
                    // clients.
                    devfs_server_.CloseAllConnections([this]() {
                      // Now call our unbind completer.
                      ZX_ASSERT(unbind_completer_);
                      unbind_completer_.complete_ok();
                    });
                  })
                  .wrap_with(scope_);
  executor_.schedule_task(std::move(task));
}

void Device::CompleteSuspend() {
  ZX_ASSERT(suspend_completer_);
  suspend_completer_.complete_ok();
}

const char* Device::Name() const { return name_.c_str(); }

bool Device::HasChildren() const { return !children_.empty(); }

fdm::SystemPowerState Device::system_power_state() const {
  return driver_ ? driver_->system_state() : fdm::SystemPowerState::kFullyOn;
}

bool Device::stop_triggered() const { return driver_ ? driver_->stop_triggered() : false; }

zx_status_t Device::Add(device_add_args_t* zx_args, zx_device_t** out) {
  if (HasChildNamed(zx_args->name)) {
    return ZX_ERR_BAD_STATE;
  }
  if (stop_triggered()) {
    return ZX_ERR_BAD_STATE;
  }
  device_t compat_device = {
      .name = zx_args->name,
      .context = zx_args->ctx,
  };

  auto device =
      std::make_shared<Device>(compat_device, zx_args->ops, driver_, this, logger_, dispatcher_);
  // Update the compat symbol name pointer with a pointer the device owns.
  device->compat_symbol_.name = device->name_.data();

  if (driver()) {
    device->device_id_ = driver()->GetNextDeviceId();
  }

  auto outgoing_name = device->OutgoingName();

  std::optional<ServiceOffersV1> service_offers;
  if (zx_args->outgoing_dir_channel != ZX_HANDLE_INVALID) {
    service_offers = ServiceOffersV1(
        outgoing_name,
        fidl::ClientEnd<fuchsia_io::Directory>(zx::channel(zx_args->outgoing_dir_channel)),
        MakeZirconServiceOffers(zx_args), MakeDriverServiceOffers(zx_args));
  }

  if (zx_args->inspect_vmo != ZX_HANDLE_INVALID) {
    zx_status_t status = device->PublishInspect(zx::vmo(zx_args->inspect_vmo));
    if (status != ZX_OK) {
      return status;
    }
  }

  DeviceServer::BanjoConfig banjo_config{zx_args->proto_id};

  // Set the callback specifically for the base proto_id if there is one.
  if (zx_args->proto_ops != nullptr && zx_args->proto_id != 0) {
    banjo_config.callbacks[zx_args->proto_id] = [ops = zx_args->proto_ops, ctx = zx_args->ctx]() {
      return DeviceServer::GenericProtocol{.ops = const_cast<void*>(ops), .ctx = ctx};
    };
  }

  // Set a generic callback for other proto_ids.
  banjo_config.generic_callback =
      [device =
           std::weak_ptr(device)](uint32_t proto_id) -> zx::result<DeviceServer::GenericProtocol> {
    std::shared_ptr dev = device.lock();
    if (!dev) {
      return zx::error(ZX_ERR_BAD_STATE);
    }

    DeviceServer::GenericProtocol protocol;
    if (HasOp(dev->ops_, &zx_protocol_device_t::get_protocol)) {
      zx_status_t status =
          dev->ops_->get_protocol(dev->compat_symbol_.context, proto_id, &protocol);
      if (status != ZX_OK) {
        return zx::error(status);
      }

      return zx::ok(protocol);
    }

    return zx::error(ZX_ERR_PROTOCOL_NOT_SUPPORTED);
  };

  device->device_server_.Initialize(outgoing_name, std::move(service_offers),
                                    std::move(banjo_config));

  // Add the metadata from add_args:
  for (size_t i = 0; i < zx_args->metadata_count; i++) {
    auto status =
        device->AddMetadata(zx_args->metadata_list[i].type, zx_args->metadata_list[i].data,
                            zx_args->metadata_list[i].length);
    if (status != ZX_OK) {
      return status;
    }
  }

  device->properties_ = CreateProperties(arena_, *logger_, zx_args);
  device->device_flags_ = zx_args->flags;

  if (zx_args->bus_info != nullptr) {
    device->bus_info_ = *reinterpret_cast<const fdf::BusInfo*>(zx_args->bus_info);
  }

  if (out) {
    *out = device->ZxDevice();
  }

  if (HasOp(device->ops_, &zx_protocol_device_t::init)) {
    // We have to schedule the init task so that it is run in the dispatcher context,
    // as we are currently in the device context from device_add_from_driver().
    // (We are not allowed to re-enter the device context).
    device->executor_.schedule_task(fpromise::make_ok_promise().and_then(
        [device]() mutable { device->ops_->init(device->compat_symbol_.context); }));
  } else {
    device->InitReply(ZX_OK);
  }

  children_.push_back(std::move(device));
  return ZX_OK;
}

zx_status_t Device::ExportAfterInit() {
  if (stop_triggered()) {
    return ZX_ERR_BAD_STATE;
  }
  if (zx_status_t status = device_server_.Serve(dispatcher_, &driver()->outgoing());
      status != ZX_OK) {
    logger_->log(fdf::INFO, "Device {} failed to add to outgoing directory: {}", OutgoingName(),
                 zx::make_result(status));
    return status;
  }

  if (zx_status_t status = CreateNode(); status != ZX_OK) {
    logger_->log(fdf::ERROR, "Device {}: failed to create node: {}", OutgoingName(),
                 zx::make_result(status));
    return status;
  }

  return ZX_OK;
}

zx_status_t Device::CreateNode() {
  // Create NodeAddArgs from `zx_args`.
  fidl::Arena arena;
  auto offers = device_server_.CreateOffers2(arena);

  std::vector<fdf::wire::NodeSymbol> symbols;
  symbols.emplace_back(fdf::wire::NodeSymbol::Builder(arena)
                           .name(kDeviceSymbol)
                           .address(reinterpret_cast<uint64_t>(&compat_symbol_))
                           .Build());
  symbols.emplace_back(fdf::wire::NodeSymbol::Builder(arena)
                           .name(kOps)
                           .address(reinterpret_cast<uint64_t>(ops_))
                           .Build());

  auto args_builder =
      fdf::wire::NodeAddArgs::Builder(arena)
          .name(fidl::StringView::FromExternal(name_))
          .symbols(fidl::VectorView<fdf::wire::NodeSymbol>::FromExternal(symbols))
          .properties2(fidl::VectorView<fdf::wire::NodeProperty2>::FromExternal(properties_))
          .offers2(fidl::VectorView<fdf::wire::Offer>::FromExternal(offers.data(), offers.size()));

  if (bus_info_) {
    args_builder.bus_info(fidl::ToWire(arena, bus_info_.value()));
  }

  // Create NodeController, so we can control the device.
  auto controller_ends = fidl::CreateEndpoints<fdf::NodeController>();
  if (controller_ends.is_error()) {
    return controller_ends.status_value();
  }

  fpromise::bridge<> teardown_bridge;
  controller_teardown_finished_.emplace(teardown_bridge.consumer.promise());
  controller_.Bind(
      std::move(controller_ends->client), dispatcher_,
      fidl::ObserveTeardown([device = weak_from_this(),
                             completer = std::move(teardown_bridge.completer)]() mutable {
        // Because the dispatcher can be multi-threaded, we must use a
        // `fidl::WireSharedClient`. The `fidl::WireSharedClient` uses a
        // two-phase destruction to teardown the client.
        //
        // Because of this, the teardown might be happening after the
        // Device has already been erased. This is likely to occur if the
        // Driver is asked to shutdown. If that happens, the Driver will
        // free its Devices, the Device will release its NodeController,
        // and then this shutdown will occur later. In order to not have a
        // Use-After-Free here, only try to remove the Device if the
        // weak_ptr still exists.
        //
        // The weak pointer will be valid here if the NodeController
        // representing the Device exits on its own. This represents the
        // Device's child Driver exiting, and in that instance we want to
        // Remove the Device.
        if (auto ptr = device.lock()) {
          ptr->controller_ = {};
          // Only remove us if the driver requested it (normally via device_async_remove)
          if (ptr->pending_removal_) {
            ptr->UnbindAndRelease();
          } else {
            // TODO(https://fxbug.dev/42051188): We currently do not remove the DFv1 child
            // if the NodeController is removed but the driver didn't ask to be
            // removed. We need to investigate the correct behavior here.
            ptr->logger().log(fdf::INFO, "Device {} has its NodeController unexpectedly removed",
                              (ptr)->OutgoingName());
          }
        }
        completer.complete_ok();
      }));

  // If the node is not bindable, we own the node.
  fidl::ServerEnd<fdf::Node> node_server;
  if ((device_flags_ & DEVICE_ADD_NON_BINDABLE) != 0) {
    auto node_ends = fidl::CreateEndpoints<fdf::Node>();
    if (node_ends.is_error()) {
      return node_ends.status_value();
    }
    node_.Bind(std::move(node_ends->client), dispatcher_);
    node_server = std::move(node_ends->server);
  }

  if (!parent_.value()->node_.is_valid()) {
    if (parent_.value()->device_flags_ & DEVICE_ADD_NON_BINDABLE) {
      logger_->log(fdf::ERROR, "Cannot add device, as parent '{}' does not have a valid node",
                   (*parent_)->OutgoingName());
    } else {
      logger_->log(fdf::ERROR, "Cannot add device, as parent '{}' is not marked NON_BINDABLE.",
                   (*parent_)->OutgoingName());
    }
    return ZX_ERR_NOT_SUPPORTED;
  }

  // Set up devfs information.
  {
    if (!devfs_connector_.has_value() || !devfs_controller_connector_.has_value()) {
      logger_->log(fdf::ERROR, "Device {} failed to add to devfs: no devfs_connector",
                   OutgoingName());
      return ZX_ERR_INTERNAL;
    }

    if (devfs_connector_->binding().has_value()) {
      devfs_connector_->binding().reset();
    }

    if (devfs_controller_connector_->binding().has_value()) {
      devfs_controller_connector_->binding().reset();
    }

    zx::result connector = devfs_connector_.value().Bind(dispatcher());
    if (connector.is_error()) {
      logger_->log(fdf::ERROR, "Device {} failed to create devfs connector: {}", OutgoingName(),
                   connector);
      return connector.error_value();
    }

    zx::result controller_connector = devfs_controller_connector_.value().Bind(dispatcher());
    if (controller_connector.is_error()) {
      logger_->log(fdf::ERROR, "Device {} failed to create devfs controller_connector: {}",
                   OutgoingName(), controller_connector);
      return controller_connector.error_value();
    }
    auto devfs_args = fdf::wire::DevfsAddArgs::Builder(arena)
                          .connector(std::move(connector.value()))
                          .connector_supports(fuchsia_device_fs::ConnectionType::kDevice |
                                              fuchsia_device_fs::ConnectionType::kController)
                          .controller_connector(std::move(controller_connector.value()));
    fidl::StringView class_name = ProtocolIdToClassName(device_server_.proto_id());
    if (!class_name.empty()) {
      devfs_args.class_name(class_name);
    }

    // TODO(b/324637276): this is where the component is exporting its data back to driver_manager
    if (inspect_vmo_.has_value()) {
      zx::vmo inspect;
      zx_status_t status = inspect_vmo_->duplicate(ZX_RIGHT_SAME_RIGHTS, &inspect);
      if (status != ZX_OK) {
        logger_->log(fdf::ERROR, "Failed to duplicate inspect vmo: {}", zx::make_result(status));
      } else {
        devfs_args.inspect(std::move(inspect));
      }
    }
    args_builder.devfs_args(devfs_args.Build());
  }

  // Add the device node.
  fpromise::bridge<void, std::variant<zx_status_t, fdf::NodeError>> bridge;
  auto callback = [completer = std::move(bridge.completer)](
                      fidl::WireUnownedResult<fdf::Node::AddChild>& result) mutable {
    if (!result.ok()) {
      completer.complete_error(result.error().status());
      return;
    }
    if (result->is_error()) {
      completer.complete_error(result->error_value());
      return;
    }
    completer.complete_ok();
  };
  parent_.value()
      ->node_
      ->AddChild(args_builder.Build(), std::move(controller_ends->server), std::move(node_server))
      .ThenExactlyOnce(std::move(callback));

  auto task =
      bridge.consumer.promise()
          .then([this](fpromise::result<void, std::variant<zx_status_t, fdf::NodeError>>& result) {
            if (result.is_ok()) {
              if (HasOp(ops_, &zx_protocol_device_t::made_visible)) {
                ops_->made_visible(compat_symbol_.context);
              }
              return;
            }
            if (auto error = std::get_if<zx_status_t>(&result.error()); error) {
              if (*error == ZX_ERR_PEER_CLOSED) {
                // This is a warning because it can happen during shutdown.
                logger_->log(fdf::WARN, "{}: Node channel closed while adding device", Name());
              } else {
                logger_->log(fdf::ERROR, "Failed to add device: {}: status: {}", Name(),
                             zx::make_result(*error));
              }
            } else if (auto error = std::get_if<fdf::NodeError>(&result.error()); error) {
              if (*error == fdf::NodeError::kNodeRemoved) {
                // This is a warning because it can happen if the parent driver is unbound while we
                // are still setting up.
                logger_->log(fdf::WARN, "Failed to add device '{}' while parent was removed",
                             Name());
              } else {
                logger_->log(fdf::ERROR, "Failed to add device: NodeError: '{}': {}", Name(),
                             static_cast<unsigned int>(*error));
              }
            }
          })
          .wrap_with(scope_);
  executor_.schedule_task(std::move(task));
  return ZX_OK;
}

fpromise::promise<void> Device::RemoveChildren() {
  std::vector<fpromise::promise<void>> promises;
  for (auto& child : children_) {
    promises.push_back(child->Remove());
  }
  return fpromise::join_promise_vector(std::move(promises))
      .then([](fpromise::result<std::vector<fpromise::result<void>>>& results) {
        if (results.is_error()) {
          return fpromise::make_error_promise();
        }
        for (auto& result : results.value()) {
          if (result.is_error()) {
            return fpromise::make_error_promise();
          }
        }
        return fpromise::make_ok_promise();
      });
}

fpromise::promise<void> Device::SuspendChildren() {
  std::vector<fpromise::promise<void>> promises;
  for (auto& child : children_) {
    promises.push_back(child->SuspendOp());
  }
  return fpromise::join_promise_vector(std::move(promises))
      .then([](fpromise::result<std::vector<fpromise::result<void>>>& results) {
        if (results.is_error()) {
          return fpromise::make_error_promise();
        }
        for (auto& result : results.value()) {
          if (result.is_error()) {
            return fpromise::make_error_promise();
          }
        }
        return fpromise::make_ok_promise();
      });
}

fpromise::promise<void> Device::Remove() {
  fpromise::bridge<void> finished_bridge;
  remove_completers_.push_back(std::move(finished_bridge.completer));

  // We purposefully do not capture a shared_ptr to Device in the lambda.
  // This is as we want the device to be destructed on the parent's executor
  // as scheduled by UnbindAndRelease(). Otherwise, it would be possible for
  // this task to be holding the last shared_ptr reference, and the executor
  // will assert that a task is still running (ourself) during shutdown.
  //
  // We are guaranteed that the pointer will still be alive, as either
  // the device has not yet been destructed, or the device has been
  // destructed and the executor has purged all queued tasks during shutdown.
  //
  // Since all executors for the compat devices in the driver share a dispatcher,
  // we are guaranteed that this task cannot be running at the same time as
  // the task that destructs the device.
  executor_.schedule_task(
      WaitForInitToComplete().then([device = this](fpromise::result<void, zx_status_t>& init) {
        // If we don't have a controller, return early.
        // We are probably in a state where we are waiting for the controller to finish being
        // removed.
        if (!device->controller_) {
          if (!device->pending_removal_) {
            // Our controller is already gone but we weren't in a removal, so manually remove
            // ourself now.
            device->pending_removal_ = true;
            device->UnbindAndRelease();
          }
          return;
        }

        device->pending_removal_ = true;
        auto result = device->controller_->Remove();
        // If we hit an error calling remove, we should log it.
        // We don't need to log if the error is that we cannot connect
        // to the protocol, because that means we are already in the process
        // of shutting down.
        if (!result.ok() && !result.is_canceled()) {
          device->logger_->log(fdf::ERROR, "Failed to remove device '{}': {}", device->Name(),
                               result.error());
        }
      }));
  return finished_bridge.consumer.promise();
}

void Device::UnbindAndRelease() {
  ZX_ASSERT_MSG(parent_.has_value(), "UnbindAndRelease called without a parent_: %s",
                OutgoingName().c_str());

  // We schedule our removal on our parent's executor because we can't be removed
  // while being run in a promise on our own executor.
  parent_.value()->executor_.schedule_task(
      UnbindOp().then([device = shared_from_this()](fpromise::result<void>& init) {
        if (device->parent_.value()->parent_ == std::nullopt &&
            device->parent_.value()->children_.size() == 1) {
          // We are the last remaining child. We should delay
          // calling the driver's release hook until the driver destructs, so the hook
          // is only invoked after the the dispatcher is shutdown.
          device->release_after_dispatcher_shutdown_ = true;
          if (device->ShouldCallRelease()) {
            auto op = std::make_unique<DelayedReleaseOp>(device);
            device->parent_.value()->AddDelayedChildReleaseOp(std::move(op));
          }
          // The device will otherwise destruct as normal.
        }
        // Our device should be destructed at the end of this callback when the reference to the
        // shared pointer is removed.
        device->parent_.value()->children_.remove(device);
      }));
}

std::string Device::OutgoingName() {
  auto outgoing_name = name_ + "-" + std::to_string(device_id_);
  std::replace(outgoing_name.begin(), outgoing_name.end(), ':', '_');
  return outgoing_name;
}

bool Device::HasChildNamed(std::string_view name) const {
  return std::any_of(children_.begin(), children_.end(),
                     [name](const auto& child) { return name == child->Name(); });
}

zx_status_t Device::GetProtocol(uint32_t proto_id, void* out) const {
  if (HasOp(ops_, &zx_protocol_device_t::get_protocol)) {
    return ops_->get_protocol(compat_symbol_.context, proto_id, out);
  }

  if (!device_server_.has_banjo_config()) {
    if (driver_ == nullptr) {
      logger_->log(fdf::ERROR, "Driver is null");
      return ZX_ERR_BAD_STATE;
    }

    return driver_->GetProtocol(proto_id, out);
  }

  compat::DeviceServer::GenericProtocol device_server_out;
  zx_status_t status = device_server_.GetProtocol(proto_id, &device_server_out);
  if (status != ZX_OK) {
    return status;
  }

  if (!out) {
    return ZX_OK;
  }

  struct GenericProtocol {
    const void* ops;
    void* ctx;
  };

  auto proto = static_cast<GenericProtocol*>(out);
  proto->ctx = device_server_out.ctx;
  proto->ops = device_server_out.ops;
  return ZX_OK;
}

zx_status_t Device::GetFragmentProtocol(const char* fragment, uint32_t proto_id, void* out) {
  if (driver() == nullptr) {
    logger_->log(fdf::ERROR, "Driver is null");
    return ZX_ERR_BAD_STATE;
  }

  return driver()->GetFragmentProtocol(fragment, proto_id, out);
}

zx_status_t Device::AddMetadata(uint32_t type, const void* data, size_t size) {
  return device_server_.AddMetadata(type, data, size);
}

zx_status_t Device::GetMetadata(uint32_t type, void* buf, size_t buflen, size_t* actual) {
  return device_server_.GetMetadata(type, buf, buflen, actual);
}

zx_status_t Device::RegisterServiceMember(component::AnyHandler handler, const char* service_name,
                                          const char* instance_name, const char* member_name) {
  std::string fullpath = std::format("svc/{}/{}", service_name, instance_name);
  zx::result result = driver_->outgoing().component().AddUnmanagedProtocolAt(std::move(handler),
                                                                             fullpath, member_name);
  if (result.is_error()) {
    logger_->log(fdf::ERROR, "Registering driver failed. {}", result.error_value());
  }
  return result.status_value();
}

bool Device::MessageOp(fidl::IncomingHeaderAndMessage msg, device_fidl_txn_t txn) {
  if (HasOp(ops_, &zx_protocol_device_t::message)) {
    ops_->message(compat_symbol_.context, std::move(msg).ReleaseToEncodedCMessage(), txn);
    return true;
  }
  return false;
}

void Device::InitReply(zx_status_t status) {
  fpromise::promise<void, zx_status_t> promise =
      fpromise::make_result_promise<void, zx_status_t>(fpromise::ok());
  // If we have a parent, we want to only finish our init after they finish their init.
  if (parent_.has_value()) {
    promise = parent_.value()->WaitForInitToComplete();
  }

  executor().schedule_task(promise.then(
      [this, init_status = status](fpromise::result<void, zx_status_t>& result) mutable {
        zx_status_t status = init_status;
        if (parent_.has_value() && driver()) {
          if (status == ZX_OK) {
            // We want to export ourselves now that we're initialized.
            // We can only do this if we have a parent, if we don't have a parent we've already been
            // exported.
            status = ExportAfterInit();
            if (status != ZX_OK) {
              logger_->log(fdf::WARN, "Device {} failed to create node: {}", OutgoingName(),
                           zx::make_result(status));
            }
          }

          // We need to complete start after the first device the driver added completes it's init
          // hook.
          constexpr uint32_t kFirstDeviceId = 1;
          if (device_id_ == kFirstDeviceId) {
            if (status == ZX_OK) {
              driver()->CompleteStart(zx::ok());
            } else {
              driver()->CompleteStart(zx::error(status));
            }
          }
        }

        if (status != ZX_OK) {
          Remove();
        }

        // Finish the init by alerting any waiters.
        {
          std::scoped_lock lock(init_lock_);
          init_is_finished_ = true;
          init_status_ = init_status;
          for (auto& waiter : init_waiters_) {
            if (init_status_ == ZX_OK) {
              waiter.complete_ok();
            } else {
              waiter.complete_error(init_status_);
            }
          }
          init_waiters_.clear();
        }
      }));
}

fpromise::promise<void, zx_status_t> Device::WaitForInitToComplete() {
  std::scoped_lock lock(init_lock_);
  if (init_is_finished_) {
    if (init_status_ == ZX_OK) {
      return fpromise::make_result_promise<void, zx_status_t>(fpromise::ok());
    }
    return fpromise::make_result_promise<void, zx_status_t>(fpromise::error(init_status_));
  }
  fpromise::bridge<void, zx_status_t> bridge;
  init_waiters_.push_back(std::move(bridge.completer));

  return bridge.consumer.promise_or(fpromise::error(ZX_ERR_UNAVAILABLE));
}

zx_status_t Device::ConnectFragmentFidl(const char* fragment_name, const char* service_name,
                                        const char* protocol_name, zx::channel request,
                                        bool zircon_transport) {
  if (std::string_view(fragment_name) != "default") {
    if (zircon_transport) {
      if (!driver_->service_validator().IsValidZirconServiceInstance(service_name, fragment_name)) {
        logger_->log(fdf::WARN, "Tried to connect to instance '{}' but it's not available",
                     fragment_name);
        return ZX_ERR_NOT_FOUND;
      }
    } else {
      if (!driver_->service_validator().IsValidDriverServiceInstance(service_name, fragment_name)) {
        logger_->log(fdf::WARN, "Tried to connect to instance '{}' but it's not available",
                     fragment_name);
        return ZX_ERR_NOT_FOUND;
      }
    }
  }

  auto protocol_path =
      std::string(service_name).append("/").append(fragment_name).append("/").append(protocol_name);

  auto result = component::internal::ConnectAtRaw(driver_->driver_namespace().svc_dir(),
                                                  std::move(request), protocol_path.c_str());
  if (result.is_error()) {
    logger_->log(fdf::ERROR, "Error connecting: {}", result);
    return result.status_value();
  }

  return ZX_OK;
}

zx_status_t Device::AddCompositeNodeSpec(const char* name, const composite_node_spec_t* spec) {
  if (!name || !spec) {
    return ZX_ERR_INVALID_ARGS;
  }

  if (!spec->parents || spec->parent_count == 0) {
    return ZX_ERR_INVALID_ARGS;
  }

  auto composite_node_manager =
      driver_->driver_namespace().Connect<fuchsia_driver_framework::CompositeNodeManager>();
  if (composite_node_manager.is_error()) {
    logger_->log(fdf::ERROR, "Error connecting: {}", composite_node_manager);
    return composite_node_manager.status_value();
  }

  fidl::Arena allocator;
  auto parents = fidl::VectorView<fdf::wire::ParentSpec>(allocator, spec->parent_count);
  for (size_t i = 0; i < spec->parent_count; i++) {
    auto parents_result = ConvertNodeRepresentation(allocator, spec->parents[i]);
    if (!parents_result.is_ok()) {
      return parents_result.error_value();
    }
    parents[i] = std::move(parents_result.value());
  }

  auto fidl_spec = fdf::wire::CompositeNodeSpec::Builder(allocator)
                       .name(fidl::StringView(allocator, name))
                       .parents(std::move(parents))
                       .Build();

  auto result = fidl::WireCall(*composite_node_manager)->AddSpec(std::move(fidl_spec));
  if (result.status() != ZX_OK) {
    logger_->log(fdf::ERROR, "Error calling connect fidl: {}", result.error());
    return result.status();
  }

  return ZX_OK;
}

zx_status_t Device::ConnectFragmentRuntime(const char* fragment_name, const char* service_name,
                                           const char* protocol_name, fdf::Channel request) {
  zx::channel client_token, server_token;
  auto status = zx::channel::create(0, &client_token, &server_token);
  if (status != ZX_OK) {
    return status;
  }
  status = fdf::ProtocolConnect(std::move(client_token), std::move(request));
  if (status != ZX_OK) {
    return status;
  }

  return ConnectFragmentFidl(fragment_name, service_name, protocol_name, std::move(server_token),
                             false);
}

zx_status_t Device::ConnectNsProtocol(const char* protocol_name, zx::channel request) {
  return component::internal::ConnectAtRaw(driver()->driver_namespace().svc_dir(),
                                           std::move(request), protocol_name)
      .status_value();
}

zx_status_t Device::PublishInspect(zx::vmo inspect_vmo) {
  inspect_vmo_.emplace(std::move(inspect_vmo));
  zx::vmo publishable;
  auto status = inspect_vmo_->duplicate(ZX_RIGHT_SAME_RIGHTS, &publishable);
  if (status != ZX_OK) {
    logger_->log(fdf::ERROR, "Device {} failed to duplicate vmo", OutgoingName());
    return status;
  }

  inspect::PublishVmo(
      dispatcher(), std::move(publishable),
      inspect::VmoOptions{
          .tree_name = Name(),
          .client_end =
              driver()->driver_namespace().Connect<fuchsia_inspect::InspectSink>().value(),
      });

  return ZX_OK;
}

void Device::AddDelayedChildReleaseOp(std::unique_ptr<DelayedReleaseOp> op) {
  delayed_child_release_ops_.push_back(std::move(op));
}

void Device::LogError(const char* error) {
  logger_->log(fdf::ERROR, "{}: {}", OutgoingName(), error);
}
bool Device::IsUnbound() { return pending_removal_; }

void Device::ConnectToDeviceFidl(ConnectToDeviceFidlRequestView request,
                                 ConnectToDeviceFidlCompleter::Sync& completer) {
  devfs_server_.ServeDeviceFidl(std::move(request->server));
}

void Device::ConnectToController(ConnectToControllerRequestView request,
                                 ConnectToControllerCompleter::Sync& completer) {
  dev_controller_bindings_.AddBinding(dispatcher_, std::move(request->server), this,
                                      fidl::kIgnoreBindingClosure);
}

void Device::Bind(BindRequestView request, BindCompleter::Sync& completer) {
  fidl::Arena arena;
  auto bind_request = fdf::wire::NodeControllerRequestBindRequest::Builder(arena)
                          .force_rebind(false)
                          .driver_url_suffix(request->driver);
  if (!controller_.is_valid()) {
    completer.Reply(zx::error(ZX_ERR_INTERNAL));
    return;
  }
  controller_->RequestBind(bind_request.Build())
      .ThenExactlyOnce(
          [completer = completer.ToAsync()](
              fidl::WireUnownedResult<fdf::NodeController::RequestBind>& result) mutable {
            if (!result.ok()) {
              completer.ReplyError(result.status());
              return;
            }
            completer.Reply(result.value());
          });
}

void Device::Rebind(RebindRequestView request, RebindCompleter::Sync& completer) {
  fidl::Arena arena;
  auto bind_request = fdf::wire::NodeControllerRequestBindRequest::Builder(arena)
                          .force_rebind(true)
                          .driver_url_suffix(request->driver);
  if (!controller_.is_valid()) {
    completer.Reply(zx::error(ZX_ERR_INTERNAL));
    return;
  }
  controller_->RequestBind(bind_request.Build())
      .ThenExactlyOnce(
          [completer = completer.ToAsync()](
              fidl::WireUnownedResult<fdf::NodeController::RequestBind>& result) mutable {
            if (!result.ok()) {
              completer.ReplyError(result.status());
              return;
            }
            if (result->is_error() && result->error_value() == ZX_ERR_NOT_FOUND) {
              // We do not forward failures to find a driver to bind to back to the user.
              // TODO(https://fxbug.dev/42076016): Forward ZX_ERR_NOT_FOUND to the user.
              completer.Reply(zx::ok());
              return;
            }
            completer.Reply(result.value());
          });
}

void Device::UnbindChildren(UnbindChildrenCompleter::Sync& completer) {
  // If we have children, we can just schedule their removal, and they will handle
  // dropping any associated nodes.
  if (!children_.empty()) {
    executor().schedule_task(RemoveChildren().then(
        [completer = completer.ToAsync()](fpromise::result<>& result) mutable {
          completer.ReplySuccess();
        }));
    return;
  }

  // If we don't have children, we need to check if there is a driver bound to us,
  // and if so unbind it.
  // TODO(https://fxbug.dev/42077603): we pass a bad URL to |NodeController::RequestBind|
  // to unbind the driver of a node but not rebind it. This is a temporary
  // workaround to pass the fshost tests in DFv2.
  fidl::Arena arena;
  auto bind_request = fdf::wire::NodeControllerRequestBindRequest::Builder(arena)
                          .force_rebind(true)
                          .driver_url_suffix(kKnownBadDriverUrl);
  controller_->RequestBind(bind_request.Build())
      .ThenExactlyOnce(
          [completer = completer.ToAsync()](
              fidl::WireUnownedResult<fdf::NodeController::RequestBind>& result) mutable {
            if (!result.ok()) {
              completer.ReplyError(result.status());
              return;
            }
            completer.Reply(zx::ok());
          });
}

void Device::ScheduleUnbind(ScheduleUnbindCompleter::Sync& completer) {
  Remove();
  completer.ReplySuccess();
}

void Device::GetTopologicalPath(GetTopologicalPathCompleter::Sync& completer) {
  ZX_ASSERT_MSG(false, "CALLED GetTopologicalPath ON THE COMPAT DEVICE!!!!");
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

}  // namespace compat

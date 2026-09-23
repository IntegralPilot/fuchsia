// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/misc/drivers/compat/driver.h"

#include <fidl/fuchsia.driver.framework/cpp/wire.h>
#include <fidl/fuchsia.scheduler/cpp/wire.h>
#include <fidl/fuchsia.system.state/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/ddk/binding_priv.h>
#include <lib/driver/compat/cpp/connect.h>
#include <lib/driver/component/cpp/internal/start_args.h>
#include <lib/driver/component/cpp/internal/symbols.h>
#include <lib/driver/logging/cpp/structured_logger.h>
#include <lib/driver/promise/cpp/promise.h>
#include <lib/fidl/cpp/wire/connect_service.h>
#include <lib/fit/defer.h>
#include <lib/fpromise/bridge.h>
#include <lib/fpromise/promise.h>
#include <lib/fpromise/single_threaded_executor.h>
#include <lib/sync/cpp/completion.h>
#include <zircon/dlfcn.h>

#include "src/devices/lib/log/log.h"
#include "src/devices/misc/drivers/compat/compat_driver_server.h"
#include "src/lib/driver_symbols/symbols.h"

namespace fdf {

using namespace fuchsia_driver_framework;

}
namespace fio = fuchsia_io;
namespace fkernel = fuchsia_kernel;
namespace fldsvc = fuchsia_ldsvc;
namespace fdm = fuchsia_system_state;

using fpromise::bridge;
using fpromise::error;
using fpromise::join_promises;
using fpromise::ok;
using fpromise::promise;
using fpromise::result;

namespace {

constexpr auto kOpenFlags =
    fio::Flags::kPermReadBytes | fio::Flags::kPermExecute | fio::Flags::kProtocolFile;
constexpr auto kVmoFlags =
    fio::wire::VmoFlags::kRead | fio::wire::VmoFlags::kExecute | fio::wire::VmoFlags::kPrivateClone;

std::string_view GetFilename(std::string_view path) {
  size_t index = path.rfind('/');
  return index == std::string_view::npos ? path : path.substr(index + 1);
}

zx::result<zx::vmo> LoadVmo(fdf::Namespace& ns, const char* path, fuchsia_io::Flags flags) {
  zx::result file = ns.Open<fuchsia_io::File>(path, flags);
  if (file.is_error()) {
    return file.take_error();
  }
  fidl::WireResult result = fidl::WireCall(file.value())->GetBackingMemory(kVmoFlags);
  if (!result.ok()) {
    return zx::error(result.status());
  }
  if (result.value().is_error()) {
    return result.value().take_error();
  }
  return zx::ok(std::move(result.value().value()->vmo));
}

zx::result<zx::resource> GetMmioResource(fdf::Namespace& ns) {
  zx::result resource = ns.Connect<fkernel::MmioResource>();
  if (resource.is_error()) {
    return resource.take_error();
  }
  fidl::WireResult result = fidl::WireCall(resource.value())->Get();
  if (!result.ok()) {
    return zx::error(result.status());
  }
  return zx::ok(std::move(result.value().resource));
}

zx::result<zx::resource> GetPowerResource(fdf::Namespace& ns) {
  zx::result resource = ns.Connect<fkernel::PowerResource>();
  if (resource.is_error()) {
    return resource.take_error();
  }
  fidl::WireResult result = fidl::WireCall(resource.value())->Get();
  if (!result.ok()) {
    return zx::error(result.status());
  }
  return zx::ok(std::move(result.value().resource));
}

zx::result<zx::resource> GetIommuResource(fdf::Namespace& ns) {
  zx::result resource = ns.Connect<fkernel::IommuResource>();
  if (resource.is_error()) {
    return resource.take_error();
  }
  fidl::WireResult result = fidl::WireCall(resource.value())->Get();
  if (!result.ok()) {
    return zx::error(result.status());
  }
  return zx::ok(std::move(result.value().resource));
}

zx::result<zx::resource> GetIoportResource(fdf::Namespace& ns) {
  zx::result resource = ns.Connect<fkernel::IoportResource>();
  if (resource.is_error()) {
    return resource.take_error();
  }
  fidl::WireResult result = fidl::WireCall(resource.value())->Get();
  if (!result.ok()) {
    return zx::error(result.status());
  }
  return zx::ok(std::move(result.value().resource));
}

zx::result<zx::resource> GetIrqResource(fdf::Namespace& ns) {
  zx::result resource = ns.Connect<fkernel::IrqResource>();
  if (resource.is_error()) {
    return resource.take_error();
  }
  fidl::WireResult result = fidl::WireCall(resource.value())->Get();
  if (!result.ok()) {
    return zx::error(result.status());
  }
  return zx::ok(std::move(result.value().resource));
}

zx::result<zx::resource> GetSmcResource(fdf::Namespace& ns) {
  zx::result resource = ns.Connect<fkernel::SmcResource>();
  if (resource.is_error()) {
    return resource.take_error();
  }
  fidl::WireResult result = fidl::WireCall(resource.value())->Get();
  if (!result.ok()) {
    return zx::error(result.status());
  }
  return zx::ok(std::move(result.value().resource));
}

zx::result<zx::resource> GetInfoResource(fdf::Namespace& ns) {
  zx::result resource = ns.Connect<fkernel::InfoResource>();
  if (resource.is_error()) {
    return resource.take_error();
  }
  fidl::WireResult result = fidl::WireCall(resource.value())->Get();
  if (!result.ok()) {
    return zx::error(result.status());
  }
  return zx::ok(std::move(result.value().resource));
}

zx::result<zx::resource> GetMsiResource(fdf::Namespace& ns) {
  zx::result resource = ns.Connect<fkernel::MsiResource>();
  if (resource.is_error()) {
    return resource.take_error();
  }
  fidl::WireResult result = fidl::WireCall(resource.value())->Get();
  if (!result.ok()) {
    return zx::error(result.status());
  }
  return zx::ok(std::move(result.value().resource));
}

}  // namespace

namespace compat {

// This lock protects the global logger list.
std::mutex kGlobalLoggerListLock;

// This contains all the loggers in this driver host.
#ifdef DRIVER_COMPAT_ADD_NODE_NAMES_TO_LOG_TAGS
GlobalLoggerList global_logger_list __TA_GUARDED(kGlobalLoggerListLock)(true);
#else
GlobalLoggerList global_logger_list __TA_GUARDED(kGlobalLoggerListLock)(false);
#endif

zx_status_t AddMetadata(Device* device,
                        fidl::VectorView<fuchsia_driver_compat::wire::Metadata> data) {
  for (auto& metadata : data) {
    size_t size;
    zx_status_t status = metadata.data.get_property(ZX_PROP_VMO_CONTENT_SIZE, &size, sizeof(size));
    if (status != ZX_OK) {
      return status;
    }
    std::vector<uint8_t> data(size);
    status = metadata.data.read(data.data(), 0, data.size());
    if (status != ZX_OK) {
      return status;
    }

    status = device->AddMetadata(metadata.type, data.data(), data.size());
    if (status != ZX_OK) {
      return status;
    }
  }
  return ZX_OK;
}

promise<void, zx_status_t> GetAndAddMetadata(
    fidl::WireClient<fuchsia_driver_compat::Device>& client, Device* device) {
  bridge<void, zx_status_t> bridge;
  if (!client) {
    bridge.completer.complete_ok();
    return bridge.consumer.promise();
  }

  client->GetMetadata().Then(
      [device, completer = std::move(bridge.completer)](
          fidl::WireUnownedResult<fuchsia_driver_compat::Device::GetMetadata>& result) mutable {
        if (!result.ok()) {
          return;
        }
        auto* response = result.Unwrap();
        if (response->is_error()) {
          completer.complete_error(response->error_value());
          return;
        }
        zx_status_t status = AddMetadata(device, response->value()->metadata);
        if (status != ZX_OK) {
          completer.complete_error(status);
          return;
        }
        completer.complete_ok();
      });
  return bridge.consumer.promise_or(error(ZX_ERR_INTERNAL));
}

bool GlobalLoggerList::LoggerInstances::IsSeverityEnabled(FuchsiaLogSeverity severity) const {
  std::lock_guard guard(kGlobalLoggerListLock);
  auto it = loggers_.begin();

  if (it == loggers_.end()) {
    return severity >= driver_logger::GetLogger().GetSeverity();
  }

  return severity >= (*it)->GetSeverity();
}

void GlobalLoggerList::LoggerInstances::Log(FuchsiaLogSeverity severity, const char* tag,
                                            const char* file, int line, const char* msg,
                                            va_list args) {
  std::lock_guard guard(kGlobalLoggerListLock);
  auto it = loggers_.begin();

  if (it == loggers_.end()) {
    LOGF(WARNING, "No logger available in this LoggerInstances. Using host logger.");
    driver_logger::GetLogger().VLogWrite(severity, tag, msg, args, file, line);
    return;
  }

  if (!log_node_names_) {
    (*it)->logvf(severity, tag, file, line, msg, args);
    return;
  }

  if (tag) {
    node_names_.push_back(tag);
  }

  (*it)->logvf(severity, node_names_, file, line, msg, args);

  if (tag) {
    node_names_.pop_back();
  }
}

zx_driver_t* GlobalLoggerList::LoggerInstances::ZxDriver() {
  return static_cast<zx_driver_t*>(this);
}

void GlobalLoggerList::LoggerInstances::AddLogger(std::shared_ptr<fdf::Logger>& logger,
                                                  const std::optional<std::string>& node_name) {
  loggers_.insert(logger);
  if (log_node_names_ && node_name.has_value()) {
    node_names_.push_back(node_name.value());
  }
}

void GlobalLoggerList::LoggerInstances::RemoveLogger(std::shared_ptr<fdf::Logger>& logger,
                                                     const std::optional<std::string>& node_name) {
  loggers_.erase(logger);
  if (log_node_names_ && node_name.has_value()) {
    node_names_.erase(std::remove(node_names_.begin(), node_names_.end(), node_name.value()),
                      node_names_.end());
  }
}

zx_driver_t* GlobalLoggerList::AddLogger(const std::string& driver_path,
                                         std::shared_ptr<fdf::Logger>& logger,
                                         const std::optional<std::string>& node_name) {
  auto& instances = instances_.try_emplace(driver_path, log_node_names_).first->second;
  instances.AddLogger(logger, node_name);
  return instances.ZxDriver();
}

void GlobalLoggerList::RemoveLogger(const std::string& driver_path,
                                    std::shared_ptr<fdf::Logger>& logger,
                                    const std::optional<std::string>& node_name) {
  auto it = instances_.find(driver_path);
  if (it != instances_.end()) {
    it->second.RemoveLogger(logger, node_name);
    // Don't erase the instance even if it becomes empty. There are some drivers that incorrectly
    // log after they have been destroyed. We want to make sure that the logger instance that we
    // put for them is kept around. The empty loggers will just cause it to log with the
    // driver host's logger.
  }
}

std::optional<size_t> GlobalLoggerList::loggers_count_for_testing(const std::string& driver_path) {
  auto it = instances_.find(driver_path);
  if (it != instances_.end()) {
    return (*it).second.count();
  }

  return std::nullopt;
}

Driver::Driver(
    zx::vmo config_vmo, fdf::UnownedSynchronizedDispatcher driver_dispatcher, device_t device,
    const zx_protocol_device_t* ops, std::string_view driver_path,
    std::optional<std::vector<fuchsia_driver_framework::NodePropertyEntry2>> node_properties)
    : Base("compat"),
      driver_dispatcher_(std::move(driver_dispatcher)),
      executor_(driver_dispatcher_->async_dispatcher()),
      driver_path_(driver_path),
      device_(device, ops, this, std::nullopt, nullptr, driver_dispatcher_->async_dispatcher()),
      config_vmo_(std::move(config_vmo)),
      node_properties_(std::move(node_properties)) {}

Driver::~Driver() {
  if (ShouldCallRelease()) {
    record_->ops->release(context_);
  }
  dlclose(library_);
  {
    std::lock_guard guard(kGlobalLoggerListLock);
    global_logger_list.RemoveLogger(driver_path(), inner_logger_, node_name_);
  }
}

void Driver::Start(fdf::DriverContext context, fdf::StartCompleter completer) {
  DriverBaseInternalInit(context, fdf::UnownedSynchronizedDispatcher(driver_dispatcher_));
  url_ = context.url();
  node_name_ = context.node_name();
  node_offers_ = context.node_offers();
  incoming_ = context.take_incoming();

  // Give the parent device the correct node.
  device_.Bind({take_node(), dispatcher()});
  // Call this so the parent device is in the post-init state.
  device_.InitReply(ZX_OK);
  ZX_ASSERT(url_.has_value());

  service_validator_.emplace(node_offers_);

  zx::result driver_vmo = LoadVmo(*incoming_, driver_path_.c_str(), kOpenFlags);
  if (driver_vmo.is_error()) {
    logger_->log(fdf::ERROR, "Failed to open driver vmo: {}", driver_vmo);
    completer(driver_vmo.take_error());
    return;
  }

  // Give the driver's VMO a name.
  std::string_view vmo_name = GetFilename(driver_path_);
  if (zx_status_t status = driver_vmo->set_property(ZX_PROP_NAME, vmo_name.data(), vmo_name.size());
      status != ZX_OK) {
    LOGF(ERROR, "Failed to name driver's DFv1 vmo '{}': {}", vmo_name, zx::make_result(status));
    // We don't need to exit on this error, there will just be less debugging information.
  }

  if (zx::result result =
          LoadDriver(driver_path_, std::move(driver_vmo.value()), context.symbols());
      result.is_error()) {
    logger_->log(fdf::ERROR, "Failed to load driver: {}", result);
    completer(result.take_error());
    return;
  }

  // Store start completer to be replied to later. It will either be done when the below promises
  // hit an error or after the init hook is replied to and the node has been created and a devfs
  // node has been exported.
  start_completer_.emplace(std::move(completer));

  auto start_driver =
      Driver::ConnectToParentDevices()
          .and_then(fit::bind_member<&Driver::GetDeviceInfo>(this))
          .then([this](result<void, zx_status_t>& result) -> fpromise::result<void, zx_status_t> {
            if (result.is_error()) {
              logger_->log(fdf::WARN, "Getting DeviceInfo failed with: {}",
                           zx::make_result(result.error()));
            }
            if (zx::result result = StartDriver(); result.is_error()) {
              logger_->log(fdf::ERROR, "Failed to start driver '{}': {}", url_.value(), result);
              device_.Unbind();
              CompleteStart(result.take_error());
              return error(result.error_value());
            }
            return ok();
          })
          .wrap_with(scope_);
  executor_.schedule_task(std::move(start_driver));
}

bool Driver::IsComposite() { return !parent_clients_.empty(); }

zx_handle_t Driver::GetMmioResource() {
  if (!mmio_resource_.is_valid()) {
    zx::result resource = ::GetMmioResource(*incoming_);
    if (resource.is_ok()) {
      mmio_resource_ = std::move(resource.value());
    } else {
      logger_->log(fdf::WARN, "Failed to get mmio_resource '{}'", resource);
    }
  }
  return mmio_resource_.get();
}

zx_handle_t Driver::GetMsiResource() {
  if (!msi_resource_.is_valid()) {
    zx::result resource = ::GetMsiResource(*incoming_);
    if (resource.is_ok()) {
      msi_resource_ = std::move(resource.value());
    } else {
      logger_->log(fdf::WARN, "Failed to get msi_resource '{}'", resource);
    }
  }
  return msi_resource_.get();
}

zx_handle_t Driver::GetPowerResource() {
  if (!power_resource_.is_valid()) {
    zx::result resource = ::GetPowerResource(*incoming_);
    if (resource.is_ok()) {
      power_resource_ = std::move(resource.value());
    } else {
      logger_->log(fdf::WARN, "Failed to get power_resource '{}'", resource);
    }
  }
  return power_resource_.get();
}

zx_handle_t Driver::GetIommuResource() {
  if (!iommu_resource_.is_valid()) {
    zx::result resource = ::GetIommuResource(*incoming_);
    if (resource.is_ok()) {
      iommu_resource_ = std::move(resource.value());
    } else {
      logger_->log(fdf::WARN, "Failed to get iommu_resource '{}'", resource);
    }
  }
  return iommu_resource_.get();
}

zx_handle_t Driver::GetIoportResource() {
  if (!ioport_resource_.is_valid()) {
    zx::result resource = ::GetIoportResource(*incoming_);
    if (resource.is_ok()) {
      ioport_resource_ = std::move(resource.value());
    } else {
      logger_->log(fdf::WARN, "Failed to get ioport_resource '{}'", resource);
    }
  }
  return ioport_resource_.get();
}

zx_handle_t Driver::GetIrqResource() {
  if (!irq_resource_.is_valid()) {
    zx::result resource = ::GetIrqResource(*incoming_);
    if (resource.is_ok()) {
      irq_resource_ = std::move(resource.value());
    } else {
      logger_->log(fdf::WARN, "Failed to get irq_resource '{}'", resource);
    }
  }
  return irq_resource_.get();
}

zx_handle_t Driver::GetSmcResource() {
  if (!smc_resource_.is_valid()) {
    zx::result resource = ::GetSmcResource(*incoming_);
    if (resource.is_ok()) {
      smc_resource_ = std::move(resource.value());
    } else {
      logger_->log(fdf::WARN, "Failed to get smc_resource '{}'", resource);
    }
  }
  return smc_resource_.get();
}

zx::vmo& Driver::GetConfigVmo() { return config_vmo_; }

zx_status_t Driver::GetProperties(device_props_args_t* out_args,
                                  const std::string& parent_node_name) {
  if (!out_args) {
    return ZX_ERR_INVALID_ARGS;
  }

  auto set_str_prop_value =
      [this](
          const ::fuchsia_driver_framework::NodePropertyValue& value) -> zx_device_str_prop_val_t {
    zx_device_str_prop_val_t out_value;
    switch (value.Which()) {
      case fuchsia_driver_framework::NodePropertyValue::Tag::kIntValue:
        out_value.data_type = ZX_DEVICE_PROPERTY_VALUE_INT;
        out_value.data.int_val = value.int_value().value();
        break;
      case fuchsia_driver_framework::NodePropertyValue::Tag::kStringValue:
        out_value.data_type = ZX_DEVICE_PROPERTY_VALUE_STRING;
        out_value.data.str_val = value.string_value()->data();
        break;
      case fuchsia_driver_framework::NodePropertyValue::Tag::kBoolValue:
        out_value.data_type = ZX_DEVICE_PROPERTY_VALUE_BOOL;
        out_value.data.bool_val = value.bool_value().value();
        break;
      case fuchsia_driver_framework::NodePropertyValue::Tag::kEnumValue:
        out_value.data_type = ZX_DEVICE_PROPERTY_VALUE_ENUM;
        out_value.data.enum_val = value.enum_value()->data();
        break;
      default:
        logger_->log(fdf::ERROR, "Unsupported property type, value: %lu",
                     static_cast<fidl_xunion_tag_t>(value.Which()));
        break;
    }
    return out_value;
  };

  cpp20::span<const fuchsia_driver_framework::NodeProperty2> props;
  if (node_properties_.has_value()) {
    for (const auto& entry : node_properties_.value()) {
      if (entry.name() == parent_node_name) {
        props = entry.properties();
        break;
      }
    }
  }
  uint32_t str_prop_count = 0;
  for (auto& prop : props) {
    if (str_prop_count >= out_args->str_prop_count) {
      out_args->actual_str_prop_count = str_prop_count;
      return ZX_ERR_BUFFER_TOO_SMALL;
    }
    str_prop_count++;
    out_args->str_props[str_prop_count - 1].key = prop.key().c_str();
    out_args->str_props[str_prop_count - 1].property_value = set_str_prop_value(prop.value());
  }
  out_args->actual_str_prop_count = str_prop_count;
  return ZX_OK;
}

zx_handle_t Driver::GetInfoResource() {
  if (!info_resource_.is_valid()) {
    zx::result resource = ::GetInfoResource(*incoming_);
    if (resource.is_ok()) {
      info_resource_ = std::move(resource.value());
    } else {
      logger_->log(fdf::WARN, "Failed to get info_resource '{}'", resource);
    }
  }
  return info_resource_.get();
}

bool Driver::IsRunningOnDispatcher() const {
  fdf::Unowned<fdf::Dispatcher> current_dispatcher = fdf::Dispatcher::GetCurrent();
  if (current_dispatcher == fdf::Unowned<fdf::Dispatcher>{}) {
    return false;
  }
  return current_dispatcher->async_dispatcher() == dispatcher();
}

zx_status_t Driver::RunOnDispatcher(fit::callback<zx_status_t()> task) {
  if (IsRunningOnDispatcher()) {
    return task();
  }

  libsync::Completion completion;
  zx_status_t task_status;
  auto discarded = fit::defer([&] {
    task_status = ZX_ERR_CANCELED;
    completion.Signal();
  });
  zx_status_t status =
      async::PostTask(dispatcher(), [&task_status, &completion, task = std::move(task),
                                     discarded = std::move(discarded)]() mutable {
        discarded.cancel();
        task_status = task();
        completion.Signal();
      });
  if (status != ZX_OK) {
    return status;
  }
  completion.Wait();
  return task_status;
}

void Driver::Stop(fdf::StopCompleter completer) {
  zx::result client = this->incoming_->Connect<fuchsia_system_state::SystemStateTransition>();
  if (client.is_error()) {
    logger_->log(fdf::ERROR, "failed to connect to fuchsia.system.state/SystemStateTransition: {}",
                 client);
    completer(client.take_error());
    return;
  }
  fidl::WireResult result = fidl::WireCall(client.value())->GetTerminationSystemState();
  if (!result.ok()) {
    logger_->log(fdf::ERROR, "failed to get termination state: {}", client);
    completer(zx::error(result.error().status()));
    return;
  }

  system_state_ = result->state;
  stop_triggered_ = true;

  executor_.schedule_task(device_.HandleStopSignal().then(
      [completer = std::move(completer)](fpromise::result<void>& init) mutable {
        completer(zx::ok());
      }));
}

zx::result<> Driver::LoadDriver(
    std::string_view module_name, zx::vmo driver_vmo,
    const std::optional<std::vector<fuchsia_driver_framework::NodeSymbol>>& symbols) {
  std::string_view url_str = url_.value();

  auto result = driver_symbols::FindRestrictedSymbols(zx::unowned(driver_vmo), url_str);
  if (result.is_error()) {
    logger_->log(fdf::WARN, "Driver '{}' failed to validate as ELF: {}", url_str,
                 result.status_value());
  } else if (!result->empty()) {
    logger_->log(fdf::ERROR, "Driver '{}' referenced {} restricted symbols: ", url_str,
                 result->size());
    for (auto& str : *result) {
      LOGF(ERROR, str.c_str());
    }
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  // Find symbols
  module_name.remove_prefix(5);  // Remove leading "/pkg/"
  auto* note = fdf_internal::GetSymbol<const zircon_driver_note_t*>(symbols, module_name,
                                                                    "__zircon_driver_note__");
  if (note == nullptr) {
    logger_->log(fdf::ERROR, "Failed to load driver '{}', driver note not found", url_str);
    return zx::error(ZX_ERR_BAD_STATE);
  }
  driver_name_ = note->payload.name;
  logger_->log(fdf::INFO, "Loaded driver '{}'", driver_name_);
  record_ =
      fdf_internal::GetSymbol<zx_driver_rec_t*>(symbols, module_name, "__zircon_driver_rec__");
  if (record_ == nullptr) {
    logger_->log(fdf::ERROR, "Failed to load driver '{}', driver record not found", url_str);
    return zx::error(ZX_ERR_BAD_STATE);
  }
  if (record_->ops == nullptr) {
    logger_->log(fdf::ERROR, "Failed to load driver '{}', missing driver ops", url_str);
    return zx::error(ZX_ERR_BAD_STATE);
  }
  if (record_->ops->version != DRIVER_OPS_VERSION) {
    logger_->log(fdf::ERROR, "Failed to load driver '{}', incorrect driver version", url_str);
    return zx::error(ZX_ERR_WRONG_TYPE);
  }
  if (record_->ops->bind == nullptr && record_->ops->create == nullptr) {
    logger_->log(fdf::ERROR, "Failed to load driver '{}', missing '{}'", url_str,
                 (record_->ops->bind == nullptr ? "bind" : "create"));
    return zx::error(ZX_ERR_BAD_STATE);
  }
  if (record_->ops->bind != nullptr && record_->ops->create != nullptr) {
    logger_->log(fdf::ERROR, "Failed to load driver '{}', both 'bind' and 'create' are defined",
                 url_str);
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  // Create our logger. This leaves out |DriverStartArgs.log_sink| because the DriverBase
  // constructor has already consumed it, so |Create2| will fallback to opening a log channel from
  // |incoming()|.
  auto logger = fdf::Logger::Create2(*incoming_, dispatcher(), note->payload.name);

  // Move the logger over into a shared_ptr instead of unique_ptr so we can pass it to the global
  // logging manager and compat::Device.
  inner_logger_ = std::shared_ptr<fdf::Logger>(logger.release());
  device_.set_logger(inner_logger_);
  {
    std::lock_guard guard(kGlobalLoggerListLock);
    record_->driver = global_logger_list.AddLogger(driver_path(), inner_logger_, node_name_);
  }

  return zx::ok();
}

zx::result<> Driver::StartDriver() {
  std::string_view url_str = url_.value();
  if (record_->ops->init != nullptr) {
    // If provided, run init.
    zx_status_t status = record_->ops->init(&context_);
    if (status != ZX_OK) {
      logger_->log(fdf::ERROR, "Failed to load driver '{}', 'init' failed: {}", url_str,
                   zx::make_result(status));
      return zx::error(status);
    }
  }

  if (record_->ops->bind != nullptr) {
    // If provided, run bind and return.
    zx_status_t status = record_->ops->bind(context_, device_.ZxDevice());
    if (status != ZX_OK) {
      logger_->log(fdf::ERROR, "Failed to load driver '{}', 'bind' failed: {}", url_str,
                   zx::make_result(status));
      return zx::error(status);
    }
  } else {
    // Else, run create and return.
    zx_status_t status =
        record_->ops->create(context_, device_.ZxDevice(), "proxy", ZX_HANDLE_INVALID);
    if (status != ZX_OK) {
      logger_->log(fdf::ERROR, "Failed to load driver '{}', 'create' failed: {}", url_str,
                   zx::make_result(status));
      return zx::error(status);
    }
  }
  if (!device_.HasChildren()) {
    logger_->log(fdf::ERROR, "Driver '{}' did not add a child device", url_str);
    return zx::error(ZX_ERR_BAD_STATE);
  }
  return zx::ok();
}

fpromise::promise<void, zx_status_t> Driver::ConnectToParentDevices() {
  bridge<void, zx_status_t> bridge;
  auto task = compat::ConnectToParentDevices(
      dispatcher(), incoming_.get(),
      [this, completer = std::move(bridge.completer)](
          zx::result<std::vector<compat::ParentDevice>> devices) mutable {
        if (devices.is_error()) {
          completer.complete_error(devices.error_value());
          return;
        }
        std::vector<std::string> parents_names;
        for (auto& device : devices.value()) {
          if (device.name == "default") {
            parent_client_ = fidl::WireClient<fuchsia_driver_compat::Device>(
                std::move(device.client), dispatcher());
            continue;
          }

          // TODO(https://fxbug.dev/42051759): When services stop adding extra instances
          // separated by ',' then remove this check.
          if (device.name.find(',') != std::string::npos) {
            continue;
          }

          parents_names.push_back(device.name);
          parent_clients_[device.name] = fidl::WireClient<fuchsia_driver_compat::Device>(
              std::move(device.client), dispatcher());
        }
        device_.set_fragments(std::move(parents_names));
        completer.complete_ok();
      });
  async_tasks_.AddTask(std::move(task));
  return bridge.consumer.promise_or(error(ZX_ERR_INTERNAL)).wrap_with(scope_);
}

promise<void, zx_status_t> Driver::GetDeviceInfo() {
  std::vector<promise<void, zx_status_t>> promises;

  // Get our metadata from our fragments if we are a composite,
  // or our primary parent.
  if (IsComposite()) {
    for (auto& client : parent_clients_) {
      promises.push_back(GetAndAddMetadata(client.second, &device_));
    }
  } else {
    promises.push_back(GetAndAddMetadata(parent_client_, &device_));
  }

  // Collect all our promises and return the first error we see.
  return join_promise_vector(std::move(promises))
      .then([](fpromise::result<std::vector<fpromise::result<void, zx_status_t>>>& results) {
        if (results.is_error()) {
          return fpromise::make_result_promise(error(ZX_ERR_INTERNAL));
        }
        for (auto& result : results.value()) {
          if (result.is_error()) {
            return fpromise::make_result_promise(error(result.error()));
          }
        }
        return fpromise::make_result_promise<void, zx_status_t>(ok());
      });
}

void* Driver::Context() const { return context_; }

zx::result<zx::vmo> Driver::LoadFirmware(Device* device, const char* filename, size_t* size) {
  std::string full_filename = "/pkg/lib/firmware/";
  full_filename.append(filename);
  fpromise::result connect_result = fpromise::run_single_threaded(
      fdf::Open(*incoming_, dispatcher(), full_filename.c_str(), kOpenFlags));
  if (connect_result.is_error()) {
    return zx::error(connect_result.take_error());
  }

  fidl::WireResult get_backing_memory_result =
      connect_result.take_value().sync()->GetBackingMemory(fio::wire::VmoFlags::kRead);
  if (!get_backing_memory_result.ok()) {
    if (get_backing_memory_result.is_peer_closed()) {
      return zx::error(ZX_ERR_NOT_FOUND);
    }
    return zx::error(get_backing_memory_result.status());
  }
  const auto* res = get_backing_memory_result.Unwrap();
  if (res->is_error()) {
    return zx::error(res->error_value());
  }
  zx::vmo& vmo = res->value()->vmo;
  if (zx_status_t status = vmo.get_prop_content_size(size); status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(std::move(vmo));
}

zx_status_t Driver::AddDevice(Device* parent, device_add_args_t* args, zx_device_t** out) {
  return RunOnDispatcher([&] {
    zx_device_t* child;
    zx_status_t status = parent->Add(args, &child);
    if (status != ZX_OK) {
      logger_->log(fdf::ERROR, "Failed to add device {}: {}", args->name, zx::make_result(status));
      return status;
    }
    if (out) {
      *out = child;
    }
    return ZX_OK;
  });
}

zx::result<> Driver::SetProfileByRole(zx::unowned_thread thread, std::string_view role) {
  auto role_manager = incoming_->Connect<fuchsia_scheduler::RoleManager>();
  if (role_manager.is_error()) {
    return role_manager.take_error();
  }

  zx::thread duplicate_thread;
  zx_status_t status =
      thread->duplicate(ZX_RIGHT_TRANSFER | ZX_RIGHT_MANAGE_THREAD, &duplicate_thread);
  if (status != ZX_OK) {
    return zx::error(status);
  }

  fidl::Arena arena;
  auto request =
      fuchsia_scheduler::wire::RoleManagerSetRoleRequest::Builder(arena)
          .target(fuchsia_scheduler::wire::RoleTarget::WithThread(std::move(duplicate_thread)))
          .role(fuchsia_scheduler::wire::RoleName{fidl::StringView::FromExternal(role)})
          .Build();
  auto result = fidl::WireCall(*role_manager)->SetRole(request);
  if (result.status() != ZX_OK) {
    return zx::error(result.status());
  }
  if (!result.value().is_ok()) {
    return result.value().take_error();
  }
  return zx::ok();
}

zx_status_t Driver::GetProtocol(uint32_t proto_id, void* out) {
  if (!parent_client_) {
    logger_->log(fdf::WARN,
                 "Invalid fuchsia.driver.compat.Device client. Failed to retrieve Banjo protocol.");
    return ZX_ERR_NOT_SUPPORTED;
  }

  return RunOnDispatcher([proto_id, out, &client = parent_client_, &logger = *logger_]() {
    static uint64_t process_koid = []() {
      zx_info_handle_basic_t basic;
      ZX_ASSERT(zx::process::self()->get_info(ZX_INFO_HANDLE_BASIC, &basic, sizeof(basic), nullptr,
                                              nullptr) == ZX_OK);
      return basic.koid;
    }();

    fidl::WireResult result = client.sync()->GetBanjoProtocol(proto_id, process_koid);
    if (!result.ok()) {
      logger.log(fdf::ERROR, "Failed to send request to get banjo protocol: {}", result.error());
      return result.status();
    }
    if (result->is_error()) {
      logger.log(fdf::DEBUG, "Failed to get banjo protocol: {}",
                 zx::make_result(result->error_value()));
      return result->error_value();
    }

    struct GenericProtocol {
      const void* ops;
      void* ctx;
    };

    auto proto = static_cast<GenericProtocol*>(out);
    proto->ops = reinterpret_cast<const void*>(result->value()->ops);
    proto->ctx = reinterpret_cast<void*>(result->value()->context);
    return ZX_OK;
  });
}

zx_status_t Driver::GetFragmentProtocol(const char* fragment, uint32_t proto_id, void* out) {
  auto iter = parent_clients_.find(fragment);
  if (iter == parent_clients_.end()) {
    logger_->log(fdf::ERROR, "Failed to find compat client of fragment");
    return ZX_ERR_NOT_FOUND;
  }
  fidl::WireClient<fuchsia_driver_compat::Device>& client = iter->second;

  static uint64_t process_koid = []() {
    zx_info_handle_basic_t basic;
    ZX_ASSERT(zx::process::self()->get_info(ZX_INFO_HANDLE_BASIC, &basic, sizeof(basic), nullptr,
                                            nullptr) == ZX_OK);
    return basic.koid;
  }();

  fidl::WireResult result = client.sync()->GetBanjoProtocol(proto_id, process_koid);
  if (!result.ok()) {
    logger_->log(fdf::ERROR, "Failed to send request to get banjo protocol: {}", result.error());
    return result.status();
  }
  if (result->is_error()) {
    logger_->log(fdf::DEBUG, "Failed to get banjo protocol: {}",
                 zx::make_result(result->error_value()));
    return result->error_value();
  }

  struct GenericProtocol {
    const void* ops;
    void* ctx;
  };

  auto proto = static_cast<GenericProtocol*>(out);
  proto->ops = reinterpret_cast<const void*>(result->value()->ops);
  proto->ctx = reinterpret_cast<void*>(result->value()->context);
  return ZX_OK;
}

void Driver::CompleteStart(zx::result<> result) {
  if (start_completer_.has_value()) {
    start_completer_.value()(result);
    start_completer_.reset();
  } else {
    // This can happen if the driver's bind hook ends up returning an error after successfully
    // creating a device through DdkAdd. This is because the device add will schedule an InitReply,
    // inside of which we always call CompleteStart for this initial device. Regardless of if the
    // InitReply is calling this successfully or with an error, since the driver's bind hook
    // returned an error already to the start completer, we can just log it.
    //
    // TODO(https://fxbug.dev/323581670): Improve the compat driver state flow so this isn't needed.
    logger_->log(fdf::INFO,
                 "Called Driver::CompleteStart with {}, but start completer has already been used.",
                 result);
  }
}

}  // namespace compat

EXPORT_FUCHSIA_DRIVER_REGISTRATION_V1(compat::CompatDriverServer::initialize,
                                      compat::CompatDriverServer::destroy);

// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "devfs.h"

#include <fidl/fuchsia.component/cpp/common_types_format.h>
#include <fidl/fuchsia.device.fs/cpp/wire.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/async/cpp/wait.h>
#include <lib/ddk/driver.h>
#include <lib/fdio/directory.h>
#include <lib/fidl/coding.h>
#include <lib/fidl/txn_header.h>
#include <lib/zx/channel.h>
#include <stdio.h>
#include <string.h>
#include <zircon/types.h>

#include <functional>
#include <memory>
#include <random>
#include <unordered_set>

#include <fbl/ref_ptr.h>

#include "src/devices/bin/driver_manager/devfs/builtin_devices.h"
#include "src/devices/bin/driver_manager/devfs/class_names.h"
#include "src/devices/lib/log/log.h"
#include "src/lib/fxl/strings/split_string.h"
#include "src/lib/fxl/strings/string_printf.h"
#include "src/storage/lib/vfs/cpp/fuchsia_vfs.h"
#include "src/storage/lib/vfs/cpp/service.h"
#include "src/storage/lib/vfs/cpp/vfs_types.h"

namespace driver_manager {

namespace fio = fuchsia_io;

std::string_view Devnode::name() const {
  if (name_.has_value()) {
    return name_.value();
  }
  return {};
}

void PathServer::Bind(zx::channel channel, const std::string& class_name) {
  // Only allow binding if the class is on the allowlist
  if (!kClassesThatAllowTopologicalPath.contains(class_name)) {
    std::string error_msg = std::format(
        "Access to the topological path channel is not permitted for class {}.\n  "
        "To enable this class to access to topological paths, you must add '{}' to"
        " kClassesThatAllowTopologicalPath\n"
        "in src/devices/bin/driver_manager/devfs/class_names.h.",
        class_name, class_name);
    // Debug assert or just print error and drop channel if not debug
    ZX_DEBUG_ASSERT_MSG(false, "%s", error_msg.c_str());
    fdf_log::error("{}", error_msg);
    return;
  }
  bindings_.AddBinding(dispatcher_,
                       fidl::ServerEnd<fuchsia_device_fs::TopologicalPath>(std::move(channel)),
                       this, fidl::kIgnoreBindingClosure);
}

void Devnode::advertise_modified() {
  ZX_ASSERT(parent_ != nullptr);
  parent_->Notify(name(), fio::wire::WatchEvent::kRemoved);
  parent_->Notify(name(), fio::wire::WatchEvent::kAdded);
}

Devnode::VnodeImpl::VnodeImpl(Devnode& holder, Target target)
    : holder_(holder), target_(std::move(target)) {}

bool Devnode::VnodeImpl::IsDirectory() const { return !target_.has_value(); }

fuchsia_io::NodeProtocolKinds Devnode::VnodeImpl::GetProtocols() const {
  fuchsia_io::NodeProtocolKinds protocols = fuchsia_io::NodeProtocolKinds::kDirectory;
  if (!IsDirectory()) {
    protocols = protocols | fuchsia_io::NodeProtocolKinds::kConnector;
  }
  return protocols;
}

zx_status_t Devnode::VnodeImpl::ConnectService(zx::channel channel) {
  if (!target_.has_value()) {
    return ZX_ERR_NOT_SUPPORTED;
  }
  return (*target_->device_connect.get())(std::move(channel));
}

zx::result<fs::VnodeAttributes> Devnode::VnodeImpl::GetAttributes() const {
  return children().GetAttributes();
}

zx_status_t Devnode::VnodeImpl::Lookup(std::string_view name, fbl::RefPtr<fs::Vnode>* out) {
  return children().Lookup(name, out);
}

zx_status_t Devnode::VnodeImpl::WatchDir(fs::FuchsiaVfs* vfs, fio::wire::WatchMask mask,
                                         uint32_t options,
                                         fidl::ServerEnd<fio::DirectoryWatcher> watcher) {
  return children().WatchDir(vfs, mask, options, std::move(watcher));
}

zx_status_t Devnode::VnodeImpl::Readdir(fs::VdirCookie* cookie, void* dirents, size_t len,
                                        size_t* out_actual) {
  return children().Readdir(cookie, dirents, len, out_actual);
}

namespace {

void MustAddEntry(PseudoDir& parent, const std::string_view name,
                  const fbl::RefPtr<fs::Vnode>& dn) {
  const zx_status_t status = parent.AddEntry(name, dn);
  ZX_ASSERT_MSG(status == ZX_OK, "AddEntry(%.*s): %s", static_cast<int>(name.size()), name.data(),
                zx_status_get_string(status));
}

}  // namespace

Devnode::Devnode(Devfs& devfs)
    : devfs_(devfs),
      parent_(nullptr),
      node_(fbl::MakeRefCounted<VnodeImpl>(*this, Target())),
      path_server_("", devfs.dispatcher()) {}

Devnode::Devnode(Devfs& devfs, PseudoDir& parent, Target target, fbl::String name,
                 const std::string& path, const std::string& class_name)
    : devfs_(devfs),
      parent_(&parent),
      node_(fbl::MakeRefCounted<VnodeImpl>(*this, target)),
      name_([this, &parent, name = std::move(name)]() {
        auto [it, inserted] = parent.unpublished.emplace(name, *this);
        ZX_ASSERT(inserted);
        return it->first;
      }()),
      path_server_(path, devfs.dispatcher()) {
  if (target.has_value()) {
    children().AddEntry(
        fuchsia_device_fs::wire::kDeviceControllerName,
        fbl::MakeRefCounted<fs::Service>([passthrough = target](zx::channel channel) {
          return (*passthrough->controller_connect.get())(
              fidl::ServerEnd<fuchsia_device::Controller>(std::move(channel)));
        }));
    children().AddEntry(fuchsia_device_fs::wire::kDeviceTopologyName,
                        fbl::MakeRefCounted<fs::Service>([this, class_name](zx::channel channel) {
                          path_server_.Bind(std::move(channel), class_name);
                          return ZX_OK;
                        }));

    children().AddEntry(
        fuchsia_device_fs::wire::kDeviceProtocolName,
        fbl::MakeRefCounted<fs::Service>([passthrough = target](zx::channel channel) {
          return (*passthrough->device_connect.get())(std::move(channel));
        }));
  }
}

std::optional<std::reference_wrapper<fs::Vnode>> Devfs::Lookup(PseudoDir& parent,
                                                               std::string_view name) {
  {
    fbl::RefPtr<fs::Vnode> out;
    switch (const zx_status_t status = parent.Lookup(name, &out); status) {
      case ZX_OK:
        return *out;
      case ZX_ERR_NOT_FOUND:
        break;
      default:
        ZX_PANIC("%s", zx_status_get_string(status));
    }
  }
  const auto it = parent.unpublished.find(name);
  if (it != parent.unpublished.end()) {
    return it->second.get().node();
  }
  return {};
}

void Devfs::CloseComponent() {
  if (binding_.has_value()) {
    auto result = fidl::WireSendEvent(binding_.value())
                      ->OnStop(fuchsia_component_runner::wire::ComponentStopInfo{});
    if (!result.ok()) {
      fdf_log::warn("Devfs::CloseComponent failed to send OnStop event.");
    }
  }
}

Devnode::~Devnode() {
  for (const auto& [key, child] : children().unpublished) {
    child.get().parent_ = nullptr;
  }
  children().unpublished.clear();

  children().RemoveAllEntries();

  if (service_path_.has_value() && service_name_.has_value()) {
    (void)devfs_.outgoing().RemoveProtocolAt(*service_path_, *service_name_);
    (void)devfs_.outgoing().RemoveProtocolAt(*service_path_,
                                             fuchsia_device_fs::wire::kDeviceTopologyName);
    service_path_.reset();
    service_name_.reset();
  }

  if (parent_ == nullptr) {
    return;
  }
  PseudoDir& parent = *parent_;
  const std::string_view name = this->name();
  parent.unpublished.erase(name);
  switch (const zx_status_t status = parent.RemoveEntry(name, node_.get()); status) {
    case ZX_OK:
    case ZX_ERR_NOT_FOUND:
      // Our parent may have been removed before us.
      break;
    default:
      ZX_PANIC("RemoveEntry(%.*s): %s", static_cast<int>(name.size()), name.data(),
               zx_status_get_string(status));
  }
}

void Devnode::publish() {
  ZX_ASSERT(parent_ != nullptr);
  PseudoDir& parent = *parent_;

  const std::string_view name = this->name();
  const auto it = parent.unpublished.find(name);
  ZX_ASSERT(it != parent.unpublished.end());
  ZX_ASSERT(&it->second.get() == this);
  parent.unpublished.erase(it);

  MustAddEntry(parent, name, node_);
}

void DevfsDevice::advertise_modified() {
  if (topological_.has_value()) {
    topological_.value().advertise_modified();
  }
  if (protocol_) {
    protocol_->advertise_modified();
  }
}

void DevfsDevice::publish() {
  if (topological_.has_value()) {
    topological_.value().publish();
  }
  if (protocol_) {
    protocol_->publish();
  }
}

void DevfsDevice::unpublish() {
  topological_.reset();
  protocol_.reset();
}

zx::result<std::string> Devfs::MakeInstanceName(std::string_view class_name) {
  // Don't allow classes not listed in class_names.h
  if (!class_entries_.contains(std::string(class_name))) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  if (classes_that_assume_ordering.contains(std::string(class_name))) {
    // must give a sequential id:
    return zx::ok(std::format("{:03d}", classes_that_assume_ordering[class_name]++));
  }
  std::uniform_int_distribution<uint32_t> distrib(0, 0xffffffff);
  return zx::ok(std::format("{}", distrib(device_number_generator_)));
}

void Devfs::SetController(fidl::ClientEnd<fuchsia_component::Controller> component_controller) {
  component_controller_.Bind(std::move(component_controller), dispatcher(), this);
}

void Devfs::OnComponentStarted(const std::weak_ptr<BootupTracker>& bootup_tracker,
                               const std::string& moniker, zx::result<StartedComponent> component) {
  if (component.is_error()) {
    fdf_log::error("Starting the devfs component failed {}", component.status_string());
    return;
  }
  AttachComponent(std::move(component->info), std::move(component->component_controller));
}

void Devfs::RequestStartComponent(fuchsia_process::wire::HandleInfo startup_handle,
                                  const std::string& moniker,
                                  const std::weak_ptr<BootupTracker>& bootup_tracker) {
  fidl::Arena arena;
  fidl::VectorView<fuchsia_process::wire::HandleInfo> handles(arena, 1);
  handles[0] = std::move(startup_handle);

  auto [client, server] = fidl::Endpoints<fuchsia_component::ExecutionController>::Create();

  component_controller_
      ->Start(
          fuchsia_component::wire::StartChildArgs::Builder(arena).numbered_handles(handles).Build(),
          std::move(server))
      .Then([this, moniker, bootup_tracker](
                fidl::WireUnownedResult<fuchsia_component::Controller::Start>& result) {
        bool is_error = false;
        if (!result.ok()) {
          fdf_log::error("Devfs failed to send StartComponent. {}", result.status_string());
          is_error = true;
        } else if (result->is_error()) {
          fdf_log::error("Devfs failed to StartComponent. {}", result->error_value());
          is_error = true;
        }

        if (is_error) {
          OnComponentStarted(bootup_tracker, moniker, zx::error(ZX_ERR_INTERNAL));
        }
      });
}

zx_status_t Devnode::TryAddService(std::string_view class_name, Target target,
                                   std::string_view instance_name) {
  // Lookup class name in mapping to see if we can translate it to a service name and a service
  // member
  auto name_iterator = kClassNameToService.find(class_name);
  if (name_iterator == kClassNameToService.end()) {
    return ZX_OK;  // If the class is not in the map, then we are not making it available.
  }
  auto& [key, service] = *name_iterator;
  std::string path = "svc/" + service.service_name + "/" + std::string(instance_name);
  component::AnyHandler handler = [passthrough = target](zx::channel channel) {
    (*passthrough->device_connect.get())(std::move(channel));
  };
  zx::result result =
      devfs_.outgoing().AddUnmanagedProtocolAt(std::move(handler), path, service.member_name);
  if (result.is_error()) {
    fdf_log::warn("Failed to add service entry '{}' for class '{}'  {} ({})",
                  (path + "/" + std::string(service.member_name)), class_name,
                  result.status_value(), zx_status_get_string(result.status_value()));
    return result.status_value();
  }
  fdf_log::debug("Added service entry '{}' for class '{}'",
                 (path + "/" + std::string(service.member_name)), class_name);
  // set the service name so we know that we need to remove the service if the devnode is
  // destroyed.
  service_path_ = path;
  service_name_ = service.member_name;

  // Add topological path service
  result = devfs_.outgoing().AddUnmanagedProtocolAt(
      [this, class_name = std::string(class_name)](zx::channel channel) {
        path_server_.Bind(std::move(channel), class_name);
      },
      path, fuchsia_device_fs::wire::kDeviceTopologyName);

  return result.status_value();
}

zx_status_t Devnode::add_child(std::string_view name, std::optional<std::string_view> class_name,
                               Target target, DevfsDevice& out_child) {
  // Check that the child does not have a duplicate name.
  const std::optional other = devfs_.Lookup(children(), name);
  if (other.has_value()) {
    fdf_log::warn("rejecting duplicate device name '{}'", name);
    return ZX_ERR_ALREADY_EXISTS;
  }
  std::string child_path = path_server_.GetPath() + "/" + std::string(name);
  // Export the device to its class directory.  Only if the class name exists in class_names.h
  if (class_name.has_value() && kClassNameToService.contains(class_name.value())) {
    zx::result<std::string> instance_name = devfs_.MakeInstanceName(class_name.value());
    ZX_ASSERT(
        instance_name.is_ok());  // this would only return an error if we didn't have that class
    const ServiceEntry& service_entry = kClassNameToService.at(class_name.value());
    // Add dev/class/<class_name> entry:
    if (service_entry.state & ServiceEntry::kDevfs) {
      out_child.protocol_node() = std::make_unique<Devnode>(
          devfs_, *devfs_.get_class_entry(class_name.value()), target, instance_name.value(),
          child_path, std::string(class_name.value()));
    }
    // Add service entry:
    if (service_entry.state & ServiceEntry::kService) {
      out_child.protocol_node()->TryAddService(class_name.value(), target, instance_name.value());
    }
  }
  // Add entry into dev-topological path:
  out_child.topological_node().emplace(devfs_, children(), std::move(target), name, child_path);

  return ZX_OK;
}

void Devfs::AttachComponent(
    fuchsia_component_runner::ComponentStartInfo info,
    fidl::ServerEnd<fuchsia_component_runner::ComponentController> controller) {
  // Serve the outgoing directory:
  if (!info.outgoing_dir().has_value()) {
    fdf_log::warn("No outgoing dir available for devfs component.");
    return;
  }
  auto result = outgoing_.Serve(std::move(*info.outgoing_dir()));
  if (result.is_error()) {
    fdf_log::warn("Failed to serve the devfs outgoing directory {}", result.status_string());
    return;
  }
  binding_.emplace(
      dispatcher_, std::move(controller), this, [](Devfs* devfs, fidl::UnbindInfo info) {
        devfs->binding_.reset();
        devfs->component_controller_->Destroy().Then(
            [devfs](fidl::WireUnownedResult<fuchsia_component::Controller::Destroy>& result) {
              if (!result.ok() || result->is_error()) {
                devfs->component_controller_ = {};
              }
            });
      });
}

// Called when the component_controller_ is closed after destruction is complete.
void Devfs::on_fidl_error(fidl::UnbindInfo info) { component_controller_ = {}; }

zx::result<fidl::ClientEnd<fio::Directory>> Devfs::Connect(fs::FuchsiaVfs& vfs) {
  auto [client, server] = fidl::Endpoints<fio::Directory>::Create();
  // NB: Serve the `PseudoDir` rather than the root `Devnode` because
  // otherwise we'd end up in the connector code path. Clients that want to open
  // the root node as a device can do so using `"."` and appropriate flags.
  return zx::make_result(vfs.ServeDirectory(root_.node_, std::move(server)), std::move(client));
}

Devfs::Devfs(std::optional<Devnode>& root, async_dispatcher_t* dispatcher)
    : root_(root.emplace(*this)), outgoing_(dispatcher), dispatcher_(dispatcher) {
  PseudoDir& pd = root_.children();
  MustAddEntry(pd, "class", class_);
  MustAddEntry(pd, kNullDevName, fbl::MakeRefCounted<BuiltinDevVnode>(true));
  MustAddEntry(pd, kZeroDevName, fbl::MakeRefCounted<BuiltinDevVnode>(false));
  {
    fbl::RefPtr builtin = fbl::MakeRefCounted<PseudoDir>();
    MustAddEntry(*builtin, kNullDevName, fbl::MakeRefCounted<BuiltinDevVnode>(true));
    MustAddEntry(*builtin, kZeroDevName, fbl::MakeRefCounted<BuiltinDevVnode>(false));
    MustAddEntry(pd, "builtin", std::move(builtin));
  }
  for (const auto& [class_name, service_entry] : kClassNameToService) {
    class_entries_[std::string(class_name)] = fbl::MakeRefCounted<PseudoDir>();
    MustAddEntry(*class_, class_name, class_entries_[std::string(class_name)]);
  }
}

zx_status_t Devnode::export_class(Devnode::Target target, std::string_view class_name,
                                  std::vector<std::unique_ptr<Devnode>>& out) {
  zx::result<std::string> instance_name = devfs_.MakeInstanceName(class_name);
  if (instance_name.is_error()) {
    return instance_name.status_value();
  }
  Devnode& child = *out.emplace_back(std::make_unique<Devnode>(
      devfs_, *devfs_.get_class_entry(class_name), target, instance_name.value(), ""));

  child.publish();
  return ZX_OK;
}

zx_status_t Devnode::export_topological_path(Devnode::Target target,
                                             std::string_view topological_path,
                                             std::vector<std::unique_ptr<Devnode>>& out) {
  // Validate the topological path.
  const std::vector segments =
      fxl::SplitString(topological_path, "/", fxl::WhiteSpaceHandling::kKeepWhitespace,
                       fxl::SplitResult::kSplitWantAll);
  if (segments.empty() ||
      std::any_of(segments.begin(), segments.end(), std::mem_fn(&std::string_view::empty))) {
    return ZX_ERR_INVALID_ARGS;
  }

  // Walk the request export path segment-by-segment.
  Devnode* dn = this;
  for (size_t i = 0; i < segments.size(); ++i) {
    const std::string_view name = segments.at(i);
    zx::result child = [name, &children = dn->children()]() -> zx::result<Devnode*> {
      fbl::RefPtr<fs::Vnode> out;
      switch (const zx_status_t status = children.Lookup(name, &out); status) {
        case ZX_OK:
          return zx::ok(&fbl::RefPtr<Devnode::VnodeImpl>::Downcast(out)->holder_);
        case ZX_ERR_NOT_FOUND:
          break;
        default:
          return zx::error(status);
      }
      const auto it = children.unpublished.find(name);
      if (it != children.unpublished.end()) {
        return zx::ok(&it->second.get());
      }
      return zx::ok(nullptr);
    }();
    if (child.is_error()) {
      return child.status_value();
    }
    if (i != segments.size() - 1) {
      // This is not the final path segment. Use the existing node or create one
      // if it doesn't exist.
      if (child.value() != nullptr) {
        dn = child.value();
        continue;
      }
      PseudoDir& parent = dn->node().children();
      Devnode& child =
          *out.emplace_back(std::make_unique<Devnode>(devfs_, parent, Target{}, name, ""));
      child.publish();
      dn = &child;
      continue;
    }

    // At this point `dn` is the second-last path segment.
    if (child != nullptr) {
      // The full path described by `devfs_path` already exists.
      return ZX_ERR_ALREADY_EXISTS;
    }

    // Create the final child.
    {
      Devnode& child = *out.emplace_back(
          std::make_unique<Devnode>(devfs_, dn->node().children(), std::move(target), name, ""));
      child.publish();
    }
  }
  return ZX_OK;
}

zx_status_t Devnode::export_dir(Devnode::Target target,
                                std::optional<std::string_view> topological_path,
                                std::optional<std::string_view> class_path,
                                std::vector<std::unique_ptr<Devnode>>& out) {
  if (topological_path.has_value()) {
    zx_status_t status = export_topological_path(target, topological_path.value(), out);
    if (status != ZX_OK) {
      return status;
    }
  }

  if (class_path.has_value()) {
    zx_status_t status = export_class(target, class_path.value(), out);
    if (status != ZX_OK) {
      return status;
    }
  }

  return ZX_OK;
}

}  // namespace driver_manager

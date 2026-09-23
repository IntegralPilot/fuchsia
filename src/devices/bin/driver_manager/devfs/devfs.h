// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_MANAGER_DEVFS_DEVFS_H_
#define SRC_DEVICES_BIN_DRIVER_MANAGER_DEVFS_DEVFS_H_

#include <fidl/fuchsia.component.runner/cpp/fidl.h>
#include <fidl/fuchsia.device.fs/cpp/wire.h>
#include <fidl/fuchsia.device/cpp/wire.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/async/dispatcher.h>
#include <lib/component/incoming/cpp/clone.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>

#include <random>

#include <fbl/ref_ptr.h>
#include <fbl/string.h>

#include "src/devices/bin/driver_manager/component_owner.h"
#include "src/storage/lib/vfs/cpp/pseudo_dir.h"
#include "src/storage/lib/vfs/cpp/vnode.h"

namespace driver_manager {

class Devfs;
class PseudoDir;
class DevfsDevice;

// PathServer acts as a contained TopologicalPath server, allowing clients
// to connect and vending the topological path of the devnode.
// Pathserver checks the variable kClassesThatAllowTopologicalPath when clients
// attempt to bind to the service, and prohibits clients from connecting to drivers
// whose class name is not in the allowlist.
class PathServer : public fidl::WireServer<fuchsia_device_fs::TopologicalPath> {
 public:
  explicit PathServer(std::string path, async_dispatcher_t* dispatcher)
      : path_(path), dispatcher_(dispatcher) {}

  // TopologicalPath protocol:
  void GetTopologicalPath(GetTopologicalPathCompleter::Sync& completer) override {
    completer.ReplySuccess(fidl::StringView::FromExternal(GetPath()));
  }

  void Bind(zx::channel channel, const std::string& class_name);

  fidl::ProtocolHandler<fuchsia_device_fs::TopologicalPath> GetHandler() {
    return bindings_.CreateHandler(this, dispatcher_, fidl::kIgnoreBindingClosure);
  }

  std::string GetPath() { return path_; }

 private:
  std::string path_;
  async_dispatcher_t* dispatcher_;
  fidl::ServerBindingGroup<fuchsia_device_fs::TopologicalPath> bindings_;
};

class Devnode {
 public:
  // This class represents a device in devfs. It is called "passthrough" because it sends
  // the channel and the connection type to a callback function.
  struct PassThrough {
    // The Device connect callback is for accessing the /dev/class/xxx protocol for the device
    using DeviceConnectCallback = fit::function<zx_status_t(zx::channel)>;
    // The controller callback is for accessing the fuchsia.device/Controller
    // interface associated with the device.
    using ControllerConnectCallback =
        fit::function<zx_status_t(fidl::ServerEnd<fuchsia_device::Controller>)>;
    // Create a Passthrough class. The client must make sure that any captures in the callback
    // live as long as the passthrough class (for this reason it's strongly recommended to use
    // owned captures).
    explicit PassThrough(DeviceConnectCallback device_callback,
                         ControllerConnectCallback controller_callback)
        : device_connect(std::make_shared<DeviceConnectCallback>(std::move(device_callback))),
          controller_connect(
              std::make_shared<ControllerConnectCallback>(std::move(controller_callback))) {}

    std::shared_ptr<DeviceConnectCallback> device_connect;
    std::shared_ptr<ControllerConnectCallback> controller_connect;
  };

  using Target = std::optional<PassThrough>;

  // Constructs a root node.
  explicit Devnode(Devfs& devfs);

  // `parent` must outlive `this`.
  Devnode(Devfs& devfs, PseudoDir& parent, Target target, fbl::String name, const std::string& path,
          const std::string& class_name = "none");

  ~Devnode();

  Devnode(const Devnode&) = delete;
  Devnode& operator=(const Devnode&) = delete;

  Devnode(Devnode&&) = delete;
  Devnode& operator=(Devnode&&) = delete;

  // Add a child to this Devnode. The child will be added to both the topological path and under the
  // given `class_name`.
  zx_status_t add_child(std::string_view name, std::optional<std::string_view> class_name,
                        Target target, DevfsDevice& out_child);

  // Exports `target`.
  //
  // If `topological_path` is provided, then `target` will be exported at that path under `this`.
  //
  // If `class_path` is provided, then `target` will be exported under that class path.
  zx_status_t export_dir(Devnode::Target target, std::optional<std::string_view> topological_path,
                         std::optional<std::string_view> class_path,
                         std::vector<std::unique_ptr<Devnode>>& out);

  std::string_view name() const;
  PseudoDir& children() const { return node().children(); }
  void advertise_modified();

  // Publishes the node to devfs. Asserts if called more than once.
  void publish();

  // The actual vnode implementation. This is distinct from the outer class
  // because `fs::Vnode` imposes reference-counted semantics, and we want to
  // preserve owned semantics on the outer class.
  //
  // This is exposed for use in tests.
  class VnodeImpl : public fs::Vnode {
   public:
    fuchsia_io::NodeProtocolKinds GetProtocols() const final;
    zx::result<fs::VnodeAttributes> GetAttributes() const final;
    zx_status_t Lookup(std::string_view name, fbl::RefPtr<fs::Vnode>* out) final;
    zx_status_t WatchDir(fs::FuchsiaVfs* vfs, fuchsia_io::wire::WatchMask mask, uint32_t options,
                         fidl::ServerEnd<fuchsia_io::DirectoryWatcher> watcher) final;
    zx_status_t Readdir(fs::VdirCookie* cookie, void* dirents, size_t len,
                        size_t* out_actual) final;
    zx_status_t ConnectService(zx::channel channel) final;

    PseudoDir& children() const { return *children_; }

    Devnode& holder_;
    const Target target_;

   private:
    friend fbl::internal::MakeRefCountedHelper<VnodeImpl>;

    VnodeImpl(Devnode& holder, Target target);

    bool IsDirectory() const;

    fbl::RefPtr<PseudoDir> children_ = fbl::MakeRefCounted<PseudoDir>();
  };

 private:
  // Advertises a service that corresponds to the class name
  zx_status_t TryAddService(std::string_view class_name, Target target,
                            std::string_view instance_name);

  zx_status_t export_class(Devnode::Target target, std::string_view class_path,
                           std::vector<std::unique_ptr<Devnode>>& out);

  zx_status_t export_topological_path(Devnode::Target target, std::string_view topological_path,
                                      std::vector<std::unique_ptr<Devnode>>& out);

  VnodeImpl& node() const { return *node_; }
  const Target& target() const { return node_->target_; }

  friend class Devfs;
  friend class PseudoDir;

  Devfs& devfs_;

  fbl::RefPtr<PseudoDir> parent_;

  const fbl::RefPtr<VnodeImpl> node_;

  const std::optional<fbl::String> name_;
  PathServer path_server_;

  // If service_name_ is valid, it means that there is a service advertised, and should be removed
  // upon destruction of this devnode.
  std::optional<std::string> service_path_;
  std::optional<std::string> service_name_;
};

class PseudoDir : public fs::PseudoDir {
 public:
  std::unordered_map<fbl::String, std::reference_wrapper<Devnode>, std::hash<std::string_view>>
      unpublished;
};

class DevfsDevice {
 public:
  void advertise_modified();
  void publish();
  void unpublish();

  std::optional<Devnode>& topological_node() { return topological_; }

 private:
  friend Devnode;
  std::unique_ptr<Devnode>& protocol_node() { return protocol_; }

  std::optional<Devnode> topological_;
  // TODO(https://fxbug.dev/42062564): These protocol nodes are currently always empty directories.
  // Change this to a pure `RemoteNode` that doesn't expose a directory.
  std::unique_ptr<Devnode> protocol_;
};

// Manages the root functionality of devfs.
// Also acts a a proxy driver.
// Mounts as a boot driver and adversises services that are registered under
// a recognized class name.  See class_names.h for more info.
class Devfs : public fidl::Server<fuchsia_component_runner::ComponentController>,
              public fidl::WireAsyncEventHandler<fuchsia_component::Controller>,
              public ComponentOwner {
 public:
  // `root` must outlive `this`.
  explicit Devfs(std::optional<Devnode>& root, async_dispatcher_t* dispatcher);

  zx::result<fidl::ClientEnd<fuchsia_io::Directory>> Connect(fs::FuchsiaVfs& vfs);

  zx::result<std::string> MakeInstanceName(std::string_view class_name);

  // ComponentOwner
  void SetController(fidl::ClientEnd<fuchsia_component::Controller> component_controller) override;
  void OnComponentStarted(const std::weak_ptr<BootupTracker>& bootup_tracker,
                          const std::string& moniker,
                          zx::result<StartedComponent> component) override;
  void RequestStartComponent(fuchsia_process::wire::HandleInfo startup_handle,
                             const std::string& moniker,
                             const std::weak_ptr<BootupTracker>& bootup_tracker) override;
  bool SkipInjectedOffers() const override { return true; }

  fbl::RefPtr<PseudoDir> get_class_entry(std::string_view class_name) {
    ZX_ASSERT(class_entries_.contains(std::string(class_name)));
    return class_entries_[std::string(class_name)];
  }

  async_dispatcher_t* dispatcher() { return dispatcher_; }
  component::OutgoingDirectory& outgoing() { return outgoing_; }

  // Called by the Driver Runner when the special devfs driver component is
  // created.
  void AttachComponent(fuchsia_component_runner::ComponentStartInfo info,
                       fidl::ServerEnd<fuchsia_component_runner::ComponentController> controller);

  // fuchsia_component_runner::ComponentController
  void Stop(StopCompleter::Sync& completer) override { CloseComponent(); }
  void Kill(KillCompleter::Sync& completer) override { CloseComponent(); }
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_component_runner::ComponentController> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {}

  // fidl::WireAsyncEventHandler<fuchsia_component::Controller>
  void on_fidl_error(fidl::UnbindInfo info) override;
  void handle_unknown_event(
      fidl::UnknownEventMetadata<fuchsia_component::Controller> metadata) override {}

 private:
  friend class Devnode;

  static std::optional<std::reference_wrapper<fs::Vnode>> Lookup(PseudoDir& parent,
                                                                 std::string_view name);

  // close the fake driver component
  void CloseComponent();

  Devnode& root_;
  component::OutgoingDirectory outgoing_;
  async_dispatcher_t* dispatcher_;
  std::optional<fidl::ServerBinding<fuchsia_component_runner::ComponentController>> binding_;
  fidl::WireClient<fuchsia_component::Controller> component_controller_;
  std::default_random_engine device_number_generator_;

  fbl::RefPtr<PseudoDir> class_ = fbl::MakeRefCounted<PseudoDir>();
  std::unordered_map<std::string, fbl::RefPtr<PseudoDir>> class_entries_;
};

}  // namespace driver_manager

#endif  // SRC_DEVICES_BIN_DRIVER_MANAGER_DEVFS_DEVFS_H_

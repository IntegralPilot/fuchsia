// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_BIN_VULKAN_LOADER_APP_H_
#define SRC_GRAPHICS_BIN_VULKAN_LOADER_APP_H_

#include <lib/async-loop/cpp/loop.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/fit/defer.h>
#include <lib/fit/thread_checker.h>
#include <lib/inspect/component/cpp/component.h>

#include <unordered_map>

#include "src/graphics/bin/vulkan_loader/gpu_device.h"
#include "src/graphics/bin/vulkan_loader/structured_config_lib.h"
#include "src/lib/fsl/io/device_watcher.h"
#include "src/lib/fxl/macros.h"
#include "src/lib/fxl/observer_list.h"
#include "src/lib/fxl/synchronization/thread_annotations.h"
#include "src/storage/lib/vfs/cpp/pseudo_dir.h"
#include "src/storage/lib/vfs/cpp/synchronous_vfs.h"

class MagmaDevice;
class IcdComponent;

class LoaderApp {
 public:
  class Observer {
   public:
    // Called if the ICD list may have changed.
    virtual void OnIcdListChanged(LoaderApp* app) = 0;
  };

  // This token represents the existence of an outstanding operation that could
  // affect the ICD list. It will defer the signaling that an ICD doesn't exist
  // until it's destroyed.
  class PendingActionToken {
   public:
    ~PendingActionToken();

   private:
    friend class LoaderApp;

    explicit PendingActionToken(LoaderApp* app) : app_(app) {
      std::lock_guard lock(app->pending_action_mutex_);
      app->pending_action_count_++;
    }

    LoaderApp* app_;

    FXL_DISALLOW_COPY_ASSIGN_AND_MOVE(PendingActionToken);
  };

  explicit LoaderApp(component::OutgoingDirectory* outgoing_dir, async_dispatcher_t* dispatcher,
                     structured_config_lib::Config structured_config);

  ~LoaderApp();

  zx_status_t InitDeviceWatcher();

  zx_status_t ServeDeviceFs(fidl::ServerEnd<fuchsia_io::Directory> server_end);
  zx_status_t ServeTrustedDeviceFs(fidl::ServerEnd<fuchsia_io::Directory> server_end);
  zx_status_t ServeManifestFs(fidl::ServerEnd<fuchsia_io::Directory> server_end);

  // Initialize and serve the debug directory for the loader app.
  zx_status_t InitDebugFs();

  zx::result<std::shared_ptr<IcdComponent>> CreateIcdComponent(const std::string& component_url);

  void AddDevice(std::unique_ptr<GpuDevice> device) { devices_.push_back(std::move(device)); }
  void RemoveDevice(GpuDevice* device);

  // Notify observers that an ICD list has changed.
  void NotifyIcdsChanged();

  void AddObserver(Observer* obs) { observer_list_.AddObserver(obs); }
  void RemoveObserver(Observer* obs) { observer_list_.RemoveObserver(obs); }

  // Returns an ICD vmo that matches system_lib_name.
  std::optional<zx::vmo> GetMatchingIcd(const std::string& system_lib_name);

  size_t device_count() const { return devices_.size(); }
  const std::vector<std::unique_ptr<GpuDevice>>& devices() const { return devices_; }

  async_dispatcher_t* dispatcher() { return dispatcher_; }
  async_dispatcher_t* fdio_loop_dispatcher() { return fdio_loop_.dispatcher(); }

  std::unique_ptr<PendingActionToken> GetPendingActionToken();

  fbl::RefPtr<fs::PseudoDir> manifest_fs_root_node() { return manifest_fs_root_node_; }

  bool HavePendingActions() const {
    std::lock_guard lock(pending_action_mutex_);
    return pending_action_count_ > 0 || icd_notification_pending_;
  }

  bool allow_magma_icds() const { return allow_magma_icds_; }
  bool allow_goldfish_icd() const { return allow_goldfish_icd_; }
  bool allow_lavapipe_icd() const { return allow_lavapipe_icd_; }
  std::string lavapipe_icd_url() const { return lavapipe_icd_url_; }

 private:
  friend class LoaderActionToken;
  void NotifyIcdsChangedOnMainThread();
  void NotifyIcdsChangedLocked() FXL_REQUIRE(pending_action_mutex_);

  zx_status_t InitDeviceFs();
  zx_status_t InitTrustedDeviceFs();
  zx_status_t InitCommonDeviceFs(fbl::RefPtr<fs::PseudoDir>& root_node);

  FIT_DECLARE_THREAD_CHECKER(main_thread_)

  component::OutgoingDirectory* outgoing_dir_;
  async_dispatcher_t* dispatcher_;
  inspect::ComponentInspector inspector_;
  inspect::Node devices_node_;
  inspect::Node config_node_;
  inspect::Node icds_node_;

  mutable std::mutex pending_action_mutex_;
  bool icd_notification_pending_ FXL_GUARDED_BY(pending_action_mutex_) = false;

  // Keep track of the number of pending operations that have the potential to modify the tree.
  uint64_t pending_action_count_ FXL_GUARDED_BY(pending_action_mutex_) = 0;

  fs::SynchronousVfs debug_fs_;
  fbl::RefPtr<fs::PseudoDir> debug_root_node_;
  fbl::RefPtr<fs::PseudoDir> device_root_node_;
  // Like device_root_node_, but contains trusted services
  fbl::RefPtr<fs::PseudoDir> trusted_device_root_node_;
  fbl::RefPtr<fs::PseudoDir> manifest_fs_root_node_;

  std::vector<std::unique_ptr<fsl::DeviceWatcher>> device_watchers_;

  std::vector<std::unique_ptr<GpuDevice>> devices_;

  std::unordered_map<std::string, std::shared_ptr<IcdComponent>> icd_components_;

  fxl::ObserverList<Observer> observer_list_;

  // The FDIO loop is used to run FDIO commands that may access an ICD
  // component's package. Those commands may block because they require the
  // IcdRunner to service them.
  async::Loop fdio_loop_;

  // Read from structured config.  When these are false, the corresponding type of device is never
  // added to |devices_|.  For device types that we would ordinarily watch for changes in device
  // availability, we don't bother watching, since we wouldn't add the device to |devices_| anyway.
  bool allow_magma_icds_ = false;
  bool allow_goldfish_icd_ = false;
  bool allow_lavapipe_icd_ = false;
  std::string lavapipe_icd_url_;

  bool lavapipe_added_ = false;
};

#endif  // SRC_GRAPHICS_BIN_VULKAN_LOADER_APP_H_

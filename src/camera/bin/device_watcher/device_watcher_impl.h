// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CAMERA_BIN_DEVICE_WATCHER_DEVICE_WATCHER_IMPL_H_
#define SRC_CAMERA_BIN_DEVICE_WATCHER_DEVICE_WATCHER_IMPL_H_

#include <fidl/fuchsia.camera2.hal/cpp/fidl.h>
#include <fidl/fuchsia.camera3/cpp/fidl.h>
#include <fidl/fuchsia.component/cpp/fidl.h>
#include <fidl/fuchsia.hardware.camera/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fidl/cpp/binding.h>
#include <lib/fit/function.h>
#include <lib/fpromise/result.h>
#include <zircon/status.h>

#include <memory>
#include <queue>
#include <set>
#include <unordered_map>

#include "lib/async/dispatcher.h"
#include "src/camera/bin/device_watcher/device_instance.h"

namespace camera {

constexpr auto kCameraPath = "/svc/fuchsia.hardware.camera.Service";

constexpr std::string_view kMipiCsiDeviceInstanceCollectionName{"csi_camera_devices"};
constexpr std::string_view kMipiCsiDeviceInstanceNamePrefix{"csi_camera_device_"};
constexpr std::string_view kMipiCsiDeviceInstanceUrl{
    "fuchsia-pkg://fuchsia.com/camera_device#meta/camera_device.cm"};

constexpr std::string_view kUvcDeviceInstanceCollectionName{"usb_camera_devices"};
constexpr std::string_view kUvcDeviceInstanceNamePrefix{"usb_camera_device_"};
constexpr std::string_view kUvcDeviceInstanceUrl{
    "fuchsia-pkg://fuchsia.com/usb_camera_device#meta/usb_camera_device.cm"};

using ClientId = uint64_t;
using TransientDeviceId = uint64_t;
using PersistentDeviceId = uint64_t;

enum CameraType {
  kCameraTypeMipiCsi = 1,
  kCameraTypeUvc = 2,
};

struct UniqueDevice {
  TransientDeviceId id;
  std::unique_ptr<DeviceInstance> instance;
};

using DevicesMap = std::unordered_map<PersistentDeviceId, UniqueDevice>;

class DeviceWatcherImpl {
 public:
  static fpromise::result<std::unique_ptr<DeviceWatcherImpl>, zx_status_t> Create(
      fidl::ClientEnd<fuchsia_component::Realm> realm, async_dispatcher_t* dispatcher);

  fpromise::result<CameraType, zx_status_t> GetDeviceInfoAndIdentifyCameraType(
      fidl::SyncClient<fuchsia_hardware_camera::Device>& dev,
      fuchsia_camera2::DeviceInfo& device_info, const std::string& full_path);
  void AddDeviceByPath(const std::string& path);
  void UpdateClients();
  fidl::ProtocolHandler<fuchsia_camera3::DeviceWatcher> GetHandler();

  void OnNewRequest(fidl::ServerEnd<fuchsia_camera3::DeviceWatcher> request);

 private:
  void ConnectDynamicChild(fidl::ServerEnd<fuchsia_camera3::Device> request,
                           const UniqueDevice& unique_device);

  fpromise::result<PersistentDeviceId, zx_status_t> AddMipiCsiDevice(
      fidl::ClientEnd<fuchsia_hardware_camera::Device> camera,
      fuchsia_camera2::DeviceInfo& device_info, const std::string& path);

  fpromise::result<PersistentDeviceId, zx_status_t> AddUvcDevice(
      fidl::ClientEnd<fuchsia_hardware_camera::Device> camera,
      fuchsia_camera2::DeviceInfo& device_info, const std::string& path);

  // Implements the server endpoint for a single client, and maintains per-client state.
  class Client : public fidl::Server<fuchsia_camera3::DeviceWatcher> {
   public:
    explicit Client(DeviceWatcherImpl& watcher);
    static fpromise::result<std::unique_ptr<Client>, zx_status_t> Create(
        DeviceWatcherImpl& watcher, ClientId id,
        fidl::ServerEnd<fuchsia_camera3::DeviceWatcher> request, async_dispatcher_t* dispatcher);
    void UpdateDevices(const DevicesMap& devices);
    explicit operator bool();

   private:
    void CheckDevicesChanged();
    // |fuchsia_camera3::DeviceWatcher|
    void WatchDevices(WatchDevicesCompleter::Sync& completer) override;
    void ConnectToDevice(ConnectToDeviceRequest& request,
                         ConnectToDeviceCompleter::Sync& completer) override;

    DeviceWatcherImpl& watcher_;
    ClientId id_;
    std::optional<fidl::ServerBinding<fuchsia_camera3::DeviceWatcher>> binding_;
    std::optional<WatchDevicesCompleter::Async> completer_;
    std::set<TransientDeviceId> last_known_ids_;
    std::optional<std::set<TransientDeviceId>> last_sent_ids_;
  };

  async_dispatcher_t* dispatcher_;

  fidl::Client<fuchsia_component::Realm> realm_;
  TransientDeviceId device_id_next_ = 1;
  DevicesMap devices_;
  ClientId client_id_next_ = 1;
  std::unordered_map<ClientId, std::unique_ptr<Client>> clients_;
  bool initial_update_received_ = false;
  std::queue<fidl::ServerEnd<fuchsia_camera3::DeviceWatcher>> requests_;
};

}  // namespace camera

#endif  // SRC_CAMERA_BIN_DEVICE_WATCHER_DEVICE_WATCHER_IMPL_H_

// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.camera.test/cpp/fidl.h>
#include <fidl/fuchsia.camera3/cpp/fidl.h>
#include <fidl/fuchsia.component/cpp/fidl.h>
#include <fidl/fuchsia.hardware.camera/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fdio.h>
#include <lib/fidl/cpp/binding_set.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>

#include "src/camera/bin/device_watcher/device_watcher_impl.h"
#include "src/lib/fsl/io/device_watcher.h"

class DeviceWatcherTesterImpl : public fidl::Server<fuchsia_camera_test::DeviceWatcherTester> {
 public:
  using InjectDeviceCallback =
      fit::function<void(fidl::ClientEnd<fuchsia_hardware_camera::Device>)>;
  using InjectDeviceByPathCallback = fit::function<void(std::string)>;

  explicit DeviceWatcherTesterImpl(InjectDeviceCallback callback)
      : callback_(std::move(callback)) {}
  explicit DeviceWatcherTesterImpl(InjectDeviceByPathCallback callback)
      : by_path_callback_(std::move(callback)) {}

  // |fuchsia_camera_test::DeviceWatcherTester|
  void InjectDevice(InjectDeviceRequest& request, InjectDeviceCompleter::Sync& completer) override {
    ZX_ASSERT(callback_);
    callback_(std::move(request.camera()));
  }

  void InjectDeviceByPath(InjectDeviceByPathRequest& request,
                          InjectDeviceByPathCompleter::Sync& completer) override {
    ZX_ASSERT(by_path_callback_);
    by_path_callback_(std::move(request.path()));
  }

  fidl::ProtocolHandler<fuchsia_camera_test::DeviceWatcherTester> GetHandler(
      async_dispatcher_t* dispatcher) {
    return bindings_.CreateHandler(this, dispatcher, &DeviceWatcherTesterImpl::OnClosed);
  }

  static void OnClosed(fidl::UnbindInfo info) {
    if (info.is_user_initiated()) {
      return;
    }
    if (info.is_peer_closed()) {
      FX_LOGS(INFO) << "Client disconnected.";
    } else {
      FX_LOGS(ERROR) << "Server error: " << info;
    }
  }

 private:
  fidl::ServerBindingGroup<fuchsia_camera_test::DeviceWatcherTester> bindings_;
  InjectDeviceCallback callback_ = nullptr;
  InjectDeviceByPathCallback by_path_callback_ = nullptr;
};

int main(int argc, char* argv[]) {
  fuchsia_logging::LogSettingsBuilder builder;
  builder.WithMinLogSeverity(CAMERA_MIN_LOG_LEVEL)
      .WithTags({"camera", "camera_device_watcher"})
      .BuildAndInitialize();

  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  async_dispatcher_t* dispatcher = loop.dispatcher();

  auto realm_client = component::Connect<fuchsia_component::Realm>();
  if (realm_client.is_error()) {
    FX_LOGS(FATAL) << "Failed to connect to realm service: " << realm_client.status_string();
    return EXIT_FAILURE;
  }

  auto server_create_result =
      camera::DeviceWatcherImpl::Create(std::move(realm_client.value()), loop.dispatcher());
  if (server_create_result.is_error()) {
    FX_PLOGS(FATAL, server_create_result.error());
    return EXIT_FAILURE;
  }

  auto server = server_create_result.take_value();
  auto watcher = fsl::DeviceWatcher::CreateWithIdleCallback(
      camera::kCameraPath,
      [&](const fidl::ClientEnd<fuchsia_io::Directory>& dir, const std::string& path) {
        server->AddDeviceByPath(path);
      },
      [&]() { server->UpdateClients(); });
  if (!watcher) {
    FX_LOGS(FATAL) << "Failed to create fsl::DeviceWatcher";
    return EXIT_FAILURE;
  }

  auto outgoing = component::OutgoingDirectory(dispatcher);

  zx::result directory_result = outgoing.ServeFromStartupInfo();
  if (directory_result.is_error()) {
    FX_LOGS(ERROR) << "Failed to serve outgoing directory: " << directory_result.status_string();
    return -1;
  }

  zx::result server_result = outgoing.AddUnmanagedProtocol<fuchsia_camera3::DeviceWatcher>(
      [&](fidl::ServerEnd<fuchsia_camera3::DeviceWatcher> server_end) {
        server->OnNewRequest(std::move(server_end));
      });
  if (server_result.is_error()) {
    FX_LOGS(ERROR) << "Failed to add DeviceWatcher protocol: " << server_result.status_string();
    return -1;
  }

  DeviceWatcherTesterImpl tester([&](const std::string& path) {
    server->AddDeviceByPath(path);
    server->UpdateClients();
  });
  zx::result tester_result =
      outgoing.AddUnmanagedProtocol<fuchsia_camera_test::DeviceWatcherTester>(
          tester.GetHandler(dispatcher));
  if (tester_result.is_error()) {
    FX_LOGS(ERROR) << "Failed to add DeviceWatcherTesterImpl protocol: "
                   << tester_result.status_string();
  }

  loop.Run();
  return EXIT_SUCCESS;
}

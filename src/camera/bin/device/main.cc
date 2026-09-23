// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.camera/cpp/wire.h>
#include <fuchsia/camera2/hal/cpp/fidl.h>
#include <fuchsia/hardware/camera/cpp/fidl.h>
#include <fuchsia/ui/policy/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/executor.h>
#include <lib/async/cpp/wait.h>
#include <lib/component/incoming/cpp/service_member_watcher.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fdio.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace-provider/provider.h>
#include <lib/zx/result.h>

#include <filesystem>
#include <string>

#include "src/camera/bin/device/device_impl.h"
#include "src/camera/bin/device/metrics_reporter.h"

namespace camera {

using DeviceHandle = fuchsia::hardware::camera::DeviceHandle;

static zx::result<DeviceHandle> GetCameraHandle() {
  component::SyncServiceMemberWatcher<fuchsia_hardware_camera::Service::Device> watcher;
  auto client_end = watcher.GetNextInstance(true);
  if (client_end.is_error()) {
    if (client_end.error_value() == ZX_ERR_STOP) {
      return zx::error(ZX_ERR_NOT_FOUND);
    }
    return client_end.take_error();
  }
  return zx::ok(DeviceHandle(client_end.value().TakeChannel()));
}

}  // namespace camera

int main(int argc, char* argv[]) {
  fuchsia_logging::LogSettingsBuilder builder;
  builder.WithMinLogSeverity(CAMERA_MIN_LOG_LEVEL)
      .WithTags({"camera", "camera_device"})
      .BuildAndInitialize();

  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  async::Executor executor(loop.dispatcher());
  trace::TraceProviderWithFdio trace_provider(loop.dispatcher());
  auto context = sys::ComponentContext::Create();

  std::string outgoing_service_name("fuchsia.camera3.Device");

  // Special hack to connect to controller. Works for one device, but not for multiple.
  // TODO(ernesthua) - Need to make this scalable to multiple devices. camera_device_watcher knows
  // about the specific device just found, so that information must be passed to camera_device
  // instead of hardcoding it here.
  fuchsia::camera2::hal::ControllerSyncPtr controller;
  {
    zx::result result = camera::GetCameraHandle();
    if (result.is_error()) {
      FX_PLOGS(INFO, result.status_value())
          << "Couldn't get camera. This device will not be exposed to clients.";
      return EXIT_FAILURE;
    }
    camera::DeviceHandle& device_handle = result.value();
    fuchsia::hardware::camera::DeviceSyncPtr device;
    device.Bind(std::move(device_handle));

    auto status = device->GetChannel2(controller.NewRequest());
    if (status != ZX_OK) {
      FX_PLOGS(FATAL, status) << "Failed to request controller service.";
      return EXIT_FAILURE;
    }

    fuchsia::camera2::DeviceInfo device_info;
    status = controller->GetDeviceInfo(&device_info);
    if (status != ZX_OK) {
      FX_PLOGS(FATAL, status) << "Failed to probe for device info.";
      return EXIT_FAILURE;
    }
  }

  // Connect to required environment services.
  fuchsia::sysmem2::AllocatorHandle allocator;
  auto status = context->svc()->Connect(allocator.NewRequest());
  if (status != ZX_OK) {
    FX_PLOGS(FATAL, status) << "Failed to request allocator service.";
    return EXIT_FAILURE;
  }

  fuchsia::ui::policy::DeviceListenerRegistryHandle registry;
  status = context->svc()->Connect(registry.NewRequest());
  if (status != ZX_OK) {
    FX_PLOGS(FATAL, status) << "Failed to request registry service.";
    return EXIT_FAILURE;
  }

  // Post a quit task in the event the device enters a bad state.
  zx::event event;
  FX_CHECK(zx::event::create(0, &event) == ZX_OK);
  async::Wait wait(event.get(), ZX_EVENT_SIGNALED, 0,
                   [&](async_dispatcher_t* dispatcher, async::Wait* wait, zx_status_t status,
                       const zx_packet_signal_t* signal) {
                     FX_LOGS(FATAL) << "Device signaled bad state.";
                     loop.Quit();
                   });
  ZX_ASSERT(wait.Begin(loop.dispatcher()) == ZX_OK);

  // Create our metrics reporter.
  camera::MetricsReporter::Initialize(*context, /* enable_cobalt = */ true);

  // Create the device and publish its service.
  auto result =
      camera::DeviceImpl::Create(loop.dispatcher(), executor, std::move(controller),
                                 std::move(allocator), std::move(registry), std::move(event));
  std::unique_ptr<camera::DeviceImpl> device;
  executor.schedule_task(
      result.then([&context, &device, &loop, &outgoing_service_name](
                      fpromise::result<std::unique_ptr<camera::DeviceImpl>, zx_status_t>& result) {
        if (result.is_error()) {
          FX_PLOGS(FATAL, result.error()) << "Failed to create device.";
          loop.Quit();
          return;
        }
        device = result.take_value();

        // TODO(https://fxbug.dev/42121063): publish discoverable service name once supported
        zx_status_t status =
            context->outgoing()->AddPublicService(device->GetHandler(), outgoing_service_name);
        if (status != ZX_OK) {
          FX_PLOGS(FATAL, status) << "Failed to publish service.";
          loop.Quit();
          return;
        }
        context->outgoing()->ServeFromStartupInfo();
      }));

  loop.Run();
  return EXIT_SUCCESS;
}

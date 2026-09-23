// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.camera/cpp/wire.h>
#include <fuchsia/camera/cpp/fidl.h>
#include <fuchsia/hardware/camera/cpp/fidl.h>
#include <fuchsia/ui/policy/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/executor.h>
#include <lib/async/cpp/wait.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/result.h>
#include <zircon/processargs.h>

#include <string>

#include "src/camera/bin/usb_device/device_impl.h"

zx::result<fuchsia::camera::ControlSyncPtr> OpenCamera(
    fidl::ClientEnd<fuchsia_hardware_camera::Device> client_end) {
  fuchsia::camera::ControlSyncPtr ctrl;
  zx_status_t status =
      fidl::WireCall(client_end)->GetChannel(ctrl.NewRequest().TakeChannel()).status();
  if (status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Call to GetChannel failed";
    return zx::error(status);
  }

  fuchsia::camera::DeviceInfo info_return;
  status = ctrl->GetDeviceInfo(&info_return);
  if (status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Call to GetDeviceInfo failed";
    return zx::error(status);
  }

  FX_LOGS(INFO) << "Got Device Info:";
  FX_LOGS(INFO) << "Vendor: " << info_return.vendor_name << " (" << info_return.vendor_id << ")";
  return zx::ok(std::move(ctrl));
}

int main(int argc, char* argv[]) {
  fuchsia_logging::LogSettingsBuilder builder;
  builder.WithMinLogSeverity(CAMERA_MIN_LOG_LEVEL)
      .WithTags({"camera", "camera_device"})
      .BuildAndInitialize();

  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  async::Executor executor(loop.dispatcher());
  auto context = sys::ComponentContext::Create();

  // Removed argument parsing.
  // TODO(ernesthua) - Need to bring back argument parsing on merge back.
  std::string outgoing_service_name("fuchsia.camera3.Device");

  // We receive a channel that we interpret as a fuchsia.camera.Control
  // connection.
  zx::channel camera_channel(zx_take_startup_handle(PA_HND(PA_USER0, 0)));
  if (!camera_channel.is_valid()) {
    FX_LOGS(FATAL) << "Received invalid camera handle";
    return EXIT_FAILURE;
  }

  // Connect to USB camera device.
  zx::result status_or =
      OpenCamera(fidl::ClientEnd<fuchsia_hardware_camera::Device>{std::move(camera_channel)});
  if (status_or.is_error()) {
    FX_PLOGS(FATAL, status_or.error_value())
        << "Failed to request camera device: error: " << status_or.error_value();
    return EXIT_FAILURE;
  }
  auto control_sync_ptr = std::move(*(status_or));

  // Connect to required environment services.
  fuchsia::sysmem2::AllocatorHandle allocator_handle;
  fuchsia::sysmem2::AllocatorPtr allocator_ptr;
  zx_status_t status = context->svc()->Connect(allocator_handle.NewRequest());
  if (status != ZX_OK) {
    FX_PLOGS(FATAL, status) << "Failed to request allocator service.";
    return EXIT_FAILURE;
  }
  allocator_ptr = allocator_handle.Bind();

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

  // Create the device and publish its service.
  std::unique_ptr<camera::DeviceImpl> device;
  auto create_device_and_add_service =
      camera::DeviceImpl::Create(loop.dispatcher(), std::move(control_sync_ptr),
                                 std::move(allocator_ptr), std::move(event))
          .and_then([&](std::unique_ptr<camera::DeviceImpl>& dev) {
            device = std::move(dev);

            // TODO(https://fxbug.dev/42121063): publish discoverable service name once
            // supported
            zx_status_t status =
                context->outgoing()->AddPublicService(device->GetHandler(), outgoing_service_name);
            if (status != ZX_OK) {
              FX_PLOGS(FATAL, status) << "Failed to publish service.";
              loop.Quit();
              return;
            }
            context->outgoing()->ServeFromStartupInfo();
          })
          .or_else([&loop](zx_status_t& error) {
            FX_PLOGS(FATAL, error) << "Failed to create device.";
            loop.Quit();
          });

  executor.schedule_task(std::move(create_device_and_add_service));
  loop.Run();
  return EXIT_SUCCESS;
}

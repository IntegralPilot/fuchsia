// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "debug_client.h"

#include <fidl/fuchsia.hardware.camera/cpp/wire.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/component/incoming/cpp/service_member_watcher.h>
#include <lib/fdio/directory.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/status.h>

#include <filesystem>

namespace camera {

DebugClient::DebugClient(async::Loop& loop)
    : loop_(loop), context_(sys::ComponentContext::Create()) {}

void DebugClient::Start(uint32_t mode) {
  ConnectToServer();
  SendCommand(mode);
}

bool DebugClient::ConnectToServer() {
  fuchsia::hardware::camera::DeviceHandle device_handle;
  component::SyncServiceMemberWatcher<fuchsia_hardware_camera::Service::Device> watcher;
  auto result = watcher.GetNextInstance(/*stop_at_idle=*/true);
  if (result.is_error()) {
    FX_PLOGS(ERROR, result.status_value()) << "Failed to connect to controller device";
    Quit();
  }

  fuchsia::hardware::camera::DeviceSyncPtr device_sync_ptr;
  device_sync_ptr.Bind(result.value().TakeChannel());

  if (zx_status_t status =
          device_sync_ptr->GetDebugChannel(debug_ptr_.NewRequest(loop_.dispatcher()));
      status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Failed to access Debug API";
    Quit();
  }

  debug_ptr_.set_error_handler([this](zx_status_t status) {
    FX_PLOGS(ERROR, status) << "Debug API disconnected";
    Quit();
  });
  return true;
}

void DebugClient::SendCommand(uint32_t mode) {
  debug_ptr_->SetDefaultSensorMode(mode, [this](zx_status_t status) {
    if (status != ZX_OK) {
      FX_PLOGS(ERROR, status) << "SetDefaultSensorMode failed";
    }
    Quit();
  });
}

void DebugClient::Quit() {
  loop_.Quit();
  loop_.JoinThreads();
}

}  // namespace camera

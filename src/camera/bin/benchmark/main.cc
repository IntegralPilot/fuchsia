// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include <fidl/fuchsia.hardware.ram.metrics/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/incoming/cpp/service_member_watcher.h>
#include <lib/fdio/directory.h>
#include <lib/sys/cpp/component_context.h>

#include <iostream>

#include "src/camera/bin/benchmark/bandwidth.h"

int main(int argc, char* argv[]) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  auto context = sys::ComponentContext::Create();

  fuchsia::sysmem2::AllocatorHandle sysmem_allocator;
  context->svc()->Connect(sysmem_allocator.NewRequest());

  fuchsia::camera3::DeviceWatcherHandle camera_device_watcher;
  context->svc()->Connect(camera_device_watcher.NewRequest());

  zx::result client_end =
      component::SyncServiceMemberWatcher<fuchsia_hardware_ram_metrics::Service::Device>()
          .GetNextInstance(true);
  fuchsia::hardware::ram::metrics::DeviceHandle metrics_device;
  metrics_device.set_channel(client_end->TakeChannel());

  camera::benchmark::Bandwidth bandwidth(std::move(sysmem_allocator),
                                         std::move(camera_device_watcher),
                                         std::move(metrics_device), loop.dispatcher());
  bandwidth.Profile(std::cout, [&] { loop.Quit(); });

  loop.Run();
  return 0;
}

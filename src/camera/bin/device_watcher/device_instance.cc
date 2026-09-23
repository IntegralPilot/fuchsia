// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/camera/bin/device_watcher/device_instance.h"

#include <fidl/fuchsia.camera2.hal/cpp/fidl.h>
#include <fidl/fuchsia.component/cpp/fidl.h>
#include <fidl/fuchsia.io/cpp/fidl.h>
#include <fidl/fuchsia.process/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/errors.h>
#include <zircon/processargs.h>

#include "lib/fit/function.h"

namespace camera {

fpromise::result<std::unique_ptr<DeviceInstance>, zx_status_t> DeviceInstance::Create(
    fidl::ClientEnd<fuchsia_hardware_camera::Device> camera,
    const fidl::Client<fuchsia_component::Realm>& realm, async_dispatcher_t* dispatcher,
    const std::string& collection_name, const std::string& child_name, const std::string& url) {
  auto instance = std::make_unique<DeviceInstance>();
  instance->dispatcher_ = dispatcher;
  instance->name_ = child_name;
  instance->collection_name_ = collection_name;

  // Launch the child device.
  fuchsia_component_decl::CollectionRef collection{{.name = collection_name}};
  fuchsia_component_decl::Child child;
  child.name(child_name);
  child.url(url);
  child.startup(fuchsia_component_decl::StartupMode::kLazy);

  fuchsia_component::CreateChildArgs args;

  // Pass the camera DeviceHandle to the child so the child can communicate with the correct
  // instance.
  fuchsia_process::HandleInfo handle_info{{
      .handle = zx::handle(camera.TakeChannel().release()),
      .id = PA_HND(PA_USER0, 0),
  }};
  std::vector<fuchsia_process::HandleInfo> numbered_handles;
  numbered_handles.push_back(std::move(handle_info));
  args.numbered_handles(std::move(numbered_handles));

  realm->CreateChild({{.collection = collection, .decl = child, .args = std::move(args)}})
      .Then([child_name](fidl::Result<fuchsia_component::Realm::CreateChild>& result) {
        if (result.is_error()) {
          FX_LOGS(ERROR) << "Failed to create camera device child. Result: "
                         << result.error_value();
          ZX_ASSERT(false);  // Should never happen.
        }
        FX_LOGS(INFO) << "Created camera device child: " << child_name;
      });

  // TODO(b/244178394) - Need to have handlers/callbacks for child component exits or crashes.

  return fpromise::ok(std::move(instance));
}

}  // namespace camera

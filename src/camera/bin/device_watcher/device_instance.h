// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CAMERA_BIN_DEVICE_WATCHER_DEVICE_INSTANCE_H_
#define SRC_CAMERA_BIN_DEVICE_WATCHER_DEVICE_INSTANCE_H_

#include <fidl/fuchsia.camera2.hal/cpp/fidl.h>
#include <fidl/fuchsia.camera3/cpp/fidl.h>
#include <fidl/fuchsia.component/cpp/fidl.h>
#include <fidl/fuchsia.hardware.camera/cpp/fidl.h>
#include <fidl/fuchsia.io/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fit/function.h>
#include <lib/fpromise/result.h>
#include <zircon/status.h>

namespace camera {

// Represents a launched device process.
class DeviceInstance {
 public:
  static fpromise::result<std::unique_ptr<DeviceInstance>, zx_status_t> Create(
      fidl::ClientEnd<fuchsia_hardware_camera::Device> camera,
      const fidl::Client<fuchsia_component::Realm>& realm, async_dispatcher_t* dispatcher,
      const std::string& collection_name, const std::string& child_name, const std::string& url);
  const std::string& name() { return name_; }
  const std::string& collection_name() { return collection_name_; }

 private:
  async_dispatcher_t* dispatcher_;
  std::string name_;
  std::string collection_name_;
  fidl::Client<fuchsia_hardware_camera::Device> camera_;
};

}  // namespace camera

#endif  // SRC_CAMERA_BIN_DEVICE_WATCHER_DEVICE_INSTANCE_H_

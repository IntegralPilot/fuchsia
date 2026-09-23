// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/bin/vulkan_loader/goldfish_device.h"

#include <lib/component/incoming/cpp/protocol.h>
#include <lib/syslog/cpp/macros.h>

#include "src/graphics/bin/vulkan_loader/app.h"

// static
std::unique_ptr<GoldfishDevice> GoldfishDevice::Create(
    LoaderApp* app, const fidl::ClientEnd<fuchsia_io::Directory>& dir, const std::string& name,
    inspect::Node* parent) {
  std::unique_ptr<GoldfishDevice> device(new GoldfishDevice(app));
  if (!device->Initialize(dir, name, parent))
    return nullptr;
  return device;
}

bool GoldfishDevice::Initialize(const fidl::ClientEnd<fuchsia_io::Directory>& dir,
                                const std::string& name, inspect::Node* parent) {
  node() = parent->CreateChild("goldfish-" + name);
  icd_list_.Initialize(&node());
  auto pending_action_token = app()->GetPendingActionToken();

  zx::result<fidl::ClientEnd<fuchsia_hardware_goldfish::PipeDevice>> device_result =
      component::ConnectAt<fuchsia_hardware_goldfish::PipeDevice>(dir, name);
  if (device_result.is_error()) {
    FX_PLOGS(ERROR, device_result.error_value()) << "Failed to connect to the";
    return false;
  }

  device_.Bind(std::move(device_result).value(), app()->dispatcher(), this);

  auto data = node().CreateChild("0");
  std::string component_url = "fuchsia-pkg://fuchsia.com/libvulkan_goldfish#meta/vulkan.cm";
  data.RecordString("component_url", component_url);

  zx::result icd_component = app()->CreateIcdComponent(component_url);
  if (icd_component.is_error()) {
    FX_LOGS(ERROR) << "Failed to create ICD component: " << icd_component.status_string();
    return false;
  }

  icd_list_.Add(std::move(*icd_component));
  icds().push_back(std::move(data));
  return true;
}

void GoldfishDevice::on_fidl_error(fidl::UnbindInfo unbind_info) { app()->RemoveDevice(this); }

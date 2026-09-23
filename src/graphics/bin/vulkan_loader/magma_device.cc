// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/bin/vulkan_loader/magma_device.h"

#include <lib/component/incoming/cpp/service.h>
#include <lib/fit/thread_checker.h>
#include <lib/syslog/cpp/macros.h>

#include "src/graphics/bin/vulkan_loader/app.h"

// static
zx::result<std::unique_ptr<MagmaDevice>> MagmaDevice::Create(
    LoaderApp* app, const fidl::ClientEnd<fuchsia_io::Directory>& dir, const std::string& name,
    inspect::Node* parent) {
  std::unique_ptr<MagmaDevice> device(new MagmaDevice(app));
  if (zx::result status = device->Initialize(dir, name, parent); status.is_error()) {
    return status.take_error();
  }
  return zx::ok(std::move(device));
}

zx::result<> MagmaDevice::Initialize(const fidl::ClientEnd<fuchsia_io::Directory>& dir,
                                     const std::string& name, inspect::Node* parent) {
  FIT_DCHECK_IS_THREAD_VALID(main_thread_);
  node() = parent->CreateChild("magma-" + name);
  icd_list_.Initialize(&node());
  auto pending_action_token = app()->GetPendingActionToken();

  zx::result client_end = component::ConnectAt<fuchsia_gpu_magma::IcdLoaderDevice>(dir, name);
  if (client_end.is_error()) {
    FX_LOGS(ERROR) << "Failed to connect to ICD loader device: " << client_end.status_string();
    return client_end.take_error();
  }

  device_ = fidl::WireClient(std::move(*client_end), app()->dispatcher(), this);

  device_->GetIcdList().Then(
      [this, pending_action_token = std::move(pending_action_token)](
          fidl::WireUnownedResult<fuchsia_gpu_magma::IcdLoaderDevice::GetIcdList>& result) {
        FIT_DCHECK_IS_THREAD_VALID(main_thread_);
        if (!result.ok()) {
          FX_LOGS(ERROR) << "GetIcdList transport error: " << result.error();
          return;  // Device will be removed by `on_fidl_error`.
        }
        uint32_t i = 0;
        for (auto& icd : result->icd_list) {
          if (!icd.has_component_url()) {
            FX_LOGS(ERROR) << "ICD missing component URL";
            continue;
          }
          if (!icd.has_flags()) {
            FX_LOGS(ERROR) << "ICD missing flags";
            continue;
          }
          auto data = node().CreateChild(std::to_string(i++));
          data.RecordString("component_url", icd.component_url().data());
          data.RecordUint("flags", static_cast<uint32_t>(icd.flags()));
          if (icd.flags() & fuchsia_gpu_magma::wire::IcdFlags::kSupportsVulkan) {
            zx::result icd_component = app()->CreateIcdComponent(icd.component_url().data());
            if (icd_component.is_error()) {
              FX_LOGS(ERROR) << "Failed to create ICD component: " << icd_component.status_string();
              continue;
            }
            icd_list_.Add(std::move(*icd_component));
          }

          icds().push_back(std::move(data));
        }
      });

  return zx::ok();
}

void MagmaDevice::on_fidl_error(fidl::UnbindInfo unbind_info) { app()->RemoveDevice(this); }

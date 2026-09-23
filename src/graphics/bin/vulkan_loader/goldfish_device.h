// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_BIN_VULKAN_LOADER_GOLDFISH_DEVICE_H_
#define SRC_GRAPHICS_BIN_VULKAN_LOADER_GOLDFISH_DEVICE_H_

#include <fidl/fuchsia.hardware.goldfish/cpp/wire.h>
#include <lib/inspect/cpp/inspect.h>

#include <string>

#include "src/graphics/bin/vulkan_loader/gpu_device.h"
#include "src/graphics/bin/vulkan_loader/icd_list.h"
#include "src/lib/fxl/macros.h"

class LoaderApp;
class GoldfishDevice : public GpuDevice,
                       public fidl::WireAsyncEventHandler<fuchsia_hardware_goldfish::PipeDevice> {
 public:
  static std::unique_ptr<GoldfishDevice> Create(LoaderApp* app,
                                                const fidl::ClientEnd<fuchsia_io::Directory>& dir,
                                                const std::string& name, inspect::Node* parent);

  IcdList& icd_list() override { return icd_list_; }

  // `fidl::WireAsyncEventHandler<fuchsia_hardware_goldfish::PipeDevice>`:
  void on_fidl_error(::fidl::UnbindInfo error) override;

 private:
  explicit GoldfishDevice(LoaderApp* app) : GpuDevice(app) {}

  bool Initialize(const fidl::ClientEnd<fuchsia_io::Directory>& dir, const std::string& name,
                  inspect::Node* parent);

  IcdList icd_list_;
  fidl::WireClient<fuchsia_hardware_goldfish::PipeDevice> device_;

  FXL_DISALLOW_COPY_ASSIGN_AND_MOVE(GoldfishDevice);
};

#endif  // SRC_GRAPHICS_BIN_VULKAN_LOADER_GOLDFISH_DEVICE_H_

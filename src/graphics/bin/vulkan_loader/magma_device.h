// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_BIN_VULKAN_LOADER_MAGMA_DEVICE_H_
#define SRC_GRAPHICS_BIN_VULKAN_LOADER_MAGMA_DEVICE_H_

#include <fidl/fuchsia.gpu.magma/cpp/wire.h>
#include <lib/fit/thread_checker.h>
#include <lib/inspect/cpp/inspect.h>

#include <string>

#include "src/graphics/bin/vulkan_loader/gpu_device.h"
#include "src/graphics/bin/vulkan_loader/icd_list.h"
#include "src/lib/fxl/macros.h"

class LoaderApp;

// Represents a hardware GPU.
class MagmaDevice : public GpuDevice,
                    public fidl::WireAsyncEventHandler<fuchsia_gpu_magma::IcdLoaderDevice> {
 public:
  static zx::result<std::unique_ptr<MagmaDevice>> Create(
      LoaderApp* app, const fidl::ClientEnd<fuchsia_io::Directory>& dir, const std::string& name,
      inspect::Node* parent);

  IcdList& icd_list() override { return icd_list_; }

 private:
  void on_fidl_error(fidl::UnbindInfo unbind_info) override;

  explicit MagmaDevice(LoaderApp* app) : GpuDevice(app) {}

  zx::result<> Initialize(const fidl::ClientEnd<fuchsia_io::Directory>& dir,
                          const std::string& name, inspect::Node* parent);

  FIT_DECLARE_THREAD_CHECKER(main_thread_)
  IcdList icd_list_;
  fidl::WireClient<fuchsia_gpu_magma::IcdLoaderDevice> device_;

  FXL_DISALLOW_COPY_ASSIGN_AND_MOVE(MagmaDevice);
};

#endif  // SRC_GRAPHICS_BIN_VULKAN_LOADER_MAGMA_DEVICE_H_

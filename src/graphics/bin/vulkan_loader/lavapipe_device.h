// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_BIN_VULKAN_LOADER_LAVAPIPE_DEVICE_H_
#define SRC_GRAPHICS_BIN_VULKAN_LOADER_LAVAPIPE_DEVICE_H_

#include <lib/fit/thread_checker.h>

#include <memory>
#include <string>

#include "src/graphics/bin/vulkan_loader/gpu_device.h"
#include "src/graphics/bin/vulkan_loader/icd_list.h"
#include "src/lib/fxl/macros.h"

class LoaderApp;

// Connects to a well-known component to obtain an ICD which uses Lavapipe to provide a software
// Vulkan implementation.
class LavapipeDevice : public GpuDevice {
 public:
  static std::unique_ptr<LavapipeDevice> Create(LoaderApp* app, const std::string& name,
                                                inspect::Node* parent,
                                                const std::string& lavapipe_icd_url);

  IcdList& icd_list() override { return icd_list_; }

 private:
  explicit LavapipeDevice(LoaderApp* app) : GpuDevice(app) {}

  bool Initialize(const std::string& name, inspect::Node* parent,
                  const std::string& lavapipe_icd_url);

  FIT_DECLARE_THREAD_CHECKER(main_thread_)
  IcdList icd_list_;
  std::string lavapipe_icd_url_;

  FXL_DISALLOW_COPY_ASSIGN_AND_MOVE(LavapipeDevice);
};

#endif  // SRC_GRAPHICS_BIN_VULKAN_LOADER_LAVAPIPE_DEVICE_H_

// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_BIN_VULKAN_LOADER_GPU_DEVICE_H_
#define SRC_GRAPHICS_BIN_VULKAN_LOADER_GPU_DEVICE_H_

#include <lib/inspect/cpp/inspect.h>

#include <cstdint>
#include <vector>

class IcdList;
class LoaderApp;

// Represents a GPU (hardware or virtual).  It is responsible for generating a list of ICDs which
// can be used to drive the device.  More precisely, this list consists of connections to components
// which are each capable of returning a VMO containing an ICD shared library corresponding to the
// relevant device.
class GpuDevice {
 public:
  virtual ~GpuDevice() = default;

  virtual IcdList& icd_list() = 0;

  uint64_t icd_count() { return icds_.size(); }

 protected:
  explicit GpuDevice(LoaderApp* app) : app_(app) {}

  LoaderApp* app() { return app_; }
  inspect::Node& node() { return node_; }
  std::vector<inspect::Node>& icds() { return icds_; }

 private:
  LoaderApp* app_;
  inspect::Node node_;
  std::vector<inspect::Node> icds_;
};

#endif  // SRC_GRAPHICS_BIN_VULKAN_LOADER_GPU_DEVICE_H_

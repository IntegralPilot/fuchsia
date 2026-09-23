// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_BIN_SYSTEM_MONITOR_SYSTEM_MONITOR_RENDERER_H_
#define SRC_UI_BIN_SYSTEM_MONITOR_SYSTEM_MONITOR_RENDERER_H_

#include <fidl/fuchsia.element/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <fidl/fuchsia.ui.views/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>

#include <string>

#include "src/lib/fxl/memory/ref_counted.h"
#include "src/ui/lib/escher/debug/debug_font.h"
#include "src/ui/lib/escher/escher.h"
#include "src/ui/lib/escher/fs/hack_filesystem.h"
#include "src/ui/lib/escher/vk/vulkan_device_queues.h"
#include "src/ui/lib/escher/vk/vulkan_instance.h"
#include "src/ui/lib/escher/vk/vulkan_swapchain.h"
#include "src/ui/lib/escher/vk/vulkan_swapchain_helper.h"

namespace system_monitor {

// This class uses escher to render a frame containing CPU data
class SystemMonitorRenderer {
 public:
  void Initialize();
  void RenderFrame(std::string cpu_string);

 private:
  fxl::RefPtr<escher::VulkanInstance> instance_;
  vk::SurfaceKHR surface_;
  fxl::RefPtr<escher::VulkanDeviceQueues> device_;
  escher::HackFilesystemPtr filesystem_;
  std::unique_ptr<escher::Escher> escher_;
  escher::VulkanSwapchain swapchain_;
  std::unique_ptr<escher::VulkanSwapchainHelper> swapchain_helper_;
  std::unique_ptr<escher::DebugFont> debug_font_;
  fidl::SyncClient<fuchsia_element::GraphicalPresenter> presenter_client_;
};
}  // namespace system_monitor

#endif  // SRC_UI_BIN_SYSTEM_MONITOR_SYSTEM_MONITOR_RENDERER_H_

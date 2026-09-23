// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_VULKAN_SWAPCHAIN_IMAGE_PIPE_SURFACE_DISPLAY_H_
#define SRC_LIB_VULKAN_SWAPCHAIN_IMAGE_PIPE_SURFACE_DISPLAY_H_

#include <fidl/fuchsia.hardware.display.types/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <zircon/compiler.h>

#include <atomic>
#include <memory>
#include <mutex>
#include <queue>
#include <unordered_set>

#include "src/lib/vulkan/swapchain/display_coordinator_listener.h"
#include "src/lib/vulkan/swapchain/image_pipe_surface.h"
#include "vulkan/vulkan_core.h"

namespace image_pipe_swapchain {

// An implementation of ImagePipeSurface based on the display and sysmem APIs.
class ImagePipeSurfaceDisplay final
    : public ImagePipeSurface,
      public fidl::AsyncEventHandler<fuchsia_hardware_display::Coordinator> {
 public:
  ImagePipeSurfaceDisplay();

  ~ImagePipeSurfaceDisplay() override;

  bool Init() override;

  bool CreateImage(VkDevice device, VkLayerDispatchTable* pDisp, VkFormat format,
                   VkImageUsageFlags usage, VkSwapchainCreateFlagsKHR swapchain_flags,
                   VkExtent2D extent, uint32_t image_count, VkCompositeAlphaFlagBitsKHR alpha_flags,
                   const VkAllocationCallbacks* pAllocator,
                   std::vector<ImageInfo>* image_info_out) override;

  bool CanPresentPendingImage() override { return false; }

  bool GetSize(uint32_t* width_out, uint32_t* height_out) override;

  void RemoveImage(uint32_t image_id) override;

  void PresentImage(bool immediate, uint32_t image_id,
                    std::vector<std::unique_ptr<PlatformEvent>> acquire_fences,
                    std::vector<std::unique_ptr<PlatformEvent>> release_fences,
                    VkQueue queue) override;

  SupportedImageProperties& GetSupportedImageProperties() override;

  // fidl::AsyncEventHandler<fuchsia_hardware_display::Coordinator>
  void on_fidl_error(fidl::UnbindInfo error) override;

 private:
  // `release_fence` will be signaled when a vsync is received with a larger/later config stamp.
  struct ReleaseFenceEntry {
    fuchsia_hardware_display::ConfigStamp config_stamp;
    zx::event release_fence;
  };

  void ControllerOnDisplaysChanged(std::vector<fuchsia_hardware_display::Info>,
                                   std::vector<fuchsia_hardware_display_types::DisplayId>);
  void ControllerOnVsync(fuchsia_hardware_display_types::DisplayId, zx::time timestamp,
                         fuchsia_hardware_display::ConfigStamp displayed_config_stamp,
                         fuchsia_hardware_display::VsyncAckCookie cookie);

  // TODO(https://fxbug.dev/377322342): consider making `display_coordinator_` a sync client,
  // so that it isn't necessary to call `WaitForAsyncMessage()`.
  bool WaitForAsyncMessage();

  fuchsia_hardware_display::ConfigStamp NextConfigStamp() {
    return fuchsia_hardware_display::ConfigStamp(++last_displayed_config_stamp_);
  }

  // This loop is manually pumped in method calls and doesn't have its own
  // thread.
  async::Loop client_loop_;
  async::Loop listener_loop_;

  std::unordered_set<uint64_t> image_ids;

  bool display_connection_exited_ = false;
  bool got_message_response_ = false;
  std::atomic_bool have_display_ = false;
  uint32_t width_ = 0;
  uint32_t height_ = 0;

  // ID of the first display returned, whenever the displays are updated.
  fuchsia_hardware_display_types::DisplayId display_id_ = {
      fuchsia_hardware_display_types::kInvalidDispId};
  // The maximum number of layers supported by the display device specified by `display_id_`.
  uint32_t max_layer_count_ = 0;

  // Initialized in `CreateImage()`; doesn't change.
  fuchsia_hardware_display::LayerId layer_id_{fuchsia_hardware_display_types::kInvalidDispId};

  std::atomic_uint64_t last_displayed_config_stamp_ =
      fuchsia_hardware_display::kInvalidConfigStampValue;
  uint64_t next_layer_id_ = 1;
  std::queue<ReleaseFenceEntry> pending_release_fences_ __TA_GUARDED(mutex_);

  fidl::SharedClient<fuchsia_hardware_display::Coordinator> display_coordinator_;
  std::unique_ptr<DisplayCoordinatorListener> display_coordinator_listener_;
  fidl::SyncClient<fuchsia_sysmem2::Allocator> sysmem_allocator_;
  SupportedImageProperties supported_image_properties_;

  std::mutex mutex_;
};

}  // namespace image_pipe_swapchain

#endif  // SRC_LIB_VULKAN_SWAPCHAIN_IMAGE_PIPE_SURFACE_DISPLAY_H_

// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_FLATLAND_ENGINE_DISPLAY_COMPOSITOR_H_
#define SRC_UI_SCENIC_LIB_FLATLAND_ENGINE_DISPLAY_COMPOSITOR_H_

#include <fidl/fuchsia.hardware.display.types/cpp/fidl.h>
#include <fidl/fuchsia.hardware.display/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <lib/async/dispatcher.h>
#include <lib/zx/time.h>

#include <cstdint>
#include <deque>
#include <memory>
#include <optional>
#include <unordered_map>

#include "src/lib/fxl/synchronization/thread_annotations.h"
#include "src/ui/scenic/lib/allocation/buffer_collection_importer.h"
#include "src/ui/scenic/lib/display/coordinator_proxy.h"
#include "src/ui/scenic/lib/display/display.h"
#include "src/ui/scenic/lib/display/fidl_id_types.h"
#include "src/ui/scenic/lib/flatland/engine/color_conversion_state_machine.h"
#include "src/ui/scenic/lib/flatland/engine/engine_types.h"
#include "src/ui/scenic/lib/flatland/engine/release_fence_manager.h"
#include "src/ui/scenic/lib/flatland/renderer/renderer.h"

namespace flatland {

namespace test {
class DisplayCompositorSmokeTest;
class DisplayCompositorPixelTest;
class DisplayCompositorTest;
}  // namespace test

using allocation::BufferCollectionUsage;

// Provides overridable default values for constructing a DisplayCompositor.
struct DisplayCompositorConfig {
  // Whether to attempt display composition at all. If false we always fall back to GPU-compositing.
  bool enable_direct_to_display = true;

  // If true, all images will be tinted when we fall back to GPU-compositing.
  bool tint_gpu_fallback_images = false;

  // If true, the current frame number will be displayed above all other images.
  bool enable_frame_counter_overlay = false;
};

// The DisplayCompositor is responsible for compositing Flatland render data onto the display(s).
// It accomplishes this either by direct hardware compositing via the display coordinator
// interface, or rendering on the GPU via a custom renderer API. It also handles the
// registration of sysmem buffer collections and importation of images to both the
// display coordinator and the renderer via the BufferCollectionImporter interface. The
// BufferCollectionImporter interface is how Flatland instances communicate with the
// DisplayCompositor, providing it with the necessary data to render without exposing to Flatland
// the DisplayCoordinator or other dependencies.
class DisplayCompositor final : public allocation::BufferCollectionImporter,
                                public std::enable_shared_from_this<DisplayCompositor> {
 public:
  // Describes the result of RenderFrame().  If it succeeds it is either by showing client images
  // directly on the display, or by first using the GPU to composite them into a single image.
  enum class RenderFrameResult { kDirectToDisplay, kGpuComposition, kFailure };
  // Args which can be passed to customize RenderFrame() behavior in tests.  The default values are
  // the ones used in production.
  struct RenderFrameTestArgs {
    bool force_gpu_composition = false;

    // This is a workaround so that RenderFrame() can provide a default value, while still allowing
    // callers to use aggregate initialization syntax.  Adding a default constructor would sacrifice
    // this ability.  See:
    // https://stackoverflow.com/questions/53408962/try-to-understand-compiler-error-message-default-member-initializer-required-be
    static RenderFrameTestArgs Default() { return {}; }
  };

  // TODO(https://fxbug.dev/42145655): The DisplayCompositor has multiple parts of its code where
  // usage of the display coordinator is protected by locks, because of the multithreaded
  // environment of flatland. Ideally, we'd want the DisplayCompositor to have sole ownership of the
  // display coordinator - meaning that it would require a unique_ptr instead of a shared_ptr. But
  // since access to the real display coordinator is provided to clients via a shared_ptr, we take
  // in a shared_ptr as a parameter here. However, this could cause problems with our locking
  // mechanisms, as other display-coordinator clients could be accessing the same functions and/or
  // state at the same time as the DisplayCompositor without making use of locks.
  DisplayCompositor(async_dispatcher_t* main_dispatcher,
                    std::shared_ptr<display::CoordinatorProxy> coordinator_proxy,
                    const std::shared_ptr<Renderer>& renderer,
                    fidl::WireClient<fuchsia_sysmem2::Allocator> sysmem_allocator,
                    const DisplayCompositorConfig& config);

  ~DisplayCompositor() override;

  // |BufferCollectionImporter|
  // Only called from the main thread.
  fpromise::promise<> ImportBufferCollection(
      allocation::GlobalBufferCollectionId collection_id,
      fidl::WireClient<fuchsia_sysmem2::Allocator>& sysmem_allocator,
      fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token, BufferCollectionUsage usage,
      std::optional<fuchsia_math::SizeU> size) override FXL_LOCKS_EXCLUDED(lock_);

  // |BufferCollectionImporter|
  // Only called from the main thread.
  void ReleaseBufferCollection(allocation::GlobalBufferCollectionId collection_id,
                               BufferCollectionUsage usage_type) override FXL_LOCKS_EXCLUDED(lock_);

  // |BufferCollectionImporter|
  // Called from main thread or Flatland threads.
  fpromise::promise<> ImportBufferImage(const allocation::ImageMetadata& metadata,
                                        BufferCollectionUsage usage_type) override
      FXL_LOCKS_EXCLUDED(lock_);

  // |BufferCollectionImporter|
  // Called from main thread or Flatland threads.
  void ReleaseBufferImage(allocation::GlobalImageId image_id) override FXL_LOCKS_EXCLUDED(lock_);

  // Generates frame and presents it to display.  This may involve directly scanning out client
  // images, or it may involve first using the GPU to composite (some of) these images into a single
  // image which is then scanned out.
  //
  // |args| can be used to customize behavior in tests.  Production code should omit this arg; the
  // default values are correct for production use cases.
  //
  // Only called from the main thread.
  RenderFrameResult RenderFrame(
      uint64_t frame_number, zx::time presentation_time,
      std::span<const RenderData> render_data_list, std::vector<zx::event> release_fences,
      std::vector<zx::counter> release_counters, std::vector<zx::counter> present_fences,
      scheduling::FramePresentedCallback callback,
      // Allows customization of behavior for tests.  Default values are used in production.
      RenderFrameTestArgs test_args = RenderFrameTestArgs::Default()) FXL_LOCKS_EXCLUDED(lock_);

  // Register a new display to the DisplayCompositor, which also generates the render targets to be
  // presented on the display when compositing on the GPU. If |num_render_targets| is 0, this
  // function will not create any render targets for GPU composition for that display. The buffer
  // collection info is also returned back to the caller via an output parameter
  // |num_render_targets| is 0. Otherwise, a valid handle to return the buffer collection data is
  // required.
  // TODO(https://fxbug.dev/42137737): We need to figure out exactly how we want the display to
  // anchor to the Flatland hierarchy. Only called from the main thread.
  fpromise::promise<> AddDisplay(
      display::Display* display, DisplayInfo info, uint32_t num_render_targets,
      fuchsia_sysmem2::BufferCollectionInfo* out_collection_info = nullptr)
      FXL_LOCKS_EXCLUDED(lock_);

  // Values needed to adjust the color of the framebuffer as a postprocessing effect.
  // Only called from the main thread.
  void SetColorConversionValues(const fidl::Array<float, 9>& coefficients,
                                const fidl::Array<float, 3>& preoffsets,
                                const fidl::Array<float, 3>& postoffsets);

  // Clamps the minimum value for all channels on all pixels on the display to this number.
  // Only called from the main thread.
  bool SetMinimumRgb(uint8_t minimum_rgb) FXL_LOCKS_EXCLUDED(lock_);

  display::CoordinatorProxy* GetDisplayCoordinatorForTest() { return &display_coordinator_; }

 private:
  friend class test::DisplayCompositorSmokeTest;
  friend class test::DisplayCompositorPixelTest;
  friend class test::DisplayCompositorTest;

  struct DisplayConfigResponse {
    // Whether or not the config can be successfully applied or not.
    display::WireConfigResult result;
  };

  struct FrameEventData {
    display::EventId wait_id;
    zx::event wait_event;
  };

  struct DisplayEngineData {
    // The maximum number of hardware layers supported by this display.
    uint32_t max_layer_count = 0;

    // The layer used to render an empty scene to the display through a solid black color.
    display::LayerId empty_scene_layer;

    // The hardware layers we've created to use on this display.
    std::vector<display::LayerId> layers;

    // The number of vmos we are using in the case of software composition
    // (1 for each render target).
    uint32_t vmo_count = 0;

    // The current target that is being rendered to by the software renderer.
    uint32_t curr_vmo = 0;

    // The information used to create images for each render target from the vmo data.
    std::vector<allocation::ImageMetadata> render_targets;

    // The information used to create images for each render target from the vmo data.
    std::vector<allocation::ImageMetadata> protected_render_targets;

    // Used to synchronize buffer rendering with setting the buffer on the display.
    std::vector<FrameEventData> frame_event_datas;

    // Keeps track of display mode that needs to be set before next `ApplyConfig()`.
    std::optional<display::WireDisplayMode> updated_display_mode;
  };

  // Notifies the compositor that a vsync has occurred, in response to a display configuration
  // applied by the compositor.  It is the compositor's responsibility to signal any release fences
  // corresponding to the frame identified by |frame_number|.
  void OnVsync(zx::time_monotonic timestamp, display::WireConfigStamp displayed_config_stamp);

  fpromise::promise<std::vector<allocation::ImageMetadata>> AllocateDisplayRenderTargets(
      bool use_protected_memory, uint32_t num_render_targets, const fuchsia_math::SizeU& size,
      fuchsia_images2::PixelFormat pixel_format,
      fuchsia_sysmem2::BufferCollectionInfo* out_collection_info = nullptr)
      FXL_LOCKS_EXCLUDED(lock_);

  // Generates a new FrameEventData struct to be used with a render target on a display.
  FrameEventData NewFrameEventData() FXL_EXCLUSIVE_LOCKS_REQUIRED(lock_);

  // Used when we're forced to fall back to GPU rendering.
  bool PerformGpuComposition(
      uint64_t frame_number, uint64_t trace_flow_id, zx::time presentation_time,
      std::span<const RenderData> render_data_list, std::vector<zx::event> release_fences,
      std::vector<zx::counter> release_counters, std::vector<zx::counter> present_fences,
      scheduling::FramePresentedCallback callback) FXL_EXCLUSIVE_LOCKS_REQUIRED(lock_);

  // Does all the setup for applying the render data, which includes images and rectangles,
  // onto the display via the display coordinator interface. Returns false if this cannot
  // be completed.
  bool SetRenderDataOnDisplay(const RenderData& data) FXL_EXCLUSIVE_LOCKS_REQUIRED(lock_);

  // Calls SetRenderData for each item in |render_data_list| and applies direct-to-display color
  // conversion. Return false if this fails for any RenderData.
  bool TryDirectToDisplay(std::span<const RenderData> render_data_list, uint64_t frame_number,
                          uint64_t trace_flow_id) FXL_EXCLUSIVE_LOCKS_REQUIRED(lock_);

  // Sets the provided layers onto the display referenced by the given display_id.
  void SetDisplayLayers(display::DisplayId display_id, const std::span<display::LayerId>& layers)
      FXL_EXCLUSIVE_LOCKS_REQUIRED(lock_);

  // Takes a solid color rectangle and directly composites it to a hardware layer on the display.
  void ApplyLayerColor(const display::LayerId& layer_id, const SrcToDest& geometry,
                       const std::array<float, 4>& color, const types::BlendMode& blend_mode)
      FXL_EXCLUSIVE_LOCKS_REQUIRED(lock_);

  // Takes a ResolvedLayer and directly composites it to a hardware layer on the display.
  void ApplyLayerImage(const display::LayerId& layer_id, const ResolvedLayer& layer,
                       const display::EventId& wait_id) FXL_EXCLUSIVE_LOCKS_REQUIRED(lock_);

  // Applies the config to the display coordinator and record the corresponding ConfigStamp, so that
  // we can observe Vsync events to know when this config was actually displayed.
  //
  // This should only be called after CheckConfig() has verified that the config is okay, since
  // ApplyConfig does not return any errors.
  zx::result<> ApplyConfig(uint64_t frame_number, uint64_t trace_flow_id)
      FXL_EXCLUSIVE_LOCKS_REQUIRED(lock_);

  bool ImportBufferCollectionToDisplayCoordinator(
      allocation::GlobalBufferCollectionId identifier,
      fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token,
      const fuchsia_hardware_display_types::wire::ImageBufferUsage& image_buffer_usage)
      FXL_EXCLUSIVE_LOCKS_REQUIRED(lock_);

  // Works around inconvenient `fuchsia.hardware.display.Coordinator` APIs.  We can't set the
  // display mode immediately when notified of a new display, because the API doesn't allow a
  // display config with no layers.  So we stash it and apply it the next time we have a config
  // to apply.
  bool MaybeSetPendingDisplayMode(const display::DisplayId& display_id)
      FXL_EXCLUSIVE_LOCKS_REQUIRED(lock_);
  void ClearAllPendingDisplayModes(std::span<const RenderData> render_data_list)
      FXL_EXCLUSIVE_LOCKS_REQUIRED(lock_);

  // This mutex protects access to class members that are accessed on main thread and the Flatland
  // threads. All the methods of this class are run of |main_dispatcher_| except for
  // ImportBufferImage() and ReleaseBufferImage(), where the shared data structures are guarded by
  // this.
  //
  // TODO(https://fxbug.dev/42120738): Convert this to a lock-free structure. This is a unique
  // case since we are talking to a FIDL interface (display_coordinator_) through a lock.
  // We either need lock-free threadsafe FIDL bindings, multiple channels to the display
  // coordinator, or something else.
  mutable std::mutex lock_;

  // References the coordinator to keep it alive. Don't use; instead use `display_coordinator_`.
  std::shared_ptr<display::CoordinatorProxy> display_coordinator_shared_ptr_ FXL_GUARDED_BY(lock_);

  // Thin proxy to optimize communication with `fuchsia.hardware.display/Coordinator`.
  display::CoordinatorProxy& display_coordinator_ FXL_GUARDED_BY(lock_);

  // Maps a buffer collection ID to a BufferCollection in the same domain as the token with
  // display constraints set. This is used as a bridge between ImportBufferCollection() and
  // ImportBufferImage() calls, so that we can check if the existing allocation is
  // display-compatible.
  std::unordered_map<allocation::GlobalBufferCollectionId,
                     fidl::SyncClient<fuchsia_sysmem2::BufferCollection>>
      display_buffer_collection_ptrs_ FXL_GUARDED_BY(lock_);

  // Maps a buffer collection ID to a boolean indicating if it can be imported into display.
  std::unordered_map<allocation::GlobalBufferCollectionId, bool> buffer_collection_supports_display_
      FXL_GUARDED_BY(lock_);

  // Maps an image ID to its tiling type.
  std::unordered_map<allocation::GlobalImageId, uint32_t> image_tiling_type_map_
      FXL_GUARDED_BY(lock_);

  // Maps a buffer collection ID to a collection tiling type.
  // TODO(https://fxbug.dev/42150686): Delete after we don't need the tiling type anymore.
  // TODO(https://fxbug.dev/406066267): We never clear values added to this map.  Until we can
  // delete this, we might want to add them to a separate map scoped to individual images, rather
  // than to the buffer collection.
  std::unordered_map<allocation::GlobalBufferCollectionId, uint32_t>
      buffer_collection_tiling_type_map_ FXL_GUARDED_BY(lock_);

  /// The below members are either thread-safe or only manipulated from the main thread and
  /// therefore don't need locks.

  // Software renderer used when render data cannot be directly composited to the display.
  const std::shared_ptr<Renderer> renderer_;

  // Maps a display ID to a struct of all the information needed to properly render to
  // that display in both the hardware and software composition paths.
  std::unordered_map<display::DisplayId, DisplayEngineData> display_engine_data_map_;
  ReleaseFenceManager release_fence_manager_;

  // Stores information about the last ApplyConfig() call to display.
  struct ApplyConfigInfo {
    display::WireConfigStamp config_stamp;
    uint64_t frame_number;
    uint64_t trace_flow_id;
  };

  // The next ConfigStamp value used in an ApplyConfig() call.
  display::WireConfigStamp next_config_stamp_{1};

  // A queue storing all display frame configurations that are applied but not yet shown on the
  // display device.
  std::deque<ApplyConfigInfo> pending_apply_configs_;

  // The last frame number called in RenderFrame(), this number is use assert the frame number is
  // strictly increasing.
  std::optional<uint64_t> last_frame_number_;

  // Stores the ConfigStamp information of the latest frame shown on the display. If no frame
  // has been presented, its value will be nullopt.
  std::optional<display::WireConfigStamp> last_presented_config_stamp_ = std::nullopt;

  fidl::WireClient<fuchsia_sysmem2::Allocator> sysmem_allocator_;

  ColorConversionStateMachine cc_state_machine_;

  const async_dispatcher_t* const main_dispatcher_;

  const DisplayCompositorConfig config_;

  inspect::Node inspect_node_;
};

}  // namespace flatland

#endif  // SRC_UI_SCENIC_LIB_FLATLAND_ENGINE_DISPLAY_COMPOSITOR_H_

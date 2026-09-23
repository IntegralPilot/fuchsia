// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_BIN_APP_H_
#define SRC_UI_SCENIC_BIN_APP_H_

#include <fidl/fuchsia.io/cpp/fidl.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/async/cpp/executor.h>
#include <lib/async_patterns/cpp/dispatcher_bound.h>

#include <memory>
#include <optional>

#include "src/graphics/display/lib/coordinator-getter/client.h"
#include "src/lib/fsl/io/device_watcher.h"
#include "src/ui/lib/escher/escher.h"
#include "src/ui/scenic/bin/health_inspector.h"
#include "src/ui/scenic/lib/allocation/allocator.h"
#include "src/ui/scenic/lib/display/color_converter.h"
#include "src/ui/scenic/lib/display/display_manager.h"
#include "src/ui/scenic/lib/display/display_power_manager.h"
#include "src/ui/scenic/lib/display/singleton_display_service.h"
#include "src/ui/scenic/lib/display/vsync_source_manager.h"
#include "src/ui/scenic/lib/flatland/engine/display_compositor.h"
#include "src/ui/scenic/lib/flatland/flatland_factory.h"
#include "src/ui/scenic/lib/flatland/flatland_manager.h"
#include "src/ui/scenic/lib/flatland/flatland_presenter_impl.h"
#include "src/ui/scenic/lib/flatland/link_system.h"
#include "src/ui/scenic/lib/flatland/trusted_flatland_factory.h"
#include "src/ui/scenic/lib/flatland/uber_struct_system.h"
#include "src/ui/scenic/lib/focus/focus_manager.h"
#include "src/ui/scenic/lib/input/input_manager.h"
#include "src/ui/scenic/lib/input/input_system.h"
#include "src/ui/scenic/lib/scheduling/default_frame_scheduler.h"
#include "src/ui/scenic/lib/screen_capture/screen_capture_manager.h"
#include "src/ui/scenic/lib/screen_capture2/screen_capture2_manager.h"
#include "src/ui/scenic/lib/screenshot/screenshot_manager.h"
#include "src/ui/scenic/lib/shutdown/shutdown_manager.h"
#include "src/ui/scenic/lib/utils/metrics_impl.h"
#include "src/ui/scenic/lib/view_tree/geometry_provider.h"
#include "src/ui/scenic/lib/view_tree/observer_registry.h"
#include "src/ui/scenic/lib/view_tree/scoped_observer_registry.h"
#include "src/ui/scenic/lib/view_tree/view_ref_installed_impl.h"
#include "src/ui/scenic/lib/view_tree/view_tree_snapshotter.h"
#include "src/ui/scenic/scenic_structured_config.h"

namespace scenic_impl {

class DisplayInfoDelegate {
 public:
  explicit DisplayInfoDelegate(std::shared_ptr<display::Display> display);

  fuchsia_math::SizeU GetDisplayDimensions();

 private:
  std::shared_ptr<display::Display> display_ = nullptr;
};

// What type of renderer is used by Scenic.
// LINT.IfChange
enum class RendererType : uint8_t {
  // Send all rendering operations to void.
  NULL_RENDERER,
  // Use Vulkan for rendering.
  VULKAN,
};
// LINT.ThenChange(//src/lib/assembly/config_schema/src/platform_settings/ui_config.rs)

// `App` instantiates and stitches together all of the subsystems which comprise the Scenic
// component.
//
// Lifetime requirement: the owner must not destroy `App` while work can still run on either
// `flatland_dispatcher` or `input_dispatcher`.  `App` hands closures capturing `this` to FIDL
// service handlers, the frame scheduler, display callbacks, and posted tasks, with no weak
// pointers.  The destructor cancels none of these; nothing inside `App` enforces the requirement.
class App {
 public:
  App(async_dispatcher_t* flatland_dispatcher, async_dispatcher_t* input_dispatcher,
      std::unique_ptr<sys::ComponentContext> app_context,
      fidl::ClientEnd<fuchsia_io::Directory> pkg_dir,
      fidl::ServerEnd<fuchsia_io::Directory> out_dir, scenic_structured_config::Config config,
      inspect::Node& root_node,
      fpromise::promise<::display::CoordinatorClientChannels, zx_status_t> dc_handles_promise,
      fit::closure quit_callback);

  ~App();

 private:
  void InitializeServices(escher::EscherUniquePtr escher,
                          std::shared_ptr<display::Display> display);
  void InitializeGraphics(std::shared_ptr<display::Display> display);

  void InitializeHeartbeat(display::Display& display);

  void InitializeInput();

  async::Executor executor_;
  async_dispatcher_t* const flatland_dispatcher_;
  async_dispatcher_t* const input_dispatcher_;
  std::unique_ptr<sys::ComponentContext> app_context_;
  fidl::SyncClient<fuchsia_io::Directory> pkg_dir_;
  const scenic_structured_config::Config config_values_;

  std::shared_ptr<ShutdownManager> shutdown_manager_;
  metrics::MetricsImpl metrics_logger_;
  inspect::Node inspect_node_;

  // FrameScheduler must be initialized early, since it must outlive all its
  // dependencies.
  scheduling::DefaultFrameScheduler frame_scheduler_;

  RendererType renderer_type_;
  std::optional<display::DisplayManager> display_manager_;
  std::optional<display::SingletonDisplayService> singleton_display_service_;
  std::optional<DisplayInfoDelegate> display_info_delegate_;
  // DisplayPowerManager has a reference to |display_manager_|, so it should be
  // destroyed before |display_manager_|.
  std::optional<display::DisplayPowerManager> display_power_manager_;
  // VsyncSourceManager has a reference to |display_manager_|, so it should be
  // destroyed before |display_manager_|.
  std::optional<display::VsyncSourceManager> vsync_source_manager_;
  escher::EscherUniquePtr escher_;

  std::unique_ptr<fsl::DeviceWatcher> device_watcher_;

  std::shared_ptr<allocation::Allocator> allocator_;

  std::shared_ptr<flatland::UberStructSystem> uber_struct_system_;
  std::shared_ptr<flatland::LinkSystem> link_system_;
  std::shared_ptr<flatland::FlatlandPresenterImpl> flatland_presenter_;
  std::shared_ptr<flatland::FlatlandManager> flatland_manager_;
  std::unique_ptr<flatland::FlatlandFactoryImpl> flatland_factory_;
  std::unique_ptr<flatland::TrustedFlatlandFactoryImpl> trusted_flatland_factory_;
  std::shared_ptr<flatland::DisplayCompositor> flatland_compositor_;
  std::shared_ptr<flatland::Engine> flatland_engine_;

  display::ColorConverter color_converter_;

  std::optional<view_tree::ViewTreeSnapshotter> view_tree_snapshotter_;
  std::optional<screen_capture::ScreenCaptureManager> screen_capture_manager_;
  std::optional<screen_capture2::ScreenCapture2Manager> screen_capture2_manager_;
  std::optional<screenshot::ScreenshotManager> screenshot_manager_;

  std::shared_ptr<view_tree::SnapshotHolder> snapshot_holder_;
  async_patterns::DispatcherBound<input::InputManager> input_manager_;

  uint64_t flatland_frame_count_ = 0;
  uint64_t skipped_frame_count_ = 0;

  const bool enable_snapshot_dump_ = false;

  // Must be last to ensure it is destroyed before the members it references
  // (e.g., display_manager_, display_power_manager_).
  HealthInspector health_inspector_;
};

void PrefetchBinary(zx_handle_t pkg_dir, const char* binary_path);

}  // namespace scenic_impl

#endif  // SRC_UI_SCENIC_BIN_APP_H_

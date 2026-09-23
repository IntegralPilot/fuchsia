// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/bin/app.h"

#include <fidl/fuchsia.hardware.display/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <fidl/fuchsia.ui.display.singleton/cpp/fidl.h>
#include <fidl/fuchsia.ui.input.accessibility/cpp/fidl.h>
#include <fidl/fuchsia.ui.pointer.augment/cpp/fidl.h>
#include <fidl/fuchsia.ui.pointer/cpp/fidl.h>
#include <fidl/fuchsia.ui.views/cpp/fidl.h>
#include <fidl/fuchsia.vulkan.loader/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/async/default.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/directory.h>
#include <lib/syslog/cpp/macros.h>

#include <cstdint>
#include <memory>
#include <optional>

#include "src/graphics/display/lib/coordinator-getter/client.h"
#include "src/lib/fxl/functional/cancelable_callback.h"
#include "src/ui/lib/escher/vk/pipeline_builder.h"
#include "src/ui/scenic/lib/display/color_converter.h"
#include "src/ui/scenic/lib/display/display_manager.h"
#include "src/ui/scenic/lib/display/display_power_manager.h"
#include "src/ui/scenic/lib/display/fidl_typedefs.h"
#include "src/ui/scenic/lib/flatland/engine/engine.h"
#include "src/ui/scenic/lib/flatland/renderer/null_renderer.h"
#include "src/ui/scenic/lib/flatland/renderer/vk_renderer.h"
#include "src/ui/scenic/lib/scheduling/frame_metrics_registry.cb.h"
#include "src/ui/scenic/lib/scheduling/windowed_frame_predictor.h"
#include "src/ui/scenic/lib/screen_capture/screen_capture_buffer_collection_importer.h"
#include "src/ui/scenic/lib/screen_capture/screen_capture_manager.h"
#include "src/ui/scenic/lib/screenshot/screenshot_manager.h"
#include "src/ui/scenic/lib/utils/escher_provider.h"
#include "src/ui/scenic/lib/utils/helpers.h"
#include "src/ui/scenic/lib/utils/metrics_impl.h"
#include "src/ui/scenic/lib/utils/range_inclusive.h"
#include "src/ui/scenic/lib/view_tree/snapshot_dump.h"
#include "src/ui/scenic/scenic_structured_config.h"

namespace {

using scenic_impl::RendererType;

constexpr zx::duration kShutdownTimeout = zx::sec(1);

// After every Flatland frame is sent to the display, we kick off a task for Escher to clean up
// unused Vulkan resources such as command buffers, which repeats with the specified interval until
// all resources are cleaned up.
constexpr zx::duration kEscherCleanupRetryInterval{10'000'000};  // 10 millisecond

// See "Config for Fuchsia Visual Debugging": go/config-fuchsia-visual-debugging
constexpr uint8_t VISUAL_DEBUGGING_LEVEL_INFO = 2;
constexpr uint8_t VISUAL_DEBUGGING_LEVEL_INFO_PLUS = 3;

std::optional<display::WireDisplayId> GetDisplayId(const scenic_structured_config::Config& values) {
  if (values.i_can_haz_display_id() < 0) {
    return std::nullopt;
  }
  return std::make_optional<display::WireDisplayId>({
      .value = static_cast<uint64_t>(values.i_can_haz_display_id()),
  });
}

std::optional<uint64_t> GetDisplayMode(const scenic_structured_config::Config& values) {
  if (values.i_can_haz_display_mode() < 0) {
    return std::nullopt;
  }
  return values.i_can_haz_display_mode();
}

utils::RangeInclusive<int> CreateRangeFromStructuredConfigValues(int left, int right) {
  if (left >= 0 && right >= 0) {
    ZX_DEBUG_ASSERT(left <= right);
    return utils::RangeInclusive<int>(left, right);
  }
  if (left >= 0) {
    return utils::RangeInclusive<int>(left, utils::PositiveInfinity{});
  }
  if (right >= 0) {
    return utils::RangeInclusive<int>(utils::NegativeInfinity{}, right);
  }
  return utils::RangeInclusive<int>();
}

display::DisplayModeConstraints GetDisplayModeConstraints(
    const scenic_structured_config::Config& values) {
  return {
      .width_px_range =
          CreateRangeFromStructuredConfigValues(values.min_display_horizontal_resolution_px(),
                                                values.max_display_horizontal_resolution_px()),
      .height_px_range = CreateRangeFromStructuredConfigValues(
          values.min_display_vertical_resolution_px(), values.max_display_vertical_resolution_px()),
      .refresh_rate_millihertz_range =
          CreateRangeFromStructuredConfigValues(values.min_display_refresh_rate_millihertz(),
                                                values.max_display_refresh_rate_millihertz()),
  };
}

std::string ToString(RendererType type) {
  switch (type) {
    case RendererType::NULL_RENDERER:
      return "null";
    case RendererType::VULKAN:
      return "vulkan";
  }
}

RendererType GetRendererType(const scenic_structured_config::Config& values) {
  if (ToString(RendererType::NULL_RENDERER) == values.renderer())
    return RendererType::NULL_RENDERER;
  if (ToString(RendererType::VULKAN) == values.renderer())
    return RendererType::VULKAN;
  FX_LOGS(WARNING) << "Unknown renderer type: " << values.renderer() << ". Falling back to vulkan";
  return RendererType::VULKAN;
}

uint64_t GetDisplayRotation(scenic_structured_config::Config values) {
  uint64_t rotation = values.display_rotation();
  if (rotation >= 0) {
    FX_CHECK(rotation < 360) << "Rotation should be less than 360 degrees.";
    return rotation;
  }
  FX_LOGS(WARNING) << "Invalid value for display_rotation. Falling back to the default value 0.";
  return 0;
}

display::CoordinatorProxy::CheckConfigHeuristics GetCheckConfigHeuristics(
    const scenic_structured_config::Config& values) {
  return display::CoordinatorProxy::CheckConfigHeuristics{
      .enable_heuristics = values.display_composition_enable_heuristics(),
  };
}

// Logs Scenic's structured config values.
void LogConfig(const scenic_structured_config::Config& values) {
  FX_LOGS(INFO) << "Scenic renderer: " << ToString(GetRendererType(values))
                << " min_predicted_frame_duration(us): "
                << values.frame_scheduler_min_predicted_frame_duration_in_us()
                << " frame_prediction_margin(us): " << values.frame_prediction_margin_in_us()
                << " pointer auto focus: " << values.pointer_auto_focus()
                << " display_composition: " << values.display_composition()
                << " display_composition_enable_heuristics: "
                << values.display_composition_enable_heuristics() << " i_can_haz_display_id: "
                << GetDisplayId(values)
                       .value_or(display::WireDisplayId{
                           .value = fuchsia_hardware_display_types::kInvalidDispId,
                       })
                       .value
                << " i_can_haz_display_mode: " << GetDisplayMode(values).value_or(0)
                << " display_rotation: " << GetDisplayRotation(values)
                << " visual_debugging_level: " << static_cast<int>(values.visual_debugging_level())
                << " enable_frame_counter_overlay: " << values.enable_frame_counter_overlay()
                << " use_separate_input_thread: " << values.use_separate_input_thread();
}

// Interval at which we log that Scenic is waiting for Vulkan or display.
static constexpr zx::duration kWaitWarningInterval = zx::sec(5);

void PostDelayedTaskUntilCancelled(fit::closure cb, zx::duration delay, bool first_run = true) {
  if (!cb)
    return;
  if (!first_run)
    cb();
  async::PostDelayedTask(
      async_get_default_dispatcher(),
      [cb = std::move(cb), delay]() mutable {
        PostDelayedTaskUntilCancelled(std::move(cb), delay, false);
      },
      delay);
}

}  // namespace

namespace scenic_impl {

DisplayInfoDelegate::DisplayInfoDelegate(std::shared_ptr<display::Display> display_)
    : display_(display_) {
  FX_CHECK(display_);
}

fuchsia_math::SizeU DisplayInfoDelegate::GetDisplayDimensions() {
  return {display_->width_in_px(), display_->height_in_px()};
}

App::App(async_dispatcher_t* flatland_dispatcher, async_dispatcher_t* input_dispatcher,
         std::unique_ptr<sys::ComponentContext> app_context,
         fidl::ClientEnd<fuchsia_io::Directory> pkg_dir,
         fidl::ServerEnd<fuchsia_io::Directory> out_dir, scenic_structured_config::Config config,
         inspect::Node& root_node,
         fpromise::promise<::display::CoordinatorClientChannels, zx_status_t> dc_handles_promise,
         fit::closure quit_callback)
    : executor_(flatland_dispatcher),
      flatland_dispatcher_(flatland_dispatcher),
      input_dispatcher_(input_dispatcher),
      app_context_(std::move(app_context)),
      config_values_(std::move(config)),
      // TODO(https://fxbug.dev/42117030): subsystems requiring graceful shutdown *on a loop* should
      // register themselves. It is preferable to cleanly shutdown using destructors only, if
      // possible.
      shutdown_manager_(ShutdownManager::New(flatland_dispatcher_, std::move(quit_callback))),
      metrics_logger_(flatland_dispatcher_, fidl::ClientEnd<fuchsia_io::Directory>(
                                                app_context_->svc()->CloneChannel().TakeChannel())),
      inspect_node_(root_node.CreateChild("scenic")),
      frame_scheduler_(
          std::make_unique<scheduling::WindowedFramePredictor>(
              zx::usec(config_values_.frame_scheduler_min_predicted_frame_duration_in_us()),
              scheduling::DefaultFrameScheduler::kInitialRenderDuration,
              scheduling::DefaultFrameScheduler::kInitialUpdateDuration,
              zx::usec(config_values_.frame_prediction_margin_in_us())),
          inspect_node_.CreateChild("FrameScheduler"), &metrics_logger_),
      renderer_type_(GetRendererType(config_values_)),
      uber_struct_system_(std::make_shared<flatland::UberStructSystem>()),
      link_system_(
          std::make_shared<flatland::LinkSystem>(uber_struct_system_->GetNextInstanceId())),
      flatland_presenter_(std::make_shared<flatland::FlatlandPresenterImpl>(
          async_get_default_dispatcher(), frame_scheduler_)),
      color_converter_(
          app_context_.get(),
          /*set_color_conversion_values*/
          [this](const auto& coefficients, const auto& preoffsets, const auto& postoffsets) {
            FX_DCHECK(flatland_compositor_);
            flatland_compositor_->SetColorConversionValues(coefficients, preoffsets, postoffsets);
          },
          /*set_minimum_rgb*/
          display::SetMinimumRgbFunc([this](const uint8_t minimum_rgb) {
            FX_DCHECK(flatland_compositor_);
            return flatland_compositor_->SetMinimumRgb(minimum_rgb);
          })),
      input_manager_(input_dispatcher_),
      health_inspector_(display_manager_, display_power_manager_, root_node) {
  LogConfig(config_values_);
  pkg_dir_.Bind(std::move(pkg_dir));
  fpromise::bridge<escher::EscherUniquePtr> escher_bridge;
  fpromise::bridge<std::shared_ptr<display::Display>> display_bridge;

  auto [dir, dir_server] = *fidl::CreateEndpoints<fuchsia_io::Directory>();
  auto vulkan_loader = component::Connect<fuchsia_vulkan_loader::Loader>();
  if (vulkan_loader.is_ok()) {
    fidl::SyncClient sync_loader(std::move(*vulkan_loader));
    auto result =
        sync_loader->ConnectToManifestFs({{.options = {}, .channel = dir_server.TakeChannel()}});
    if (result.is_error()) {
      FX_LOGS(ERROR) << "Failed to connect to Vulkan manifest FS: "
                     << result.error_value().FormatDescription();
    }
  } else {
    FX_LOGS(ERROR) << "Failed to connect to fuchsia.vulkan.loader.Loader: "
                   << vulkan_loader.status_string();
  }

  // Log a warning if Scenic is waiting for the Vulkan to load.
  //
  // Vulkan is required for Scenic to work. If you see this message printed
  // for a prolonged time, the issue is upstream of Scenic.
  auto vulkan_wait_log = std::make_unique<fxl::CancelableClosure>(
      [] { FX_LOGS(WARNING) << "SCENIC IS WAITING FOR VULKAN TO BE AVAILABLE..."; });
  PostDelayedTaskUntilCancelled(vulkan_wait_log->callback(), kWaitWarningInterval);

  if (renderer_type_ == RendererType::VULKAN) {
    // Wait for a Vulkan ICD to become advertised before trying to launch escher.
    FX_DCHECK(!device_watcher_);
    device_watcher_ = fsl::DeviceWatcher::CreateWithIdleCallback(
        std::move(dir),
        [this, completer = std::move(escher_bridge.completer),
         vulkan_wait_log = std::move(vulkan_wait_log)](
            const fidl::ClientEnd<fuchsia_io::Directory>& dir,
            const std::string& filename) mutable {
          auto escher = utils::CreateEscher(app_context_.get(), pkg_dir_);
          if (!escher) {
            FX_LOGS(WARNING) << "Escher creation failed.";
            // This should almost never happen, but might if the device was removed quickly after it
            // was added or if the Vulkan driver doesn't actually work on this hardware. Retry when
            // a new device is added.
            return;
          }
          completer.complete_ok(std::move(escher));
          device_watcher_.reset();
        },
        [] {});
    FX_DCHECK(device_watcher_);
  } else {
    // Immediately complete promise if we aren't using vulkan renderer.
    escher_bridge.completer.complete_ok(nullptr);
  }

  // Log a warning if Scenic is waiting for the Display to become available.
  //
  // Display is required for Scenic to work. If you see this message printed
  // for a prolonged time, and you expect to have a display, the issue is
  // upstream of Scenic.
  auto display_wait_log = std::make_unique<fxl::CancelableClosure>(
      [] { FX_LOGS(WARNING) << "SCENIC IS WAITING FOR DISPLAY TO BE AVAILABLE..."; });
  PostDelayedTaskUntilCancelled(display_wait_log->callback(), kWaitWarningInterval);

  // Instantiate DisplayManager and schedule a task to inject the display coordinator into it, once
  // it becomes available.
  display_manager_.emplace(
      GetDisplayId(config_values_), GetDisplayMode(config_values_),
      GetDisplayModeConstraints(config_values_), this->inspect_node_.CreateChild("DisplayManager"),
      [this, completer = std::move(display_bridge.completer),
       display_wait_log = std::move(display_wait_log)]() mutable {
        completer.complete_ok(display_manager_->default_display_shared());
      },
      GetCheckConfigHeuristics(config_values_));

  // Log a warning if Scenic is waiting for the Display Coordinator channels to be provided.
  auto dc_handles_wait_log = std::make_unique<fxl::CancelableClosure>([] {
    FX_LOGS(WARNING) << "SCENIC IS WAITING FOR DISPLAY COORDINATOR HANDLES TO BE AVAILABLE...";
  });
  PostDelayedTaskUntilCancelled(dc_handles_wait_log->callback(), kWaitWarningInterval);

  executor_.schedule_task(dc_handles_promise.then(
      [this, dc_handles_wait_log = std::move(dc_handles_wait_log)](
          fpromise::result<::display::CoordinatorClientChannels, zx_status_t>&
              client_channels) mutable {
        FX_CHECK(client_channels.is_ok()) << "Failed to get display coordinator:"
                                          << zx_status_get_string(client_channels.error());
        auto [coordinator_client, listener_server] = std::move(client_channels.value());
        display_manager_->BindDefaultDisplayCoordinator(async_get_default_dispatcher(),
                                                        std::move(coordinator_client),
                                                        std::move(listener_server));
      }));

  // Schedule a task to finish initialization once all promises have been completed.
  // This closure is placed on |executor_|, which is owned by App, so it is safe to use |this|.
  {
    auto p =
        fpromise::join_promises(escher_bridge.consumer.promise(), display_bridge.consumer.promise())
            .and_then([this, out_dir = std::move(out_dir)](
                          std::tuple<fpromise::result<escher::EscherUniquePtr>,
                                     fpromise::result<std::shared_ptr<display::Display>>>&
                              results) mutable {
              InitializeServices(std::move(std::get<0>(results).value()),
                                 std::move(std::get<1>(results).value()));
              fidl::InterfaceRequest<fuchsia::io::Directory> directory_request(
                  out_dir.TakeChannel());
              // Should be run after all outgoing services are published.
              app_context_->outgoing()->Serve(std::move(directory_request));
            });

    executor_.schedule_task(std::move(p));
  }
}

void App::InitializeServices(escher::EscherUniquePtr escher,
                             std::shared_ptr<display::Display> display) {
  TRACE_DURATION("gfx", "App::InitializeServices");

  if (!display) {
    FX_LOGS(ERROR) << "No default display, Graphics system exiting";
    shutdown_manager_->Shutdown(kShutdownTimeout);
    return;
  }

  if (renderer_type_ == RendererType::VULKAN) {
    if (!escher || !escher->device()) {
      FX_LOGS(ERROR) << "No Vulkan on device, Graphics system exiting.";
      shutdown_manager_->Shutdown(kShutdownTimeout);
      return;
    }

    escher_ = std::move(escher);
  }

  InitializeGraphics(display);
  InitializeHeartbeat(*display);
  InitializeInput();
}

App::~App() {}

void App::InitializeGraphics(std::shared_ptr<display::Display> display) {
  TRACE_DURATION("gfx", "App::InitializeGraphics");
  FX_LOGS(INFO) << "App::InitializeGraphics() " << display->width_in_px() << "x"
                << display->height_in_px() << "px  " << display->width_in_mm() << "x"
                << display->height_in_mm() << "mm";

  // Replace Escher's default pipeline builder with one which will log to Cobalt upon each
  // unexpected lazy pipeline creation.  This allows us to detect when this slips through our
  // testing and occurs in the wild.  In order to detect problems ASAP during development, debug
  // builds CHECK instead of logging to Cobalt.
  if (renderer_type_ == RendererType::VULKAN) {
    auto pipeline_builder = std::make_unique<escher::PipelineBuilder>(escher_->vk_device());
    pipeline_builder->set_log_pipeline_creation_callback(
        [metrics_logger = &metrics_logger_](const vk::GraphicsPipelineCreateInfo* graphics_info,
                                            const vk::ComputePipelineCreateInfo* compute_info) {
          // TODO(https://fxbug.dev/42126999): pre-warm compute pipelines in addition to graphics
          // pipelines.
          if (compute_info) {
            FX_LOGS(WARNING) << "Unexpected lazy creation of Vulkan compute pipeline.";
            return;
          }

          // Debug builds should crash for early detection; release builds should log to Cobalt.
          FX_DCHECK(false) << "Unexpected lazy creation of Vulkan pipeline.";
          FX_LOGS(WARNING) << "Unexpected lazy creation of Vulkan pipeline.";

          metrics_logger->LogRareEvent(
              cobalt_registry::ScenicRareEventMigratedMetricDimensionEvent::LazyPipelineCreation);
        });
    escher_->set_pipeline_builder(std::move(pipeline_builder));
  }

  {
    singleton_display_service_.emplace(display);
    singleton_display_service_->AddPublicService(app_context_->outgoing().get());
    display_info_delegate_.emplace(display);
  }

  std::shared_ptr<flatland::Renderer> flatland_renderer;
  switch (renderer_type_) {
    case RendererType::NULL_RENDERER:
      flatland_renderer = std::make_shared<flatland::NullRenderer>();
      break;
    case RendererType::VULKAN:
      flatland_renderer = std::make_shared<flatland::VkRenderer>(escher_->GetWeakPtr());
      break;
  }
  // TODO(https://fxbug.dev/42158284): flatland::VkRenderer hardcodes the framebuffer pixel format.
  // Eventually we won't, instead choosing one from the list of acceptable formats advertised by
  // each plugged-in display.  This will raise the issue of where to do pipeline cache warming: it
  // will be too early to do it here, since we're not yet aware of any displays nor the formats they
  // support.  It will probably be OK to warm the cache when a new display is plugged in, because
  // users don't expect plugging in a display to be completely jank-free.

  flatland_renderer->WarmPipelineCache();

  // TODO(https://fxbug.dev/42073146) Support camera image in shader pre-warmup.
  // Disabling this line allows any shaders that weren't warmed up to be lazily created later.
  // flatland_renderer->set_disable_lazy_pipeline_creation(true);

  // Flatland compositor must be made first; it is needed by the manager and the engine.
  {
    TRACE_DURATION("gfx", "App::InitializeServices[flatland_display_compositor]");

    flatland_compositor_ = std::make_shared<flatland::DisplayCompositor>(
        async_get_default_dispatcher(), display_manager_->coordinator_proxy(), flatland_renderer,
        utils::CreateSysmemAllocatorClientWithSvc(app_context_->svc().get(),
                                                  async_get_default_dispatcher(),
                                                  "flatland::DisplayCompositor"),
        flatland::DisplayCompositorConfig{
            .enable_direct_to_display = config_values_.display_composition(),
            .tint_gpu_fallback_images =
                (config_values_.visual_debugging_level() >= VISUAL_DEBUGGING_LEVEL_INFO),
            .enable_frame_counter_overlay =
                config_values_.enable_frame_counter_overlay() ||
                (config_values_.visual_debugging_level() >= VISUAL_DEBUGGING_LEVEL_INFO_PLUS),
        });
  }

  // Flatland manager depends on compositor, and is required by engine.
  {
    TRACE_DURATION("gfx", "App::InitializeServices[flatland_manager]");

    std::vector<std::shared_ptr<allocation::BufferCollectionImporter>> importers{
        flatland_compositor_};

    flatland_manager_ = std::make_shared<flatland::FlatlandManager>(
        async_get_default_dispatcher(), flatland_presenter_, uber_struct_system_, link_system_,
        display, std::move(importers),
        /*register_view_focuser*/
        [this](fidl::ServerEnd<fuchsia_ui_views::Focuser> focuser, zx_koid_t view_ref_koid) {
          input_manager_.AsyncCall(&input::InputManager::RegisterViewFocuser, std::move(focuser),
                                   view_ref_koid);
        },
        /*register_view_ref_focused*/
        [this](fidl::ServerEnd<fuchsia_ui_views::ViewRefFocused> vrf, zx_koid_t view_ref_koid) {
          input_manager_.AsyncCall(&input::InputManager::RegisterViewRefFocused, std::move(vrf),
                                   view_ref_koid);
        },
        /*register_touch_source*/
        [this](fidl::ServerEnd<fuchsia_ui_pointer::TouchSource> touch_source,
               zx_koid_t view_ref_koid) {
          input_manager_.AsyncCall(&input::InputManager::RegisterTouchSource,
                                   std::move(touch_source), view_ref_koid);
        },
        /*register_mouse_source*/
        [this](fidl::ServerEnd<fuchsia_ui_pointer::MouseSource> mouse_source,
               zx_koid_t view_ref_koid) {
          input_manager_.AsyncCall(&input::InputManager::RegisterMouseSource,
                                   std::move(mouse_source), view_ref_koid);
        },
        /*register_touch_source_v2*/
        [this](fidl::ServerEnd<fuchsia_ui_pointer::TouchSourceV2> touch_source_v2,
               zx_koid_t view_ref_koid) {
          input_manager_.AsyncCall(&input::InputManager::RegisterTouchSourceV2,
                                   std::move(touch_source_v2), view_ref_koid);
        },
        /*register_mouse_source_v2*/
        [this](fidl::ServerEnd<fuchsia_ui_pointer::MouseSourceV2> mouse_source_v2,
               zx_koid_t view_ref_koid) {
          input_manager_.AsyncCall(&input::InputManager::RegisterMouseSourceV2,
                                   std::move(mouse_source_v2), view_ref_koid);
        });

    // TODO(https://fxbug.dev/42146099): these should be moved into FlatlandManager.
    {
      // Note: can't use `fit::bind_member()` here, because `CreateFlatland()` returns non-void.
      FX_CHECK(app_context_->outgoing()->AddProtocol<fuchsia_ui_composition::Flatland>(
                   [flatland_manager = flatland_manager_.get()](
                       fidl::ServerEnd<fuchsia_ui_composition::Flatland> request) {
                     flatland_manager->CreateFlatland(std::move(request));
                   }) == ZX_OK);
    }
    {
      FX_CHECK(app_context_->outgoing()->AddProtocol<fuchsia_ui_composition::FlatlandDisplay>(
                   fit::bind_member(flatland_manager_.get(),
                                    &flatland::FlatlandManager::CreateFlatlandDisplay)) == ZX_OK);
    }
    {
      flatland_factory_ = std::make_unique<flatland::FlatlandFactoryImpl>(flatland_manager_);
      FX_CHECK(app_context_->outgoing()->AddProtocol<fuchsia_ui_composition::FlatlandFactory>(
                   flatland_factory_->GetHandler()) == ZX_OK);
    }
    {
      trusted_flatland_factory_ =
          std::make_unique<flatland::TrustedFlatlandFactoryImpl>(flatland_manager_);
      FX_CHECK(
          app_context_->outgoing()->AddProtocol<fuchsia_ui_composition::TrustedFlatlandFactory>(
              trusted_flatland_factory_->GetHandler()) == ZX_OK);
    }
  }

  const auto screen_capture_buffer_collection_importer =
      std::make_shared<screen_capture::ScreenCaptureBufferCollectionImporter>(
          utils::CreateSysmemAllocatorClientWithSvc(app_context_->svc().get(),
                                                    async_get_default_dispatcher(),
                                                    "ScreenCaptureBufferCollectionImporter"),
          flatland_renderer);

  // Allocator service needs Flatland DisplayCompositor to act as a BufferCollectionImporter.
  {
    std::vector<std::shared_ptr<allocation::BufferCollectionImporter>> screen_capture_importers;
    screen_capture_importers.push_back(screen_capture_buffer_collection_importer);

    std::vector<std::shared_ptr<allocation::BufferCollectionImporter>> default_importers;
    default_importers.push_back(flatland_compositor_);

    allocator_ = std::make_shared<allocation::Allocator>(
        async_get_default_dispatcher(), app_context_.get(), default_importers,
        screen_capture_importers,
        utils::CreateSysmemAllocatorClientWithSvc(
            app_context_->svc().get(), async_get_default_dispatcher(), "ScenicAllocator"),
        inspect_node_.CreateChild("Allocator API"));
  }

  // Flatland engine requires FlatlandManager and DisplayCompositor to be constructed first.
  {
    TRACE_DURATION("gfx", "App::InitializeServices[flatland_engine]");

    flatland_engine_ = std::make_shared<flatland::Engine>(
        flatland_compositor_, flatland_presenter_, uber_struct_system_, link_system_,
        inspect_node_.CreateChild("FlatlandEngine"),
        /*get_root_transform*/
        [this] {
          FX_DCHECK(flatland_manager_);
          const auto display = flatland_manager_->GetPrimaryFlatlandDisplayForRendering();
          return display ? std::optional<flatland::TransformHandle>(display->root_transform())
                         : std::nullopt;
        });
    display_manager_->SetDisplayAddedCallback(
        [weak_engine = std::weak_ptr{flatland_engine_}](display::Display& display) {
          if (auto engine = weak_engine.lock()) {
            engine->AddDisplay(display);
          }
        });
  }

  // Make ScreenCaptureManager.
  {
    TRACE_DURATION("gfx", "App::InitializeServices[screen_capture_manager]");

    std::vector<std::shared_ptr<allocation::BufferCollectionImporter>> screen_capture_importers;
    screen_capture_importers.push_back(screen_capture_buffer_collection_importer);

    // Capture flatland_manager since the primary display may not have been initialized yet.
    screen_capture_manager_.emplace(flatland_engine_, flatland_renderer, flatland_manager_,
                                    std::move(screen_capture_importers));

    zx::result<> add_screen_capture_protocol_result = zx::make_result(
        app_context_->outgoing()->AddProtocol<fuchsia_ui_composition::ScreenCapture>(
            [this](fidl::ServerEnd<fuchsia_ui_composition::ScreenCapture> server_end) {
              screen_capture_manager_->CreateClient(std::move(server_end));
            }));
    FX_CHECK(add_screen_capture_protocol_result.is_ok())
        << "Add ScreenCapture protocol failed: "
        << add_screen_capture_protocol_result.status_string();
  }

  // Make ScreenCapture2Manager.
  {
    TRACE_DURATION("gfx", "App::InitializeServices[screen_capture2_manager]");

    // Capture flatland_manager since the primary display may not have been initialized yet.
    screen_capture2_manager_.emplace(
        flatland_renderer, screen_capture_buffer_collection_importer, [this]() {
          FX_DCHECK(flatland_manager_);
          FX_DCHECK(flatland_engine_);

          auto display = flatland_manager_->GetPrimaryFlatlandDisplayForRendering();
          if (!display) {
            FX_LOGS(WARNING)
                << "No FlatlandDisplay attached at root. Returning an empty screenshot.";
            return flatland::Renderables();
          }

          return flatland_engine_->GetRenderables(*display);
        });

    zx::result<> add_screen_capture2_protocol_result = zx::make_result(
        app_context_->outgoing()->AddProtocol<fuchsia_ui_composition_internal::ScreenCapture>(
            [this](fidl::ServerEnd<fuchsia_ui_composition_internal::ScreenCapture> server_end) {
              screen_capture2_manager_->CreateClient(std::move(server_end));
            }));
    FX_CHECK(add_screen_capture2_protocol_result.is_ok())
        << "Add ScreenCapture2 protocol failed: "
        << add_screen_capture2_protocol_result.status_string();
  }

  // Make ScreenshotManager for the client-friendly screenshot protocol.
  {
    TRACE_DURATION("gfx", "App::InitializeServices[screenshot_manager]");

    std::vector<std::shared_ptr<allocation::BufferCollectionImporter>> screen_capture_importers;
    screen_capture_importers.push_back(screen_capture_buffer_collection_importer);

    // Capture flatland_manager since the primary display may not have been initialized yet.
    screenshot_manager_.emplace(
        app_context_.get(), allocator_, flatland_renderer,
        [this]() {
          FX_DCHECK(flatland_manager_);
          FX_DCHECK(flatland_engine_);

          auto display = flatland_manager_->GetPrimaryFlatlandDisplayForRendering();
          if (!display) {
            FX_LOGS(WARNING)
                << "No FlatlandDisplay attached at root. Returning an empty screenshot.";
            return flatland::Renderables();
          }

          return flatland_engine_->GetRenderables(*display);
        },
        std::move(screen_capture_importers), display_info_delegate_->GetDisplayDimensions(),
        GetDisplayRotation(config_values_));

    FX_CHECK(app_context_->outgoing()->AddProtocol<fuchsia_ui_composition::Screenshot>(
                 [this](fidl::ServerEnd<fuchsia_ui_composition::Screenshot> server_end) {
                   screenshot_manager_->CreateBinding(std::move(server_end));
                 }) == ZX_OK);
  }

  {
    TRACE_DURATION("gfx", "App::InitializeServices[display_power]");
    display_power_manager_.emplace(display_manager_.value(), inspect_node_);
    FX_CHECK(app_context_->outgoing()->AddProtocol<fuchsia_ui_display_singleton::DisplayPower>(
                 display_power_manager_->GetHandler()) == ZX_OK);
  }

  {
    TRACE_DURATION("gfx", "App::InitializeServices[vsync_source_manager_]");
    vsync_source_manager_.emplace(display_manager_.value());
    FX_CHECK(app_context_->outgoing()->AddProtocol<fuchsia_ui_display_singleton::VsyncSource>(
                 [this](fidl::ServerEnd<fuchsia_ui_display_singleton::VsyncSource> request) {
                   vsync_source_manager_->CreateBinding(std::move(request));
                 }) == ZX_OK);
  }
}

void App::InitializeInput() {
  snapshot_holder_ = std::make_shared<view_tree::SnapshotHolder>();
  input_manager_.emplace(async_patterns::PassDispatcher, snapshot_holder_,
                         inspect_node_.CreateChild("input"), config_values_.pointer_auto_focus());

  // sys::OutgoingDirectory is thread-hostile. To avoid thread-safety risks and races with
  // outgoing()->Serve(), all public services must be registered synchronously on the main thread.
  // Connection requests are then forwarded asynchronously to the input thread via
  // `input_manager_.AsyncCall()`.

  // Register FocusChainListenerRegistry
  FX_CHECK(app_context_->outgoing()->AddProtocol<fuchsia_ui_focus::FocusChainListenerRegistry>(
               [this](fidl::ServerEnd<fuchsia_ui_focus::FocusChainListenerRegistry> server_end) {
                 input_manager_.AsyncCall(&input::InputManager::BindFocusChainListenerRegistry,
                                          std::move(server_end));
               }) == ZX_OK);

  // Register ViewRefInstalled
  FX_CHECK(app_context_->outgoing()->AddProtocol<fuchsia_ui_views::ViewRefInstalled>(
               [this](fidl::ServerEnd<fuchsia_ui_views::ViewRefInstalled> server_end) {
                 input_manager_.AsyncCall(&input::InputManager::BindViewRefInstalled,
                                          std::move(server_end));
               }) == ZX_OK);

  // Register test Observer Registry
  FX_CHECK(app_context_->outgoing()->AddProtocol<fuchsia_ui_observation_test::Registry>(
               [this](fidl::ServerEnd<fuchsia_ui_observation_test::Registry> server_end) {
                 input_manager_.AsyncCall(&input::InputManager::BindObserverRegistry,
                                          std::move(server_end));
               }) == ZX_OK);

  // Register scoped Observer Registry
  FX_CHECK(app_context_->outgoing()->AddProtocol<fuchsia_ui_observation_scope::Registry>(
               [this](fidl::ServerEnd<fuchsia_ui_observation_scope::Registry> server_end) {
                 input_manager_.AsyncCall(&input::InputManager::BindScopedObserverRegistry,
                                          std::move(server_end));
               }) == ZX_OK);

  // Register Pointerinjector Registry
#if !defined(FUCHSIA_DSO)
  // Zircon channel version of pointerinjector registry.
  FX_CHECK(app_context_->outgoing()->AddProtocol<fuchsia_ui_pointerinjector::Registry>(
               [this](fidl::ServerEnd<fuchsia_ui_pointerinjector::Registry> server_end) {
                 input_manager_.AsyncCall(&input::InputManager::BindPointerinjectorRegistry,
                                          std::move(server_end));
               }) == ZX_OK);
#else
  // Driver transport version of pointerinjector registry.
  app_context_->outgoing()->AddPublicService(
      [this](zx::channel channel, async_dispatcher_t* unused_dispatcher) mutable {
        input_manager_.AsyncCall(&input::InputManager::BindPointerinjectorRegistry,
                                 std::move(channel));
      },
      fuchsia_ui_pointerinjector_dso::Registry::kDiscoverableName);
#endif

  // Register LocalHit upgrade registry
  FX_CHECK(app_context_->outgoing()->AddProtocol<fuchsia_ui_pointer_augment::LocalHit>(
               [this](fidl::ServerEnd<fuchsia_ui_pointer_augment::LocalHit> server_end) {
                 input_manager_.AsyncCall(&input::InputManager::BindLocalHit,
                                          std::move(server_end));
               }) == ZX_OK);

  // Register Accessibility PointerEventRegistry
  FX_CHECK(
      app_context_->outgoing()->AddProtocol<fuchsia_ui_input_accessibility::PointerEventRegistry>(
          [this](fidl::ServerEnd<fuchsia_ui_input_accessibility::PointerEventRegistry> server_end) {
            input_manager_.AsyncCall(&input::InputManager::BindA11yPointerEventRegistry,
                                     std::move(server_end));
          }) == ZX_OK);
}

void App::InitializeHeartbeat(display::Display& display) {
  TRACE_DURATION("gfx", "App::InitializeHeartbeat");

  // Initialize ViewTreeSnapshotter
  {
    // These callbacks are be called once per frame (at the end of OnCpuWorkDone()) and the results
    // used to build the ViewTreeSnapshot.
    // We create one per compositor.
    std::vector<view_tree::SubtreeSnapshotGenerator> subtrees_generator_callbacks;
    subtrees_generator_callbacks.emplace_back([this] {
      if (auto display = flatland_manager_->GetPrimaryFlatlandDisplayForRendering()) {
        return flatland_engine_->GenerateViewTreeSnapshot(display->root_transform());
      }
      return view_tree::GeneratedSubtreeSnapshot(std::make_unique<view_tree::SubtreeSnapshot>());
    });

    // All subscriber callbacks get called with the new snapshot every time one is generated (once
    // per frame).
    std::vector<view_tree::ViewTreeSnapshotter::Subscriber> subscribers;

    subscribers.push_back({.on_new_view_tree = [this](auto snapshot) {
      // The snapshot must be updated on the main thread because the dispatcher does not provide
      // FIFO guarantees. Subsequent FIDL calls could be scheduled and processed on the input
      // thread before the snapshot task is run on the input thread. Updating the snapshot
      // synchronously on the main thread ensures consistency. See b/42155704 for more detail.
      snapshot_holder_->SetSnapshot(snapshot);
      input_manager_.AsyncCall(&input::InputManager::OnNewViewTreeSnapshot);
    }});

    if (enable_snapshot_dump_) {
      subscribers.push_back({.on_new_view_tree = [](auto snapshot) {
        view_tree::SnapshotDump::OnNewViewTreeSnapshot(std::move(snapshot));
      }});
    }

    view_tree_snapshotter_.emplace(std::move(subtrees_generator_callbacks), std::move(subscribers));
  }

  // Set up what to do each time a FrameScheduler event fires.
  frame_scheduler_.Initialize(
      display.vsync_timing(),
      /*update_sessions*/
      [this](auto& sessions_to_update, auto trace_id) {
        TRACE_DURATION("gfx", "App update_sessions");
        flatland_manager_->UpdateInstances(sessions_to_update);
        flatland_presenter_->AccumulateFences(sessions_to_update);
      },
      /*on_cpu_work_done*/
      [this] {
        TRACE_DURATION("gfx", "App on_cpu_work_done");
        if (view_tree_snapshotter_->UpdateSnapshot()) {
          // The check above is loose: it is possible for the view tree to change without any change
          // to the link system.  However, any changes to the link system that need to be published
          // will also have triggered recomputation of the view tree.
          //
          // See `Engine::GenerateViewTreeSnapshot()` for details; a new subtree snapshot is
          // generated whenever the link topology changes.
          flatland_engine_->UpdateLinkWatchersAfterViewTreePublished();
        }
        // Clears scene state, so must happen after ViewTree update, etc.
        flatland_engine_->CleanUpFrame();

        async::PostTask(async_get_default_dispatcher(), [this] {
          flatland_manager_->SendHintsToStartRendering();
          screen_capture2_manager_->RenderPendingScreenCaptures();
          if (escher_) {
            escher_->Cleanup();
          }
        });
      },
      /*on_frame_presented*/
      [this](auto latched_times, auto present_times) {
        TRACE_DURATION("gfx", "App on_frame_presented");
        flatland_manager_->OnFramePresented(latched_times, present_times);
      },
      /*render_scheduled_frame*/
      [this](auto frame_number, auto presentation_time, auto frame_presented_callback) {
        TRACE_DURATION("gfx", "App render_scheduled_frame");
        FX_CHECK(flatland_frame_count_ + skipped_frame_count_ == frame_number - 1);
        if (auto display = flatland_manager_->GetPrimaryFlatlandDisplayForRendering()) {
          flatland_engine_->RenderScheduledFrame(frame_number, presentation_time, *display,
                                                 std::move(frame_presented_callback));
          ++flatland_frame_count_;
        } else {
          FX_LOGS(INFO) << "No FlatlandDisplay; skipping render scheduled frame.";
          skipped_frame_count_++;
          flatland_engine_->SkipRender(std::move(frame_presented_callback));
        }
      });
}

void PrefetchBinary(zx_handle_t pkg_dir, const char* binary_path) {
  auto [client_end, server_end] = fidl::Endpoints<fuchsia_io::File>::Create();
  zx_status_t status =
      fdio_open3_at(pkg_dir, binary_path,
                    static_cast<uint64_t>(fuchsia_io::kPermReadable | fuchsia_io::kPermExecutable),
                    server_end.TakeChannel().release());
  FX_CHECK(status == ZX_OK) << "Failed to open " << binary_path << ": "
                            << zx_status_get_string(status);

  fidl::SyncClient file(std::move(client_end));
  auto result =
      file->GetBackingMemory(fuchsia_io::VmoFlags::kRead | fuchsia_io::VmoFlags::kExecute);
  FX_CHECK(result.is_ok()) << "Failed to get backing memory for " << binary_path << ": "
                           << result.error_value();

  zx::vmo vmo = std::move(result->vmo());
  uint64_t size;
  status = vmo.get_size(&size);
  FX_CHECK(status == ZX_OK) << "Failed to get VMO size for " << binary_path << ": "
                            << zx_status_get_string(status);

  status = vmo.op_range(ZX_VMO_OP_ALWAYS_NEED, 0, size, nullptr, 0);
  FX_CHECK(status == ZX_OK) << "Failed to pin VMO for " << binary_path << ": "
                            << zx_status_get_string(status);

  FX_LOGS(INFO) << "Prefetched " << binary_path;
}

}  // namespace scenic_impl

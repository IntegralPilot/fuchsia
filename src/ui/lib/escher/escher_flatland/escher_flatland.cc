// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "./escher_flatland.h"

#include <fidl/fuchsia.element/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <fidl/fuchsia.ui.views/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/async/cpp/wait.h>
#include <lib/component/incoming/cpp/protocol.h>

#include <filesystem>

#include "src/ui/lib/escher/debug/debug_font.h"
#include "src/ui/lib/escher/escher.h"
#include "src/ui/lib/escher/fs/hack_filesystem.h"
#include "src/ui/lib/escher/renderer/batch_gpu_uploader.h"
#include "src/ui/lib/escher/renderer/semaphore.h"
#include "src/ui/lib/escher/vk/image.h"
#include "src/ui/lib/escher/vk/vulkan_device_queues.h"
#include "src/ui/lib/escher/vk/vulkan_instance.h"
#include "src/ui/lib/escher/vk/vulkan_swapchain.h"
#include "src/ui/lib/escher/vk/vulkan_swapchain_helper.h"
#include "vulkan/vulkan_core.h"

#include "vulkan/vulkan_enums.hpp"
#include "vulkan/vulkan_structs.hpp"

namespace escher {

const fuchsia_ui_composition::TransformId kTransform = {1};
const fuchsia_ui_composition::TransformId kTransformNone = {0};

VulkanInstance::Params GetVulkanInstanceParams() {
  VulkanInstance::Params instance_params{
      {"VK_LAYER_FUCHSIA_imagepipe_swapchain"},
      {VK_EXT_DEBUG_UTILS_EXTENSION_NAME, VK_FUCHSIA_IMAGEPIPE_SURFACE_EXTENSION_NAME,
       VK_KHR_EXTERNAL_MEMORY_CAPABILITIES_EXTENSION_NAME,
       VK_KHR_EXTERNAL_SEMAPHORE_CAPABILITIES_EXTENSION_NAME, VK_KHR_SURFACE_EXTENSION_NAME,
       VK_KHR_GET_PHYSICAL_DEVICE_PROPERTIES_2_EXTENSION_NAME},
  };

  auto validation_layer_name = VulkanInstance::GetValidationLayerName();
  if (validation_layer_name) {
    instance_params.layer_names.insert(*validation_layer_name);
  }

  return instance_params;
}

vk::SurfaceKHR CreateSurface(const vk::Instance& instance,
                             fuchsia_ui_views::ViewCreationToken view_creation_token) {
  vk::ImagePipeSurfaceCreateInfoFUCHSIA create_info;
  FX_CHECK(view_creation_token.value());
  create_info.imagePipeHandle = view_creation_token.value().release();
  vk::SurfaceKHR surface;

  auto result = instance.createImagePipeSurfaceFUCHSIA(&create_info, nullptr, &surface);
  FX_CHECK(result == vk::Result::eSuccess);

  return surface;
}

VulkanSwapchain CreateSwapchain(Escher& escher, vk::SurfaceKHR surface, vk::Extent2D extent,
                                vk::SwapchainKHR old_swapchain) {
  vk::PhysicalDevice physical_device = escher.vk_physical_device();
  vk::Device device = escher.vk_device();

  constexpr uint32_t kDesiredNumOfSwapchainImages = 2;
  constexpr vk::Format kFormat = vk::Format::eB8G8R8A8Unorm;
  constexpr vk::ColorSpaceKHR kColorSpace = vk::ColorSpaceKHR::eSrgbNonlinear;

  constexpr vk::SurfaceTransformFlagBitsKHR kPreTransform =
      vk::SurfaceTransformFlagBitsKHR::eIdentity;

  // FIFO is always available, and is good enough for this example.
  constexpr vk::PresentModeKHR kPresentMode = vk::PresentModeKHR::eFifo;

  // We're not using any render passes, only blitting, therefore it is OK to omit eColorAttachment.
  constexpr vk::ImageUsageFlags kImageUsage = vk::ImageUsageFlagBits::eTransferDst;

  vk::SurfaceCapabilitiesKHR surface_caps;
  {
    auto result = physical_device.getSurfaceCapabilitiesKHR(surface, &surface_caps);
    FX_CHECK(result == vk::Result::eSuccess);
  }

  // TODO(https://fxbug.dev/345269489): VK_LAYER_FUCHSIA_imagepipe_swapchain only supports eOpaque.
  // For some use cases, we want to respect translucency in the presented image.
  const vk::CompositeAlphaFlagBitsKHR kCompositeAlpha =
      static_cast<bool>(surface_caps.supportedCompositeAlpha &
                        vk::CompositeAlphaFlagBitsKHR::ePostMultiplied)
          ? vk::CompositeAlphaFlagBitsKHR::ePostMultiplied
          : vk::CompositeAlphaFlagBitsKHR::eOpaque;

  // Verify that hard-coded surface format and color space are supported.
  {
    auto result = physical_device.getSurfaceFormatsKHR(surface);
    FX_CHECK(result.result == vk::Result::eSuccess);

    bool found_supported_format = false;
    for (vk::SurfaceFormatKHR& supported_format : result.value) {
      if (supported_format.format == kFormat && supported_format.colorSpace == kColorSpace) {
        found_supported_format = true;
      }
    }
    FX_CHECK(found_supported_format);
  }

  // Create swapchain.
  auto swapchain_return =
      device.createSwapchainKHR(vk::SwapchainCreateInfoKHR()
                                    .setSurface(surface)
                                    .setMinImageCount(kDesiredNumOfSwapchainImages)
                                    .setImageFormat(kFormat)
                                    .setImageColorSpace(kColorSpace)
                                    .setImageExtent(extent)
                                    .setImageArrayLayers(1)
                                    .setImageUsage(kImageUsage)
                                    .setImageSharingMode(vk::SharingMode::eExclusive)
                                    .setPreTransform(kPreTransform)
                                    .setCompositeAlpha(kCompositeAlpha)
                                    .setPresentMode(kPresentMode)
                                    .setClipped(true)
                                    .setOldSwapchain(old_swapchain));

  if (old_swapchain) {
    // Note: destroying the swapchain also cleans up all its associated
    // presentable images once the platform is done with them.
    device.destroySwapchainKHR(old_swapchain);
  }

  FX_CHECK(swapchain_return.result == vk::Result::eSuccess)
      << vk::to_string(swapchain_return.result);

  // Obtain swapchain images and buffers.
  auto result = device.getSwapchainImagesKHR(swapchain_return.value);
  FX_CHECK(result.result == vk::Result::eSuccess);

  std::vector<vk::Image> images(std::move(result.value));
  std::vector<ImagePtr> escher_images;
  escher_images.reserve(images.size());
  for (auto& im : images) {
    ImageInfo image_info;
    image_info.format = kFormat;
    image_info.width = extent.width;
    image_info.height = extent.height;
    image_info.usage = kImageUsage;

    auto escher_image =
        Image::WrapVkImage(escher.resource_recycler(), image_info, im, vk::ImageLayout::eUndefined);
    FX_CHECK(escher_image);
    escher_images.push_back(escher_image);
  }

  return VulkanSwapchain(swapchain_return.value, escher_images, extent.width, extent.height,
                         kFormat, kColorSpace);
}

void escher::EscherFlatland::RenderFrame(RenderFrameFn render_frame) {
  static uint64_t frame_number = 1;
  swapchain_helper_->DrawFrame([&](const escher::ImagePtr& output_image,
                                   const escher::SemaphorePtr& framebuffer_acquired,
                                   const escher::SemaphorePtr& render_finished) {
    auto frame = escher_->NewFrame("escher-flatland", frame_number++, false);

    frame->cmds()->AddWaitSemaphore(framebuffer_acquired, vk::PipelineStageFlagBits::eTransfer);

    // It is OK (and possibly more efficient) to transition from eUndefined, since we will be
    // clearing the whole image anyway.
    frame->cmds()->TransitionImageLayout(output_image, vk::ImageLayout::eUndefined,
                                         vk::ImageLayout::eTransferDstOptimal);

    render_frame(output_image, image_extent_, frame);

    frame->cmds()->TransitionImageLayout(output_image, vk::ImageLayout::eTransferDstOptimal,
                                         vk::ImageLayout::ePresentSrcKHR);

    frame->EndFrame(render_finished, []() {});
  });
}

void escher::EscherFlatland::RenderLoop(RenderFrameFn render_frame) {
  RenderFrame(render_frame);
  async::PostTask(dispatcher_,
                  [this, render_frame = std::move(render_frame)] { RenderLoop(render_frame); });
}

zx::eventpair escher::EscherFlatland::ConnectPowerResources() {
  // Connect to sag and obtain an activity lease.
  zx::result sag_client_end = component::Connect<fuchsia_power_system::ActivityGovernor>();
  FX_CHECK(sag_client_end.is_ok());
  sag_ = fidl::SyncClient(std::move(*sag_client_end));

  // Hold onto a wake lease to stop going to suspend mode before running CPU and GPU commands.
  return GetWakeLease();
}

void escher::EscherFlatland::ConnectTimerResources() {
  // Connect to fuchsia.time.alarms.WakeAlarms.
  zx::result wake_client_end = component::Connect<fuchsia_time_alarms::WakeAlarms>();
  FX_CHECK(wake_client_end.is_ok());
  wake_alarms_ = fidl::SyncClient(std::move(*wake_client_end));
}

void escher::EscherFlatland::RenderLoopWithWakingDelay(RenderFrameFn render_frame,
                                                       zx::eventpair wake_lease) {
  if (!enable_wake_alarms_) {
    FX_LOGS(INFO) << "EscherFlatland::RenderLoopWithWakingDelay was called while"
                  << " use of wake alarms was disabled; skipping render.";
    return;
  }

  FX_CHECK(sag_) << "Missing SAG. Make sure to \"ConnectPowerResources\" first.";
  FX_CHECK(wake_alarms_) << "Missing wake alarms. Make sure to \"ConnectTimerResources\" first.";

  // Get a wake lease to wake GPU for render work.
  auto activity_lease = GetWakeLease();
  RenderFrame(render_frame);

  // TODO(https://fxbug.dev/376925106): This delay is a workaround, because we need to
  // actually wait for gpu work to be done and Scenic to put the image on display. We
  // currently don't have these signals as we are using the vulkan imagepipe swapchain
  // extension. We should use Flatland directly instead and rely on OnFramePresented()
  // signals.
  async::PostDelayedTask(
      dispatcher_,
      [activity_lease = std::move(
           activity_lease)]() mutable {  // Drop the lease as render work should be completed.
      },
      kSuspendDelayForComposition);

  FX_CHECK(wake_lease.is_valid());
  fidl::Result<fuchsia_time_alarms::WakeAlarms::SetAndWait> result = wake_alarms_->SetAndWait(
      {next_render_time_, fuchsia_time_alarms::SetMode::WithKeepAlive(std::move(wake_lease)),
       name_});
  if (result.is_ok()) {
    // We do not need to check `enable_wake_alarms_` before the recursive call
    // because a cancelled alarm would have resulted in `WakeError::DROPPED`.
    async::PostTask(dispatcher_, [render_frame = std::move(render_frame),
                                  lease = std::move(result->keep_alive()), this]() mutable {
      RenderLoopWithWakingDelay(render_frame, std::move(lease));
    });
  } else {
    FX_LOGS(ERROR) << "SetAndWait failed: " << result.error_value().FormatDescription();
  }
}

void escher::EscherFlatland::GetStatus(OnStatusFn on_status) {
  parent_viewport_watcher_->GetStatus().Then(on_status);
}

zx::eventpair escher::EscherFlatland::GetWakeLease() {
  FX_CHECK(sag_) << "Missing SAG. Make sure to \"ConnectPowerResources\" first.";
  auto acquire_wake_lease_result = sag_->AcquireWakeLease(name_);
  if (acquire_wake_lease_result.is_ok()) {
    return std::move(acquire_wake_lease_result->token());
  } else {
    FX_LOGS(ERROR) << "AcquireWakeLease failed: " << acquire_wake_lease_result.error_value();
    return zx::eventpair();
  }
}

void escher::EscherFlatland::EnableWakeAlarms(bool enable) {
  if (enable_wake_alarms_ == enable) {
    return;
  }

  enable_wake_alarms_ = enable;
  if (enable) {
    return;
  }

  const auto& cancel_result = wake_alarms_->Cancel({{.alarm_id = name_}});
  if (!cancel_result.is_ok()) {
    FX_LOGS(ERROR) << "Cancel failed: " << cancel_result.error_value();
  }
}

void escher::EscherFlatland::SetVisible(bool is_visible) {
  fuchsia_ui_composition::TransformId transform_id = is_visible ? kTransform : kTransformNone;
  const auto& set_root_transform_result = flatland_->SetRootTransform(transform_id);
  if (!set_root_transform_result.is_ok()) {
    FX_LOGS(FATAL) << "SetRootTransform failed: " << set_root_transform_result.error_value();
  }
  fuchsia_ui_composition::PresentArgs present_args;
  const auto& present_result = flatland_->Present({{.args = std::move(present_args)}});
  if (!present_result.is_ok()) {
    FX_LOGS(FATAL) << "Present failed: " << present_result.error_value();
  }
}

EscherFlatland::EscherFlatland(async_dispatcher_t* dispatcher, std::string name)
    : dispatcher_(dispatcher), name_(name) {
  // Create graphical_presenter_token and local_flatland_view_token.
  fuchsia_ui_views::ViewCreationToken local_flatland_view_token;
  fuchsia_ui_views::ViewportCreationToken graphical_presenter_token;
  {
    zx::channel::create(0, &local_flatland_view_token.value(), &graphical_presenter_token.value());
    FX_CHECK(local_flatland_view_token.value() && graphical_presenter_token.value());
  }

  // Connect to Flatland and create View.
  zx::result flatland_client_end = component::Connect<fuchsia_ui_composition::Flatland>();
  FX_CHECK(flatland_client_end.is_ok());
  flatland_ = fidl::SyncClient(std::move(*flatland_client_end));
  auto parent_viewport_watcher_endpoints =
      fidl::CreateEndpoints<fuchsia_ui_composition::ParentViewportWatcher>();
  if (!parent_viewport_watcher_endpoints.is_ok()) {
    FX_LOGS(ERROR) << "CreateEndpoints failed.";
  }
  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      std::move(parent_viewport_watcher_endpoints.value());
  parent_viewport_watcher_.Bind(std::move(parent_viewport_watcher_client_end), dispatcher_);
  const auto& create_view_result = flatland_->CreateView(
      {{.token = std::move(local_flatland_view_token.value()),
        .parent_viewport_watcher = std::move(parent_viewport_watcher_server_end)}});
  if (!create_view_result.is_ok()) {
    FX_LOGS(FATAL) << "CreateView failed: " << create_view_result.error_value();
  }

  // Connect the swapchain view to the graphical presenter.
  zx::result presenter_client_end = component::Connect<fuchsia_element::GraphicalPresenter>();

  FX_CHECK(presenter_client_end.is_ok());
  fidl::SyncClient presenter_client{std::move(*presenter_client_end)};

  // Connect the flatland view to the graphical presenter.
  {
    fuchsia_element::ViewSpec spec;
    spec.viewport_creation_token(std::move(graphical_presenter_token.value()));

    fuchsia_element::AnnotationKey key{{.namespace_ = "window_manager", .value = "view_id"}};
    auto value = fuchsia_element::AnnotationValue::WithText(name_);
    fuchsia_element::Annotations annotations;
    annotations.emplace_back(std::move(key), std::move(value));
    spec.annotations(std::move(annotations));

    auto controller_endpoints = fidl::CreateEndpoints<fuchsia_element::ViewController>();
    FX_CHECK(controller_endpoints.is_ok());

    fidl::Result result = presenter_client->PresentView(
        {{.view_spec = std::move(spec),
          .view_controller_request = std::move(controller_endpoints->server)}});
    FX_CHECK(result.is_ok()) << "error value: " << result.error_value();
  }

  // Get the layout size.
  const auto& get_layout_result = parent_viewport_watcher_.sync()->GetLayout();
  if (!get_layout_result.ok()) {
    FX_LOGS(FATAL) << "GetLayout failed: " << get_layout_result.error().FormatDescription().c_str();
  }
  const fuchsia_math::SizeU logical_size = {get_layout_result->info.logical_size().width,
                                            get_layout_result->info.logical_size().height};

  // Create local_flatland_viewport_token and swapchain_view_token.
  fuchsia_ui_views::ViewCreationToken swapchain_view_token;
  fuchsia_ui_views::ViewportCreationToken local_flatland_viewport_token;
  {
    zx::channel::create(0, &swapchain_view_token.value(), &local_flatland_viewport_token.value());
    FX_CHECK(swapchain_view_token.value() && local_flatland_viewport_token.value());
  }

  // Create viewport for swapchain.
  const auto& set_debug_name_result = flatland_->SetDebugName({"escher_clockface"});
  if (!set_debug_name_result.is_ok()) {
    FX_LOGS(FATAL) << "SetDebugName failed: " << set_debug_name_result.error_value();
  }
  const auto& create_transform_result = flatland_->CreateTransform(kTransform);
  if (!create_transform_result.is_ok()) {
    FX_LOGS(FATAL) << "CreateTransform failed: " << create_transform_result.error_value();
  }
  const fuchsia_ui_composition::ContentId kContent = {1};
  fuchsia_ui_composition::ViewportProperties viewport_properties;
  viewport_properties.logical_size(logical_size);
  auto child_view_watcher_endpoints =
      fidl::CreateEndpoints<fuchsia_ui_composition::ChildViewWatcher>();
  if (!child_view_watcher_endpoints.is_ok()) {
    FX_LOGS(ERROR) << "CreateEndpoints failed.";
  }
  auto [child_view_watcher_client_end, child_view_watcher_server_end] =
      std::move(child_view_watcher_endpoints.value());
  const auto& create_viewport_result =
      flatland_->CreateViewport({{.viewport_id = kContent,
                                  .token = std::move(local_flatland_viewport_token.value()),
                                  .properties = std::move(viewport_properties),
                                  .child_view_watcher = std::move(child_view_watcher_server_end)}});
  if (!create_viewport_result.is_ok()) {
    FX_LOGS(FATAL) << "CreateViewport failed: " << create_viewport_result.error_value();
  }
  const auto& set_content_result =
      flatland_->SetContent({{.transform_id = kTransform, .content_id = kContent}});
  if (!set_content_result.is_ok()) {
    FX_LOGS(FATAL) << "SetContent failed: " << set_content_result.error_value();
  }
  fuchsia_ui_composition::PresentArgs present_args;
  const auto& present_result = flatland_->Present({{.args = std::move(present_args)}});
  if (!present_result.is_ok()) {
    FX_LOGS(FATAL) << "Present failed: " << present_result.error_value();
  }

  // Create Vulkan resources
  auto instance = escher::VulkanInstance::New(GetVulkanInstanceParams());
  FX_CHECK(instance);
  auto surface = CreateSurface(instance->vk_instance(), std::move(swapchain_view_token.value()));
  auto device = escher::VulkanDeviceQueues::New(
      instance,
      {{VK_FUCHSIA_EXTERNAL_MEMORY_EXTENSION_NAME, VK_KHR_MAINTENANCE1_EXTENSION_NAME,
        VK_KHR_BIND_MEMORY_2_EXTENSION_NAME, VK_KHR_SAMPLER_YCBCR_CONVERSION_EXTENSION_NAME},
       {},
       std::move(surface)});

  auto filesystem = escher::HackFilesystem::New(nullptr);
  escher_ = std::make_unique<escher::Escher>(device, filesystem, nullptr);

  image_extent_ = vk::Extent2D(logical_size.width(), logical_size.height());
  FX_LOGS(INFO) << "viewport size: (" << logical_size.width() << ", " << logical_size.height()
                << ").";
  auto swapchain = CreateSwapchain(*escher_, surface, image_extent_, vk::SwapchainKHR());
  swapchain_helper_ = std::make_unique<escher::VulkanSwapchainHelper>(
      swapchain, device->vk_device(), device->vk_main_queue());

  {
    auto frame = escher_->NewFrame((name_ + "-init").c_str(), 1, false);
    auto gpu_uploader =
        std::make_shared<BatchGpuUploader>(escher_->GetWeakPtr(), frame->frame_number());

    debug_font_ = DebugFont::New(gpu_uploader.get(), escher_->image_cache());
    gpu_uploader->Submit();

    frame->EndFrame(SemaphorePtr(), []() {});

    // Normally we don't want to wait synchronously for the GPU to finish work,
    // but this is just once at init time.
    auto result = escher_->vk_device().waitIdle();
    FX_CHECK(result == vk::Result::eSuccess);
  }
}

}  // namespace escher

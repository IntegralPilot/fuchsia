// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.display/cpp/fidl.h>
#include <fidl/fuchsia.images2/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <lib/async/default.h>
#include <lib/component/incoming/cpp/service_member_watcher.h>
#include <lib/fdio/directory.h>
#include <lib/fit/defer.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/zircon-internal/align.h>

#include <span>

#include "src/graphics/display/lib/coordinator-getter/client.h"
#include "src/lib/fsl/handles/object_info.h"
#include "src/lib/testing/loop_fixture/real_loop_fixture.h"
#include "src/lib/testing/predicates/status.h"
#include "src/ui/lib/escher/impl/vulkan_utils.h"
#include "src/ui/lib/escher/test/common/gtest_escher.h"
#include "src/ui/scenic/lib/allocation/buffer_collection_importer.h"
#include "src/ui/scenic/lib/display/display_manager.h"
#include "src/ui/scenic/lib/flatland/buffers/util.h"
#include "src/ui/scenic/lib/flatland/engine/display_compositor.h"
#include "src/ui/scenic/lib/flatland/flatland_types.h"
#include "src/ui/scenic/lib/flatland/renderer/null_renderer.h"
#include "src/ui/scenic/lib/flatland/renderer/vk_renderer.h"
#include "src/ui/scenic/lib/flatland/testing/build_display_realm.h"
#include "src/ui/scenic/lib/utils/helpers.h"
#include "src/ui/scenic/tests/utils/promise.h"

using allocation::BufferCollectionUsage;
using allocation::ImageMetadata;

namespace flatland {
namespace test {

// The smoke tests are used to ensure that we can get testing of the Flatland
// Display Compositor across a variety of test hardware configurations, including
// those that do not have a real display, and those where making sysmem buffer
// collection vmos host-accessible (i.e. cpu accessible) is not allowed, precluding
// the possibility of doing a pixel readback on the framebuffers.
class DisplayCompositorSmokeTest : public gtest::RealLoopFixture {
 protected:
  void SetUp() override {
    gtest::RealLoopFixture::SetUp();
    async_set_default_dispatcher(dispatcher());

    realm_root_ = testing::BuildFakeDisplayRealm(dispatcher(), testing::DisplayRealmConfig{});

    // Create the SysmemAllocator.
    // Create the SysmemAllocator.
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_sysmem2::Allocator>::Create();
    zx_status_t status =
        fdio_service_connect("/svc/fuchsia.sysmem2.Allocator", server_end.TakeChannel().release());
    EXPECT_EQ(status, ZX_OK);
    sysmem_allocator_.Bind(std::move(client_end), dispatcher());

    fidl::Arena arena;
    fidl::OneWayStatus result = sysmem_allocator_->SetDebugClientInfo(
        fuchsia_sysmem2::wire::AllocatorSetDebugClientInfoRequest::Builder(arena)
            .name(arena, fsl::GetCurrentProcessName() + " DisplayCompositorSmokeTest")
            .id(fsl::GetCurrentProcessKoid())
            .Build());
    EXPECT_TRUE(result.ok());
    executor_ = std::make_unique<async::Executor>(dispatcher());

    display_manager_ = std::make_unique<display::DisplayManager>([]() {});

    fidl::ClientEnd<fuchsia_io::Directory> svc_root(
        realm_root_->component().CloneExposedDir().TakeChannel());
    component::SyncServiceMemberWatcher<fuchsia_hardware_display::Service::Provider> watcher(
        svc_root.borrow());
    zx::result<fidl::ClientEnd<fuchsia_hardware_display::Provider>> provider_result =
        watcher.GetNextInstance(/*stop_at_idle=*/false);
    ASSERT_OK(provider_result);
    fidl::ClientEnd<fuchsia_hardware_display::Provider> provider =
        std::move(provider_result).value();

    fpromise::promise<display::CoordinatorClientChannels, zx_status_t> display_coordinator_promise =
        display::GetCoordinator(std::move(provider));
    executor_->schedule_task(display_coordinator_promise.then(
        [this](fpromise::result<display::CoordinatorClientChannels, zx_status_t>& client_channels) {
          ASSERT_TRUE(client_channels.is_ok()) << "Failed to get display coordinator:"
                                               << zx_status_get_string(client_channels.error());
          auto [coordinator_client, listener_server] = std::move(client_channels.value());
          display_manager_->BindDefaultDisplayCoordinator(
              dispatcher(), std::move(coordinator_client), std::move(listener_server));
        }));

    RunLoopUntil([this] { return display_manager_->default_display() != nullptr; });
  }

  void TearDown() override {
    RunLoopUntilIdle();
    executor_.reset();
    display_manager_.reset();
    gtest::RealLoopFixture::TearDown();
  }

  bool IsDisplaySupported(DisplayCompositor* display_compositor,
                          allocation::GlobalBufferCollectionId id) {
    std::scoped_lock lock(display_compositor->lock_);
    return display_compositor->buffer_collection_supports_display_[id];
  }

 protected:
  static constexpr fuchsia_images2::PixelFormat kPixelFormat =
      fuchsia_images2::PixelFormat::kB8G8R8A8;

  std::optional<component_testing::RealmRoot> realm_root_;
  fidl::WireClient<fuchsia_sysmem2::Allocator> sysmem_allocator_;
  std::unique_ptr<async::Executor> executor_;
  std::unique_ptr<display::DisplayManager> display_manager_;

  // Run promise on this test case's executor.
  // Return true if result is_ok().
  bool RunPromise(fpromise::promise<> promise) {
    return integration_tests::RunPromise(
        *executor_, [this](bool& done) { RunLoopUntil([&done] { return done; }); },
        std::move(promise));
  }

  static std::pair<std::unique_ptr<escher::Escher>, std::shared_ptr<flatland::VkRenderer>>
  NewVkRenderer() {
    auto env = escher::test::EscherEnvironment::GetGlobalTestEnvironment();
    auto unique_escher = std::make_unique<escher::Escher>(
        env->GetVulkanDevice(), env->GetFilesystem(), /*gpu_allocator*/ nullptr);
    return {std::move(unique_escher),
            std::make_shared<flatland::VkRenderer>(unique_escher->GetWeakPtr())};
  }

  static std::shared_ptr<flatland::NullRenderer> NewNullRenderer() {
    return std::make_shared<flatland::NullRenderer>();
  }

  // Sets up the buffer collection information for collections that will be imported
  // into the engine.
  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> SetupClientTextures(
      DisplayCompositor* display_compositor, allocation::GlobalBufferCollectionId collection_id,
      fuchsia_images2::PixelFormat pixel_format, uint32_t width, uint32_t height, uint32_t num_vmos,
      fuchsia_sysmem2::BufferCollectionInfo* collection_info) {
    // Setup the buffer collection that will be used for the flatland rectangle's texture.
    auto [local_token, dup_token] = SysmemTokens::Create(sysmem_allocator_);

    auto import_promise = display_compositor->ImportBufferCollection(
        collection_id, sysmem_allocator_, std::move(dup_token), BufferCollectionUsage::kClientImage,
        std::nullopt);

    bool import_success = RunPromise(std::move(import_promise));
    EXPECT_TRUE(import_success);

    auto [buffer_usage, memory_constraints] = GetUsageAndMemoryConstraintsForCpuWriteOften();
    fidl::SyncClient<fuchsia_sysmem2::BufferCollection> texture_collection =
        CreateBufferCollectionSyncPtrAndSetConstraints(
            sysmem_allocator_, std::move(local_token), num_vmos, width, height,
            std::move(buffer_usage), pixel_format,
            std::make_optional(std::move(memory_constraints)),
            std::make_optional(fuchsia_images2::PixelFormatModifier::kLinear));

    // Have the client wait for buffers allocated so it can populate its information
    // struct with the vmo data.
    auto wait_result = texture_collection->WaitForAllBuffersAllocated();
    EXPECT_TRUE(wait_result.is_ok());
    *collection_info = std::move(wait_result.value().buffer_collection_info().value());

    return texture_collection;
  }
};

class DisplayCompositorParameterizedSmokeTest
    : public DisplayCompositorSmokeTest,
      public ::testing::WithParamInterface<fuchsia_images2::PixelFormat> {};

namespace {

// Renders a fullscreen rectangle to the provided display. This tests the engine's ability to
// properly read in flatland uberstruct data and then pass the data along to the display-coordinator
// interface to be composited directly in hardware. The Astro display coordinator only handles full
// screen rects.
VK_TEST_P(DisplayCompositorParameterizedSmokeTest, FullscreenRectangleTest) {
  // Even though we are rendering directly with the display coordinator in this test,
  // we still use the VkRenderer so that all of the same constraints we'd expect to
  // see set in a real production setting are reproduced here.
  auto [escher, renderer] = NewVkRenderer();
  auto display_compositor = std::make_shared<flatland::DisplayCompositor>(
      dispatcher(), display_manager_->coordinator_proxy(), renderer,
      utils::CreateSysmemAllocatorClient(dispatcher(), "display_compositor_pixeltest"),
      flatland::DisplayCompositorConfig{});
  auto display = display_manager_->default_display();

  const uint64_t kTextureCollectionId = allocation::GenerateUniqueBufferCollectionId();

  // Setup the collection for the texture. Due to display coordinator limitations, the size of
  // the texture needs to match the size of the rect. So since we have a fullscreen rect, we
  // must also have a fullscreen texture to match.
  const uint32_t kRectWidth = display->width_in_px(), kTextureWidth = display->width_in_px();
  const uint32_t kRectHeight = display->height_in_px(), kTextureHeight = display->height_in_px();
  fuchsia_sysmem2::BufferCollectionInfo texture_collection_info;
  auto texture_collection =
      SetupClientTextures(display_compositor.get(), kTextureCollectionId, GetParam(), kTextureWidth,
                          kTextureHeight, 1, &texture_collection_info);
  EXPECT_TRUE(texture_collection.is_valid());
  auto release_texture_collection = fit::defer([display_compositor, kTextureCollectionId] {
    display_compositor->ReleaseBufferCollection(kTextureCollectionId,
                                                BufferCollectionUsage::kClientImage);
  });

  // Import the texture to the engine.
  auto image_metadata = ImageMetadata{.collection_id = kTextureCollectionId,
                                      .identifier = allocation::GenerateUniqueImageId(),
                                      .vmo_index = 0,
                                      .width = kTextureWidth,
                                      .height = kTextureHeight};
  EXPECT_TRUE(RunPromise(
      display_compositor->ImportBufferImage(image_metadata, BufferCollectionUsage::kClientImage)));

  // We cannot send to display because it is not supported in allocations.
  EXPECT_TRUE(IsDisplaySupported(display_compositor.get(), kTextureCollectionId));

  DisplayInfo display_info{
      .dimensions = glm::uvec2(display->width_in_px(), display->height_in_px()),
      .formats = {kPixelFormat}};
  display_compositor->AddDisplay(display, display_info, /*num_vmos*/ 0,
                                 /*out_collection_info*/ nullptr);

  ResolvedLayer layer = {
      .geometry =
          SrcToDest(types::RectangleF({.x = 0,
                                       .y = 0,
                                       .width = static_cast<float>(image_metadata.width),
                                       .height = static_cast<float>(image_metadata.height)}),
                    types::RectangleF({.x = 0,
                                       .y = 0,
                                       .width = static_cast<float>(kRectWidth),
                                       .height = static_cast<float>(kRectHeight)}),
                    types::RotateFlip::kIdentity()),
      .multiply_color = {1.f, 1.f, 1.f, 1.f},
      .blend_mode = BlendMode::kReplace(),
      .content = ResolvedLayer::ImageContent{.image_id = image_metadata.identifier,
                                             .width = image_metadata.width,
                                             .height = image_metadata.height},
  };
  RenderData render_data = {.display_id = display->display_id(),
                            .layers = std::span<const ResolvedLayer>(&layer, 1)};

  // Now we can finally render.
  display_compositor->RenderFrame(1, zx::time(1), std::span<const RenderData>(&render_data, 1), {},
                                  {}, {}, [](const scheduling::Timestamps&) {});
}

// TODO(https://fxbug.dev/42154038): Add YUV formats when they are supported by fake or real
// display.
INSTANTIATE_TEST_SUITE_P(PixelFormats, DisplayCompositorParameterizedSmokeTest,
                         ::testing::Values(fuchsia_images2::PixelFormat::kB8G8R8A8,
                                           fuchsia_images2::PixelFormat::kR8G8B8A8));

}  // namespace

}  // namespace test
}  // namespace flatland

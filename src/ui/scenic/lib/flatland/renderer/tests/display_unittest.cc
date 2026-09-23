// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.display.types/cpp/fidl.h>
#include <fidl/fuchsia.hardware.display/cpp/fidl.h>
#include <fidl/fuchsia.math/cpp/fidl.h>
#include <lib/async-testing/test_loop.h>
#include <lib/async/cpp/executor.h>
#include <lib/async/cpp/wait.h>
#include <lib/async/default.h>
#include <lib/component/incoming/cpp/service_member_watcher.h>
#include <lib/fdio/directory.h>
#include <lib/fit/defer.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>

#include "src/graphics/display/lib/coordinator-getter/client.h"
#include "src/lib/testing/loop_fixture/real_loop_fixture.h"
#include "src/lib/testing/predicates/status.h"
#include "src/ui/lib/escher/test/common/gtest_escher.h"
#include "src/ui/lib/escher/test/common/gtest_vulkan.h"
#include "src/ui/scenic/lib/allocation/buffer_collection_importer.h"
#include "src/ui/scenic/lib/allocation/id.h"
#include "src/ui/scenic/lib/display/display_manager.h"
#include "src/ui/scenic/lib/display/util.h"
#include "src/ui/scenic/lib/flatland/buffers/util.h"
#include "src/ui/scenic/lib/flatland/renderer/vk_renderer.h"
#include "src/ui/scenic/lib/flatland/testing/build_display_realm.h"
#include "src/ui/scenic/lib/utils/helpers.h"
#include "src/ui/scenic/tests/utils/promise.h"

#include <glm/gtc/constants.hpp>
#include <glm/gtx/matrix_transform_2d.hpp>

namespace {

class DisplayTest : public gtest::RealLoopFixture {
 protected:
  void SetUp() override {
    if (VK_TESTS_SUPPRESSED()) {
      return;
    }
    gtest::RealLoopFixture::SetUp();

    realm_root_ = flatland::testing::BuildFakeDisplayRealm(dispatcher(),
                                                           flatland::testing::DisplayRealmConfig{});

    sysmem_allocator_ = utils::CreateSysmemAllocatorClient(dispatcher(), "display_unittest::Setup");

    async_set_default_dispatcher(dispatcher());
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
    if (VK_TESTS_SUPPRESSED()) {
      return;
    }
    executor_.reset();
    display_manager_.reset();
    sysmem_allocator_ = {};
    gtest::RealLoopFixture::TearDown();
  }

  fidl::WireSharedClient<fuchsia_hardware_display::Coordinator>& raw_display_coordinator() {
    FX_CHECK(display_manager_->coordinator_proxy());
    return display_manager_->coordinator_proxy()->raw();
  }

  display::WireLayerId InitializeDisplayLayer(
      const fidl::WireSharedClient<fuchsia_hardware_display::Coordinator>& display_coordinator,
      display::Display* display) {
    display::WireLayerId layer_id = {.value = next_layer_id_++};
    const auto create_layer_result = display_coordinator.sync()->CreateLayer(layer_id);
    if (!create_layer_result.ok()) {
      FX_LOGS(ERROR) << "Failed to call FIDL CreateLayer: " << create_layer_result.status_string();
      return {.value = fuchsia_hardware_display_types::kInvalidDispId};
    }
    if (!create_layer_result->is_ok()) {
      FX_LOGS(ERROR) << "Failed to call CreateLayer: "
                     << zx_status_get_string(create_layer_result->error_value());
      return {.value = fuchsia_hardware_display_types::kInvalidDispId};
    }

    const auto set_display_layers_result = display_coordinator.sync()->SetDisplayLayers(
        display->display_id().ToFidl(),
        fidl::VectorView<fuchsia_hardware_display::wire::LayerId>::FromExternal(&layer_id, 1));
    if (!set_display_layers_result.ok()) {
      FX_LOGS(ERROR) << "Failed to call FIDL SetDisplayLayers: "
                     << set_display_layers_result.status_string();
      return {.value = fuchsia_hardware_display_types::kInvalidDispId};
    }
    return layer_id;
  }

  // Wait until a vsync is received with a stamp that is >= `target_stamp`.  Return ZX_ERR_TIMED_OUT
  // if no such vsync is received before `timeout` elapses.
  zx::result<> WaitForVsync(display::WireConfigStamp target_stamp, zx::duration timeout) {
    std::optional<display::WireConfigStamp> received_stamp;
    auto vsync_callback_id = display_manager_->default_display()->AddVsyncCallback(
        [&](zx::time, display::WireConfigStamp displayed_config_stamp) {
          received_stamp = displayed_config_stamp;
        });

    bool success = RunLoopWithTimeoutOrUntil(
        [&]() { return received_stamp && received_stamp->value >= target_stamp.value; }, timeout);

    display_manager_->default_display()->RemoveVsyncCallback(vsync_callback_id);

    if (success) {
      return zx::ok();
    }
    return zx::error(ZX_ERR_TIMED_OUT);
  }

  // Run promise on this test case's executor.
  // Return true if result is_ok().
  bool RunPromise(fpromise::promise<> promise) {
    return integration_tests::RunPromise(
        *executor_, [this](bool& done) { RunLoopUntil([&done] { return done; }); },
        std::move(promise));
  }

  std::optional<component_testing::RealmRoot> realm_root_;
  std::unique_ptr<async::Executor> executor_;
  std::unique_ptr<display::DisplayManager> display_manager_;
  fidl::WireClient<fuchsia_sysmem2::Allocator> sysmem_allocator_;
  uint64_t next_layer_id_ = 100;
};

// Create a buffer collection and set constraints on the display, the vulkan renderer
// and the client, and make sure that the collection is still properly allocated.
VK_TEST_F(DisplayTest, SetAllConstraintsTest) {
  const uint64_t kWidth = 8;
  const uint64_t kHeight = 16;

  // Grab the display coordinator.
  auto& display_coordinator = raw_display_coordinator();

  // Create the VK renderer.
  auto env = escher::test::EscherEnvironment::GetGlobalTestEnvironment();
  auto unique_escher = std::make_unique<escher::Escher>(
      env->GetVulkanDevice(), env->GetFilesystem(), /*gpu_allocator*/ nullptr);
  flatland::VkRenderer renderer(unique_escher->GetWeakPtr());

  // First create the pair of sysmem tokens, one for the client, one for the renderer.
  auto [local_token, dup_token] = flatland::SysmemTokens::Create(sysmem_allocator_);
  auto [client_end, server_end] = fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();

  fidl::Arena arena;
  auto result =
      fidl::WireCall(local_token)
          ->Duplicate(fuchsia_sysmem2::wire::BufferCollectionTokenDuplicateRequest::Builder(arena)
                          .rights_attenuation_mask(ZX_RIGHT_SAME_RIGHTS)
                          .token_request(std::move(server_end))
                          .Build());
  FX_DCHECK(result.ok());

  // Register the collection with the renderer, which sets the vk constraints.
  const auto collection_id = allocation::GenerateUniqueBufferCollectionId();
  const display::WireBufferCollectionId display_collection_id =
      display::ToDisplayFidlBufferCollectionId(collection_id);
  auto image_id = allocation::GenerateUniqueImageId();
  auto import_promise = renderer.ImportBufferCollection(
      collection_id, sysmem_allocator_, std::move(dup_token),
      allocation::BufferCollectionUsage::kClientImage, std::nullopt);
  EXPECT_TRUE(RunPromise(std::move(import_promise)));

  allocation::ImageMetadata metadata = {.collection_id = collection_id,
                                        .identifier = image_id,
                                        .vmo_index = 0,
                                        .width = kWidth,
                                        .height = kHeight};

  // Importing an image should fail at this point because we've only set the renderer constraints.
  {
    auto import_image_promise =
        renderer.ImportBufferImage(metadata, allocation::BufferCollectionUsage::kClientImage);
    EXPECT_FALSE(RunPromise(std::move(import_image_promise)));
  }

  // Set the display constraints on the display coordinator.
  fuchsia_hardware_display_types::wire::ImageBufferUsage image_buffer_usage = {
      .tiling_type = fuchsia_hardware_display_types::kImageTilingTypeLinear,
  };
  bool res = display::ImportBufferCollection(collection_id, display_coordinator,
                                             std::move(client_end), image_buffer_usage);
  ASSERT_TRUE(res);
  auto release_buffer_collection = fit::defer([&display_coordinator, display_collection_id] {
    // Release the buffer collection.
    const fidl::OneWayStatus release_buffer_collection_result =
        display_coordinator.sync()->ReleaseBufferCollection(display_collection_id);
    EXPECT_TRUE(release_buffer_collection_result.ok())
        << "Failed to call FIDL ReleaseBufferCollection: "
        << release_buffer_collection_result.status_string();
  });

  // Importing should fail again, because we've only set 2 of the 3 constraints.
  {
    auto import_image_promise =
        renderer.ImportBufferImage(metadata, allocation::BufferCollectionUsage::kClientImage);
    EXPECT_FALSE(RunPromise(std::move(import_image_promise)));
  }

  // Create a client-side handle to the buffer collection and set the client constraints.
  auto client_collection = flatland::CreateBufferCollectionSyncPtrAndSetConstraints(
      sysmem_allocator_, std::move(local_token),
      /*image_count*/ 1,
      /*width*/ kWidth,
      /*height*/ kHeight,
      /*usage*/ flatland::get_none_usage(), fuchsia_images2::PixelFormat::kB8G8R8A8,
      /*memory_constraints*/ std::nullopt,
      std::make_optional(fuchsia_images2::PixelFormatModifier::kLinear));

  // Have the client wait for buffers allocated so it can populate its information
  // struct with the vmo data.
  fuchsia_sysmem2::BufferCollectionInfo client_collection_info;
  {
    auto wait_result = client_collection->WaitForAllBuffersAllocated();
    EXPECT_TRUE(wait_result.is_ok());
    client_collection_info = std::move(wait_result->buffer_collection_info().value());
  }

  // Now that the renderer, client, and the display have set their constraints, we import one last
  // time and this time it should return true.
  {
    auto import_image_promise =
        renderer.ImportBufferImage(metadata, allocation::BufferCollectionUsage::kClientImage);
    EXPECT_TRUE(RunPromise(std::move(import_image_promise)));
  }

  // We should now be able to also import an image to the display coordinator, using the
  // display-specific buffer collection id. If it returns OK, then we know that the renderer
  // did fully set the DC constraints.
  display::WireImageMetadata image_metadata{
      .dimensions = display::WireSizeU{.width = kWidth, .height = kHeight},
      .tiling_type = fuchsia_hardware_display_types::kImageTilingTypeLinear,
  };

  // Try to import the image into the display coordinator API and make sure it succeeds.
  allocation::GlobalImageId display_image_id = allocation::GenerateUniqueImageId();

  const auto import_image_result = display_coordinator.sync()->ImportImage(
      image_metadata, display_collection_id, 0, display_image_id.ToFidl());
  ASSERT_TRUE(import_image_result.ok())
      << "Failed to call FIDL ImportImage: " << import_image_result.status_string();
  EXPECT_TRUE(import_image_result->is_ok())
      << "Failed to call ImportImage: " << zx_status_get_string(import_image_result->error_value());
}

// Test out event signaling on the Display Coordinator by importing a buffer collection and its 2
// images, setting the first image to a display layer with a signal event, and
// then setting the second image on the layer which has a wait event. When the wait event is
// signaled, this will cause the second layer image to go up, which in turn will cause the first
// layer image's event to be signaled.
// TODO(https://fxbug.dev/42132767): Check to see if there is a more appropriate place to test
// display coordinator events and/or if there already exist adequate tests that cover all of the use
// cases being covered by this test.
VK_TEST_F(DisplayTest, SetDisplayImageTest) {
  // Grab the display coordinator.
  auto& display_coordinator = raw_display_coordinator();

  auto display = display_manager_->default_display();
  ASSERT_TRUE(display);

  display::WireLayerId layer_id = InitializeDisplayLayer(display_coordinator, display);
  ASSERT_NE(layer_id.value, fuchsia_hardware_display_types::kInvalidDispId);

  const uint32_t kWidth = display->width_in_px();
  const uint32_t kHeight = display->height_in_px();
  const uint32_t kNumVmos = 2;

  // First create the pair of sysmem tokens, one for the client, one for the display.
  auto [local_token, dup_token] = flatland::SysmemTokens::Create(sysmem_allocator_);

  // Set the display constraints on the display coordinator.
  fuchsia_hardware_display_types::wire::ImageBufferUsage image_buffer_usage = {
      .tiling_type = fuchsia_hardware_display_types::kImageTilingTypeLinear,
  };
  auto global_collection_id = allocation::GenerateUniqueBufferCollectionId();
  ASSERT_NE(global_collection_id, ZX_KOID_INVALID);
  const display::WireBufferCollectionId display_collection_id =
      display::ToDisplayFidlBufferCollectionId(global_collection_id);

  bool res = display::ImportBufferCollection(global_collection_id, display_coordinator,
                                             std::move(dup_token), image_buffer_usage);
  ASSERT_TRUE(res);

  flatland::SetClientConstraintsAndWaitForAllocated(sysmem_allocator_, std::move(local_token),
                                                    kNumVmos, kWidth, kHeight);

  // Import the images to the display.
  display::WireImageMetadata image_metadata{
      .dimensions = display::WireSizeU{.width = kWidth, .height = kHeight},
      .tiling_type = fuchsia_hardware_display_types::kImageTilingTypeLinear,
  };
  allocation::GlobalImageId image_ids[kNumVmos];
  for (uint32_t i = 0; i < kNumVmos; i++) {
    image_ids[i] = allocation::GenerateUniqueImageId();
    const auto import_image_result = display_coordinator.sync()->ImportImage(
        image_metadata, display_collection_id, i, image_ids[i].ToFidl());
    ASSERT_TRUE(import_image_result.ok())
        << "Failed to call FIDL ImportImage: " << import_image_result.status_string();
    ASSERT_TRUE(import_image_result->is_ok())
        << "Failed to call ImportImage: "
        << zx_status_get_string(import_image_result->error_value());
    ASSERT_NE(image_ids[i], display::kInvalidImageId);
  }

  // It is safe to release buffer collection because we are not going to import any more images.
  const fidl::OneWayStatus release_result =
      display_coordinator.sync()->ReleaseBufferCollection(display_collection_id);
  EXPECT_TRUE(release_result.ok())
      << "Failed to call FIDL ReleaseBufferCollection: " << release_result.status_string();

  // Create the events used by the display.
  zx::event display_wait_fence;
  auto status = zx::event::create(0, &display_wait_fence);
  EXPECT_EQ(status, ZX_OK);

  // Import the above events to the display.
  display::EventId display_wait_event_id =
      display::ImportEventForTest(display_coordinator, display_wait_fence);
  EXPECT_NE(display_wait_event_id, display::kInvalidEventId);

  // Set the layer image and apply the config.
  const fidl::OneWayStatus set_layer_primary_config_result =
      display_coordinator.sync()->SetLayerPrimaryConfig(layer_id, image_metadata);
  EXPECT_TRUE(set_layer_primary_config_result.ok())
      << "Failed to call FIDL SetLayerPrimaryConfig: "
      << set_layer_primary_config_result.status_string();

  static const display::WireEventId kInvalidEventId = {
      .value = fuchsia_hardware_display_types::kInvalidDispId,
  };
  const fidl::OneWayStatus set_layer_image_result =
      display_coordinator.sync()->SetLayerImage2(layer_id, image_ids[0].ToFidl(), kInvalidEventId);
  EXPECT_TRUE(set_layer_image_result.ok())
      << "Failed to call FIDL SetLayerImage2: " << set_layer_image_result.status_string();

  // Apply the config.
  const auto check_config_result = display_coordinator.sync()->CheckConfig();
  EXPECT_TRUE(check_config_result.ok())
      << "Failed to call FIDL CheckConfig: " << check_config_result.status_string();

  const display::WireConfigStamp kFirstConfigStamp(11);
  {
    fidl::Arena arena;
    auto request = fuchsia_hardware_display::wire::CoordinatorCommitConfigRequest::Builder(arena)
                       .stamp(kFirstConfigStamp)
                       .Build();
    const fidl::OneWayStatus result = display_coordinator.sync()->CommitConfig(request);
    EXPECT_TRUE(result.ok()) << "Failed to call FIDL CommitConfig: " << result.status_string();
  }

  // Wait for the first Vsync.  This should arrive because there is no wait fence to block
  // application of the config.
  zx::result vsync_result = WaitForVsync(kFirstConfigStamp, zx::msec(3000));
  EXPECT_TRUE(vsync_result.is_ok())
      << "first WaitForVsync() failed with status: " << vsync_result.status_string();

  // Set the layer image again, to the second image, so that our first call to SetLayerImage2()
  // above will signal.
  const fidl::OneWayStatus set_layer_image_result2 = display_coordinator.sync()->SetLayerImage2(
      layer_id, image_ids[1].ToFidl(), display_wait_event_id.ToFidl());
  EXPECT_TRUE(set_layer_image_result2.ok())
      << "Failed to call FIDL SetLayerImage2: " << set_layer_image_result2.status_string();

  // Apply the config to display the second image.
  const auto check_config_result2 = display_coordinator.sync()->CheckConfig();
  ASSERT_TRUE(check_config_result2.ok())
      << "Failed to call FIDL CheckConfig: " << check_config_result2.status_string();
  EXPECT_EQ(check_config_result2->res, fuchsia_hardware_display_types::ConfigResult::kOk);

  const display::WireConfigStamp kSecondConfigStamp(22);
  {
    fidl::Arena arena;
    auto request = fuchsia_hardware_display::wire::CoordinatorCommitConfigRequest::Builder(arena)
                       .stamp(kSecondConfigStamp)
                       .Build();
    const fidl::OneWayStatus result = display_coordinator->CommitConfig(request);
    EXPECT_TRUE(result.ok()) << "Failed to call FIDL CommitConfig: " << result.status_string();
  }

  // Wait for the second Vsync.  This won't come, because the display coordinator will block on the
  // wait fence.
  vsync_result = WaitForVsync(kSecondConfigStamp, zx::msec(3000));
  EXPECT_TRUE(vsync_result.is_error() && vsync_result.status_value() == ZX_ERR_TIMED_OUT)
      << "second WaitForVsync() unexpected status: " << vsync_result.status_string();

  // Now we signal wait on the second layer.
  display_wait_fence.signal(0, ZX_EVENT_SIGNALED);

  // Now we that the wait fence has been signaled, we should receive a vsync corresponding to the
  // second config.
  vsync_result = WaitForVsync(kSecondConfigStamp, zx::msec(3000));
  EXPECT_TRUE(vsync_result.is_ok())
      << "final WaitForVsync() failed with status: " << vsync_result.status_string();
}

}  // namespace

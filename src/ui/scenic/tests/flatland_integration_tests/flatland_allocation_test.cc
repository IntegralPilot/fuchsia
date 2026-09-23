// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.images2/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <fidl/fuchsia.ui.views/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/ui/scenic/cpp/buffer_collection_import_export_tokens.h>
#include <lib/ui/scenic/cpp/view_creation_tokens.h>
#include <lib/ui/scenic/cpp/view_identity.h>

#include <cstdint>
#include <memory>
#include <thread>
#include <vector>

#include <zxtest/zxtest.h>

#include "src/ui/scenic/lib/utils/helpers.h"
#include "src/ui/scenic/tests/utils/blocking_present.h"
#include "src/ui/scenic/tests/utils/scenic_ctf_test_base.h"

namespace integration_tests {

namespace fuc = fuchsia_ui_composition;
namespace fuv = fuchsia_ui_views;

constexpr auto kDefaultSize = 128;
const fuc::TransformId kRootTransform(1);

fuchsia_sysmem2::BufferCollectionConstraints GetDefaultBufferConstraints() {
  fuchsia_sysmem2::BufferCollectionConstraints constraints;
  auto& bmc = constraints.buffer_memory_constraints().emplace();
  bmc.ram_domain_supported(true);
  bmc.cpu_domain_supported(true);
  constraints.usage().emplace().cpu(fuchsia_sysmem2::kCpuUsageRead);
  constraints.min_buffer_count(1);
  auto& image_constraints = constraints.image_format_constraints().emplace().emplace_back();
  image_constraints.pixel_format(fuchsia_images2::PixelFormat::kB8G8R8A8);
  image_constraints.pixel_format_modifier(fuchsia_images2::PixelFormatModifier::kLinear);
  image_constraints.color_spaces().emplace().emplace_back(fuchsia_images2::ColorSpace::kSrgb);
  image_constraints.required_min_size(
      fuchsia_math::SizeU{{.width = kDefaultSize, .height = kDefaultSize}});
  image_constraints.required_max_size(
      fuchsia_math::SizeU{{.width = kDefaultSize, .height = kDefaultSize}});
  return constraints;
}

// Test fixture that sets up an environment with a Scenic we can connect to.
class AllocationTest : public ScenicCtfTest {
 public:
  void SetUp() override {
    ScenicCtfTest::SetUp();

    auto [client_end, server_end] = fidl::Endpoints<fuchsia_sysmem2::Allocator>::Create();
    ASSERT_EQ(ZX_OK, LocalServiceDirectory()->Connect(fuchsia_sysmem2::Allocator::kDiscoverableName,
                                                      server_end.TakeChannel()));
    sysmem_allocator_.Bind(std::move(client_end), dispatcher());

    // Create a root Flatland.
    root_flatland_ = std::make_unique<FlatlandClientWithEventHandler>(
        ConnectIntoRealm<fuc::Flatland>(), dispatcher());

    // Attach |root_flatland_| as the only Flatland under the environment's FlatlandDisplay.
    auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
    SetFlatlandDisplayContent(std::move(parent_token));
    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
    ASSERT_TRUE((*root_flatland_)
                    ->CreateView2({{.token = std::move(child_token),
                                    .view_identity = scenic::cpp::NewViewIdentityOnCreation(),
                                    .protocols = {},
                                    .parent_viewport_watcher =
                                        std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());
    ASSERT_TRUE((*root_flatland_)->CreateTransform(kRootTransform).is_ok());
    ASSERT_TRUE((*root_flatland_)->SetRootTransform(kRootTransform).is_ok());
  }

  void TearDown() override {
    root_flatland_.reset();

    ScenicCtfTest::TearDown();
  }

 protected:
  fuchsia_sysmem2::BufferCollectionInfo SetConstraintsAndAllocateBuffer(
      fidl::WireClient<fuchsia_sysmem2::Allocator>& sysmem_allocator,
      fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token,
      fuchsia_sysmem2::BufferCollectionConstraints constraints) {
    auto [buffer_collection_client_end, buffer_collection_server_end] =
        fidl::CreateEndpoints<fuchsia_sysmem2::BufferCollection>().value();
    fidl::SyncClient<fuchsia_sysmem2::BufferCollection> buffer_collection(
        std::move(buffer_collection_client_end));
    fidl::Arena arena;
    fidl::OneWayStatus result = sysmem_allocator->BindSharedCollection(
        fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
            .token(std::move(token))
            .buffer_collection_request(std::move(buffer_collection_server_end))
            .Build());
    FX_CHECK(result.ok());

    uint32_t constraints_min_buffer_count = constraints.min_buffer_count().value();

    fuchsia_sysmem2::BufferCollectionSetConstraintsRequest set_constraints_request;
    set_constraints_request.constraints(std::move(constraints));
    auto set_res = buffer_collection->SetConstraints(std::move(set_constraints_request));
    FX_CHECK(set_res.is_ok());

    auto wait_result = buffer_collection->WaitForAllBuffersAllocated();
    FX_CHECK(wait_result.is_ok());
    auto buffer_collection_info = std::move(wait_result.value().buffer_collection_info().value());
    EXPECT_EQ(constraints_min_buffer_count, buffer_collection_info.buffers()->size());
    FX_CHECK(buffer_collection->Release().is_ok());
    return buffer_collection_info;
  }

  fidl::WireClient<fuchsia_sysmem2::Allocator> sysmem_allocator_;
  std::unique_ptr<FlatlandClientWithEventHandler> root_flatland_;
};

TEST_F(AllocationTest, CreateAndReleaseImage) {
  fidl::SyncClient flatland_allocator = ConnectSyncIntoRealm<fuc::Allocator>();

  auto [local_token, scenic_token] = utils::SysmemTokens::Create(sysmem_allocator_);

  // Send one token to Flatland Allocator.
  allocation::cpp::BufferCollectionImportExportTokens bc_tokens =
      allocation::cpp::BufferCollectionImportExportTokens::New();
  fuc::RegisterBufferCollectionArgs rbc_args = {};
  rbc_args.export_token(std::move(bc_tokens.export_token));
  rbc_args.buffer_collection_token2(std::move(scenic_token));
  ASSERT_TRUE(flatland_allocator->RegisterBufferCollection(std::move(rbc_args)).is_ok());

  // Use the local token to set constraints.
  auto info = SetConstraintsAndAllocateBuffer(sysmem_allocator_, std::move(local_token),
                                              GetDefaultBufferConstraints());

  fuc::ImageProperties image_properties = {};
  image_properties.size(fuchsia_math::SizeU{{.width = kDefaultSize, .height = kDefaultSize}});
  const fuc::ContentId kImageContentId(1);

  ASSERT_TRUE((*root_flatland_)
                  ->CreateImage({{.image_id = kImageContentId,
                                  .import_token = std::move(bc_tokens.import_token),
                                  .vmo_index = 0,
                                  .properties = std::move(image_properties)}})
                  .is_ok());
  ASSERT_TRUE((*root_flatland_)
                  ->SetContent({{.transform_id = kRootTransform, .content_id = kImageContentId}})
                  .is_ok());
  BlockingPresent(this, *root_flatland_);

  // Release image and remove content to actually deallocate.
  ASSERT_TRUE((*root_flatland_)->ReleaseImage(kImageContentId).is_ok());
  ASSERT_TRUE((*root_flatland_)
                  ->SetContent({{.transform_id = kRootTransform, .content_id = fuc::ContentId(0)}})
                  .is_ok());
  BlockingPresent(this, *root_flatland_);
}

TEST_F(AllocationTest, CreateAndReleaseMultipleImages) {
  const auto kImageCount = 3;
  fidl::SyncClient flatland_allocator = ConnectSyncIntoRealm<fuc::Allocator>();

  for (uint64_t i = 1; i <= kImageCount; ++i) {
    auto [local_token, scenic_token] = utils::SysmemTokens::Create(sysmem_allocator_);

    // Send one token to root_flatland_ Allocator.
    allocation::cpp::BufferCollectionImportExportTokens bc_tokens =
        allocation::cpp::BufferCollectionImportExportTokens::New();
    fuc::RegisterBufferCollectionArgs rbc_args = {};
    rbc_args.export_token(std::move(bc_tokens.export_token));
    rbc_args.buffer_collection_token2(std::move(scenic_token));
    ASSERT_TRUE(flatland_allocator->RegisterBufferCollection(std::move(rbc_args)).is_ok());

    // Use the local token to set constraints.
    auto info = SetConstraintsAndAllocateBuffer(sysmem_allocator_, std::move(local_token),
                                                GetDefaultBufferConstraints());

    fuc::ImageProperties image_properties = {};
    image_properties.size(fuchsia_math::SizeU{{.width = kDefaultSize, .height = kDefaultSize}});
    const fuc::ContentId kImageContentId(i);
    ASSERT_TRUE((*root_flatland_)
                    ->CreateImage({{.image_id = kImageContentId,
                                    .import_token = std::move(bc_tokens.import_token),
                                    .vmo_index = 0,
                                    .properties = std::move(image_properties)}})
                    .is_ok());
    const fuc::TransformId kImageTransformId(i + 1);
    ASSERT_TRUE((*root_flatland_)->CreateTransform(kImageTransformId).is_ok());
    ASSERT_TRUE(
        (*root_flatland_)
            ->SetContent({{.transform_id = kImageTransformId, .content_id = kImageContentId}})
            .is_ok());
    ASSERT_TRUE((*root_flatland_)
                    ->AddChild({{.parent_transform_id = kRootTransform,
                                 .child_transform_id = kImageTransformId}})
                    .is_ok());
  }
  BlockingPresent(this, *root_flatland_);

  for (uint64_t i = 1; i <= kImageCount; ++i) {
    // Release image and remove content to actually deallocate.
    const fuc::ContentId kImageContentId(i);
    ASSERT_TRUE((*root_flatland_)->ReleaseImage(kImageContentId).is_ok());
    const fuc::TransformId kImageTransformId(i + 1);
    ASSERT_TRUE((*root_flatland_)
                    ->RemoveChild({{.parent_transform_id = kRootTransform,
                                    .child_transform_id = kImageTransformId}})
                    .is_ok());
    ASSERT_TRUE((*root_flatland_)->ReleaseTransform(kImageTransformId).is_ok());
  }
  BlockingPresent(this, *root_flatland_);
}

TEST_F(AllocationTest, MultipleClientsCreateAndReleaseImages) {
  const auto kClientCount = 8;

  // Add Viewports for as many as kClientCount.
  std::vector<fuv::ViewCreationToken> view_creation_tokens;
  for (uint64_t i = 1; i <= kClientCount; ++i) {
    auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
    view_creation_tokens.emplace_back(std::move(child_token));
    auto [child_view_watcher_client_end, child_view_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ChildViewWatcher>().value();
    fuc::ViewportProperties properties;
    properties.logical_size(fuchsia_math::SizeU{{.width = kDefaultSize, .height = kDefaultSize}});
    const fuc::ContentId kViewportContentId(i);
    ASSERT_TRUE(
        (*root_flatland_)
            ->CreateViewport({{.viewport_id = kViewportContentId,
                               .token = std::move(parent_token),
                               .properties = std::move(properties),
                               .child_view_watcher = std::move(child_view_watcher_server_end)}})
            .is_ok());
    const fuc::TransformId kViewportTransformId(i + 1);
    ASSERT_TRUE((*root_flatland_)->CreateTransform(kViewportTransformId).is_ok());
    ASSERT_TRUE((*root_flatland_)
                    ->AddChild({{.parent_transform_id = kRootTransform,
                                 .child_transform_id = kViewportTransformId}})
                    .is_ok());
  }
  BlockingPresent(this, *root_flatland_);

  std::vector<std::thread> threads;
  for (uint64_t i = 0; i < kClientCount; ++i) {
    threads.emplace_back([this, i, &view_creation_tokens]() {
      ui_testing::LoggingEventLoop present_loop;
      fidl::SyncClient flatland_allocator = ConnectSyncIntoRealm<fuc::Allocator>();

      auto [client_end, server_end] = fidl::Endpoints<fuchsia_sysmem2::Allocator>::Create();
      ASSERT_EQ(ZX_OK,
                LocalServiceDirectory()->Connect(fuchsia_sysmem2::Allocator::kDiscoverableName,
                                                 server_end.TakeChannel()));
      fidl::WireClient<fuchsia_sysmem2::Allocator> thread_sysmem_allocator;
      thread_sysmem_allocator.Bind(std::move(client_end), present_loop.dispatcher());

      auto [local_token, scenic_token] = utils::SysmemTokens::Create(thread_sysmem_allocator);

      // Send one token to Flatland Allocator.
      allocation::cpp::BufferCollectionImportExportTokens bc_tokens =
          allocation::cpp::BufferCollectionImportExportTokens::New();
      fuc::RegisterBufferCollectionArgs rbc_args = {};
      rbc_args.export_token(std::move(bc_tokens.export_token));
      rbc_args.buffer_collection_token2(std::move(scenic_token));
      ASSERT_TRUE(flatland_allocator->RegisterBufferCollection(std::move(rbc_args)).is_ok());

      // Use the local token to set constraints.
      auto info = SetConstraintsAndAllocateBuffer(thread_sysmem_allocator, std::move(local_token),
                                                  GetDefaultBufferConstraints());

      FlatlandClientWithEventHandler flatland(ConnectIntoRealm<fuc::Flatland>(),
                                              present_loop.dispatcher());
      auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
          fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
      ASSERT_TRUE(flatland
                      ->CreateView2({{.token = std::move(view_creation_tokens[i]),
                                      .view_identity = scenic::cpp::NewViewIdentityOnCreation(),
                                      .protocols = {},
                                      .parent_viewport_watcher =
                                          std::move(parent_viewport_watcher_server_end)}})
                      .is_ok());
      ASSERT_TRUE(flatland->CreateTransform(kRootTransform).is_ok());
      ASSERT_TRUE(flatland->SetRootTransform(kRootTransform).is_ok());

      fuc::ImageProperties image_properties;
      image_properties.size(fuchsia_math::SizeU{{.width = kDefaultSize, .height = kDefaultSize}});
      const fuc::ContentId kImageContentId(1);
      ASSERT_TRUE(flatland
                      ->CreateImage({{.image_id = kImageContentId,
                                      .import_token = std::move(bc_tokens.import_token),
                                      .vmo_index = 0,
                                      .properties = std::move(image_properties)}})
                      .is_ok());
      // Make each overlapping child slightly smaller, so all Images are visible.
      const auto size = kDefaultSize - static_cast<uint32_t>(i);
      ASSERT_TRUE(flatland
                      ->SetImageDestinationSize({{.image_id = kImageContentId,
                                                  .size = {{.width = size, .height = size}}}})
                      .is_ok());
      ASSERT_TRUE(
          flatland->SetContent({{.transform_id = kRootTransform, .content_id = kImageContentId}})
              .is_ok());
      BlockingPresent(&present_loop, flatland);

      // Release image and remove content to actually deallocate.
      ASSERT_TRUE(flatland->ReleaseImage(kImageContentId).is_ok());
      ASSERT_TRUE(
          flatland->SetContent({{.transform_id = kRootTransform, .content_id = fuc::ContentId(0)}})
              .is_ok());
      BlockingPresent(&present_loop, flatland);
    });
  }
  for (auto& thread : threads) {
    thread.join();
  }
}

}  // namespace integration_tests

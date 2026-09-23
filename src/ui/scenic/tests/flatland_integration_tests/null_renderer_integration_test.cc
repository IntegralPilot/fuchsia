// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/ui/scenic/cpp/buffer_collection_import_export_tokens.h>
#include <lib/ui/scenic/cpp/view_creation_tokens.h>
#include <lib/ui/scenic/cpp/view_identity.h>

#include <zxtest/zxtest.h>

#include "src/ui/scenic/lib/utils/helpers.h"
#include "src/ui/scenic/tests/utils/blocking_present.h"
#include "src/ui/scenic/tests/utils/scenic_ctf_test_base.h"
#include "src/ui/scenic/tests/utils/utils.h"
#include "src/ui/testing/util/screenshot_helper.h"

namespace integration_tests {

namespace fuc {

using fuchsia_ui_composition::Allocator;
using fuchsia_ui_composition::ContentId;
using fuchsia_ui_composition::Flatland;
using fuchsia_ui_composition::ParentViewportWatcher;
using fuchsia_ui_composition::Screenshot;
using fuchsia_ui_composition::TransformId;

}  // namespace fuc

using ui_testing::Screenshot;

namespace {
const fuc::TransformId kRootTransform(1);
}  // namespace

class NullRendererIntegrationTest : public ScenicCtfTest {
 public:
  void SetUp() override {
    ScenicCtfTest::SetUp();

    // Set up `sysmem_allocator_`.
    auto [client_end, server_end] = fidl::CreateEndpoints<fuchsia_sysmem2::Allocator>().value();
    sysmem_allocator_.Bind(std::move(client_end), dispatcher());
    ASSERT_EQ(ZX_OK, LocalServiceDirectory()->Connect(fuchsia_sysmem2::Allocator::kDiscoverableName,
                                                      server_end.TakeChannel()));

    flatland_allocator_ = ConnectSyncIntoRealm<fuc::Allocator>();
    root_flatland_ = std::make_unique<FlatlandClientWithEventHandler>(
        ConnectIntoRealm<fuc::Flatland>(), dispatcher());

    // Attach |root_flatland_| as the only Flatland under the environment's FlatlandDisplay.
    auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();

    SetFlatlandDisplayContent(std::move(parent_token));
    ASSERT_TRUE(root_flatland()
                    ->CreateView2({{.token = std::move(child_token),
                                    .view_identity = scenic::cpp::NewViewIdentityOnCreation(),
                                    .protocols = {},
                                    .parent_viewport_watcher =
                                        std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());

    fidl::Client<fuc::ParentViewportWatcher> parent_viewport_watcher(
        std::move(parent_viewport_watcher_client_end), dispatcher());

    // Get the display's width and height. Since there is no Present in FlatlandDisplay, receiving
    // this callback ensures that all FlatlandDisplay calls are processed.
    std::optional<fuchsia_ui_composition::LayoutInfo> info;
    parent_viewport_watcher->GetLayout().Then(
        [&info](fidl::Result<fuchsia_ui_composition::ParentViewportWatcher::GetLayout>& result) {
          ASSERT_TRUE(result.is_ok());
          info = result.value().info();
        });
    RunLoopUntil([&info] { return info.has_value(); });
    display_width_ = info->logical_size()->width();
    display_height_ = info->logical_size()->height();

    screenshotter_ = ConnectSyncIntoRealm<fuchsia_ui_composition::Screenshot>();
  }

 protected:
  void SetConstraintsAndAllocateBuffer(
      fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token) {
    auto [buffer_collection_client_end, buffer_collection_server_end] =
        fidl::CreateEndpoints<fuchsia_sysmem2::BufferCollection>().value();

    fidl::SyncClient<fuchsia_sysmem2::BufferCollection> buffer_collection(
        std::move(buffer_collection_client_end));

    fidl::Arena arena;
    ASSERT_TRUE(sysmem_allocator_.sync()
                    ->BindSharedCollection(
                        fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
                            .token(std::move(token))
                            .buffer_collection_request(std::move(buffer_collection_server_end))
                            .Build())
                    .ok());

    // Used to set constraint, and also as test expectation value.
    constexpr uint32_t kMinBufferCount = 1;

    fuchsia_sysmem2::BufferCollectionSetConstraintsRequest set_constraints_request;
    auto& constraints = set_constraints_request.constraints().emplace();
    constraints.usage().emplace().none() = fuchsia_sysmem2::kNoneUsage;
    constraints.min_buffer_count() = kMinBufferCount;
    auto& image_constraints = constraints.image_format_constraints().emplace().emplace_back();
    image_constraints.pixel_format() = fuchsia_images2::PixelFormat::kB8G8R8A8;
    image_constraints.color_spaces().emplace().emplace_back(fuchsia_images2::ColorSpace::kSrgb);
    image_constraints.required_min_size().emplace().width(display_width_).height(display_height_);
    image_constraints.required_max_size().emplace().width(display_width_).height(display_height_);

    ASSERT_TRUE(buffer_collection->SetConstraints(std::move(set_constraints_request)).is_ok());

    auto result = buffer_collection->WaitForAllBuffersAllocated();
    ASSERT_TRUE(result.is_ok());
    auto& info = result.value().buffer_collection_info().value();
    ASSERT_TRUE(info.buffers().has_value());
    EXPECT_EQ(kMinBufferCount, info.buffers().value().size());

    ASSERT_TRUE(buffer_collection->Release().is_ok());
  }

  FlatlandClientWithEventHandler& root_flatland() {
    FX_CHECK(root_flatland_);
    return *root_flatland_;
  }

  uint32_t display_width_ = 0;
  uint32_t display_height_ = 0;

  fidl::WireClient<fuchsia_sysmem2::Allocator> sysmem_allocator_;
  fidl::SyncClient<fuc::Allocator> flatland_allocator_;
  std::unique_ptr<FlatlandClientWithEventHandler> root_flatland_;
  fidl::SyncClient<fuc::Screenshot> screenshotter_;
};

TEST_F(NullRendererIntegrationTest, RendersContent) {
  auto [local_token, scenic_token] = utils::SysmemTokens::Create(sysmem_allocator_);

  // Send one token to Flatland Allocator.
  allocation::cpp::BufferCollectionImportExportTokens bc_tokens =
      allocation::cpp::BufferCollectionImportExportTokens::New();
  fuchsia_ui_composition::RegisterBufferCollectionArgs rbc_args = {};
  rbc_args.export_token() = std::move(bc_tokens.export_token);
  rbc_args.buffer_collection_token2() = std::move(scenic_token);

  ASSERT_TRUE(flatland_allocator_->RegisterBufferCollection(std::move(rbc_args)).is_ok());

  // Use the local token to allocate a protected buffer. NullRenderer sets constraint to complete
  // the allocation.
  SetConstraintsAndAllocateBuffer(std::move(local_token));

  // Create the image in the Flatland instance.
  fuchsia_ui_composition::ImageProperties image_properties = {};
  image_properties.size() = {display_width_, display_height_};
  const fuc::ContentId kImageContentId(1);
  ASSERT_TRUE(root_flatland()
                  ->CreateImage({{.image_id = kImageContentId,
                                  .import_token = std::move(bc_tokens.import_token),
                                  .vmo_index = 0,
                                  .properties = image_properties}})
                  .is_ok());

  BlockingPresent(this, root_flatland());

  // Present the created Image. Verify that render happened without any errors.
  ASSERT_TRUE(root_flatland()->CreateTransform(kRootTransform).is_ok());
  ASSERT_TRUE(root_flatland()->SetRootTransform(kRootTransform).is_ok());
  ASSERT_TRUE(root_flatland()
                  ->SetContent({{.transform_id = kRootTransform, .content_id = kImageContentId}})
                  .is_ok());
  fuchsia_ui_composition::PresentArgs args;
  args.release_fences(utils::CreateEventArray(1));
  auto release_fence_copy = utils::CopyZxHandle(args.release_fences().value()[0]);
  BlockingPresent(this, root_flatland(), std::move(args));

  // Ensure that release fence for the previous frame is singalled after a Present.
  ASSERT_TRUE(root_flatland()->Clear().is_ok());
  BlockingPresent(this, root_flatland());
  EXPECT_TRUE(utils::IsEventSignalled(release_fence_copy, ZX_EVENT_SIGNALED));
}

TEST_F(NullRendererIntegrationTest, ScreenshotIsAllZeroes) {
  auto [local_token, scenic_token] = utils::SysmemTokens::Create(sysmem_allocator_);

  // Send one token to Flatland Allocator.
  allocation::cpp::BufferCollectionImportExportTokens bc_tokens =
      allocation::cpp::BufferCollectionImportExportTokens::New();
  fuchsia_ui_composition::RegisterBufferCollectionArgs rbc_args = {};
  rbc_args.export_token() = std::move(bc_tokens.export_token);
  rbc_args.buffer_collection_token2() = std::move(scenic_token);

  ASSERT_TRUE(flatland_allocator_->RegisterBufferCollection(std::move(rbc_args)).is_ok());

  // Use the local token to allocate a protected buffer.
  SetConstraintsAndAllocateBuffer(std::move(local_token));

  // Create the image in the Flatland instance.
  fuchsia_ui_composition::ImageProperties image_properties = {};
  image_properties.size() = {display_width_, display_height_};
  const fuc::ContentId kImageContentId(1);
  ASSERT_TRUE(root_flatland()
                  ->CreateImage({{.image_id = kImageContentId,
                                  .import_token = std::move(bc_tokens.import_token),
                                  .vmo_index = 0,
                                  .properties = image_properties}})
                  .is_ok());

  BlockingPresent(this, root_flatland());

  // Present the created Image.
  ASSERT_TRUE(root_flatland()->CreateTransform(kRootTransform).is_ok());
  ASSERT_TRUE(root_flatland()->SetRootTransform(kRootTransform).is_ok());
  ASSERT_TRUE(root_flatland()
                  ->SetContent({{.transform_id = kRootTransform, .content_id = kImageContentId}})
                  .is_ok());

  BlockingPresent(this, root_flatland());

  // Verify that screenshot works and is all zeroes.
  auto screenshot = TakeScreenshot(screenshotter_, display_width_, display_height_);
  EXPECT_EQ(screenshot.Histogram()[utils::Pixel(0, 0, 0, 0)],
            screenshot.width() * screenshot.height());
}

}  // namespace integration_tests

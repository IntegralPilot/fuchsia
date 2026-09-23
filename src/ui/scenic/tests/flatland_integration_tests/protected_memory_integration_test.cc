// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.images2/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
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

namespace integration_tests {

namespace fuc = fuchsia_ui_composition;

namespace {
const fuc::TransformId kRootTransform(1);
}  // namespace

class ProtectedMemoryIntegrationTest : public ScenicCtfTest {
 public:
  void SetUp() override {
    ScenicCtfTest::SetUp();

    auto [client_end, server_end] = fidl::Endpoints<fuchsia_sysmem2::Allocator>::Create();
    ASSERT_EQ(ZX_OK, LocalServiceDirectory()->Connect(fuchsia_sysmem2::Allocator::kDiscoverableName,
                                                      server_end.TakeChannel()));
    sysmem_allocator_.Bind(std::move(client_end), dispatcher());

    flatland_allocator_ = ConnectSyncIntoRealm<fuc::Allocator>();

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

    fidl::Client<fuc::ParentViewportWatcher> parent_viewport_watcher(
        std::move(parent_viewport_watcher_client_end), dispatcher());

    // Get the display's width and height. Since there is no Present in FlatlandDisplay, receiving
    // this callback ensures that all the FlatlandDisplay calls are processed.
    std::optional<fuc::LayoutInfo> info;
    parent_viewport_watcher->GetLayout().Then(
        [&info](fidl::Result<fuc::ParentViewportWatcher::GetLayout>& result) {
          if (result.is_ok()) {
            info = std::move(result.value().info());
          }
        });
    RunLoopUntil([&info] { return info.has_value(); });
    display_width_ = info->logical_size()->width();
    display_height_ = info->logical_size()->height();

    screenshotter_ = ConnectSyncIntoRealm<fuc::Screenshot>();
  }

 protected:
  zx_status_t SetConstraintsAndAllocateBuffer(
      fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token, bool use_protected_memory) {
    auto [buffer_collection_client_end, buffer_collection_server_end] =
        fidl::CreateEndpoints<fuchsia_sysmem2::BufferCollection>().value();
    fidl::SyncClient<fuchsia_sysmem2::BufferCollection> buffer_collection(
        std::move(buffer_collection_client_end));
    fidl::Arena arena;
    fidl::OneWayStatus result = sysmem_allocator_->BindSharedCollection(
        fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
            .token(std::move(token))
            .buffer_collection_request(std::move(buffer_collection_server_end))
            .Build());
    if (!result.ok()) {
      return result.status();
    }

    fuchsia_sysmem2::NodeSetNameRequest set_name_request;
    set_name_request.priority(100u);
    set_name_request.name("ProtectedMemoryIntegrationTest");
    auto name_res = buffer_collection->SetName(std::move(set_name_request));
    if (name_res.is_error()) {
      return name_res.error_value().status();
    }

    fuchsia_sysmem2::BufferCollectionSetConstraintsRequest set_constraints_request;
    auto& constraints = set_constraints_request.constraints().emplace();
    if (use_protected_memory) {
      auto& bmc = constraints.buffer_memory_constraints().emplace();
      bmc.secure_required(true);
      bmc.inaccessible_domain_supported(true);
      bmc.cpu_domain_supported(false);
      bmc.ram_domain_supported(false);
    }
    constraints.usage().emplace().none(fuchsia_sysmem2::kNoneUsage);
    constraints.min_buffer_count(1);
    auto& image_constraints = constraints.image_format_constraints().emplace().emplace_back();
    image_constraints.pixel_format(fuchsia_images2::PixelFormat::kB8G8R8A8);
    image_constraints.color_spaces().emplace().emplace_back(fuchsia_images2::ColorSpace::kSrgb);
    image_constraints.required_min_size(
        fuchsia_math::SizeU{{.width = display_width_, .height = display_height_}});
    image_constraints.required_max_size(
        fuchsia_math::SizeU{{.width = display_width_, .height = display_height_}});

    auto set_res = buffer_collection->SetConstraints(std::move(set_constraints_request));
    if (set_res.is_error()) {
      return set_res.error_value().status();
    }

    auto wait_result = buffer_collection->WaitForAllBuffersAllocated();
    if (wait_result.is_error()) {
      if (wait_result.error_value().is_framework_error()) {
        return wait_result.error_value().framework_error().status();
      }
      return ZX_ERR_INTERNAL;
    }

    auto& buffer_collection_info = wait_result.value().buffer_collection_info().value();
    EXPECT_EQ(1u, buffer_collection_info.buffers().value().size());
    EXPECT_TRUE(buffer_collection->Release().is_ok());
    return ZX_OK;
  }

  uint32_t display_width_ = 0;
  uint32_t display_height_ = 0;

  fidl::WireClient<fuchsia_sysmem2::Allocator> sysmem_allocator_;
  fidl::SyncClient<fuc::Allocator> flatland_allocator_;
  std::unique_ptr<FlatlandClientWithEventHandler> root_flatland_;
  fidl::SyncClient<fuc::Screenshot> screenshotter_;
};

TEST_F(ProtectedMemoryIntegrationTest, RendersProtectedImage) {
  auto [local_token, scenic_token] = utils::SysmemTokens::Create(sysmem_allocator_);

  // Send one token to Flatland Allocator.
  allocation::cpp::BufferCollectionImportExportTokens bc_tokens =
      allocation::cpp::BufferCollectionImportExportTokens::New();
  fuchsia_ui_composition::RegisterBufferCollectionArgs rbc_args = {};
  rbc_args.export_token() = std::move(bc_tokens.export_token);
  rbc_args.buffer_collection_token2() = std::move(scenic_token);
  ASSERT_TRUE(flatland_allocator_->RegisterBufferCollection(std::move(rbc_args)).is_ok());

  // Use the local token to allocate a protected buffer.
  zx_status_t status =
      SetConstraintsAndAllocateBuffer(std::move(local_token), /*use_protected_memory=*/true);
  if (status == ZX_ERR_PEER_CLOSED) {
    ZXTEST_SKIP("Protected memory not supported");
  }
  if (status != ZX_OK) {
    return;
  }

  // Create the image in the Flatland instance.
  fuchsia_ui_composition::ImageProperties image_properties = {};
  image_properties.size(fuchsia_math::SizeU{{.width = display_width_, .height = display_height_}});
  const fuc::ContentId kImageContentId(1);
  ASSERT_TRUE((*root_flatland_)
                  ->CreateImage({{.image_id = kImageContentId,
                                  .import_token = std::move(bc_tokens.import_token),
                                  .vmo_index = 0,
                                  .properties = std::move(image_properties)}})
                  .is_ok());
  BlockingPresent(this, *root_flatland_);

  // Present the created Image.
  ASSERT_TRUE((*root_flatland_)->CreateTransform(kRootTransform).is_ok());
  ASSERT_TRUE((*root_flatland_)->SetRootTransform(kRootTransform).is_ok());
  ASSERT_TRUE((*root_flatland_)
                  ->SetContent({{.transform_id = kRootTransform, .content_id = kImageContentId}})
                  .is_ok());
  BlockingPresent(this, *root_flatland_);

  // Verify that render happened without any errors.
}

TEST_F(ProtectedMemoryIntegrationTest, ScreenshotReplacesProtectedImage) {
  auto [local_token, scenic_token] = utils::SysmemTokens::Create(sysmem_allocator_);

  // Send one token to Flatland Allocator.
  allocation::cpp::BufferCollectionImportExportTokens bc_tokens =
      allocation::cpp::BufferCollectionImportExportTokens::New();
  fuchsia_ui_composition::RegisterBufferCollectionArgs rbc_args = {};
  rbc_args.export_token() = std::move(bc_tokens.export_token);
  rbc_args.buffer_collection_token2() = std::move(scenic_token);
  ASSERT_TRUE(flatland_allocator_->RegisterBufferCollection(std::move(rbc_args)).is_ok());

  // Use the local token to allocate a protected buffer.
  zx_status_t status =
      SetConstraintsAndAllocateBuffer(std::move(local_token), /*use_protected_memory=*/true);
  if (status == ZX_ERR_PEER_CLOSED) {
    ZXTEST_SKIP("Protected memory not supported");
  }
  if (status != ZX_OK) {
    return;
  }

  // Create the image in the Flatland instance.
  fuchsia_ui_composition::ImageProperties image_properties = {};
  image_properties.size(fuchsia_math::SizeU{{.width = display_width_, .height = display_height_}});
  const fuc::ContentId kImageContentId(1);
  ASSERT_TRUE((*root_flatland_)
                  ->CreateImage({{.image_id = kImageContentId,
                                  .import_token = std::move(bc_tokens.import_token),
                                  .vmo_index = 0,
                                  .properties = std::move(image_properties)}})
                  .is_ok());
  BlockingPresent(this, *root_flatland_);

  // Present the created Image.
  ASSERT_TRUE((*root_flatland_)->CreateTransform(kRootTransform).is_ok());
  ASSERT_TRUE((*root_flatland_)->SetRootTransform(kRootTransform).is_ok());
  ASSERT_TRUE((*root_flatland_)
                  ->SetContent({{.transform_id = kRootTransform, .content_id = kImageContentId}})
                  .is_ok());
  BlockingPresent(this, *root_flatland_);

  // Verify that screenshot works and replaced the content with black.
  auto screenshot = TakeScreenshot(screenshotter_, display_width_, display_height_);
  EXPECT_EQ(screenshot.Histogram()[utils::kBlack], screenshot.width() * screenshot.height());
}

}  // namespace integration_tests

// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.images2/cpp/fidl.h>
#include <fidl/fuchsia.math/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition.internal/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <fidl/fuchsia.ui.views/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/ui/scenic/cpp/buffer_collection_import_export_tokens.h>
#include <lib/ui/scenic/cpp/view_creation_tokens.h>
#include <lib/ui/scenic/cpp/view_identity.h>
#include <lib/ui/scenic/cpp/view_ref_pair.h>
#include <sys/types.h>
#include <zircon/status.h>

#include <cstdint>
#include <iostream>
#include <optional>
#include <utility>

#include <zxtest/zxtest.h>

#include "src/ui/scenic/lib/utils/helpers.h"
#include "src/ui/scenic/tests/utils/blocking_present.h"
#include "src/ui/scenic/tests/utils/flatland_client_with_event_handler.h"
#include "src/ui/scenic/tests/utils/scenic_ctf_test_base.h"
#include "src/ui/scenic/tests/utils/screen_capture_utils.h"
#include "src/ui/scenic/tests/utils/simple_watcher_client.h"

namespace integration_tests {

namespace fuc = fuchsia_ui_composition;
namespace fuci = fuchsia_ui_composition_internal;
namespace fuv = fuchsia_ui_views;

namespace {
const fuc::TransformId kChildRootTransform(1);
}  // namespace

class ScreenCapture2IntegrationTest : public ScenicCtfTest {
 public:
  void SetUp() override {
    ScenicCtfTest::SetUp();

    auto [client_end, server_end] = fidl::Endpoints<fuchsia_sysmem2::Allocator>::Create();
    LocalServiceDirectory()->Connect("fuchsia.sysmem2.Allocator", server_end.TakeChannel());
    sysmem_allocator_.Bind(std::move(client_end), dispatcher());

    flatland_allocator_ = ConnectSyncIntoRealm<fuc::Allocator>();
    root_session_.emplace(ConnectIntoRealm<fuc::Flatland>(), dispatcher());

    auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
    SetFlatlandDisplayContent(std::move(parent_token));

    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    root_view_ref_ = scenic::cpp::CloneViewRef(identity.view_ref());
    auto [pv_client_end, pv_server_end] = fidl::Endpoints<fuc::ParentViewportWatcher>::Create();
    parent_viewport_watcher_ = std::make_unique<SimpleWatcherClient<fuc::ParentViewportWatcher>>(
        std::move(pv_client_end), dispatcher());

    FX_CHECK((*root_session_)
                 ->CreateView2({{.token = std::move(child_token),
                                 .view_identity = std::move(identity),
                                 .protocols = {},
                                 .parent_viewport_watcher = std::move(pv_server_end)}})
                 .is_ok());

    parent_viewport_watcher_->client()->GetLayout().Then(
        [this](fidl::Result<fuc::ParentViewportWatcher::GetLayout>& result) {
          ASSERT_TRUE(result.is_ok());
          ASSERT_TRUE(result->info().logical_size().has_value());
          const auto size = result->info().logical_size().value();
          display_width_ = size.width();
          display_height_ = size.height();
          num_pixels_ = display_width_ * display_height_;
        });
    BlockingPresent(this, *root_session_);

    // Wait until we get the display size.
    RunLoopUntil([this] { return display_width_ != 0 && display_height_ != 0; });

    // Set up the root graph.
    auto [cv_client_end, cv_server_end] = fidl::Endpoints<fuc::ChildViewWatcher>::Create();
    child_view_watcher_ = std::make_unique<SimpleWatcherClient<fuc::ChildViewWatcher>>(
        std::move(cv_client_end), dispatcher());
    auto [child_token2, parent_token2] = scenic::cpp::ViewCreationTokenPair::New();
    fuc::ViewportProperties properties;
    properties.logical_size(
        fuchsia_math::SizeU{{.width = display_width_, .height = display_height_}});
    const fuc::TransformId kRootTransform(1);
    const fuc::ContentId kRootContent(1);
    FX_CHECK((*root_session_)->CreateTransform({{.transform_id = kRootTransform}}).is_ok());
    FX_CHECK((*root_session_)
                 ->CreateViewport({{.viewport_id = kRootContent,
                                    .token = std::move(parent_token2),
                                    .properties = std::move(properties),
                                    .child_view_watcher = std::move(cv_server_end)}})
                 .is_ok());
    FX_CHECK((*root_session_)->SetRootTransform({{.transform_id = kRootTransform}}).is_ok());
    FX_CHECK((*root_session_)
                 ->SetContent({{.transform_id = kRootTransform, .content_id = kRootContent}})
                 .is_ok());
    BlockingPresent(this, *root_session_);

    // Set up the child view.
    child_session_.emplace(ConnectIntoRealm<fuc::Flatland>(), dispatcher());
    auto [pv_client_end2, pv_server_end2] = fidl::Endpoints<fuc::ParentViewportWatcher>::Create();
    parent_viewport_watcher2_ = std::make_unique<SimpleWatcherClient<fuc::ParentViewportWatcher>>(
        std::move(pv_client_end2), dispatcher());
    auto child_identity = scenic::cpp::NewViewIdentityOnCreation();
    auto child_view_ref = scenic::cpp::CloneViewRef(child_identity.view_ref());
    fuc::ViewBoundProtocols bound_protocols;
    FX_CHECK((*child_session_)
                 ->CreateView2({{.token = std::move(child_token2),
                                 .view_identity = std::move(child_identity),
                                 .protocols = std::move(bound_protocols),
                                 .parent_viewport_watcher = std::move(pv_server_end2)}})
                 .is_ok());
    FX_CHECK((*child_session_)->CreateTransform({{.transform_id = kChildRootTransform}}).is_ok());
    FX_CHECK((*child_session_)->SetRootTransform({{.transform_id = kChildRootTransform}}).is_ok());
    BlockingPresent(this, *child_session_);

    // Create ScreenCapture client.
    screen_capture_ = ConnectAsyncIntoRealm<fuci::ScreenCapture>();

    // Set up error handling.
    root_session_->set_on_error([](fidl::Event<fuc::Flatland::OnError>& event) {
      FX_LOGS(ERROR) << "Root session error: " << fidl::ToUnderlying(event.error());
    });
    child_session_->set_on_error([](fidl::Event<fuc::Flatland::OnError>& event) {
      FX_LOGS(ERROR) << "Child session error: " << fidl::ToUnderlying(event.error());
    });
  }

  fuchsia_sysmem2::BufferCollectionInfo ConfigureScreenCapture(
      fuchsia_sysmem2::BufferCollectionConstraints constraints, const uint32_t render_target_width,
      const uint32_t render_target_height) {
    // Create buffer collection to render into for GetNextFrame().
    auto scr_ref_pair = allocation::cpp::BufferCollectionImportExportTokens::New();

    fuchsia_sysmem2::BufferCollectionInfo sc_buffer_collection_info =
        CreateBufferCollectionInfoWithConstraints(
            std::move(constraints), std::move(scr_ref_pair.export_token), flatland_allocator_,
            sysmem_allocator_, fuc::RegisterBufferCollectionUsages::kScreenshot);

    // Configure ScreenCapture client.
    fuci::ScreenCaptureConfig sc_args;
    sc_args.import_token(std::move(scr_ref_pair.import_token));
    sc_args.image_size(
        fuchsia_math::SizeU{{.width = render_target_width, .height = render_target_height}});

    std::optional<fidl::Result<fuci::ScreenCapture::Configure>> configure_result;
    screen_capture_->Configure(std::move(sc_args))
        .Then([&configure_result](fidl::Result<fuci::ScreenCapture::Configure>& result) {
          EXPECT_TRUE(result.is_ok());
          configure_result = std::move(result);
        });
    RunLoopWithTimeoutOrUntil([&configure_result] { return configure_result.has_value(); },
                              kEventDelay);
    EXPECT_TRUE(configure_result.has_value());
    EXPECT_TRUE(configure_result->is_ok());

    return sc_buffer_collection_info;
  }

  static constexpr zx::duration kEventDelay = zx::msec(1000);

  fidl::WireClient<fuchsia_sysmem2::Allocator> sysmem_allocator_;
  fidl::SyncClient<fuc::Allocator> flatland_allocator_;
  std::optional<FlatlandClientWithEventHandler> root_session_;
  std::optional<FlatlandClientWithEventHandler> child_session_;
  std::unique_ptr<SimpleWatcherClient<fuc::ParentViewportWatcher>> parent_viewport_watcher_;
  std::unique_ptr<SimpleWatcherClient<fuc::ChildViewWatcher>> child_view_watcher_;
  std::unique_ptr<SimpleWatcherClient<fuc::ParentViewportWatcher>> parent_viewport_watcher2_;
  fidl::Client<fuci::ScreenCapture> screen_capture_;
  fuv::ViewRef root_view_ref_;

  uint32_t display_width_ = 0;
  uint32_t display_height_ = 0;
  uint32_t num_pixels_ = 0;
};

TEST_F(ScreenCapture2IntegrationTest, SingleColorCapture) {
  const uint32_t image_width = display_width_;
  const uint32_t image_height = display_height_;
  const uint32_t render_target_width = display_width_;
  const uint32_t render_target_height = display_height_;

  // Create buffer collection for image to add to scene graph.
  auto ref_pair = allocation::cpp::BufferCollectionImportExportTokens::New();

  fuchsia_sysmem2::BufferCollectionInfo buffer_collection_info =
      CreateBufferCollectionInfoWithConstraints(
          utils::CreateDefaultConstraints(/*buffer_count=*/1, image_width, image_height),
          std::move(ref_pair.export_token), flatland_allocator_, sysmem_allocator_,
          fuc::RegisterBufferCollectionUsages::kDefault);

  std::vector<uint8_t> write_values;
  for (uint32_t i = 0; i < num_pixels_; ++i) {
    write_values.insert(write_values.end(), kRed, kRed + kBytesPerPixel);
  }
  WriteToSysmemBuffer(write_values, buffer_collection_info, 0, kBytesPerPixel, image_width,
                      image_height);
  GenerateImageForFlatlandInstance(
      0, *child_session_, kChildRootTransform, std::move(ref_pair.import_token),
      fuchsia_math::SizeU{{.width = image_width, .height = image_height}},
      fuchsia_math::Vec{{.x = 0, .y = 0}}, 2, 2);
  BlockingPresent(this, *child_session_);

  fuchsia_sysmem2::BufferCollectionInfo sc_buffer_collection_info = ConfigureScreenCapture(
      utils::CreateDefaultConstraints(/*buffer_count=*/1, image_width, image_height),
      render_target_width, render_target_height);

  std::optional<fidl::Result<fuci::ScreenCapture::GetNextFrame>> gnf_result;
  screen_capture_->GetNextFrame().Then(
      [&gnf_result](fidl::Result<fuci::ScreenCapture::GetNextFrame>& result) {
        EXPECT_TRUE(result.is_ok());
        gnf_result = std::move(result);
      });
  RunLoopWithTimeoutOrUntil([&gnf_result] { return gnf_result.has_value(); }, kEventDelay);
  EXPECT_TRUE(gnf_result.has_value());
  EXPECT_TRUE(gnf_result->is_ok());
  auto info = std::move(gnf_result->value());

  const auto& read_values =
      ExtractScreenCapture(info.buffer_index().value(), sc_buffer_collection_info, kBytesPerPixel,
                           render_target_width, render_target_height);

  EXPECT_EQ(read_values.size(), write_values.size());

  uint32_t num_red = 0;

  for (size_t i = 0; i < read_values.size(); i += kBytesPerPixel) {
    if (PixelEquals(&read_values[i], kRed))
      num_red++;
  }

  EXPECT_EQ(num_red, num_pixels_);
}

TEST_F(ScreenCapture2IntegrationTest, FilledRectCapture) {
  const uint32_t image_width = display_width_;
  const uint32_t image_height = display_height_;
  const uint32_t render_target_width = display_width_;
  const uint32_t render_target_height = display_height_;

  const fuc::ContentId kFilledRectId(1);
  const fuc::TransformId kTransformId(2);

  // Create a red rectangle.
  FX_CHECK((*child_session_)->CreateFilledRect({{.rect_id = kFilledRectId}}).is_ok());
  FX_CHECK((*child_session_)
               ->SetSolidFill({{.rect_id = kFilledRectId,
                                .color = {{.red = 1, .green = 0, .blue = 0, .alpha = 1}},
                                .size = {{.width = image_width, .height = image_height}}}})
               .is_ok());
  FX_CHECK((*child_session_)->CreateTransform({{.transform_id = kTransformId}}).is_ok());
  FX_CHECK((*child_session_)
               ->SetContent({{.transform_id = kTransformId, .content_id = kFilledRectId}})
               .is_ok());

  // Attach the transform to the scene
  FX_CHECK((*child_session_)
               ->AddChild({{.parent_transform_id = kChildRootTransform,
                            .child_transform_id = kTransformId}})
               .is_ok());
  BlockingPresent(this, *child_session_);

  fuchsia_sysmem2::BufferCollectionInfo sc_buffer_collection_info = ConfigureScreenCapture(
      utils::CreateDefaultConstraints(/*buffer_count=*/1, image_width, image_height),
      render_target_width, render_target_height);

  std::optional<fidl::Result<fuci::ScreenCapture::GetNextFrame>> gnf_result;
  screen_capture_->GetNextFrame().Then(
      [&gnf_result](fidl::Result<fuci::ScreenCapture::GetNextFrame>& result) {
        EXPECT_TRUE(result.is_ok());
        gnf_result = std::move(result);
      });
  RunLoopWithTimeoutOrUntil([&gnf_result] { return gnf_result.has_value(); }, kEventDelay);
  EXPECT_TRUE(gnf_result.has_value());
  EXPECT_TRUE(gnf_result->is_ok());
  auto info = std::move(gnf_result->value());

  const auto& read_values =
      ExtractScreenCapture(info.buffer_index().value(), sc_buffer_collection_info, kBytesPerPixel,
                           render_target_width, render_target_height);

  EXPECT_EQ(read_values.size(), num_pixels_ * kBytesPerPixel);

  uint32_t num_red = 0;
  for (size_t i = 0; i < read_values.size(); i += kBytesPerPixel) {
    if (PixelEquals(&read_values[i], kRed))
      num_red++;
  }

  EXPECT_EQ(num_red, num_pixels_);
}

// If the client calls GetNextFrame() and they have recieved the last frame, the client should hang
// until OnCpuWorkDone() is fired.
TEST_F(ScreenCapture2IntegrationTest, OnCpuWorkDoneCapture) {
  const uint32_t image_width = display_width_;
  const uint32_t image_height = display_height_;
  const uint32_t render_target_width = display_width_;
  const uint32_t render_target_height = display_height_;

  const fuc::ContentId kFilledRectId(1);
  const fuc::TransformId kTransformId(2);

  // Create a red rectangle.
  FX_CHECK((*child_session_)->CreateFilledRect({{.rect_id = kFilledRectId}}).is_ok());
  FX_CHECK((*child_session_)
               ->SetSolidFill({{.rect_id = kFilledRectId,
                                .color = {{.red = 1, .green = 0, .blue = 0, .alpha = 1}},
                                .size = {{.width = image_width, .height = image_height}}}})
               .is_ok());
  FX_CHECK((*child_session_)->CreateTransform({{.transform_id = kTransformId}}).is_ok());
  FX_CHECK((*child_session_)
               ->SetContent({{.transform_id = kTransformId, .content_id = kFilledRectId}})
               .is_ok());

  // Attach the transform to the scene
  FX_CHECK((*child_session_)
               ->AddChild({{.parent_transform_id = kChildRootTransform,
                            .child_transform_id = kTransformId}})
               .is_ok());
  BlockingPresent(this, *child_session_);

  fuchsia_sysmem2::BufferCollectionInfo sc_buffer_collection_info = ConfigureScreenCapture(
      utils::CreateDefaultConstraints(/*buffer_count=*/1, image_width, image_height),
      render_target_width, render_target_height);

  std::optional<fidl::Result<fuci::ScreenCapture::GetNextFrame>> gnf_result;
  screen_capture_->GetNextFrame().Then(
      [&gnf_result](fidl::Result<fuci::ScreenCapture::GetNextFrame>& result) {
        EXPECT_TRUE(result.is_ok());
        gnf_result = std::move(result);
      });
  RunLoopWithTimeoutOrUntil([&gnf_result] { return gnf_result.has_value(); }, kEventDelay);
  EXPECT_TRUE(gnf_result.has_value());
  EXPECT_TRUE(gnf_result->is_ok());
  auto info = std::move(gnf_result->value());

  const auto& read_values =
      ExtractScreenCapture(info.buffer_index().value(), sc_buffer_collection_info, kBytesPerPixel,
                           render_target_width, render_target_height);
  EXPECT_EQ(read_values.size(), num_pixels_ * kBytesPerPixel);

  // Compare read and write values.
  uint32_t num_red_count = 0;

  for (size_t i = 0; i < read_values.size(); i += kBytesPerPixel) {
    if (PixelEquals(&read_values[i], kRed))
      num_red_count++;
  }

  EXPECT_EQ(num_red_count, num_pixels_);

  // Release buffer.
  zx::eventpair token = std::move(info.buffer_release_token().value());
  EXPECT_EQ(token.signal_peer(0, ZX_EVENTPAIR_SIGNALED), ZX_OK);

  // Now change the color of the screen.
  const fuc::ContentId kFilledRectId2(2);
  const fuc::TransformId kTransformId2(3);

  // Create a blue rectangle.
  FX_CHECK((*child_session_)->CreateFilledRect({{.rect_id = kFilledRectId2}}).is_ok());
  FX_CHECK((*child_session_)
               ->SetSolidFill({{.rect_id = kFilledRectId2,
                                .color = {{.red = 0, .green = 0, .blue = 1, .alpha = 1}},
                                .size = {{.width = image_width, .height = image_height}}}})
               .is_ok());
  FX_CHECK((*child_session_)->CreateTransform({{.transform_id = kTransformId2}}).is_ok());
  FX_CHECK((*child_session_)
               ->SetContent({{.transform_id = kTransformId2, .content_id = kFilledRectId2}})
               .is_ok());

  // Attach the transform to child but do not Present.
  FX_CHECK((*child_session_)
               ->AddChild({{.parent_transform_id = kChildRootTransform,
                            .child_transform_id = kTransformId2}})
               .is_ok());

  std::optional<fidl::Result<fuci::ScreenCapture::GetNextFrame>> gnf_result2;
  screen_capture_->GetNextFrame().Then(
      [&gnf_result2](fidl::Result<fuci::ScreenCapture::GetNextFrame>& result) {
        EXPECT_TRUE(result.is_ok());
        gnf_result2 = std::move(result);
      });

  // Client has recieved last frame so will hang until OnCpuWorkDone fires MaybeRenderFrame().
  RunLoopWithTimeoutOrUntil([&gnf_result2] { return gnf_result2.has_value(); }, kEventDelay);
  EXPECT_FALSE(gnf_result2.has_value());
  FX_CHECK((*child_session_)->Present({}).is_ok());

  RunLoopWithTimeoutOrUntil([&gnf_result2] { return gnf_result2.has_value(); }, kEventDelay);
  EXPECT_TRUE(gnf_result2.has_value());
  EXPECT_TRUE(gnf_result2->is_ok());
  auto info2 = std::move(gnf_result2->value());

  const auto& read_values2 =
      ExtractScreenCapture(info2.buffer_index().value(), sc_buffer_collection_info, kBytesPerPixel,
                           render_target_width, render_target_height);

  EXPECT_EQ(read_values2.size(), num_pixels_ * kBytesPerPixel);

  uint32_t num_blue_count = 0;

  for (size_t i = 0; i < read_values2.size(); i += kBytesPerPixel) {
    if (PixelEquals(&read_values2[i], kBlue))
      num_blue_count++;
  }

  EXPECT_EQ(num_blue_count, num_pixels_);
}

// If there are no available buffers for GetNextFrame() to render into, the client should hang until
// they release a buffer and then receive the frame immedietly.
TEST_F(ScreenCapture2IntegrationTest, ClientReleaseBufferCapture) {
  const uint32_t image_width = display_width_;
  const uint32_t image_height = display_height_;
  const uint32_t render_target_width = display_width_;
  const uint32_t render_target_height = display_height_;

  auto ref_pair = allocation::cpp::BufferCollectionImportExportTokens::New();

  fuchsia_sysmem2::BufferCollectionInfo buffer_collection_info =
      CreateBufferCollectionInfoWithConstraints(
          utils::CreateDefaultConstraints(/*buffer_count=*/1, image_width, image_height),
          std::move(ref_pair.export_token), flatland_allocator_, sysmem_allocator_,
          fuc::RegisterBufferCollectionUsages::kDefault);

  fuchsia_sysmem2::BufferCollectionInfo sc_buffer_collection_info = ConfigureScreenCapture(
      utils::CreateDefaultConstraints(/*buffer_count=*/1, image_width, image_height),
      render_target_width, render_target_height);

  std::optional<fidl::Result<fuci::ScreenCapture::GetNextFrame>> gnf_result;
  screen_capture_->GetNextFrame().Then(
      [&gnf_result](fidl::Result<fuci::ScreenCapture::GetNextFrame>& result) {
        EXPECT_TRUE(result.is_ok());
        gnf_result = std::move(result);
      });
  RunLoopWithTimeoutOrUntil([&gnf_result] { return gnf_result.has_value(); }, kEventDelay);
  EXPECT_TRUE(gnf_result.has_value());
  EXPECT_TRUE(gnf_result->is_ok());
  auto info = std::move(gnf_result->value());

  std::vector<uint8_t> write_values;
  for (uint32_t i = 0; i < num_pixels_; ++i) {
    write_values.insert(write_values.end(), kRed, kRed + kBytesPerPixel);
  }

  WriteToSysmemBuffer(write_values, buffer_collection_info, 0, kBytesPerPixel, image_width,
                      image_height);
  GenerateImageForFlatlandInstance(
      0, *child_session_, kChildRootTransform, std::move(ref_pair.import_token),
      fuchsia_math::SizeU{{.width = image_width, .height = image_height}},
      fuchsia_math::Vec{{.x = 0, .y = 0}}, 2, 2);

  std::optional<fidl::Result<fuci::ScreenCapture::GetNextFrame>> gnf_result2;
  screen_capture_->GetNextFrame().Then(
      [&gnf_result2](fidl::Result<fuci::ScreenCapture::GetNextFrame>& result) {
        EXPECT_TRUE(result.is_ok());
        gnf_result2 = std::move(result);
      });

  // Client has recieved last frame so GetNextFrame will hang.
  RunLoopWithTimeoutOrUntil([&gnf_result2] { return gnf_result2.has_value(); }, kEventDelay);
  EXPECT_FALSE(gnf_result2.has_value());

  // Client does not have any buffers available so OnCpuWorkDone will not render into buffer.
  FX_CHECK((*child_session_)->Present({}).is_ok());
  RunLoopWithTimeoutOrUntil([&gnf_result2] { return gnf_result2.has_value(); }, kEventDelay);
  EXPECT_FALSE(gnf_result2.has_value());

  // Client releases buffer.
  zx::eventpair token = std::move(info.buffer_release_token().value());
  EXPECT_EQ(token.signal_peer(0, ZX_EVENTPAIR_SIGNALED), ZX_OK);

  RunLoopWithTimeoutOrUntil([&gnf_result2] { return gnf_result2.has_value(); }, kEventDelay);
  EXPECT_TRUE(gnf_result2.has_value());
  EXPECT_TRUE(gnf_result2->is_ok());
  auto info2 = std::move(gnf_result2->value());

  const auto& read_values =
      ExtractScreenCapture(info2.buffer_index().value(), sc_buffer_collection_info, kBytesPerPixel,
                           render_target_width, render_target_height);
  EXPECT_EQ(read_values.size(), write_values.size());

  uint32_t num_red = 0;
  for (size_t i = 0; i < read_values.size(); i += kBytesPerPixel) {
    if (PixelEquals(&read_values[i], kRed))
      num_red++;
  }
  EXPECT_EQ(num_red, num_pixels_);
}

}  // namespace integration_tests

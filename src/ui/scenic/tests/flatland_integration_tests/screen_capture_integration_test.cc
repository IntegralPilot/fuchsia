// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.images2/cpp/fidl.h>
#include <fidl/fuchsia.math/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
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
#include <utility>

#include <zxtest/zxtest.h>

#include "src/ui/scenic/lib/utils/helpers.h"
#include "src/ui/scenic/tests/utils/blocking_present.h"
#include "src/ui/scenic/tests/utils/flatland_client_with_event_handler.h"
#include "src/ui/scenic/tests/utils/scenic_ctf_test_base.h"
#include "src/ui/scenic/tests/utils/screen_capture_utils.h"
#include "src/ui/scenic/tests/utils/simple_watcher_client.h"
#include "src/ui/testing/util/zxtest_helpers.h"
#include "zircon/errors.h"

namespace integration_tests {

namespace fuc = fuchsia_ui_composition;
namespace fuv = fuchsia_ui_views;

namespace {
const fuc::TransformId kChildRootTransform(1);
}  // namespace

class ScreenCaptureIntegrationTest : public ScenicCtfTest {
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
    screen_capture_ = ConnectSyncIntoRealm<fuc::ScreenCapture>();
  }

  // This function calls GetNextFrame().
  fit::result<fuc::ScreenCaptureError, fuc::FrameInfo> CaptureScreen(
      fidl::SyncClient<fuc::ScreenCapture>& screencapturer) {
    zx::event event;
    zx::event dup;
    zx_status_t status = zx::event::create(0, &event);
    EXPECT_EQ(status, ZX_OK);
    event.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup);

    fuc::GetNextFrameArgs gnf_args;
    gnf_args.event(std::move(dup));

    auto result = screencapturer->GetNextFrame(std::move(gnf_args));
    EXPECT_TRUE(result.is_ok());

    if (result.is_ok()) {
      zx::duration kDelay = zx::msec(5000);
      status = event.wait_one(ZX_EVENT_SIGNALED, zx::deadline_after(kDelay), nullptr);
      EXPECT_EQ(status, ZX_OK);
      return fit::ok(std::move(result.value()));
    }
    if (result.error_value().is_domain_error()) {
      return fit::error(result.error_value().domain_error());
    }
    return fit::error(fuc::ScreenCaptureError::kBadOperation);
  }

  static constexpr zx::duration kEventDelay = zx::msec(5000);

  fidl::WireClient<fuchsia_sysmem2::Allocator> sysmem_allocator_;
  fidl::SyncClient<fuc::Allocator> flatland_allocator_;
  std::optional<FlatlandClientWithEventHandler> root_session_;
  std::optional<FlatlandClientWithEventHandler> child_session_;
  std::unique_ptr<SimpleWatcherClient<fuc::ParentViewportWatcher>> parent_viewport_watcher_;
  std::unique_ptr<SimpleWatcherClient<fuc::ChildViewWatcher>> child_view_watcher_;
  std::unique_ptr<SimpleWatcherClient<fuc::ParentViewportWatcher>> parent_viewport_watcher2_;
  fidl::SyncClient<fuc::ScreenCapture> screen_capture_;
  fuv::ViewRef root_view_ref_;

  uint32_t display_width_ = 0;
  uint32_t display_height_ = 0;
  uint32_t num_pixels_ = 0;
};

// TODO(b/481065079): Test flakes may arise from a potential race condition where
// ScreenCapture.CaptureScreen is called while Flatland artifacts from previous tests could still
// be present in FlatlandDisplay's scene graph if FlatlandDisplay.SetContent lags. This is very
// rare because the result from ScreenCapture.Configure must wait for sysmem negotiations which
// can be pretty slow.
TEST_F(ScreenCaptureIntegrationTest, EmptyScreenshot) {
  const uint32_t render_target_width = display_width_;
  const uint32_t render_target_height = display_height_;

  // Create buffer collection to render into for GetNextFrame().
  auto scr_ref_pair = allocation::cpp::BufferCollectionImportExportTokens::New();

  fuchsia_sysmem2::BufferCollectionInfo sc_buffer_collection_info =
      CreateBufferCollectionInfoWithConstraints(
          utils::CreateDefaultConstraints(/*buffer_count=*/1, render_target_width,
                                          render_target_height),
          std::move(scr_ref_pair.export_token), flatland_allocator_, sysmem_allocator_,
          fuc::RegisterBufferCollectionUsages::kScreenshot);

  // Configure buffers in ScreenCapture client.
  fuc::ScreenCaptureConfig sc_args;
  sc_args.import_token(std::move(scr_ref_pair.import_token));
  sc_args.buffer_count(static_cast<uint32_t>(sc_buffer_collection_info.buffers()->size()));
  sc_args.size(fuchsia_math::SizeU{{.width = render_target_width, .height = render_target_height}});

  auto config_res = screen_capture_->Configure(std::move(sc_args));
  ASSERT_TRUE(config_res.is_ok());

  const auto cs_result = CaptureScreen(screen_capture_);
  EXPECT_TRUE(cs_result.is_ok());
  const auto& read_values =
      ExtractScreenCapture(cs_result.value().buffer_id().value(), sc_buffer_collection_info,
                           kBytesPerPixel, render_target_width, render_target_height);

  // Compare read and write values.
  uint32_t num_zero = 0;
  for (size_t i = 0; i < read_values.size(); i += kBytesPerPixel) {
    if (PixelEquals(&read_values[i], kZero))
      num_zero++;
  }
  EXPECT_EQ(num_zero, num_pixels_);
}

TEST_F(ScreenCaptureIntegrationTest, SingleColorUnrotatedScreenshot) {
  const uint32_t image_width = display_width_;
  const uint32_t image_height = display_height_;
  const uint32_t render_target_width = display_width_;
  const uint32_t render_target_height = display_height_;

  // Create Buffer Collection for image to add to scene graph.
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

  // The scene graph is now ready for screencapturing!

  // Create buffer collection to render into for GetNextFrame().
  auto scr_ref_pair = allocation::cpp::BufferCollectionImportExportTokens::New();

  fuchsia_sysmem2::BufferCollectionInfo sc_buffer_collection_info =
      CreateBufferCollectionInfoWithConstraints(
          utils::CreateDefaultConstraints(/*buffer_count=*/1, render_target_width,
                                          render_target_height),
          std::move(scr_ref_pair.export_token), flatland_allocator_, sysmem_allocator_,
          fuc::RegisterBufferCollectionUsages::kScreenshot);

  // Configure buffers in ScreenCapture client.
  fuc::ScreenCaptureConfig sc_args;
  sc_args.import_token(std::move(scr_ref_pair.import_token));
  sc_args.buffer_count(static_cast<uint32_t>(sc_buffer_collection_info.buffers()->size()));
  sc_args.size(fuchsia_math::SizeU{{.width = render_target_width, .height = render_target_height}});

  auto config_res = screen_capture_->Configure(std::move(sc_args));
  ASSERT_TRUE(config_res.is_ok());

  // Take Screenshot!
  const auto cs_result = CaptureScreen(screen_capture_);
  EXPECT_TRUE(cs_result.is_ok());
  const auto& read_values =
      ExtractScreenCapture(cs_result.value().buffer_id().value(), sc_buffer_collection_info,
                           kBytesPerPixel, render_target_width, render_target_height);

  EXPECT_EQ(read_values.size(), write_values.size());

  // Compare read and write values.
  uint32_t num_red = 0;

  for (size_t i = 0; i < read_values.size(); i += kBytesPerPixel) {
    if (PixelEquals(&read_values[i], kRed))
      num_red++;
  }

  EXPECT_EQ(num_red, num_pixels_);
}

// Creates this image:
//          RRRRRRRR
//          RRRRRRRR
//          GGGGGGGG
//          GGGGGGGG
//
// Rotates into this image:
//          GGGGGGGG
//          GGGGGGGG
//          RRRRRRRR
//          RRRRRRRR
TEST_F(ScreenCaptureIntegrationTest, MultiColor180DegreeRotationScreenshot) {
  const uint32_t image_width = display_width_;
  const uint32_t image_height = display_height_;
  const uint32_t render_target_width = display_width_;
  const uint32_t render_target_height = display_height_;

  // Create Buffer Collection for image#1 to add to scene graph.
  auto ref_pair = allocation::cpp::BufferCollectionImportExportTokens::New();

  fuchsia_sysmem2::BufferCollectionInfo buffer_collection_info =
      CreateBufferCollectionInfoWithConstraints(
          utils::CreateDefaultConstraints(/*buffer_count=*/1, image_width, image_height),
          std::move(ref_pair.export_token), flatland_allocator_, sysmem_allocator_,
          fuc::RegisterBufferCollectionUsages::kDefault);

  // Write the image with half green, half red
  std::vector<uint8_t> write_values;
  const uint32_t pixel_color_count = num_pixels_ / 2;

  for (uint32_t i = 0; i < pixel_color_count; ++i) {
    AppendPixel(&write_values, kRed);
  }
  for (uint32_t i = 0; i < pixel_color_count; ++i) {
    AppendPixel(&write_values, kGreen);
  }
  WriteToSysmemBuffer(write_values, buffer_collection_info, 0, kBytesPerPixel, image_width,
                      image_height);

  GenerateImageForFlatlandInstance(
      0, *child_session_, kChildRootTransform, std::move(ref_pair.import_token),
      fuchsia_math::SizeU{{.width = image_width, .height = image_height}},
      fuchsia_math::Vec{{.x = 0, .y = 0}}, 2, 2);

  BlockingPresent(this, *child_session_);

  // The scene graph is now ready for screenshotting!

  // Create buffer collection to render into for GetNextFrame().
  auto scr_ref_pair = allocation::cpp::BufferCollectionImportExportTokens::New();

  fuchsia_sysmem2::BufferCollectionInfo sc_buffer_collection_info =
      CreateBufferCollectionInfoWithConstraints(
          utils::CreateDefaultConstraints(/*buffer_count=*/1, render_target_width,
                                          render_target_height),
          std::move(scr_ref_pair.export_token), flatland_allocator_, sysmem_allocator_,
          fuc::RegisterBufferCollectionUsages::kScreenshot);

  // Configure buffers in ScreenCapture client.
  fuc::ScreenCaptureConfig sc_args;
  sc_args.import_token(std::move(scr_ref_pair.import_token));
  sc_args.buffer_count(static_cast<uint32_t>(sc_buffer_collection_info.buffers()->size()));
  sc_args.size(fuchsia_math::SizeU{{.width = render_target_width, .height = render_target_height}});
  sc_args.rotation(fuc::Rotation::kCw180Degrees);

  auto config_res = screen_capture_->Configure(std::move(sc_args));
  ASSERT_TRUE(config_res.is_ok());

  // Take Screenshot!
  const auto cs_result = CaptureScreen(screen_capture_);
  EXPECT_TRUE(cs_result.is_ok());
  const auto& read_values =
      ExtractScreenCapture(cs_result.value().buffer_id().value(), sc_buffer_collection_info,
                           kBytesPerPixel, render_target_width, render_target_height);

  EXPECT_EQ(read_values.size(), write_values.size());

  // Compare read and write values.
  uint32_t num_green = 0;
  uint32_t num_red = 0;

  for (size_t i = 0; i < read_values.size(); i += kBytesPerPixel) {
    if (PixelEquals(&read_values[i], kGreen)) {
      num_green++;
      EXPECT_TRUE(PixelEquals(&write_values[i], kRed));
    } else if (PixelEquals(&read_values[i], kRed)) {
      num_red++;
      EXPECT_TRUE(PixelEquals(&write_values[i], kGreen));
    }
  }

  EXPECT_EQ(num_green, pixel_color_count);
  // TODO(https://fxbug.dev/42067818): Switch to exact comparisons after Astro precision issues are
  // resolved.
  EXPECT_NEAR(num_red, pixel_color_count, display_width_);
}

// Creates this image:
//          RRRRRGGGGG
//          RRRRRGGGGG
//          YYYYYBBBBB
//          YYYYYBBBBB
//
// Rotates into this image:
//          YYRR
//          YYRR
//          YYRR
//          YYRR
//          YYRR
//          BBGG
//          BBGG
//          BBGG
//          BBGG
//          BBGG
TEST_F(ScreenCaptureIntegrationTest, MultiColor90DegreeRotationScreenshot) {
  const uint32_t image_width = display_width_;
  const uint32_t image_height = display_height_;
  const uint32_t render_target_width = display_height_;
  const uint32_t render_target_height = display_width_;

  // Create Buffer Collection for image#1 to add to scene graph.
  auto ref_pair = allocation::cpp::BufferCollectionImportExportTokens::New();

  fuchsia_sysmem2::BufferCollectionInfo buffer_collection_info =
      CreateBufferCollectionInfoWithConstraints(
          utils::CreateDefaultConstraints(/*buffer_count=*/1, image_width, image_height),
          std::move(ref_pair.export_token), flatland_allocator_, sysmem_allocator_,
          fuc::RegisterBufferCollectionUsages::kDefault);

  // Write the image with the color scheme displayed in ASCII above.
  std::vector<uint8_t> write_values;

  uint32_t red_pixel_count = 0;
  uint32_t green_pixel_count = 0;
  uint32_t blue_pixel_count = 0;
  uint32_t yellow_pixel_count = 0;
  const uint32_t pixel_color_count = num_pixels_ / 4;

  for (uint32_t i = 0; i < num_pixels_; ++i) {
    uint32_t row = i / image_width;
    uint32_t col = i % image_width;

    // Top-left quadrant
    if (row < image_height / 2 && col < image_width / 2) {
      AppendPixel(&write_values, kRed);
      ++red_pixel_count;
    }
    // Top-right quadrant
    else if (row < image_height / 2 && col >= image_width / 2) {
      AppendPixel(&write_values, kGreen);
      ++green_pixel_count;
    }
    // Bottom-right quadrant
    else if (row >= image_height / 2 && col >= image_width / 2) {
      AppendPixel(&write_values, kBlue);
      ++blue_pixel_count;
    }
    // Bottom-left quadrant
    else if (row >= image_height / 2 && col < image_width / 2) {
      AppendPixel(&write_values, kYellow);
      ++yellow_pixel_count;
    }
  }

  EXPECT_EQ(red_pixel_count, pixel_color_count);
  EXPECT_EQ(green_pixel_count, pixel_color_count);
  EXPECT_EQ(blue_pixel_count, pixel_color_count);
  EXPECT_EQ(yellow_pixel_count, pixel_color_count);

  WriteToSysmemBuffer(write_values, buffer_collection_info, 0, kBytesPerPixel, image_width,
                      image_height);

  GenerateImageForFlatlandInstance(
      0, *child_session_, kChildRootTransform, std::move(ref_pair.import_token),
      fuchsia_math::SizeU{{.width = image_width, .height = image_height}},
      fuchsia_math::Vec{{.x = 0, .y = 0}}, 2, 2);
  BlockingPresent(this, *child_session_);

  // The scene graph is now ready for screenshotting!

  // Create buffer collection to render into for GetNextFrame().
  auto scr_ref_pair = allocation::cpp::BufferCollectionImportExportTokens::New();

  fuchsia_sysmem2::BufferCollectionInfo sc_buffer_collection_info =
      CreateBufferCollectionInfoWithConstraints(
          utils::CreateDefaultConstraints(/*buffer_count=*/1, render_target_width,
                                          render_target_height),
          std::move(scr_ref_pair.export_token), flatland_allocator_, sysmem_allocator_,
          fuc::RegisterBufferCollectionUsages::kScreenshot);

  // Configure buffers in ScreenCapture client.
  fuc::ScreenCaptureConfig sc_args;
  sc_args.import_token(std::move(scr_ref_pair.import_token));
  sc_args.buffer_count(static_cast<uint32_t>(sc_buffer_collection_info.buffers()->size()));
  sc_args.size(fuchsia_math::SizeU{{.width = render_target_width, .height = render_target_height}});
  sc_args.rotation(fuc::Rotation::kCw90Degrees);

  auto config_res = screen_capture_->Configure(std::move(sc_args));
  ASSERT_TRUE(config_res.is_ok());

  // Take Screenshot!
  const auto cs_result = CaptureScreen(screen_capture_);
  EXPECT_TRUE(cs_result.is_ok());
  const auto& read_values =
      ExtractScreenCapture(cs_result.value().buffer_id().value(), sc_buffer_collection_info,
                           kBytesPerPixel, render_target_width, render_target_height);

  EXPECT_EQ(read_values.size(), write_values.size());

  // Compare read and write values for each quadrant.
  uint32_t top_left_correct = 0;
  uint32_t top_right_correct = 0;
  uint32_t bottom_right_correct = 0;
  uint32_t bottom_left_correct = 0;

  for (uint32_t i = 0; i < num_pixels_; ++i) {
    uint32_t row = i / render_target_width;
    uint32_t col = i % render_target_width;
    const uint8_t* read_value = &read_values[i * kBytesPerPixel];

    // Top-left quadrant
    if (row < render_target_height / 2 && col < render_target_width / 2) {
      if (PixelEquals(read_value, kYellow))
        top_left_correct++;
    }
    // Top-right quadrant
    else if (row < render_target_height / 2 && col >= render_target_width / 2) {
      if (PixelEquals(read_value, kRed))
        top_right_correct++;
    }
    // Bottom-right quadrant
    else if (row >= render_target_height / 2 && col >= render_target_width / 2) {
      if (PixelEquals(read_value, kGreen))
        bottom_right_correct++;
    }
    // Bottom-left quadrant
    else if (row >= render_target_height / 2 && col < render_target_width / 2) {
      if (PixelEquals(read_value, kBlue))
        bottom_left_correct++;
    }
  }

  // TODO(https://fxbug.dev/42067818): Switch to exact comparisons after Astro precision issues are
  // resolved.
  EXPECT_NEAR(top_left_correct, pixel_color_count, display_width_);
  EXPECT_NEAR(top_right_correct, pixel_color_count, display_width_);
  EXPECT_NEAR(bottom_left_correct, pixel_color_count, display_width_);
  EXPECT_NEAR(bottom_right_correct, pixel_color_count, display_width_);
}

// Creates this image:
//          RRRRRGGGGG
//          RRRRRGGGGG
//          YYYYYBBBBB
//          YYYYYBBBBB
//
// Rotates into this image:
//          GGBB
//          GGBB
//          GGBB
//          GGBB
//          GGBB
//          RRYY
//          RRYY
//          RRYY
//          RRYY
//          RRYY
TEST_F(ScreenCaptureIntegrationTest, MultiColor270DegreeRotationScreenshot) {
  const uint32_t image_width = display_width_;
  const uint32_t image_height = display_height_;
  const uint32_t render_target_width = display_height_;
  const uint32_t render_target_height = display_width_;

  // Create Buffer Collection for image#1 to add to scene graph.
  auto ref_pair = allocation::cpp::BufferCollectionImportExportTokens::New();

  fuchsia_sysmem2::BufferCollectionInfo buffer_collection_info =
      CreateBufferCollectionInfoWithConstraints(
          utils::CreateDefaultConstraints(/*buffer_count=*/1, image_width, image_height),
          std::move(ref_pair.export_token), flatland_allocator_, sysmem_allocator_,
          fuc::RegisterBufferCollectionUsages::kDefault);

  // Write the image with the color scheme displayed in ASCII above.
  std::vector<uint8_t> write_values;

  uint32_t red_pixel_count = 0;
  uint32_t green_pixel_count = 0;
  uint32_t blue_pixel_count = 0;
  uint32_t yellow_pixel_count = 0;
  const uint32_t pixel_color_count = num_pixels_ / 4;

  for (uint32_t i = 0; i < num_pixels_; ++i) {
    uint32_t row = i / image_width;
    uint32_t col = i % image_width;

    // Top-left quadrant
    if (row < image_height / 2 && col < image_width / 2) {
      AppendPixel(&write_values, kRed);
      ++red_pixel_count;
    }
    // Top-right quadrant
    else if (row < image_height / 2 && col >= image_width / 2) {
      AppendPixel(&write_values, kGreen);
      ++green_pixel_count;
    }
    // Bottom-right quadrant
    else if (row >= image_height / 2 && col >= image_width / 2) {
      AppendPixel(&write_values, kBlue);
      ++blue_pixel_count;
    }
    // Bottom-left quadrant
    else if (row >= image_height / 2 && col < image_width / 2) {
      AppendPixel(&write_values, kYellow);
      ++yellow_pixel_count;
    }
  }

  EXPECT_EQ(red_pixel_count, pixel_color_count);
  EXPECT_EQ(green_pixel_count, pixel_color_count);
  EXPECT_EQ(blue_pixel_count, pixel_color_count);
  EXPECT_EQ(yellow_pixel_count, pixel_color_count);

  WriteToSysmemBuffer(write_values, buffer_collection_info, 0, kBytesPerPixel, image_width,
                      image_height);

  GenerateImageForFlatlandInstance(
      0, *child_session_, kChildRootTransform, std::move(ref_pair.import_token),
      fuchsia_math::SizeU{{.width = image_width, .height = image_height}},
      fuchsia_math::Vec{{.x = 0, .y = 0}}, 2, 2);
  BlockingPresent(this, *child_session_);

  // The scene graph is now ready for screenshotting!

  // Create buffer collection to render into for GetNextFrame().
  auto scr_ref_pair = allocation::cpp::BufferCollectionImportExportTokens::New();

  fuchsia_sysmem2::BufferCollectionInfo sc_buffer_collection_info =
      CreateBufferCollectionInfoWithConstraints(
          utils::CreateDefaultConstraints(/*buffer_count=*/1, render_target_width,
                                          render_target_height),
          std::move(scr_ref_pair.export_token), flatland_allocator_, sysmem_allocator_,
          fuc::RegisterBufferCollectionUsages::kScreenshot);

  // Configure buffers in ScreenCapture client.
  fuc::ScreenCaptureConfig sc_args;
  sc_args.import_token(std::move(scr_ref_pair.import_token));
  sc_args.buffer_count(static_cast<uint32_t>(sc_buffer_collection_info.buffers()->size()));
  sc_args.size(fuchsia_math::SizeU{{.width = render_target_width, .height = render_target_height}});
  sc_args.rotation(fuc::Rotation::kCw270Degrees);

  auto config_res = screen_capture_->Configure(std::move(sc_args));
  ASSERT_TRUE(config_res.is_ok());

  // Take Screenshot!
  const auto cs_result = CaptureScreen(screen_capture_);
  EXPECT_TRUE(cs_result.is_ok());
  const auto& read_values =
      ExtractScreenCapture(cs_result.value().buffer_id().value(), sc_buffer_collection_info,
                           kBytesPerPixel, render_target_width, render_target_height);

  EXPECT_EQ(read_values.size(), write_values.size());

  // Compare read and write values for each quadrant.
  uint32_t top_left_correct = 0;
  uint32_t top_right_correct = 0;
  uint32_t bottom_right_correct = 0;
  uint32_t bottom_left_correct = 0;

  for (uint32_t i = 0; i < num_pixels_; ++i) {
    uint32_t row = i / render_target_width;
    uint32_t col = i % render_target_width;
    const uint8_t* read_value = &read_values[i * kBytesPerPixel];

    // Top-left quadrant
    if (row < render_target_height / 2 && col < render_target_width / 2) {
      if (PixelEquals(read_value, kGreen))
        top_left_correct++;
    }
    // Top-right quadrant
    else if (row < render_target_height / 2 && col >= render_target_width / 2) {
      if (PixelEquals(read_value, kBlue))
        top_right_correct++;
    }
    // Bottom-right quadrant
    else if (row >= render_target_height / 2 && col >= render_target_width / 2) {
      if (PixelEquals(read_value, kYellow))
        bottom_right_correct++;
    }
    // Bottom-left quadrant
    else if (row >= render_target_height / 2 && col < render_target_width / 2) {
      if (PixelEquals(read_value, kRed))
        bottom_left_correct++;
    }
  }

  // TODO(https://fxbug.dev/42067818): Switch to exact comparisons after Astro precision issues are
  // resolved.
  EXPECT_NEAR(top_left_correct, pixel_color_count, display_width_);
  EXPECT_NEAR(top_right_correct, pixel_color_count, display_width_);
  EXPECT_NEAR(bottom_left_correct, pixel_color_count, display_width_);
  EXPECT_NEAR(bottom_right_correct, pixel_color_count, display_width_);
}

TEST_F(ScreenCaptureIntegrationTest, FilledRectScreenshot) {
  const uint32_t image_width = display_width_;
  const uint32_t image_height = display_height_;
  const uint32_t render_target_width = display_width_;
  const uint32_t render_target_height = display_height_;

  const fuc::ContentId kFilledRectId(1);
  const fuc::TransformId kTransformId(2);

  // Create a fuchsia colored rectangle.
  FX_CHECK((*child_session_)->CreateFilledRect({{.rect_id = kFilledRectId}}).is_ok());
  FX_CHECK((*child_session_)
               ->SetSolidFill({{.rect_id = kFilledRectId,
                                .color = {{.red = 1, .green = 0, .blue = 1, .alpha = 1}},
                                .size = {{.width = image_width, .height = image_height}}}})
               .is_ok());

  // Associate the rect with a transform.
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

  // The scene graph is now ready for screencapturing!

  // Create buffer collection to render into for GetNextFrame().
  auto scr_ref_pair = allocation::cpp::BufferCollectionImportExportTokens::New();

  fuchsia_sysmem2::BufferCollectionInfo sc_buffer_collection_info =
      CreateBufferCollectionInfoWithConstraints(
          utils::CreateDefaultConstraints(/*buffer_count=*/1, render_target_width,
                                          render_target_height),
          std::move(scr_ref_pair.export_token), flatland_allocator_, sysmem_allocator_,
          fuc::RegisterBufferCollectionUsages::kScreenshot);

  // Configure buffers in ScreenCapture client.
  fuc::ScreenCaptureConfig sc_args;
  sc_args.import_token(std::move(scr_ref_pair.import_token));
  sc_args.size(fuchsia_math::SizeU{{.width = render_target_width, .height = render_target_height}});
  sc_args.buffer_count(static_cast<uint32_t>(sc_buffer_collection_info.buffers()->size()));

  auto config_res = screen_capture_->Configure(std::move(sc_args));
  ASSERT_TRUE(config_res.is_ok());

  // Take Screenshot!
  const auto cs_result = CaptureScreen(screen_capture_);
  EXPECT_TRUE(cs_result.is_ok());
  const auto& read_values =
      ExtractScreenCapture(cs_result.value().buffer_id().value(), sc_buffer_collection_info,
                           kBytesPerPixel, render_target_width, render_target_height);

  EXPECT_EQ(read_values.size(), num_pixels_ * kBytesPerPixel);

  // Compare read and write values.
  uint32_t num_fuchsia_count = 0;
  static constexpr uint8_t kFuchsia[] = {255, 0, 255, 255};

  for (size_t i = 0; i < read_values.size(); i += kBytesPerPixel) {
    if (PixelEquals(&read_values[i], kFuchsia))
      num_fuchsia_count++;
  }

  EXPECT_EQ(num_fuchsia_count, num_pixels_);
}

TEST_F(ScreenCaptureIntegrationTest, ChangeFilledRectScreenshots) {
  const uint32_t image_width = display_width_;
  const uint32_t image_height = display_height_;
  const uint32_t render_target_width = display_width_;
  const uint32_t render_target_height = display_height_;

  const fuc::ContentId kFilledRectId(1);
  const fuc::TransformId kTransformId(2);

  // Create a red rectangle.
  FX_CHECK((*child_session_)->CreateFilledRect({{.rect_id = kFilledRectId}}).is_ok());
  // Set as RGBA. Corresponds to kRed.
  FX_CHECK((*child_session_)
               ->SetSolidFill({{.rect_id = kFilledRectId,
                                .color = {{.red = 1, .green = 0, .blue = 0, .alpha = 1}},
                                .size = {{.width = image_width, .height = image_height}}}})
               .is_ok());

  // Associate the rect with a transform.
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

  // The scene graph is now ready for screencapturing!

  // Create buffer collection to render into for GetNextFrame().
  auto scr_ref_pair = allocation::cpp::BufferCollectionImportExportTokens::New();

  fuchsia_sysmem2::BufferCollectionInfo sc_buffer_collection_info =
      CreateBufferCollectionInfoWithConstraints(
          utils::CreateDefaultConstraints(/*buffer_count=*/2, render_target_width,
                                          render_target_height),
          std::move(scr_ref_pair.export_token), flatland_allocator_, sysmem_allocator_,
          fuc::RegisterBufferCollectionUsages::kScreenshot);

  // Configure buffers in ScreenCapture client.
  fuc::ScreenCaptureConfig sc_args;
  sc_args.import_token(std::move(scr_ref_pair.import_token));
  sc_args.size(fuchsia_math::SizeU{{.width = render_target_width, .height = render_target_height}});
  sc_args.buffer_count(static_cast<uint32_t>(sc_buffer_collection_info.buffers()->size()));

  auto config_res = screen_capture_->Configure(std::move(sc_args));
  ASSERT_TRUE(config_res.is_ok());

  // Take Screenshot!
  const auto cs_result = CaptureScreen(screen_capture_);
  EXPECT_TRUE(cs_result.is_ok());
  const auto& read_values =
      ExtractScreenCapture(cs_result.value().buffer_id().value(), sc_buffer_collection_info,
                           kBytesPerPixel, render_target_width, render_target_height);

  EXPECT_EQ(read_values.size(), num_pixels_ * kBytesPerPixel);

  // Compare read and write values.
  uint32_t num_red_count = 0;

  for (size_t i = 0; i < read_values.size(); i += kBytesPerPixel) {
    if (PixelEquals(&read_values[i], kRed))
      num_red_count++;
  }

  EXPECT_EQ(num_red_count, num_pixels_);

  // Now change the color of the screen.

  const fuc::ContentId kFilledRectId2(2);
  const fuc::TransformId kTransformId2(3);

  // Create a blue rectangle.
  FX_CHECK((*child_session_)->CreateFilledRect({{.rect_id = kFilledRectId2}}).is_ok());
  // Set as RGBA. Corresponds to kBlue.
  FX_CHECK((*child_session_)
               ->SetSolidFill({{.rect_id = kFilledRectId2,
                                .color = {{.red = 0, .green = 0, .blue = 1, .alpha = 1}},
                                .size = {{.width = image_width, .height = image_height}}}})
               .is_ok());

  // Associate the rect with a transform.
  FX_CHECK((*child_session_)->CreateTransform({{.transform_id = kTransformId2}}).is_ok());
  FX_CHECK((*child_session_)
               ->SetContent({{.transform_id = kTransformId2, .content_id = kFilledRectId2}})
               .is_ok());

  // Attach the transform to the scene
  FX_CHECK((*child_session_)
               ->AddChild({{.parent_transform_id = kChildRootTransform,
                            .child_transform_id = kTransformId2}})
               .is_ok());
  BlockingPresent(this, *child_session_);

  // The scene graph is now ready for screencapturing!

  // Take Screenshot!
  const auto cs_result2 = CaptureScreen(screen_capture_);
  EXPECT_TRUE(cs_result2.is_ok());
  const auto& read_values2 =
      ExtractScreenCapture(cs_result2.value().buffer_id().value(), sc_buffer_collection_info,
                           kBytesPerPixel, render_target_width, render_target_height);
  EXPECT_EQ(read_values2.size(), num_pixels_ * kBytesPerPixel);

  // Compare read and write values.
  uint32_t num_blue_count = 0;

  for (size_t i = 0; i < read_values2.size(); i += kBytesPerPixel) {
    if (PixelEquals(&read_values2[i], kBlue))
      num_blue_count++;
  }

  EXPECT_EQ(num_blue_count, num_pixels_);
}

}  // namespace integration_tests

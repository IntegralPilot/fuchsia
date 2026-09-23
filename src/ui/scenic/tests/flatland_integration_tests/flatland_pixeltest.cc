// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.images2/cpp/fidl.h>
#include <fidl/fuchsia.math/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <fidl/fuchsia.ui.display.singleton/cpp/fidl.h>
#include <fidl/fuchsia.ui.views/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/ui/scenic/cpp/buffer_collection_import_export_tokens.h>
#include <lib/ui/scenic/cpp/view_creation_tokens.h>
#include <lib/ui/scenic/cpp/view_identity.h>

#include <cmath>
#include <cstdint>
#include <optional>
#include <tuple>
#include <unordered_map>
#include <utility>

#include <zxtest/zxtest.h>

#include "src/ui/scenic/lib/utils/helpers.h"
#include "src/ui/scenic/tests/utils/blocking_present.h"
#include "src/ui/scenic/tests/utils/flatland_client_with_event_handler.h"
#include "src/ui/scenic/tests/utils/scenic_ctf_test_base.h"
#include "src/ui/scenic/tests/utils/simple_watcher_client.h"
#include "src/ui/scenic/tests/utils/utils.h"
#include "src/ui/testing/util/screenshot_helper.h"

namespace integration_tests {

namespace fuc = fuchsia_ui_composition;
namespace fuv = fuchsia_ui_views;

#define EXPECT_NEAR(val1, val2, eps)                                         \
  EXPECT_LE(std::abs(static_cast<double>(val1) - static_cast<double>(val2)), \
            static_cast<double>(eps))

const fuc::TransformId kRootTransform(1);
constexpr auto kEpsilon = 1;

fuc::ColorRgba GetColorInFloat(utils::Pixel color) {
  return {{.red = static_cast<float>(color.red) / 255.f,
           .green = static_cast<float>(color.green) / 255.f,
           .blue = static_cast<float>(color.blue) / 255.f,
           .alpha = static_cast<float>(color.alpha) / 255.f}};
}

// Asserts whether the BGRA channel value difference between |actual| and |expected| is at most
// |kEpsilon|.
void CompareColor(utils::Pixel actual, utils::Pixel expected) {
  EXPECT_NEAR(actual.blue, expected.blue, kEpsilon);
  EXPECT_NEAR(actual.green, expected.green, kEpsilon);
  EXPECT_NEAR(actual.red, expected.red, kEpsilon);
  EXPECT_NEAR(actual.alpha, expected.alpha, kEpsilon);
}

// Test fixture that sets up an environment with a Scenic we can connect to.
class FlatlandPixelTestBase : public ScenicCtfTest {
 public:
  void SetUp() override {
    ScenicCtfTest::SetUp();

    auto [client_end, server_end] = fidl::Endpoints<fuchsia_sysmem2::Allocator>::Create();
    LocalServiceDirectory()->Connect("fuchsia.sysmem2.Allocator", server_end.TakeChannel());
    sysmem_allocator_.Bind(std::move(client_end), dispatcher());

    flatland_allocator_ = ConnectSyncIntoRealm<fuc::Allocator>();

    // Create a root view.
    root_flatland_.emplace(ConnectIntoRealm<fuc::Flatland>(), dispatcher());
    root_flatland_->set_on_error([](fidl::Event<fuc::Flatland::OnError>& event) {
      FX_LOGS(ERROR) << "Flatland OnError: " << fidl::ToUnderlying(event.error());
      FAIL();
    });
    root_flatland_->set_on_close(FailOnClose("Lost connection to Scenic"));

    // Attach |root_flatland_| as the only Flatland under the environment's FlatlandDisplay.
    auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
    SetFlatlandDisplayContent(std::move(parent_token));
    auto [pv_client_end, pv_server_end] = fidl::Endpoints<fuc::ParentViewportWatcher>::Create();
    parent_viewport_watcher_ = std::make_unique<SimpleWatcherClient<fuc::ParentViewportWatcher>>(
        std::move(pv_client_end), dispatcher());
    FX_CHECK((*root_flatland_)
                 ->CreateView2({{.token = std::move(child_token),
                                 .view_identity = scenic::cpp::NewViewIdentityOnCreation(),
                                 .protocols = {},
                                 .parent_viewport_watcher = std::move(pv_server_end)}})
                 .is_ok());

    // Create the root transform.
    FX_CHECK((*root_flatland_)->CreateTransform({{.transform_id = kRootTransform}}).is_ok());
    FX_CHECK((*root_flatland_)->SetRootTransform({{.transform_id = kRootTransform}}).is_ok());

    // Get the display's width and height. Since there is no Present in FlatlandDisplay, receiving
    // this callback ensures that all FlatlandDisplay calls are processed.
    std::optional<fuc::LayoutInfo> info;
    parent_viewport_watcher_->client()->GetLayout().Then(
        [&info](fidl::Result<fuc::ParentViewportWatcher::GetLayout>& result) {
          if (result.is_ok()) {
            info = std::move(result->info());
          }
        });
    RunLoopUntil([&info] { return info.has_value(); });
    display_width_ = info->logical_size()->width();
    display_height_ = info->logical_size()->height();

    screenshotter_ = ConnectSyncIntoRealm<fuc::Screenshot>();
  }

  void TearDown() override {
    root_flatland_.reset();

    zxtest::Test::TearDown();
  }

  // `ScenicCtfTest`:
  uint32_t GetDisplayMaxLayerCount() const override { return 4; }
  bool UseDisplayComposition() const override { return true; }

  // Draws a rectangle of size |width|*|height|, color |color|, opacity |opacity| and origin
  // (|x|,|y|) in |flatland|'s view.
  // Note: |BlockingPresent| must be called after this function to present the rectangle on the
  // display.
  void DrawRectangle(FlatlandClientWithEventHandler& flatland, uint32_t width, uint32_t height,
                     int32_t x, int32_t y, utils::Pixel color,
                     fuc::BlendMode blend_mode = fuc::BlendMode::kSrc, float opacity = 1.f) {
    const fuc::ContentId kFilledRectId(get_next_resource_id());
    const fuc::TransformId kTransformId(get_next_resource_id());

    FX_CHECK(flatland->CreateFilledRect({{.rect_id = kFilledRectId}}).is_ok());
    FX_CHECK(flatland
                 ->SetSolidFill({{.rect_id = kFilledRectId,
                                  .color = GetColorInFloat(color),
                                  .size = {{.width = width, .height = height}}}})
                 .is_ok());

    // Associate the rect with a transform.
    FX_CHECK(flatland->CreateTransform({{.transform_id = kTransformId}}).is_ok());
    FX_CHECK(flatland->SetContent({{.transform_id = kTransformId, .content_id = kFilledRectId}})
                 .is_ok());
    FX_CHECK(
        flatland
            ->SetTranslation({{.transform_id = kTransformId, .translation = {{.x = x, .y = y}}}})
            .is_ok());

    // Set the opacity and the BlendMode for the rectangle.
    FX_CHECK(
        flatland->SetImageBlendingFunction({{.image_id = kFilledRectId, .blend_mode = blend_mode}})
            .is_ok());
    FX_CHECK(flatland->SetOpacity({{.transform_id = kTransformId, .value = opacity}}).is_ok());

    // Attach the transform to the view.
    FX_CHECK(flatland
                 ->AddChild(
                     {{.parent_transform_id = kRootTransform, .child_transform_id = kTransformId}})
                 .is_ok());
  }

  fuchsia_sysmem2::BufferCollectionConstraints GetBufferConstraints(
      fuchsia_images2::PixelFormat pixel_format, fuchsia_images2::ColorSpace color_space) {
    fuchsia_sysmem2::BufferCollectionConstraints constraints;
    constraints.buffer_memory_constraints() = fuchsia_sysmem2::BufferMemoryConstraints();
    constraints.buffer_memory_constraints()->ram_domain_supported(true);
    constraints.buffer_memory_constraints()->cpu_domain_supported(true);
    constraints.usage() = fuchsia_sysmem2::BufferUsage();
    constraints.usage()->cpu(fuchsia_sysmem2::kCpuUsageWriteOften);
    constraints.min_buffer_count(1);
    fuchsia_sysmem2::ImageFormatConstraints image_constraints;
    image_constraints.pixel_format(pixel_format);
    image_constraints.pixel_format_modifier(fuchsia_images2::PixelFormatModifier::kLinear);
    image_constraints.color_spaces(std::vector{color_space});
    image_constraints.required_min_size(
        fuchsia_math::SizeU{{.width = display_width_, .height = display_height_}});
    image_constraints.required_max_size(
        fuchsia_math::SizeU{{.width = display_width_, .height = display_height_}});
    constraints.image_format_constraints(std::vector{std::move(image_constraints)});
    return constraints;
  }

  // Draws the following coordinate test pattern without views:
  // ___________________________________
  // |                |                |
  // |     BLACK      |        RED     |
  // |           _____|_____           |
  // |___________|  GREEN  |___________|
  // |           |_________|           |
  // |                |                |
  // |      BLUE      |     MAGENTA    |
  // |________________|________________|
  //
  void Draw4RectanglesToDisplay() {
    const uint32_t view_width = display_width_;
    const uint32_t view_height = display_height_;

    const uint32_t pane_width =
        static_cast<uint32_t>(std::ceil(static_cast<float>(view_width) / 2.f));

    const uint32_t pane_height =
        static_cast<uint32_t>(std::ceil(static_cast<float>(view_height) / 2.f));

    // Draw the rectangles in the quadrants.
    for (uint32_t i = 0; i < 2; i++) {
      for (uint32_t j = 0; j < 2; j++) {
        utils::Pixel color(static_cast<uint8_t>(j * 255), 0, static_cast<uint8_t>(i * 255), 255);
        DrawRectangle(*root_flatland_, pane_width, pane_height, i * pane_width, j * pane_height,
                      color);
      }
    }

    // Draw the rectangle in the center.
    DrawRectangle(*root_flatland_, view_width / 4, view_height / 4, 3 * view_width / 8,
                  3 * view_height / 8, utils::kGreen);
  }

 protected:
  std::optional<fuchsia_sysmem2::BufferCollectionInfo> SetConstraintsAndAllocateBuffer(
      fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token,
      fuchsia_sysmem2::BufferCollectionConstraints constraints) {
    auto [collection_client_end, collection_server_end] =
        fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();
    fidl::SyncClient<fuchsia_sysmem2::BufferCollection> buffer_collection(
        std::move(collection_client_end));

    fidl::Arena arena;
    fidl::OneWayStatus bind_status = sysmem_allocator_->BindSharedCollection(
        fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
            .token(std::move(token))
            .buffer_collection_request(std::move(collection_server_end))
            .Build());
    FX_CHECK(bind_status.ok());

    uint32_t constraints_min_buffer_count = constraints.min_buffer_count().value_or(1);

    fuchsia_sysmem2::BufferCollectionSetConstraintsRequest set_constraints_request;
    set_constraints_request.constraints(std::move(constraints));
    auto set_constraints_res =
        buffer_collection->SetConstraints(std::move(set_constraints_request));
    FX_CHECK(set_constraints_res.is_ok());

    auto wait_result = buffer_collection->WaitForAllBuffersAllocated();
    if (wait_result.is_error() || !wait_result->buffer_collection_info().has_value()) {
      return std::nullopt;
    }

    auto buffer_collection_info = std::move(wait_result->buffer_collection_info().value());
    EXPECT_EQ(constraints_min_buffer_count, buffer_collection_info.buffers()->size());
    FX_CHECK(buffer_collection->Release().is_ok());
    return buffer_collection_info;
  }

  uint32_t display_width_ = 0;
  uint32_t display_height_ = 0;

  fidl::WireClient<fuchsia_sysmem2::Allocator> sysmem_allocator_;
  fidl::SyncClient<fuc::Allocator> flatland_allocator_;
  std::optional<FlatlandClientWithEventHandler> root_flatland_;
  std::unique_ptr<SimpleWatcherClient<fuc::ParentViewportWatcher>> parent_viewport_watcher_;
  fidl::SyncClient<fuc::Screenshot> screenshotter_;
  uint64_t get_next_resource_id() { return resource_id_++; }

 private:
  uint64_t resource_id_ = kRootTransform.value() + 1;
};

class ParameterizedPixelFormatTest
    : public FlatlandPixelTestBase,
      public zxtest::WithParamInterface<fuchsia_images2::PixelFormat> {};

class ParameterizedYUVPixelTest : public ParameterizedPixelFormatTest {};

INSTANTIATE_TEST_SUITE_P(YuvPixelFormats, ParameterizedYUVPixelTest,
                         zxtest::Values(fuchsia_images2::PixelFormat::kNv12,
                                        fuchsia_images2::PixelFormat::kI420));

TEST_P(ParameterizedYUVPixelTest, YUVTest) {
  auto [local_token, scenic_token] = utils::SysmemTokens::Create(sysmem_allocator_);

  // Send one token to Flatland Allocator.
  auto bc_tokens = allocation::cpp::BufferCollectionImportExportTokens::New();
  fuc::RegisterBufferCollectionArgs rbc_args;
  rbc_args.export_token(std::move(bc_tokens.export_token));
  rbc_args.buffer_collection_token2(
      fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken>(scenic_token.TakeChannel()));
  auto result = flatland_allocator_->RegisterBufferCollection(std::move(rbc_args));
  ASSERT_TRUE(result.is_ok());

  // Use the local token to allocate a protected buffer.
  auto info = SetConstraintsAndAllocateBuffer(
      std::move(local_token),
      GetBufferConstraints(GetParam(), fuchsia_images2::ColorSpace::kRec709));
  if (!info) {
    ZXTEST_SKIP(
        "Sysmem allocation failed. The device may not support the requested pixel format (e.g. YUV on emulator).");
    return;
  }

  // Write the pixel values to the VMO.
  const uint32_t num_pixels = display_width_ * display_height_;
  const uint64_t image_vmo_bytes = (3 * num_pixels) / 2;

  zx::vmo& image_vmo = info->buffers()->at(0).vmo().value();
  zx_status_t status = zx::vmo::create(image_vmo_bytes, 0, &image_vmo);
  EXPECT_EQ(ZX_OK, status);

  uint8_t* vmo_base;
  status = zx::vmar::root_self()->map(ZX_VM_PERM_WRITE | ZX_VM_PERM_READ, 0, image_vmo, 0,
                                      image_vmo_bytes, reinterpret_cast<uintptr_t*>(&vmo_base));
  EXPECT_EQ(ZX_OK, status);

  static const uint8_t kYValue = 110;
  static const uint8_t kUValue = 192;
  static const uint8_t kVValue = 192;

  // Set all the Y pixels at full res.
  for (uint32_t i = 0; i < num_pixels; ++i) {
    vmo_base[i] = kYValue;
  }

  if (GetParam() == fuchsia_images2::PixelFormat::kNv12) {
    // Set all the UV pixels pairwise at half res.
    for (uint32_t i = num_pixels; i < image_vmo_bytes; i += 2) {
      vmo_base[i] = kUValue;
      vmo_base[i + 1] = kVValue;
    }
  } else if (GetParam() == fuchsia_images2::PixelFormat::kI420) {
    for (uint32_t i = num_pixels; i < num_pixels + num_pixels / 4; ++i) {
      vmo_base[i] = kUValue;
    }
    for (uint32_t i = num_pixels + num_pixels / 4; i < image_vmo_bytes; ++i) {
      vmo_base[i] = kVValue;
    }
  } else {
    FX_NOTREACHED();
  }

  // Flush the cache after writing to host VMO.
  EXPECT_EQ(ZX_OK, zx_cache_flush(vmo_base, image_vmo_bytes,
                                  ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE));

  // Create the image in the Flatland instance.
  fuc::ImageProperties image_properties;
  image_properties.size(fuchsia_math::SizeU{{.width = display_width_, .height = display_height_}});
  const fuc::ContentId kImageContentId(1);

  FX_CHECK((*root_flatland_)
               ->CreateImage({{.image_id = kImageContentId,
                               .import_token = std::move(bc_tokens.import_token),
                               .vmo_index = 0,
                               .properties = std::move(image_properties)}})
               .is_ok());

  // Present the created Image.
  FX_CHECK((*root_flatland_)
               ->SetContent({{.transform_id = kRootTransform, .content_id = kImageContentId}})
               .is_ok());
  BlockingPresent(this, *root_flatland_);

  // TODO(https://fxbug.dev/42144501): provide reasoning for why this is the correct expected color.
  const utils::Pixel expected_pixel(255, 85, 249, 255);

  auto screenshot = TakeScreenshot(screenshotter_, display_width_, display_height_);
  auto histogram = screenshot.Histogram();
  EXPECT_EQ(histogram[expected_pixel], num_pixels);
}

class ParameterizedSRGBPixelTest : public ParameterizedPixelFormatTest {};

INSTANTIATE_TEST_SUITE_P(RgbPixelFormats, ParameterizedSRGBPixelTest,
                         zxtest::Values(fuchsia_images2::PixelFormat::kB8G8R8A8,
                                        fuchsia_images2::PixelFormat::kR8G8B8A8));

INSTANTIATE_TEST_SUITE_P(ExoticRgbPixelFormats, ParameterizedSRGBPixelTest,
                         zxtest::Values(fuchsia_images2::PixelFormat::kA2B10G10R10,
                                        fuchsia_images2::PixelFormat::kR8,
                                        fuchsia_images2::PixelFormat::kR5G6B5));

TEST_P(ParameterizedSRGBPixelTest, RGBTest) {
  auto [local_token, scenic_token] = utils::SysmemTokens::Create(sysmem_allocator_);

  // Send one token to Flatland Allocator.
  auto bc_tokens = allocation::cpp::BufferCollectionImportExportTokens::New();
  fuc::RegisterBufferCollectionArgs rbc_args;
  rbc_args.export_token(std::move(bc_tokens.export_token));
  rbc_args.buffer_collection_token2(
      fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken>(scenic_token.TakeChannel()));
  auto result = flatland_allocator_->RegisterBufferCollection(std::move(rbc_args));
  ASSERT_TRUE(result.is_ok());

  uint32_t bytes_per_pixel = 4;
  if (GetParam() == fuchsia_images2::PixelFormat::kR5G6B5) {
    bytes_per_pixel = 2;
  } else if (GetParam() == fuchsia_images2::PixelFormat::kR8) {
    bytes_per_pixel = 1;
  }

  // Use the local token to allocate a protected buffer.
  auto info = SetConstraintsAndAllocateBuffer(
      std::move(local_token), GetBufferConstraints(GetParam(), fuchsia_images2::ColorSpace::kSrgb));
  if (!info) {
    ZXTEST_SKIP("Unsupported constraints.");
  }

  // Write the pixel values to the VMO.
  const uint32_t num_pixels = display_width_ * display_height_;
  const uint64_t image_vmo_bytes = num_pixels * bytes_per_pixel;
  ASSERT_EQ(image_vmo_bytes, info->settings()->buffer_settings()->size_bytes().value_or(0));

  const zx::vmo& image_vmo = info->buffers()->at(0).vmo().value();

  uint8_t* vmo_base;
  auto status =
      zx::vmar::root_self()->map(ZX_VM_PERM_WRITE | ZX_VM_PERM_READ, 0, image_vmo, 0,
                                 image_vmo_bytes, reinterpret_cast<uintptr_t*>(&vmo_base));
  EXPECT_EQ(ZX_OK, status);

  utils::Pixel color = utils::kBlue;
  uint8_t color_channel = color.blue;
  vmo_base += info->buffers()->at(0).vmo_usable_start().value_or(0);

  for (uint32_t i = 0; i < num_pixels * bytes_per_pixel; i += bytes_per_pixel) {
    if (GetParam() == fuchsia_images2::PixelFormat::kR5G6B5) {
      uint16_t color16 = static_cast<uint16_t>(((color.red >> 3) << 11) |
                                               ((color.green >> 2) << 5) | (color.blue >> 3));
      *reinterpret_cast<uint16_t*>(&vmo_base[i]) = color16;
    } else if (GetParam() == fuchsia_images2::PixelFormat::kA2B10G10R10) {
      uint16_t alpha = static_cast<uint16_t>(color.alpha) >> 6;
      uint16_t blue = static_cast<uint16_t>(color.blue << 2);
      uint16_t green = static_cast<uint16_t>(color.green << 2);
      uint16_t red = static_cast<uint16_t>(color.red << 2);
      uint32_t color32 = (alpha << 30) | (blue << 20) | (green << 10) | red;
      *reinterpret_cast<uint32_t*>(&vmo_base[i]) = color32;
    } else if (GetParam() == fuchsia_images2::PixelFormat::kR8) {
      *reinterpret_cast<uint8_t*>(&vmo_base[i]) = color_channel;
    } else {
      // For BGRA32 pixel format, the first and the third byte in the pixel corresponds to the blue
      // and the red channel respectively.
      if (GetParam() == fuchsia_images2::PixelFormat::kB8G8R8A8) {
        vmo_base[i] = color.blue;
        vmo_base[i + 2] = color.red;
      }
      // For R8G8B8A8 pixel format, the first and the third byte in the pixel corresponds to the red
      // and the blue channel respectively.
      if (GetParam() == fuchsia_images2::PixelFormat::kR8G8B8A8) {
        vmo_base[i] = color.red;
        vmo_base[i + 2] = color.blue;
      }
      vmo_base[i + 1] = color.green;
      vmo_base[i + 3] = color.alpha;
    }
  }

  if (info->settings()->buffer_settings()->coherency_domain() ==
      fuchsia_sysmem2::CoherencyDomain::kRam) {
    EXPECT_EQ(ZX_OK, zx_cache_flush(vmo_base, image_vmo_bytes, ZX_CACHE_FLUSH_DATA));
  }

  // Create the image in the Flatland instance.
  fuc::ImageProperties image_properties;
  image_properties.size(fuchsia_math::SizeU{{.width = display_width_, .height = display_height_}});
  const fuc::ContentId kImageContentId(1);

  FX_CHECK((*root_flatland_)
               ->CreateImage({{.image_id = kImageContentId,
                               .import_token = std::move(bc_tokens.import_token),
                               .vmo_index = 0,
                               .properties = std::move(image_properties)}})
               .is_ok());

  // Present the created Image.
  FX_CHECK((*root_flatland_)
               ->SetContent({{.transform_id = kRootTransform, .content_id = kImageContentId}})
               .is_ok());
  BlockingPresent(this, *root_flatland_);

  fuc::ScreenshotFormat ss_format;
  switch (GetParam()) {
    case fuchsia_images2::PixelFormat::kB8G8R8A8:
      ss_format = fuc::ScreenshotFormat::kBgraRaw;
      break;
    case fuchsia_images2::PixelFormat::kR8G8B8A8:
    case fuchsia_images2::PixelFormat::kA2B10G10R10:
    case fuchsia_images2::PixelFormat::kR8:
      ss_format = fuc::ScreenshotFormat::kRgbaRaw;
      break;
    case fuchsia_images2::PixelFormat::kR5G6B5:
      ss_format = fuc::ScreenshotFormat::kBgraRaw;
      break;
    default:
      FX_LOGS(ERROR) << "Unexpected PixelFormat: " << fidl::ToUnderlying(GetParam());
      FAIL();
  }
  auto screenshot = TakeScreenshot(screenshotter_, display_width_, display_height_, ss_format);
  auto histogram = screenshot.Histogram();

  if (GetParam() == fuchsia_images2::PixelFormat::kR5G6B5) {
    color.alpha = 0xff;
  } else if (GetParam() == fuchsia_images2::PixelFormat::kR8) {
    color.alpha = 0xff;
    color.red = color_channel;
    color.green = 0;
    color.blue = 0;
  }
  EXPECT_EQ(histogram[color], num_pixels);
}

// Test a combination of orientations and image flips to ensure that images are flipped before the
// parent transform orientation is set and that the output is the expected output. For an ASCII
// representation of the input image, see |GetImageColorSetter|. For an ASCII representation of the
// expected output, see the constructor for |ParameterizedFlipAndOrientationTest|.
using FlipAndOrientationTestParams = std::tuple<fuc::Orientation, fuc::ImageFlip>;

class ParameterizedFlipAndOrientationTest
    : public FlatlandPixelTestBase,
      public zxtest::WithParamInterface<FlipAndOrientationTestParams> {
 protected:
  ParameterizedFlipAndOrientationTest() {
    // Image flip: LEFT_RIGHT; Orientation: CCW_0.
    //
    // |Bk|R |     |R |Bk|
    // |--|--| --> |--|--|
    // |G |Be|     |Be|G |
    //
    expected_colors_map.insert(
        {std::make_pair(fuc::ImageFlip::kLeftRight, fuc::Orientation::kCcw0Degrees),
         {.top_left = utils::kRed,
          .top_right = utils::kBlack,
          .bottom_left = utils::kBlue,
          .bottom_right = utils::kGreen}});

    // Image flip: LEFT_RIGHT; Orientation: CCW_90.
    //
    // |Bk|R |     |R |Bk|     |Bk|G |
    // |--|--| --> |--|--| --> |--|--|
    // |G |Be|     |Be|G |     |R |Be|
    //
    expected_colors_map.insert(
        {std::make_pair(fuc::ImageFlip::kLeftRight, fuc::Orientation::kCcw90Degrees),
         {.top_left = utils::kBlack,
          .top_right = utils::kGreen,
          .bottom_left = utils::kRed,
          .bottom_right = utils::kBlue}});

    // Image flip: LEFT_RIGHT; Orientation: CCW_180.
    //
    // |Bk|R |     |R |Bk|     |G |Be|
    // |--|--| --> |--|--| --> |--|--|
    // |G |Be|     |Be|G |     |Bk|R |
    //
    expected_colors_map.insert(
        {std::make_pair(fuc::ImageFlip::kLeftRight, fuc::Orientation::kCcw180Degrees),
         {.top_left = utils::kGreen,
          .top_right = utils::kBlue,
          .bottom_left = utils::kBlack,
          .bottom_right = utils::kRed}});

    // Image flip: LEFT_RIGHT; Orientation: CCW_270.
    //
    // |Bk|R |     |R |Bk|     |Be|R |
    // |--|--| --> |--|--| --> |--|--|
    // |G |Be|     |Be|G |     |G |Bk|
    //
    expected_colors_map.insert(
        {std::make_pair(fuc::ImageFlip::kLeftRight, fuc::Orientation::kCcw270Degrees),
         {.top_left = utils::kBlue,
          .top_right = utils::kRed,
          .bottom_left = utils::kGreen,
          .bottom_right = utils::kBlack}});

    // Image flip: UP_DOWN; Orientation: CCW_0.
    //
    // |Bk|R |     |G |Be|
    // |--|--| --> |--|--|
    // |G |Be|     |Bk|R |
    //
    expected_colors_map.insert(
        {std::make_pair(fuc::ImageFlip::kUpDown, fuc::Orientation::kCcw0Degrees),
         {.top_left = utils::kGreen,
          .top_right = utils::kBlue,
          .bottom_left = utils::kBlack,
          .bottom_right = utils::kRed}});

    // Image flip: UP_DOWN; Orientation: CCW_90.
    //
    // |Bk|R |     |G |Be|     |Be|R |
    // |--|--| --> |--|--| --> |--|--|
    // |G |Be|     |Bk|R |     |G |Bk|
    //
    expected_colors_map.insert(
        {std::make_pair(fuc::ImageFlip::kUpDown, fuc::Orientation::kCcw90Degrees),
         {.top_left = utils::kBlue,
          .top_right = utils::kRed,
          .bottom_left = utils::kGreen,
          .bottom_right = utils::kBlack}});

    // Image flip: UP_DOWN; Orientation: CCW_180.
    //
    // |Bk|R |     |G |Be|     |R |Bk|
    // |--|--| --> |--|--| --> |--|--|
    // |G |Be|     |Bk|R |     |Be|G |
    //
    expected_colors_map.insert(
        {std::make_pair(fuc::ImageFlip::kUpDown, fuc::Orientation::kCcw180Degrees),
         {.top_left = utils::kRed,
          .top_right = utils::kBlack,
          .bottom_left = utils::kBlue,
          .bottom_right = utils::kGreen}});

    // Image flip: UP_DOWN; Orientation: CCW_270.
    //
    // |Bk|R |     |G |Be|     |Bk|G |
    // |--|--| --> |--|--| --> |--|--|
    // |G |Be|     |Bk|R |     |R |Be|
    //
    expected_colors_map.insert(
        {std::make_pair(fuc::ImageFlip::kUpDown, fuc::Orientation::kCcw270Degrees),
         {.top_left = utils::kBlack,
          .top_right = utils::kGreen,
          .bottom_left = utils::kRed,
          .bottom_right = utils::kBlue}});
  }

  // Returns a color given the pixel index, to produce the following image:
  //
  // ___________________________________
  // |                |                |
  // |     BLACK      |     RED        |
  // |                |                |
  // |________________|________________|
  // |                |                |
  // |                |                |
  // |      GREEN     |     BLUE       |
  // |________________|________________|
  //
  auto GetPixelColor(unsigned int pixel_index, unsigned int bytes_per_row,
                     uint64_t image_vmo_bytes) {
    const utils::Pixel color_quadrants[2][2] = {
        {utils::kBlack, utils::kRed},
        {utils::kGreen, utils::kBlue},
    };
    int vertical_half_index = pixel_index < (image_vmo_bytes / 2) ? 0 : 1;
    int horizontal_half_index = (pixel_index % bytes_per_row) < (bytes_per_row / 2) ? 0 : 1;
    return color_quadrants[vertical_half_index][horizontal_half_index];
  }

  struct FlipAndOrientationHash {
    std::size_t operator()(std::pair<fuc::ImageFlip, fuc::Orientation> v) const {
      return static_cast<size_t>(v.first) << 16 | static_cast<size_t>(v.second);
    }
  };

  struct ExpectedColors {
    utils::Pixel top_left;
    utils::Pixel top_right;
    utils::Pixel bottom_left;
    utils::Pixel bottom_right;
  };

  std::unordered_map<std::pair<fuc::ImageFlip, fuc::Orientation>, ExpectedColors,
                     FlipAndOrientationHash>
      expected_colors_map;
};

class ParameterizedFlipAndOrientationTestBGRA : public ParameterizedFlipAndOrientationTest {};

class ParameterizedFlipAndOrientationTestRGBA : public ParameterizedFlipAndOrientationTest {};

INSTANTIATE_TEST_SUITE_P(
    ParameterizedFlipAndOrientationTestWithParams, ParameterizedFlipAndOrientationTestBGRA,
    zxtest::Combine(zxtest::Values(fuc::Orientation::kCcw0Degrees, fuc::Orientation::kCcw90Degrees,
                                   fuc::Orientation::kCcw180Degrees,
                                   fuc::Orientation::kCcw270Degrees),
                    zxtest::Values(fuc::ImageFlip::kLeftRight, fuc::ImageFlip::kUpDown)));

TEST_P(ParameterizedFlipAndOrientationTestBGRA, FlipAndOrientationRenderTest) {
  auto [orientation, image_flip] = GetParam();

  const uint32_t num_pixels = display_width_ * display_height_;
  constexpr auto kByterPerPixel = 4;
  const uint64_t image_vmo_bytes = num_pixels * kByterPerPixel;

  auto [local_token, scenic_token] = utils::SysmemTokens::Create(sysmem_allocator_);

  // Send one token to Flatland Allocator.
  auto bc_tokens = allocation::cpp::BufferCollectionImportExportTokens::New();
  fuc::RegisterBufferCollectionArgs rbc_args;
  rbc_args.export_token(std::move(bc_tokens.export_token));
  rbc_args.buffer_collection_token2(
      fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken>(scenic_token.TakeChannel()));
  auto result = flatland_allocator_->RegisterBufferCollection(std::move(rbc_args));
  ASSERT_TRUE(result.is_ok());

  // Use the local token to allocate a protected buffer.
  auto info_opt = SetConstraintsAndAllocateBuffer(
      std::move(local_token), GetBufferConstraints(fuchsia_images2::PixelFormat::kB8G8R8A8,
                                                   fuchsia_images2::ColorSpace::kSrgb));
  ASSERT_TRUE(info_opt.has_value());
  auto info = std::move(info_opt.value());

  // Write the pixel values to the VMO.
  ASSERT_EQ(image_vmo_bytes, info.settings()->buffer_settings()->size_bytes().value_or(0));

  const zx::vmo& image_vmo = info.buffers()->at(0).vmo().value();

  unsigned int current_image_content_id = 1;
  uint8_t* vmo_base;
  auto status =
      zx::vmar::root_self()->map(ZX_VM_PERM_WRITE | ZX_VM_PERM_READ, 0, image_vmo, 0,
                                 image_vmo_bytes, reinterpret_cast<uintptr_t*>(&vmo_base));
  EXPECT_EQ(ZX_OK, status);

  vmo_base += info.buffers()->at(0).vmo_usable_start().value_or(0);

  unsigned int image_width = display_width_;
  unsigned int image_height = display_height_;
  if (orientation == fuc::Orientation::kCcw90Degrees ||
      orientation == fuc::Orientation::kCcw270Degrees) {
    std::swap(image_width, image_height);
  }

  unsigned int bytes_per_row = image_width * kByterPerPixel;
  for (uint32_t i = 0; i < image_vmo_bytes; i += kByterPerPixel) {
    const utils::Pixel color = GetPixelColor(i, bytes_per_row, image_vmo_bytes);
    // For BGRA32 pixel format, the first and the third byte in the pixel corresponds to the
    // blue and the red channel respectively.
    vmo_base[i] = color.blue;
    vmo_base[i + 1] = color.green;
    vmo_base[i + 2] = color.red;
    vmo_base[i + 3] = color.alpha;
  }

  if (info.settings()->buffer_settings()->coherency_domain() ==
      fuchsia_sysmem2::CoherencyDomain::kRam) {
    EXPECT_EQ(ZX_OK, zx_cache_flush(vmo_base, image_vmo_bytes, ZX_CACHE_FLUSH_DATA));
  }

  fuc::ImageProperties image_properties;
  image_properties.size(fuchsia_math::SizeU{{.width = image_width, .height = image_height}});
  const fuc::ContentId kImageContentId(current_image_content_id++);

  FX_CHECK((*root_flatland_)
               ->CreateImage({{.image_id = kImageContentId,
                               .import_token = std::move(bc_tokens.import_token),
                               .vmo_index = 0,
                               .properties = std::move(image_properties)}})
               .is_ok());
  FX_CHECK(
      (*root_flatland_)->SetImageFlip({{.image_id = kImageContentId, .flip = image_flip}}).is_ok());

  // Present the created Image.
  FX_CHECK((*root_flatland_)
               ->SetContent({{.transform_id = kRootTransform, .content_id = kImageContentId}})
               .is_ok());
  FX_CHECK((*root_flatland_)
               ->SetOrientation({{.transform_id = kRootTransform, .orientation = orientation}})
               .is_ok());

  // Translate back into position after orientating around top-left corner.
  fuchsia_math::Vec translation;
  switch (orientation) {
    case fuc::Orientation::kCcw0Degrees:
      translation = {{.x = 0, .y = 0}};
      break;
    case fuc::Orientation::kCcw90Degrees:
      translation = {{.x = 0, .y = static_cast<int32_t>(image_width)}};
      break;
    case fuc::Orientation::kCcw180Degrees:
      translation = {
          {.x = static_cast<int32_t>(image_width), .y = static_cast<int32_t>(image_height)}};
      break;
    case fuc::Orientation::kCcw270Degrees:
      translation = {{.x = static_cast<int32_t>(image_height), .y = 0}};
      break;
  }
  FX_CHECK((*root_flatland_)
               ->SetTranslation({{.transform_id = kRootTransform, .translation = translation}})
               .is_ok());

  BlockingPresent(this, *root_flatland_);

  auto screenshot = TakeScreenshot(screenshotter_, display_width_, display_height_,
                                   fuc::ScreenshotFormat::kBgraRaw);

  // Verify that the number of pixels is the same (i.e. the image hasn't changed).
  auto histogram = screenshot.Histogram();
  const uint32_t pixel_color_count = num_pixels / 4;
  // TODO(https://fxbug.dev/42067818): Switch to exact comparisons after Astro precision issues are
  // resolved.
  EXPECT_NEAR(histogram[utils::kBlue], pixel_color_count, display_width_);
  EXPECT_NEAR(histogram[utils::kGreen], pixel_color_count, display_width_);
  EXPECT_NEAR(histogram[utils::kBlack], pixel_color_count, display_width_);
  EXPECT_NEAR(histogram[utils::kRed], pixel_color_count, display_width_);

  // Verify that the screenshot corners are the expected color.
  const auto expected_colors = expected_colors_map.find(std::make_pair(image_flip, orientation));
  ASSERT_NE(expected_colors, expected_colors_map.end());
  EXPECT_EQ(screenshot.GetPixelAt(0, 0), expected_colors->second.top_left);
  EXPECT_EQ(screenshot.GetPixelAt(screenshot.width() - 1, 0), expected_colors->second.top_right);
  EXPECT_EQ(screenshot.GetPixelAt(0, screenshot.height() - 1), expected_colors->second.bottom_left);
  EXPECT_EQ(screenshot.GetPixelAt(screenshot.width() - 1, screenshot.height() - 1),
            expected_colors->second.bottom_right);
}

INSTANTIATE_TEST_SUITE_P(
    ParameterizedFlipAndOrientationTestRGBAWithParams, ParameterizedFlipAndOrientationTestRGBA,
    zxtest::Combine(zxtest::Values(fuc::Orientation::kCcw0Degrees, fuc::Orientation::kCcw90Degrees,
                                   fuc::Orientation::kCcw180Degrees,
                                   fuc::Orientation::kCcw270Degrees),
                    zxtest::Values(fuc::ImageFlip::kLeftRight, fuc::ImageFlip::kUpDown)));

TEST_P(ParameterizedFlipAndOrientationTestRGBA, FlipAndOrientationRenderTest) {
  auto [orientation, image_flip] = GetParam();

  const uint32_t num_pixels = display_width_ * display_height_;
  constexpr auto kByterPerPixel = 4;
  const uint64_t image_vmo_bytes = num_pixels * kByterPerPixel;

  auto [local_token, scenic_token] = utils::SysmemTokens::Create(sysmem_allocator_);

  // Send one token to Flatland Allocator.
  auto bc_tokens = allocation::cpp::BufferCollectionImportExportTokens::New();
  fuc::RegisterBufferCollectionArgs rbc_args;
  rbc_args.export_token(std::move(bc_tokens.export_token));
  rbc_args.buffer_collection_token2(
      fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken>(scenic_token.TakeChannel()));
  auto result = flatland_allocator_->RegisterBufferCollection(std::move(rbc_args));
  ASSERT_TRUE(result.is_ok());

  // Use the local token to allocate a protected buffer.
  auto info_opt = SetConstraintsAndAllocateBuffer(
      std::move(local_token), GetBufferConstraints(fuchsia_images2::PixelFormat::kR8G8B8A8,
                                                   fuchsia_images2::ColorSpace::kSrgb));
  ASSERT_TRUE(info_opt.has_value());
  auto info = std::move(info_opt.value());

  // Write the pixel values to the VMO.
  ASSERT_EQ(image_vmo_bytes, info.settings()->buffer_settings()->size_bytes().value_or(0));

  const zx::vmo& image_vmo = info.buffers()->at(0).vmo().value();

  unsigned int current_image_content_id = 1;
  uint8_t* vmo_base;
  auto status =
      zx::vmar::root_self()->map(ZX_VM_PERM_WRITE | ZX_VM_PERM_READ, 0, image_vmo, 0,
                                 image_vmo_bytes, reinterpret_cast<uintptr_t*>(&vmo_base));
  EXPECT_EQ(ZX_OK, status);

  vmo_base += info.buffers()->at(0).vmo_usable_start().value_or(0);

  unsigned int image_width = display_width_;
  unsigned int image_height = display_height_;
  if (orientation == fuc::Orientation::kCcw90Degrees ||
      orientation == fuc::Orientation::kCcw270Degrees) {
    std::swap(image_width, image_height);
  }

  unsigned int bytes_per_row = image_width * kByterPerPixel;
  for (uint32_t i = 0; i < image_vmo_bytes; i += kByterPerPixel) {
    const utils::Pixel color = GetPixelColor(i, bytes_per_row, image_vmo_bytes);
    // For R8G8B8A8 pixel format, the first and the third byte in the pixel corresponds to the
    // red and the blue channel respectively.
    vmo_base[i] = color.red;
    vmo_base[i + 1] = color.green;
    vmo_base[i + 2] = color.blue;
    vmo_base[i + 3] = color.alpha;
  }

  if (info.settings()->buffer_settings()->coherency_domain() ==
      fuchsia_sysmem2::CoherencyDomain::kRam) {
    EXPECT_EQ(ZX_OK, zx_cache_flush(vmo_base, image_vmo_bytes, ZX_CACHE_FLUSH_DATA));
  }

  fuc::ImageProperties image_properties;
  image_properties.size(fuchsia_math::SizeU{{.width = image_width, .height = image_height}});
  const fuc::ContentId kImageContentId(current_image_content_id++);

  FX_CHECK((*root_flatland_)
               ->CreateImage({{.image_id = kImageContentId,
                               .import_token = std::move(bc_tokens.import_token),
                               .vmo_index = 0,
                               .properties = std::move(image_properties)}})
               .is_ok());
  FX_CHECK(
      (*root_flatland_)->SetImageFlip({{.image_id = kImageContentId, .flip = image_flip}}).is_ok());

  // Present the created Image.
  FX_CHECK((*root_flatland_)
               ->SetContent({{.transform_id = kRootTransform, .content_id = kImageContentId}})
               .is_ok());
  FX_CHECK((*root_flatland_)
               ->SetOrientation({{.transform_id = kRootTransform, .orientation = orientation}})
               .is_ok());

  // Translate back into position after orientating around top-left corner.
  fuchsia_math::Vec translation;
  switch (orientation) {
    case fuc::Orientation::kCcw0Degrees:
      translation = {{.x = 0, .y = 0}};
      break;
    case fuc::Orientation::kCcw90Degrees:
      translation = {{.x = 0, .y = static_cast<int32_t>(image_width)}};
      break;
    case fuc::Orientation::kCcw180Degrees:
      translation = {
          {.x = static_cast<int32_t>(image_width), .y = static_cast<int32_t>(image_height)}};
      break;
    case fuc::Orientation::kCcw270Degrees:
      translation = {{.x = static_cast<int32_t>(image_height), .y = 0}};
      break;
  }
  FX_CHECK((*root_flatland_)
               ->SetTranslation({{.transform_id = kRootTransform, .translation = translation}})
               .is_ok());

  BlockingPresent(this, *root_flatland_);

  auto screenshot = TakeScreenshot(screenshotter_, display_width_, display_height_,
                                   fuc::ScreenshotFormat::kRgbaRaw);

  // Verify that the number of pixels is the same (i.e. the image hasn't changed).
  auto histogram = screenshot.Histogram();
  const uint32_t pixel_color_count = num_pixels / 4;
  // TODO(https://fxbug.dev/42067818): Switch to exact comparisons after Astro precision issues are
  // resolved.
  EXPECT_NEAR(histogram[utils::kBlue], pixel_color_count, display_width_);
  EXPECT_NEAR(histogram[utils::kGreen], pixel_color_count, display_width_);
  EXPECT_NEAR(histogram[utils::kBlack], pixel_color_count, display_width_);
  EXPECT_NEAR(histogram[utils::kRed], pixel_color_count, display_width_);

  // Verify that the screenshot corners are the expected color.
  const auto expected_colors = expected_colors_map.find(std::make_pair(image_flip, orientation));
  ASSERT_NE(expected_colors, expected_colors_map.end());
  EXPECT_EQ(screenshot.GetPixelAt(0, 0), expected_colors->second.top_left);
  EXPECT_EQ(screenshot.GetPixelAt(screenshot.width() - 1, 0), expected_colors->second.top_right);
  EXPECT_EQ(screenshot.GetPixelAt(0, screenshot.height() - 1), expected_colors->second.bottom_left);
  EXPECT_EQ(screenshot.GetPixelAt(screenshot.width() - 1, screenshot.height() - 1),
            expected_colors->second.bottom_right);
}

class ParameterizedScreenshotFormatTest : public FlatlandPixelTestBase,
                                          public zxtest::WithParamInterface<fuc::ScreenshotFormat> {
};

INSTANTIATE_TEST_SUITE_P(ParameterizedScreenshotFormatTestWithParams,
                         ParameterizedScreenshotFormatTest,
                         zxtest::Values(fuc::ScreenshotFormat::kBgraRaw,
                                        fuc::ScreenshotFormat::kRgbaRaw));

TEST_P(ParameterizedScreenshotFormatTest, CoordinateViewTest) {
  Draw4RectanglesToDisplay();

  BlockingPresent(this, *root_flatland_);

  auto screenshot = TakeScreenshot(screenshotter_, display_width_, display_height_, GetParam());

  // Check pixel content at all four corners.
  EXPECT_EQ(screenshot.GetPixelAt(0, 0), utils::kBlack);  // Top left
  EXPECT_EQ(screenshot.GetPixelAt(0, screenshot.height() - 1),
            utils::kBlue);  // Bottom left
  EXPECT_EQ(screenshot.GetPixelAt(screenshot.width() - 1, 0),
            utils::kRed);  // Top right
  EXPECT_EQ(screenshot.GetPixelAt(screenshot.width() - 1, screenshot.height() - 1),
            utils::kMagenta);  // Bottom right

  // Check pixel content at center of each rectangle.
  EXPECT_EQ(screenshot.GetPixelAt(screenshot.width() / 4, screenshot.height() / 4),
            utils::kBlack);  // Top left
  EXPECT_EQ(screenshot.GetPixelAt(screenshot.width() / 4, (3 * screenshot.height()) / 4),
            utils::kBlue);  // Bottom left
  EXPECT_EQ(screenshot.GetPixelAt((3 * screenshot.width()) / 4, screenshot.height() / 4),
            utils::kRed);  // Top right
  EXPECT_EQ(screenshot.GetPixelAt((3 * screenshot.width()) / 4, (3 * screenshot.height()) / 4),
            utils::kMagenta);  // Bottom right
  EXPECT_EQ(screenshot.GetPixelAt(screenshot.width() / 2, screenshot.height() / 2),
            utils::kGreen);  // Center
}

TEST_F(FlatlandPixelTestBase, TakeScreenshotCompressionTest) {
  Draw4RectanglesToDisplay();

  BlockingPresent(this, *root_flatland_);

  auto raw_screenshot = TakeScreenshot(screenshotter_, display_width_, display_height_);
  auto png_screenshot =
      TakeScreenshot(screenshotter_, display_width_, display_height_, fuc::ScreenshotFormat::kPng);

  EXPECT_LT(png_screenshot.size(), raw_screenshot.size());
  EXPECT_GE(png_screenshot.ComputeSimilarity(raw_screenshot), 100.f);
}

TEST_F(FlatlandPixelTestBase, TakeFileScreenshotCompressionTest) {
  Draw4RectanglesToDisplay();

  BlockingPresent(this, *root_flatland_);

  auto raw_screenshot = TakeFileScreenshot(screenshotter_, display_width_, display_height_);
  auto png_screenshot = TakeFileScreenshot(screenshotter_, display_width_, display_height_,
                                           fuc::ScreenshotFormat::kPng);

  EXPECT_LT(png_screenshot.size(), raw_screenshot.size());
  EXPECT_GE(png_screenshot.ComputeSimilarity(raw_screenshot), 100.f);
}

struct OpacityTestParams {
  float opacity;
  utils::Pixel expected_pixel;
};

class ParameterizedOpacityPixelTest : public FlatlandPixelTestBase,
                                      public zxtest::WithParamInterface<OpacityTestParams> {};

// We use the same background/foreground color for each test iteration, but
// vary the opacity.  When the opacity is 0% we expect the pure background
// color, and when it is 100% we expect the pure foreground color.  When
// opacity is 50% we expect a blend of the two when |f.u.c.BlendMode| is |f.u.c.BlendMode.SRC_OVER|.
INSTANTIATE_TEST_SUITE_P(
    Opacity, ParameterizedOpacityPixelTest,
    zxtest::Values(OpacityTestParams{.opacity = 0.0f, .expected_pixel = {0, 0, 255, 255}},
                   OpacityTestParams{.opacity = 0.5f, .expected_pixel = {0, 188, 188, 255}},
                   OpacityTestParams{.opacity = 1.0f, .expected_pixel = {0, 255, 0, 255}}));

// This test first draws a rectangle of size |display_width_* display_height_| and then draws
// another rectangle having same dimensions on the top.
TEST_P(ParameterizedOpacityPixelTest, OpacityTest) {
  utils::Pixel background_color(utils::kRed);
  utils::Pixel foreground_color(utils::kGreen);

  // Draw the background rectangle.
  DrawRectangle(*root_flatland_, display_width_, display_height_, 0, 0, background_color);

  // Draw the foreground rectangle.
  DrawRectangle(*root_flatland_, display_width_, display_height_, 0, 0, foreground_color,
                fuc::BlendMode::kSrcOver, GetParam().opacity);

  BlockingPresent(this, *root_flatland_);

  const auto num_pixels = display_width_ * display_height_;

  auto screenshot = TakeScreenshot(screenshotter_, display_width_, display_height_);
  auto histogram = screenshot.Histogram();

  // There should be only one color here in the histogram.
  ASSERT_EQ(histogram.size(), 1u);
  CompareColor(histogram.begin()->first, GetParam().expected_pixel);

  EXPECT_EQ(histogram.begin()->second, num_pixels);
}

// This test checks whether any content drawn outside the view bounds are correctly clipped.
// The test draws a scene as shown below:-
//  bbbbbbbbbbxxxxxxxxxx
//  bbbbbbbbbbxxxxxxxxxx
//  bbbbbbbbbbxxxxxxxxxx
//  bbbbbbbbbbxxxxxxxxxx
//  bbbbbbbbbbxxxxxxxxxx
//  bbbbbbbbbbxxxxxxxxxx
//  bbbbbbbbbbxxxxxxxxxx
//  bbbbbbbbbbxxxxxxxxxx
//  bbbbbbbbbbxxxxxxxxxx
//  bbbbbbbbbbxxxxxxxxxx
// The first rectangle gets clipped outide the left half of the display and the second rectangle
// gets completely clipped because it was drawn outside of the view bounds.
TEST_F(FlatlandPixelTestBase, ViewBoundClipping) {
  // Create a child view.
  FlatlandClientWithEventHandler child(ConnectIntoRealm<fuc::Flatland>(), dispatcher());
  uint32_t child_width = 0, child_height = 0;

  auto [view_creation_token, viewport_token] = scenic::cpp::ViewCreationTokenPair::New();
  auto [pv_client_end, pv_server_end] = fidl::Endpoints<fuc::ParentViewportWatcher>::Create();
  SimpleWatcherClient<fuc::ParentViewportWatcher> parent_viewport_watcher(std::move(pv_client_end),
                                                                          dispatcher());
  FX_CHECK(child
               ->CreateView2({{.token = std::move(view_creation_token),
                               .view_identity = scenic::cpp::NewViewIdentityOnCreation(),
                               .protocols = {},
                               .parent_viewport_watcher = std::move(pv_server_end)}})
               .is_ok());
  BlockingPresent(this, child);

  // Connect the child view to the root view.
  const fuc::TransformId viewport_transform(get_next_resource_id());
  const fuc::ContentId viewport_content(get_next_resource_id());

  FX_CHECK((*root_flatland_)->CreateTransform({{.transform_id = viewport_transform}}).is_ok());
  fuc::ViewportProperties properties;

  // Allow the child view to draw content in the left half of the display.
  properties.logical_size(
      fuchsia_math::SizeU{{.width = display_width_ / 2, .height = display_height_}});
  auto [cv_client_end, cv_server_end] = fidl::Endpoints<fuc::ChildViewWatcher>::Create();
  SimpleWatcherClient<fuc::ChildViewWatcher> child_view_watcher(std::move(cv_client_end),
                                                                dispatcher());
  FX_CHECK((*root_flatland_)
               ->CreateViewport({{.viewport_id = viewport_content,
                                  .token = std::move(viewport_token),
                                  .properties = std::move(properties),
                                  .child_view_watcher = std::move(cv_server_end)}})
               .is_ok());
  FX_CHECK((*root_flatland_)
               ->SetContent({{.transform_id = viewport_transform, .content_id = viewport_content}})
               .is_ok());
  FX_CHECK((*root_flatland_)
               ->AddChild({{.parent_transform_id = kRootTransform,
                            .child_transform_id = viewport_transform}})
               .is_ok());
  BlockingPresent(this, *root_flatland_);

  parent_viewport_watcher.client()->GetLayout().Then(
      [&child_width,
       &child_height](fidl::Result<fuc::ParentViewportWatcher::GetLayout>& layout_info) {
        if (layout_info.is_ok() && layout_info->info().logical_size().has_value()) {
          child_width = layout_info->info().logical_size()->width();
          child_height = layout_info->info().logical_size()->height();
        }
      });
  RunLoopUntil([&child_width, &child_height] { return child_width > 0 && child_height > 0; });

  // Create the root transform for the child view.
  FX_CHECK(child->CreateTransform({{.transform_id = kRootTransform}}).is_ok());
  FX_CHECK(child->SetRootTransform({{.transform_id = kRootTransform}}).is_ok());

  const utils::Pixel default_color(0, 0, 0, 0);

  // The child view draws a rectangle partially outside of its view bounds.
  DrawRectangle(child, 2 * child_width, child_height, 0, 0, utils::kBlue);

  // The child view draws a rectangle completely outside its view bounds.
  DrawRectangle(child, 2 * child_width, child_height, display_width_ / 2, display_height_ / 2,
                utils::kGreen);
  BlockingPresent(this, child);

  auto screenshot = TakeScreenshot(screenshotter_, display_width_, display_height_);
  EXPECT_EQ(screenshot.GetPixelAt(0, 0), utils::kBlue);
  EXPECT_EQ(screenshot.GetPixelAt(0, display_height_ - 1), utils::kBlue);

  // The top left and bottom right corner of the display lies outside the child view's bounds so
  // we do not see any color there.
  EXPECT_EQ(screenshot.GetPixelAt(display_width_ - 1, 0), default_color);
  EXPECT_EQ(screenshot.GetPixelAt(display_width_ - 1, display_height_ - 1), default_color);

  auto histogram = screenshot.Histogram();
  const auto num_pixels = static_cast<uint32_t>(display_width_ * display_height_);

  // The child view can only draw content inside its view bounds, hence we see |num_pixels/2| pixels
  // for the first rectangle.
  EXPECT_EQ(histogram[utils::kBlue], num_pixels / 2);

  // No pixels are seen for the second rectangle as it was drawn completely outside the view bounds.
  EXPECT_EQ(histogram[utils::kGreen], 0u);
  EXPECT_EQ(histogram[default_color], num_pixels / 2);
}

// This test verifies the behavior of view bound clipping when multiple views exist under a node
// that itself has a translation applied to it. We initially add a child view which is subsequently
// replaced with two new views, all which have a rectangle in each. The parent view is under a node
// that is translated (display_width/2, 0). We expect the two child views added with
// ReplaceChildren to apply the parent's translation to their translation. On the other hand, the
// second view, initially added as a child of the parent view, but removed with ReplaceChildren,
// should be removed from the parent's graph. This means that what you see on the screen should
// look like the following:
//
//  xxxxxxxxxxbbbbbbbbbb
//  xxxxxxxxxxbbbbbbbbbb
//  xxxxxxxxxxbbbbbbbbbb
//  xxxxxxxxxxbbbbbbbbbb
//  xxxxxxxxxxrrrrrrrrrr
//  xxxxxxxxxxrrrrrrrrrr
//  xxxxxxxxxxgggggggggg
//  xxxxxxxxxxgggggggggg
//
//
// Where x refers to empty display pixels.
//       b refers to blue pixels covered by the parent view's bounds.
//       r refers to red pixels covered by the first child of the parent view.
//       g refers to green pixels covered by the second child of the parent view.
TEST_F(FlatlandPixelTestBase, TranslateInheritsFromParent) {
  // Draw the first rectangle in the top right quadrant.
  const fuc::ContentId kFilledRectId1(get_next_resource_id());
  const fuc::TransformId kTransformId1(get_next_resource_id());

  FX_CHECK((*root_flatland_)->CreateFilledRect({{.rect_id = kFilledRectId1}}).is_ok());
  FX_CHECK(
      (*root_flatland_)
          ->SetSolidFill({{.rect_id = kFilledRectId1,
                           .color = GetColorInFloat(utils::kBlue),
                           .size = {{.width = display_width_ / 2, .height = display_height_ / 2}}}})
          .is_ok());

  // Associate the rect with a transform.
  FX_CHECK((*root_flatland_)->CreateTransform({{.transform_id = kTransformId1}}).is_ok());
  FX_CHECK((*root_flatland_)
               ->SetContent({{.transform_id = kTransformId1, .content_id = kFilledRectId1}})
               .is_ok());
  FX_CHECK((*root_flatland_)
               ->SetTranslation(
                   {{.transform_id = kTransformId1,
                     .translation = {{.x = static_cast<int32_t>(display_width_ / 2), .y = 0}}}})
               .is_ok());

  // Attach the transform to the view.
  FX_CHECK(
      (*root_flatland_)
          ->AddChild({{.parent_transform_id = kRootTransform, .child_transform_id = kTransformId1}})
          .is_ok());

  // Draw the second rectangle which should be removed from the view, after ReplaceChildren
  // removes it's child-parent connection.
  const fuc::ContentId kFilledRectId2(get_next_resource_id());
  const fuc::TransformId kTransformId2(get_next_resource_id());

  FX_CHECK((*root_flatland_)->CreateFilledRect({{.rect_id = kFilledRectId2}}).is_ok());
  FX_CHECK(
      (*root_flatland_)
          ->SetSolidFill({{.rect_id = kFilledRectId2,
                           .color = GetColorInFloat(utils::kMagenta),
                           .size = {{.width = display_width_ / 2, .height = display_height_ / 2}}}})
          .is_ok());

  // Associate the rect with a transform.
  FX_CHECK((*root_flatland_)->CreateTransform({{.transform_id = kTransformId2}}).is_ok());
  FX_CHECK((*root_flatland_)
               ->SetContent({{.transform_id = kTransformId2, .content_id = kFilledRectId2}})
               .is_ok());
  FX_CHECK((*root_flatland_)
               ->SetTranslation(
                   {{.transform_id = kTransformId2,
                     .translation = {{.x = 0, .y = static_cast<int32_t>(display_height_ / 2)}}}})
               .is_ok());

  // Add the |kTransformId2| as the child of |kTransformId1| temporarily, but expect that
  // ReplaceChildren undoes this.
  FX_CHECK(
      (*root_flatland_)
          ->AddChild({{.parent_transform_id = kTransformId1, .child_transform_id = kTransformId2}})
          .is_ok());
  BlockingPresent(this, *root_flatland_);

  // Draw the first child rectangle which should appear in the top half of the bottom right
  // quadrant.
  const fuc::ContentId kFilledChildRectId1(get_next_resource_id());
  const fuc::TransformId kChildTransformId1(get_next_resource_id());

  FX_CHECK((*root_flatland_)->CreateFilledRect({{.rect_id = kFilledChildRectId1}}).is_ok());
  FX_CHECK(
      (*root_flatland_)
          ->SetSolidFill({{.rect_id = kFilledChildRectId1,
                           .color = GetColorInFloat(utils::kRed),
                           .size = {{.width = display_width_ / 2, .height = display_height_ / 4}}}})
          .is_ok());

  // Associate the rect with a transform.
  FX_CHECK((*root_flatland_)->CreateTransform({{.transform_id = kChildTransformId1}}).is_ok());
  FX_CHECK(
      (*root_flatland_)
          ->SetContent({{.transform_id = kChildTransformId1, .content_id = kFilledChildRectId1}})
          .is_ok());
  FX_CHECK((*root_flatland_)
               ->SetTranslation(
                   {{.transform_id = kChildTransformId1,
                     .translation = {{.x = 0, .y = static_cast<int32_t>(display_height_ / 2)}}}})
               .is_ok());

  // Draw the second child rectangle which should appear in the bottom half of the bottom right
  // quadrant.
  const fuc::ContentId kFilledChildRectId2(get_next_resource_id());
  const fuc::TransformId kChildTransformId2(get_next_resource_id());

  FX_CHECK((*root_flatland_)->CreateFilledRect({{.rect_id = kFilledChildRectId2}}).is_ok());
  FX_CHECK(
      (*root_flatland_)
          ->SetSolidFill({{.rect_id = kFilledChildRectId2,
                           .color = GetColorInFloat(utils::kGreen),
                           .size = {{.width = display_width_ / 2, .height = display_height_ / 4}}}})
          .is_ok());

  // Associate the rect with a transform.
  FX_CHECK((*root_flatland_)->CreateTransform({{.transform_id = kChildTransformId2}}).is_ok());
  FX_CHECK(
      (*root_flatland_)
          ->SetContent({{.transform_id = kChildTransformId2, .content_id = kFilledChildRectId2}})
          .is_ok());
  FX_CHECK(
      (*root_flatland_)
          ->SetTranslation(
              {{.transform_id = kChildTransformId2,
                .translation = {{.x = 0, .y = static_cast<int32_t>(3 * display_height_ / 4)}}}})
          .is_ok());

  // Add |kChildTransformId1| and |kChildTransformId2| as children of |kTransformId1| by calling
  // ReplaceChildren, which also should remove any previous children of |kTransformId1|.
  FX_CHECK((*root_flatland_)
               ->ReplaceChildren(
                   {{.parent_transform_id = kTransformId1,
                     .new_child_transform_ids = {{kChildTransformId1, kChildTransformId2}}}})
               .is_ok());
  BlockingPresent(this, *root_flatland_);

  const utils::Pixel default_color(0, 0, 0, 0);

  auto screenshot = TakeScreenshot(screenshotter_, display_width_, display_height_);

  EXPECT_EQ(screenshot.GetPixelAt(0, 0), default_color);

  // Top left corner of the first rectangle drawn aka the parent transform.
  EXPECT_EQ(screenshot.GetPixelAt(display_width_ / 2, 0), utils::kBlue);

  // Top left corner of the first child of the parent transform.
  EXPECT_EQ(screenshot.GetPixelAt(display_width_ / 2, display_height_ / 2), utils::kRed);

  // Top left corner of the second child of the parent transform.
  EXPECT_EQ(screenshot.GetPixelAt(display_width_ / 2, 3 * display_height_ / 4), utils::kGreen);

  const auto num_pixels = display_width_ * display_height_;

  auto histogram = screenshot.Histogram();

  EXPECT_EQ(histogram[default_color], num_pixels / 2);
  EXPECT_EQ(histogram[utils::kBlue], num_pixels / 4);
  // Expect |kTransformId2| was removed after ReplaceChildren, so we expect no matching pixels.
  EXPECT_EQ(histogram[utils::kMagenta], 0);
  EXPECT_EQ(histogram[utils::kRed], num_pixels / 8);
  EXPECT_EQ(histogram[utils::kGreen], num_pixels / 8);
}

// This test zooms the entire content by a factor of 2 and verifies that only the top left quadrant
// is shown.
// Before zoom:-
// ______________DISPLAY______________
// |                |                |
// |     BLACK      |        RED     |
// |                |                |
// |________________|________________|
// |                |                |
// |                |                |
// |      BLUE      |     MAGENTA    |
// |________________|________________|
//
// After zoom:-
// ______________DISPLAY______________
// |                                 |
// |                                 |
// |                                 |
// |             BLACK               |
// |                                 |
// |                                 |
// |                                 |
// |_________________________________|
//
// The remaining rectangles get clipped out because they fall outside the view bounds.
TEST_F(FlatlandPixelTestBase, ScaleTest) {
  const uint32_t view_width = display_width_;
  const uint32_t view_height = display_height_;

  const uint32_t pane_width =
      static_cast<uint32_t>(std::ceil(static_cast<float>(view_width) / 2.f));

  const uint32_t pane_height =
      static_cast<uint32_t>(std::ceil(static_cast<float>(view_height) / 2.f));

  // Draw the rectangles in the quadrants.
  for (uint32_t i = 0; i < 2; i++) {
    for (uint32_t j = 0; j < 2; j++) {
      utils::Pixel color(static_cast<uint8_t>(j * 255), 0, static_cast<uint8_t>(i * 255), 255);
      DrawRectangle(*root_flatland_, pane_width, pane_height, i * pane_width, j * pane_height,
                    color);
    }
  }

  // Set a scale factor for 2.
  FX_CHECK((*root_flatland_)
               ->SetScale({{.transform_id = kRootTransform, .scale = {{.x = 2, .y = 2}}}})
               .is_ok());
  BlockingPresent(this, *root_flatland_);

  const auto num_pixels = display_width_ * display_height_;
  auto screenshot = TakeScreenshot(screenshotter_, display_width_, display_height_);

  // Only the top left quadrant is shown on the screen as the rest of the quadrant are clipped.
  EXPECT_EQ(screenshot.GetPixelAt(0, 0), utils::kBlack);
  EXPECT_EQ(screenshot.GetPixelAt(0, display_height_ - 1), utils::kBlack);
  EXPECT_EQ(screenshot.GetPixelAt(display_width_ - 1, 0), utils::kBlack);
  EXPECT_EQ(screenshot.GetPixelAt(display_width_ - 1, display_height_ - 1), utils::kBlack);

  auto histogram = screenshot.Histogram();
  EXPECT_EQ(histogram[utils::kBlack], num_pixels);
  EXPECT_EQ(histogram[utils::kBlue], 0u);
  EXPECT_EQ(histogram[utils::kRed], 0u);
  EXPECT_EQ(histogram[utils::kMagenta], 0u);
}

// This test ensures that detaching a viewport ceases rendering the view.
TEST_F(FlatlandPixelTestBase, ViewportDetach) {
  FlatlandClientWithEventHandler child(ConnectIntoRealm<fuc::Flatland>(), dispatcher());

  // Create the child view.
  auto [view_creation_token, viewport_creation_token] = scenic::cpp::ViewCreationTokenPair::New();
  auto [pv_client_end, pv_server_end] = fidl::Endpoints<fuc::ParentViewportWatcher>::Create();
  SimpleWatcherClient<fuc::ParentViewportWatcher> parent_viewport_watcher(std::move(pv_client_end),
                                                                          dispatcher());
  FX_CHECK(child
               ->CreateView2({{.token = std::move(view_creation_token),
                               .view_identity = scenic::cpp::NewViewIdentityOnCreation(),
                               .protocols = {},
                               .parent_viewport_watcher = std::move(pv_server_end)}})
               .is_ok());
  BlockingPresent(this, child);

  // Connect the child view to the root view.
  const fuc::TransformId viewport_transform(get_next_resource_id());
  const fuc::ContentId viewport_content(get_next_resource_id());
  FX_CHECK((*root_flatland_)->CreateTransform({{.transform_id = viewport_transform}}).is_ok());
  auto [cv_client_end, cv_server_end] = fidl::Endpoints<fuc::ChildViewWatcher>::Create();
  SimpleWatcherClient<fuc::ChildViewWatcher> child_view_watcher(std::move(cv_client_end),
                                                                dispatcher());
  fuc::ViewportProperties properties;
  properties.logical_size(
      fuchsia_math::SizeU{{.width = display_width_, .height = display_height_}});
  FX_CHECK((*root_flatland_)
               ->CreateViewport({{.viewport_id = viewport_content,
                                  .token = std::move(viewport_creation_token),
                                  .properties = std::move(properties),
                                  .child_view_watcher = std::move(cv_server_end)}})
               .is_ok());
  FX_CHECK((*root_flatland_)
               ->SetContent({{.transform_id = viewport_transform, .content_id = viewport_content}})
               .is_ok());
  FX_CHECK((*root_flatland_)
               ->AddChild({{.parent_transform_id = kRootTransform,
                            .child_transform_id = viewport_transform}})
               .is_ok());

  BlockingPresent(this, *root_flatland_);

  // Child view draws a solid filled rectangle.
  FX_CHECK(child->CreateTransform({{.transform_id = kRootTransform}}).is_ok());
  FX_CHECK(child->SetRootTransform({{.transform_id = kRootTransform}}).is_ok());
  DrawRectangle(child, display_width_, display_height_, 0, 0, utils::kBlue);
  BlockingPresent(this, child);

  const auto num_pixels = display_width_ * display_height_;
  // The screenshot taken should reflect the content drawn by the child view.
  {
    auto screenshot = TakeScreenshot(screenshotter_, display_width_, display_height_);
    auto histogram = screenshot.Histogram();
    EXPECT_EQ(histogram[utils::kBlue], num_pixels);
  }

  // Root view releases the viewport.
  bool release_viewport_completed = false;
  (*root_flatland_)
      ->ReleaseViewport({{.viewport_id = viewport_content}})
      .Then([&release_viewport_completed](fidl::Result<fuc::Flatland::ReleaseViewport>& result) {
        EXPECT_TRUE(result.is_ok());
        release_viewport_completed = true;
      });
  BlockingPresent(this, *root_flatland_);
  EXPECT_TRUE(release_viewport_completed);

  // The screenshot taken should not reflect the content drawn by the child view as its viewport was
  // released.
  {
    auto screenshot = TakeScreenshot(screenshotter_, display_width_, display_height_);
    auto histogram = screenshot.Histogram();
    EXPECT_EQ(histogram[utils::kBlue], 0u);
  }
}

// This test ensures that |fuchsia.ui.composition.ViewportProperties.inset| is only used
// as hints for clients, and they won't affect rendering of views in Scenic.
TEST_F(FlatlandPixelTestBase, InsetNotEnforced) {
  FlatlandClientWithEventHandler child(ConnectIntoRealm<fuc::Flatland>(), dispatcher());

  // Create the child view.
  auto [view_creation_token, viewport_creation_token] = scenic::cpp::ViewCreationTokenPair::New();
  auto [pv_client_end, pv_server_end] = fidl::Endpoints<fuc::ParentViewportWatcher>::Create();
  SimpleWatcherClient<fuc::ParentViewportWatcher> parent_viewport_watcher(std::move(pv_client_end),
                                                                          dispatcher());
  FX_CHECK(child
               ->CreateView2({{.token = std::move(view_creation_token),
                               .view_identity = scenic::cpp::NewViewIdentityOnCreation(),
                               .protocols = {},
                               .parent_viewport_watcher = std::move(pv_server_end)}})
               .is_ok());
  BlockingPresent(this, child);

  // Connect the child view to the root view.
  const fuc::TransformId viewport_transform(get_next_resource_id());
  const fuc::ContentId viewport_content(get_next_resource_id());
  FX_CHECK((*root_flatland_)->CreateTransform({{.transform_id = viewport_transform}}).is_ok());
  auto [cv_client_end, cv_server_end] = fidl::Endpoints<fuc::ChildViewWatcher>::Create();
  SimpleWatcherClient<fuc::ChildViewWatcher> child_view_watcher(std::move(cv_client_end),
                                                                dispatcher());
  fuc::ViewportProperties properties;
  properties.logical_size(
      fuchsia_math::SizeU{{.width = display_width_, .height = display_height_}});

  // We set non-zero |inset|. These properties should work only as hints, but not affect actual
  // rendered views.
  properties.inset(fuchsia_math::Inset{{
      .top = static_cast<int32_t>(display_height_) / 4,
      .right = static_cast<int32_t>(display_width_) / 4,
      .bottom = static_cast<int32_t>(display_height_) / 4,
      .left = static_cast<int32_t>(display_width_) / 4,
  }});

  FX_CHECK((*root_flatland_)
               ->CreateViewport({{.viewport_id = viewport_content,
                                  .token = std::move(viewport_creation_token),
                                  .properties = std::move(properties),
                                  .child_view_watcher = std::move(cv_server_end)}})
               .is_ok());
  FX_CHECK((*root_flatland_)
               ->SetContent({{.transform_id = viewport_transform, .content_id = viewport_content}})
               .is_ok());
  FX_CHECK((*root_flatland_)
               ->AddChild({{.parent_transform_id = kRootTransform,
                            .child_transform_id = viewport_transform}})
               .is_ok());

  BlockingPresent(this, *root_flatland_);

  // Child view draws a solid filled rectangle.
  FX_CHECK(child->CreateTransform({{.transform_id = kRootTransform}}).is_ok());
  FX_CHECK(child->SetRootTransform({{.transform_id = kRootTransform}}).is_ok());
  DrawRectangle(child, display_width_, display_height_, 0, 0, utils::kBlue);
  BlockingPresent(this, child);

  // The size of the solid filled rectangle exceeds the child view's bounding box with
  // inset. Since inset properties are only hints, they should not affect the
  // rendered size of the rectangle.
  const auto num_pixels = display_width_ * display_height_;
  auto screenshot = TakeScreenshot(screenshotter_, display_width_, display_height_);
  auto histogram = screenshot.Histogram();
  EXPECT_EQ(histogram[utils::kBlue], num_pixels);
}

}  // namespace integration_tests

// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.display.types/cpp/fidl.h>
#include <fidl/fuchsia.hardware.display/cpp/fidl.h>
#include <fidl/fuchsia.images2/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <lib/async/default.h>
#include <lib/component/incoming/cpp/service_member_watcher.h>
#include <lib/fdio/directory.h>
#include <lib/fit/defer.h>
#include <lib/image-format/image_format.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/zircon-internal/align.h>
#include <zircon/types.h>

#include <cstdint>
#include <span>

#include <fbl/algorithm.h>

#include "src/graphics/display/lib/coordinator-getter/client.h"
#include "src/lib/fsl/handles/object_info.h"
#include "src/lib/fxl/strings/string_printf.h"
#include "src/lib/testing/loop_fixture/real_loop_fixture.h"
#include "src/lib/testing/predicates/status.h"
#include "src/ui/lib/escher/flatland/rectangle_compositor.h"
#include "src/ui/lib/escher/impl/vulkan_utils.h"
#include "src/ui/lib/escher/renderer/batch_gpu_downloader.h"
#include "src/ui/lib/escher/renderer/batch_gpu_uploader.h"
#include "src/ui/lib/escher/test/common/gtest_escher.h"
#include "src/ui/lib/escher/util/image_utils.h"
#include "src/ui/scenic/lib/allocation/buffer_collection_importer.h"
#include "src/ui/scenic/lib/allocation/id.h"
#include "src/ui/scenic/lib/display/display_manager.h"
#include "src/ui/scenic/lib/display/util.h"
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

#define EXPECT_HEX_EQ(expected, actual)                 \
  EXPECT_EQ((expected), (actual)) << fxl::StringPrintf( \
      "hexified: 0x%08X, 0x%08X", static_cast<uint32_t>(expected), static_cast<uint32_t>(actual));

namespace flatland {
namespace test {

namespace {

// Configures the geometric range and offset of the captured image against the
// golden image.
//
// The compare ranges of the captured image and the golden image specified in
// the struct must be valid.
//
// TODO(https://fxbug.dev/42076192): This should be moved to a standalone library that
// supports generic image comparison against captured images and golden images.
struct CompareConfig {
  // The index of the first row in the captured image that will be compared
  // against the golden image.
  //
  // Must be non-negative.
  int start_row = 0;

  // The index of the first column in the captured image that will be compared
  // against the golden image.
  //
  // Must be non-negative.
  int start_column = 0;

  // One (1) plus the index of the last row that will be compared against the
  // golden image in the captured image. So the rows within range [start_row,
  // end_row) in the compared image will be used for comparison.
  //
  // Must be equal to or greater than `start_row`.
  int end_row = 0;

  // One (1) plus the index of the last column that will be compared against the
  // golden image in the captured image. So the columns within range
  // [start_column, end_column) in the compared image will be used for
  // comparison.
  //
  // Must be equal to or greater than `start_column`.
  int end_column = 0;

  // Pixels of captured image may have an vertical offset from the golden image.
  // Pixels of row `r` of the golden image will be compared with row
  // `r + capture_offset_row` of the captured image.
  int capture_offset_row = 0;

  // Pixels of captured image may have an horizontal offset from the golden
  // image.
  // Pixels of column `c` of the golden image will be compared with column
  // `c + capture_offset_column` of the captured image.
  int capture_offset_column = 0;

  // The maximum allowed difference of values between a pixel on the golden
  // image and the corresponding pixel on the captured image.
  //
  // Must be non-negative.
  int color_difference_threshold = 0;
};

CompareConfig GetCompareConfigForBoard(std::string_view board_name, int display_width,
                                       int display_height) {
  if (board_name == "astro") {
    const int num_columns_to_compare = display_width - 1;
    return CompareConfig{
        // Ignore the first row. It sometimes contains junk due to a hardware
        // bug per Amlogic.
        .start_row = 1,
        .start_column = 0,

        .end_row = display_height,
        // The last column of the captured image only contains junk bytes due
        // to hardware bug.
        .end_column = display_width - 1,

        .capture_offset_row = 0,
        // The captured contents of the first column from golden image is missing.
        // The column 0 in captured image corresponds to the column 1 in the
        // golden image.
        .capture_offset_column = -1,

        // On Amlogic S905D2, the color conversion matrix and calculated results
        // are rounded to 0.5, so it's possible to see a maximum color error of
        // one (1) in the readback pixels.  HOWEVER, similar to Nelson, experiments
        // show that occasionally a few pixels are off by 2 instead of 1.
        .color_difference_threshold = 2,
    };
  }

  if (board_name == "sherlock") {
    return CompareConfig{
        // Ignore the first two rows. They sometimes contain junk (hardware bug).
        .start_row = 2,
        .start_column = 0,

        // TODO(https://fxbug.dev/42076618): The last 5 rows of the captured image on
        // sherlock may contain only zeroes; so we ignore these rows.
        .end_row = display_height - 5,
        .end_column = display_width,

        .capture_offset_row = 0,
        .capture_offset_column = 0,
        // The actual rounding mechanism used by Sherlock (Amlogic T931)
        // hardware is unknown. Experiments show that the maximum color error
        // of two (2) can be seen in the readback pixels.
        .color_difference_threshold = 2,
    };
  }

  if (board_name == "nelson") {
    return CompareConfig{
        // Ignore the first row. It sometimes contains junk (hardware bug).
        .start_row = 1,
        .start_column = 0,

        .end_row = display_height,
        .end_column = display_width,

        .capture_offset_row = 0,
        .capture_offset_column = 0,
        // The actual rounding mechanism used by Nelson (Amlogic S905D3)
        // hardware is unknown. Experiments show that the maximum color error
        // of two (2) can be seen in the readback pixels.
        .color_difference_threshold = 2,
    };
  }

  if (board_name == "vim3") {
    // TODO(https://fxbug.dev/42076618): For VIM3 with 1920-width displays, the last 2
    // rows of the captured image may contain only zeroes; so we ignore these
    // rows.
    const int end_row = (display_width == 1920) ? (display_height - 2) : display_height;
    return CompareConfig{
        // On VIM3 the first two rows may contain incorrect pixels, possibly due to
        // display engine hardware bug, so we ignore them.
        .start_row = 2,
        .start_column = 0,

        .end_row = end_row,
        .end_column = display_width,

        .capture_offset_row = 0,
        .capture_offset_column = 0,
        // On VIM3 (Amlogic A311D), the onboard color conversion caused some
        // color difference between captured image and original RGB image.
        // Currently we see a maximum difference of 3 (0xff and 0xfc) so we set
        // it as the maximum allowed color difference.
        .color_difference_threshold = 3,
    };
  }

  // fallback
  return CompareConfig{
      .start_row = 0,
      .start_column = 0,

      .end_row = display_height,
      .end_column = display_width,

      .capture_offset_row = 0,
      .capture_offset_column = 0,

      .color_difference_threshold = 0,
  };
}

[[maybe_unused]] CompareConfig GetCompareConfigForCurrentTestEnvironment(int display_width,
                                                                         int display_height) {
  static constexpr std::string_view board_name =
#if defined(PLATFORM_ASTRO)
      "astro";
#elif defined(PLATFORM_SHERLOCK)
      "sherlock";
#elif defined(PLATFORM_NELSON)
      "nelson";
#elif defined(PLATFORM_VIM3)
      "vim3";
#else
      "default";
#endif

  return GetCompareConfigForBoard(board_name, display_width, display_height);
}

static std::vector<uint8_t> GetColoredPixels(uint32_t bytes_per_row, uint32_t row_width,
                                             uint32_t num_rows,
                                             fuchsia_images2::PixelFormat pixel_format,
                                             std::array<uint8_t, 4> rgba) {
  uint32_t color = 0;
  switch (pixel_format) {
    case fuchsia_images2::PixelFormat::kR8G8B8A8:
      color = rgba[0] | rgba[1] << 8 | rgba[2] << 16 | rgba[3] << 24;
      break;
    case fuchsia_images2::PixelFormat::kB8G8R8A8:
      color = rgba[0] << 16 | rgba[1] << 8 | rgba[2] | rgba[3] << 24;
      break;
    default:
      FX_NOTREACHED();
  }

  const size_t num_bytes = bytes_per_row * num_rows;
  std::vector<uint8_t> write_bytes(num_bytes, 0);
  for (int64_t row = 0; row < num_rows; row++) {
    cpp20::span<uint32_t> row_pixels(
        reinterpret_cast<uint32_t*>(write_bytes.data() + bytes_per_row * row), row_width);
    std::fill(row_pixels.begin(), row_pixels.end(), color);
  }

  return write_bytes;
}

static std::vector<uint8_t> FillVmoWithColor(
    const fuchsia_sysmem2::BufferCollectionInfo& collection_info, uint32_t vmo_index,
    fuchsia_images2::PixelFormat pixel_format, uint32_t image_width, uint32_t image_height,
    std::array<uint8_t, 4> rgba) {
  const uint32_t bytes_per_row =
      utils::GetBytesPerRow(collection_info.settings().value(), image_width);

  auto pixels = GetColoredPixels(bytes_per_row, image_width, image_height, pixel_format, rgba);

  MapHostPointer(collection_info, vmo_index, HostPointerAccessMode::kWriteOnly,
                 [&pixels](uint8_t* vmo_host, uint32_t num_bytes) {
                   EXPECT_GE(num_bytes, pixels.size());
                   memcpy(vmo_host, pixels.data(), pixels.size());
                 });

  return pixels;
}

}  // namespace

class DisplayCompositorPixelTest : public gtest::RealLoopFixture {
 public:
  void SetUp() override {
    gtest::RealLoopFixture::SetUp();
    async_set_default_dispatcher(dispatcher());

#ifdef FAKE_DISPLAY
    realm_root_ = testing::BuildFakeDisplayRealm(dispatcher(), testing::DisplayRealmConfig{});
#else
    realm_root_ = std::nullopt;
#endif  // FAKE_DISPLAY

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
            .name(arena, fsl::GetCurrentProcessName() + " DisplayCompositorPixelTest")
            .id(fsl::GetCurrentProcessKoid())
            .Build());
    EXPECT_TRUE(result.ok());
    executor_ = std::make_unique<async::Executor>(dispatcher());

    display_manager_ = std::make_unique<display::DisplayManager>([]() {});

    fidl::ClientEnd<fuchsia_io::Directory> svc_root = OpenServiceRoot();
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

  fidl::WireSharedClient<fuchsia_hardware_display::Coordinator>& raw_display_coordinator() {
    FX_CHECK(display_manager_->coordinator_proxy());
    return display_manager_->coordinator_proxy()->raw();
  }

 protected:
  // TODO(https://fxbug.dev/42076251): This is unnecesarily hardcoded. We should consider
  // making display pixel format a test parameter as well.
  static constexpr fuchsia_images2::PixelFormat kDisplayPixelFormat =
      fuchsia_images2::PixelFormat::kB8G8R8A8;

  std::optional<component_testing::RealmRoot> realm_root_;
  fidl::WireClient<fuchsia_sysmem2::Allocator> sysmem_allocator_;
  std::unique_ptr<async::Executor> executor_;
  std::unique_ptr<display::DisplayManager> display_manager_;
  display::Display::VsyncCallbackId vsync_callback_id_{};

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

  fidl::ClientEnd<fuchsia_io::Directory> OpenServiceRoot() {
    if (realm_root_) {
      return fidl::ClientEnd<fuchsia_io::Directory>(
          realm_root_->component().CloneExposedDir().TakeChannel());
    }
    zx::result<fidl::ClientEnd<fuchsia_io::Directory>> open_service_root_result =
        component::OpenServiceRoot();
    ZX_ASSERT(open_service_root_result.is_ok());
    return std::move(open_service_root_result).value();
  }

  // To avoid flakes, tests call this function to ensure that config stamps applied by
  // the display compositor are fully applied to the display coordinator before engaging
  // in any operations (e.g. reading back pixels from the display) that first require
  // these processes to have been completed.
  void WaitOnVSync() {
    auto display = display_manager_->default_display();
    auto& display_coordinator = raw_display_coordinator();

    // Get the latest applied config stamp. This will be used to compare against the config
    // stamp in the OnSync callback function used by the display. If the two stamps match,
    // then we know that the vsync has completed and it is safe to do readbacks.
    const auto config_stamp_result = display_coordinator.sync()->GetLatestCommittedConfigStamp();
    ASSERT_TRUE(config_stamp_result.ok())
        << "Failed to call FIDL GetLatestCommittedConfigStamp method: "
        << config_stamp_result.status_string();
    display::WireConfigStamp pending_config_stamp = config_stamp_result->stamp;

    // The callback will switch this bool to |true| if the two configs match. It is initialized
    // to |false| and blocks the main thread below.
    bool configs_are_equal = false;
    auto vsync_callback_id = display->AddVsyncCallback(
        [&pending_config_stamp, &configs_are_equal](
            zx::time timestamp, display::WireConfigStamp displayed_config_stamp) {
          if (pending_config_stamp.value == displayed_config_stamp.value &&
              displayed_config_stamp.value != fuchsia_hardware_display::kInvalidConfigStampValue) {
            configs_are_equal = true;
          }
        });

    // Run loop until the configs match.
    ASSERT_TRUE(RunLoopWithTimeoutOrUntil([&configs_are_equal] { return configs_are_equal; },
                                          /*timeout*/ zx::sec(10)));

    // Now that we've finished waiting, we can reset the display callback to null as we do not want
    // this callback, which makes references to stack variables which will go out of scope once this
    // function exits, to continue being called on future vsyncs.
    display->RemoveVsyncCallback(vsync_callback_id);
  }

  // Set up the buffer collections and images to be used for capturing the diplay coordinator's
  // output. The only devices which currently implement the capture functionality on their
  // display coordinators are the AMLOGIC devices, and so we hardcode some of those AMLOGIC
  // assumptions here, such as making the pixel format for the capture image BGR24, as that
  // is the only capture format that AMLOGIC supports.
  //
  // `image_id` must be an unique image ID not used by any other image.
  //
  // TODO(https://fxbug.dev/42076499): Instead of providing hardcoded pixel type for
  // capture buffer, tests should let display driver make decision for the
  // capture buffer format, and use the sysmem format in BufferCollectionInfo
  // for capture-and-golden comparison.
  fpromise::result<fidl::SyncClient<fuchsia_sysmem2::BufferCollection>, zx_status_t> SetupCapture(
      allocation::GlobalBufferCollectionId collection_id, fuchsia_images2::PixelFormat pixel_type,
      fuchsia_sysmem2::BufferCollectionInfo* collection_info, allocation::GlobalImageId image_id) {
    auto display = display_manager_->default_display();
    EXPECT_TRUE(display);

    // This should only be running on devices with capture support.
    bool capture_supported = display::IsCaptureSupported(raw_display_coordinator());
    if (!capture_supported) {
      FX_LOGS(INFO) << "Capture is not supported on this device. Test skipped.";
      return fpromise::error(ZX_ERR_NOT_SUPPORTED);
    }

    // Set up buffer collection and image for recording a snapshot.
    fuchsia_hardware_display_types::wire::ImageBufferUsage image_buffer_usage = {
        .tiling_type = fuchsia_hardware_display_types::kImageTilingTypeCapture,
    };

    auto [local_token, dup_token] = SysmemTokens::Create(sysmem_allocator_);
    bool success = display::ImportBufferCollection(collection_id, raw_display_coordinator(),
                                                   std::move(dup_token), image_buffer_usage);
    EXPECT_TRUE(success);
    auto [collection_client_end, collection_server_end] =
        fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();
    fidl::Arena arena;
    fidl::OneWayStatus result = sysmem_allocator_->BindSharedCollection(
        fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
            .token(std::move(local_token))
            .buffer_collection_request(std::move(collection_server_end))
            .Build());
    EXPECT_TRUE(result.ok());
    fidl::SyncClient<fuchsia_sysmem2::BufferCollection> collection(
        std::move(collection_client_end));
    fuchsia_sysmem2::NodeSetNameRequest set_name_request;
    set_name_request.priority(100u);
    set_name_request.name("FlatlandTestCaptureImage");
    auto set_name_result = collection->SetName(std::move(set_name_request));
    EXPECT_TRUE(set_name_result.is_ok());

    // Set the client constraints.
    {
      fuchsia_sysmem2::BufferCollectionSetConstraintsRequest set_constraints_request;
      fuchsia_sysmem2::BufferCollectionConstraints constraints;

      fuchsia_sysmem2::BufferUsage usage;
      usage.cpu(fuchsia_sysmem2::kCpuUsageReadOften | fuchsia_sysmem2::kCpuUsageWriteOften);
      constraints.usage(std::move(usage));
      constraints.min_buffer_count_for_camping(1);
      fuchsia_sysmem2::BufferMemoryConstraints buffer_memory_constraints;
      buffer_memory_constraints.ram_domain_supported(true);
      constraints.buffer_memory_constraints(std::move(buffer_memory_constraints));

      fuchsia_sysmem2::ImageFormatConstraints image_constraints;
#ifdef FAKE_DISPLAY
      image_constraints.pixel_format(pixel_type);
#else
      // This format required for AMLOGIC capture.
      image_constraints.pixel_format(fuchsia_images2::PixelFormat::kB8G8R8);
#endif  // FAKE_DISPLAY

      image_constraints.color_spaces({{fuchsia_images2::ColorSpace::kSrgb}});
      image_constraints.min_size(fuchsia_math::SizeU(0, 0));
      image_constraints.max_size(fuchsia_math::SizeU(std::numeric_limits<uint32_t>::max(),
                                                     std::numeric_limits<uint32_t>::max()));
      image_constraints.min_bytes_per_row(0);
      image_constraints.max_bytes_per_row(std::numeric_limits<uint32_t>::max());
      image_constraints.max_width_times_height(std::numeric_limits<uint32_t>::max());
      image_constraints.size_alignment(fuchsia_math::SizeU(1, 1));
      image_constraints.bytes_per_row_divisor(1);
      image_constraints.start_offset_divisor(1);
      image_constraints.display_rect_alignment(fuchsia_math::SizeU(1, 1));

      constraints.image_format_constraints({{std::move(image_constraints)}});
      set_constraints_request.constraints(std::move(constraints));

      auto set_constraints_result = collection->SetConstraints(std::move(set_constraints_request));
      EXPECT_TRUE(set_constraints_result.is_ok());
    }

    // Have the client wait for buffers allocated so it can populate its information
    // struct with the vmo data.
    {
      auto wait_result = collection->WaitForAllBuffersAllocated();
      EXPECT_TRUE(wait_result.is_ok());
      if (!wait_result.is_ok()) {
        return fpromise::error(ZX_ERR_INTERNAL);
      }
      *collection_info = std::move(wait_result->buffer_collection_info().value());
    }

    // TODO(https://fxbug.dev/332521780): Display clients will be required to
    // pass the captured display's mode information.
    display::WireImageMetadata image_metadata{
        .dimensions = {.width = display->width_in_px(), .height = display->height_in_px()},
        .tiling_type = fuchsia_hardware_display_types::kImageTilingTypeCapture,
    };

    zx_status_t import_status = display::ImportImageForCapture(
        *display_manager_->coordinator_proxy(), image_metadata, collection_id, 0, image_id);
    EXPECT_EQ(import_status, ZX_OK);
    if (import_status != ZX_OK) {
      return fpromise::error(import_status);
    }

    return fpromise::ok(std::move(collection));
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
    EXPECT_TRUE(RunPromise(std::move(import_promise)));

    auto [buffer_usage, memory_constraints] = GetUsageAndMemoryConstraintsForCpuWriteOften();
    fidl::SyncClient<fuchsia_sysmem2::BufferCollection> texture_collection =
        CreateBufferCollectionSyncPtrAndSetConstraints(
            sysmem_allocator_, std::move(local_token), num_vmos, width, height,
            std::move(buffer_usage), pixel_format, std::move(memory_constraints),
            std::make_optional(fuchsia_images2::PixelFormatModifier::kLinear));

    // Have the client wait for buffers allocated so it can populate its information
    // struct with the vmo data.
    auto wait_result = texture_collection->WaitForAllBuffersAllocated();
    EXPECT_TRUE(wait_result.is_ok());
    if (wait_result.is_ok()) {
      *collection_info = std::move(wait_result->buffer_collection_info().value());
    }

    return texture_collection;
  }

  void ReleaseCaptureBufferCollection(allocation::GlobalBufferCollectionId collection_id) {
    const display::WireBufferCollectionId display_collection_id =
        display::ToDisplayFidlBufferCollectionId(collection_id);
    auto display = display_manager_->default_display();
    auto& display_coordinator = raw_display_coordinator();

    const fidl::OneWayStatus result =
        display_coordinator.sync()->ReleaseBufferCollection(display_collection_id);
    ASSERT_TRUE(result.ok()) << "Failed to call FIDL ReleaseBufferCollection: "
                             << result.status_string();
  }

  static void ReleaseClientTextureBufferCollection(
      DisplayCompositor* display_compositor, allocation::GlobalBufferCollectionId collection_id) {
    display_compositor->ReleaseBufferCollection(collection_id, BufferCollectionUsage::kClientImage);
  }

  // Captures the pixel values on the display and reads them into |read_values|.
  void CaptureDisplayOutput(display::CoordinatorProxy& display_coordinator,
                            const fuchsia_sysmem2::BufferCollectionInfo& collection_info,
                            allocation::GlobalImageId capture_image_id,
                            std::vector<uint8_t>* read_values, bool release_capture_image = true) {
    // Make sure the config from the DisplayCompositor has been completely applied first before
    // attempting to capture pixels from the display. This only matters for the real display.
    WaitOnVSync();

    // This ID would only be zero if we were running in an environment without capture support.
    EXPECT_NE(capture_image_id, display::kInvalidImageId);

    auto display = display_manager_->default_display();

    zx::event capture_signal_fence;
    auto status = zx::event::create(0, &capture_signal_fence);
    EXPECT_EQ(status, ZX_OK);

    display::EventId capture_signal_fence_id =
        display_coordinator.ImportEvent(capture_signal_fence);
    const auto start_capture_result = display_coordinator.raw().sync()->StartCapture(
        capture_signal_fence_id.ToFidl(), capture_image_id.ToFidl());
    ASSERT_TRUE(start_capture_result.ok())
        << "Failed to call FIDL StartCapture: " << start_capture_result.status_string();
    ASSERT_TRUE(start_capture_result->is_ok())
        << "Failed StartCapture: " << zx_status_get_string(start_capture_result->error_value());

    // We must wait for the capture to finish before we can proceed. Time out after 3 seconds.
    status = capture_signal_fence.wait_one(ZX_EVENT_SIGNALED, zx::deadline_after(zx::msec(3000)),
                                           nullptr);
    EXPECT_EQ(status, ZX_OK);

    // Read the capture values back out.
    MapHostPointer(collection_info, /*vmo_index*/ 0, HostPointerAccessMode::kReadOnly,
                   [read_values](const uint8_t* vmo_host, uint32_t num_bytes) mutable {
                     read_values->resize(num_bytes);
                     memcpy(read_values->data(), vmo_host, num_bytes);
                   });

    // Cleanup the capture.
    if (release_capture_image) {
      display_coordinator.ReleaseImage(capture_image_id);
    }
  }

  // TODO(https://fxbug.dev/42076192): This is taken from //src/graphics/display/bin/
  // display-test and modified for Scenic testing purposes; instead of making a
  // copy and make modifications, we should make this into a generic library.
#ifdef FAKE_DISPLAY
  bool CaptureCompare(cpp20::span<const uint8_t> captured_image,
                      cpp20::span<const uint8_t> input_image,
                      fuchsia_images2::PixelFormat input_image_pixel_format, uint32_t height,
                      uint32_t width) {
    PixelFormatAndModifier pixel_format_and_modifier(
        static_cast<fuchsia_images2::PixelFormat>(input_image_pixel_format),
        fuchsia_images2::PixelFormatModifier::kLinear);
    FX_CHECK(input_image_pixel_format == fuchsia_images2::PixelFormat::kR8G8B8A8 ||
             input_image_pixel_format == fuchsia_images2::PixelFormat::kB8G8R8A8);
    const int image_formats_bytes_per_pixel =
        ImageFormatStrideBytesPerWidthPixel(pixel_format_and_modifier);
    EXPECT_EQ(captured_image.size(),
              static_cast<size_t>(width) * height * image_formats_bytes_per_pixel);
    EXPECT_EQ(captured_image.size(), input_image.size());

    if (memcmp(captured_image.data(), input_image.data(), captured_image.size()) == 0)
      return true;

    return false;
  }
#else
  bool CaptureCompare(cpp20::span<const uint8_t> captured_image,
                      cpp20::span<const uint8_t> input_image,
                      fuchsia_images2::PixelFormat input_image_pixel_format_type, int height,
                      int width) {
    FX_CHECK(input_image_pixel_format_type == fuchsia_images2::PixelFormat::kR8G8B8A8 ||
             input_image_pixel_format_type == fuchsia_images2::PixelFormat::kB8G8R8A8);

    auto expected_rgba_image = std::vector<uint8_t>(input_image.begin(), input_image.end());

    // Amlogic captured images are always in packed R8G8B8 where R8 is the least
    // significant byte. To avoid out-of-order data access, we convert B8G8R8A8
    // images to R8G8B8A8 (where R8 is the least significant byte) for easier
    // comparison.
    for (size_t i = 0; i + 3 < expected_rgba_image.size(); i += 4) {
      switch (input_image_pixel_format_type) {
        case fuchsia_images2::PixelFormat::kB8G8R8A8: {
          uint8_t r, g, b, a;
          std::tie(b, g, r, a) = std::tie(expected_rgba_image[i + 0], expected_rgba_image[i + 1],
                                          expected_rgba_image[i + 2], expected_rgba_image[i + 3]);
          std::tie(expected_rgba_image[i + 0], expected_rgba_image[i + 1],
                   expected_rgba_image[i + 2], expected_rgba_image[i + 3]) = std::tie(r, g, b, a);
          break;
        }
        case fuchsia_images2::PixelFormat::kR8G8B8A8:
          // No need to convert.
          break;
        default:
          FX_NOTREACHED();
      }
    }

    // The AMLogic display engine always use formats with 3 bytes per pixel for
    // captured images.
    // TODO(https://fxbug.dev/42076192): This should not be hardcoded, instead sysmem
    // should calculate it from sysmem BufferCollectionInfo of allocated capture
    // buffer.
    constexpr uint32_t kCaptureImageBytesPerPixel = 3;
    // TODO(https://fxbug.dev/42076192): This should not be hardcoded, instead sysmem
    // should read it from the sysmem BufferCollectionInfo of allocated capture
    // buffer.
    constexpr uint32_t kCaptureImageRowByteAlignment = 64;
    const int capture_stride = static_cast<int>(
        fbl::round_up(width * kCaptureImageBytesPerPixel, kCaptureImageRowByteAlignment));

    PixelFormatAndModifier pixel_format_and_modifier(
        static_cast<fuchsia_images2::wire::PixelFormat>(input_image_pixel_format_type),
        fuchsia_images2::PixelFormatModifier::kLinear);
    const uint32_t input_image_bytes_per_pixel =
        ImageFormatStrideBytesPerWidthPixel(pixel_format_and_modifier);
    // TODO(https://fxbug.dev/42076192): This should not be hardcoded, instead sysmem
    // should read it from the sysmem BufferCollectionInfo of allocated input
    // image buffer.
    constexpr uint32_t kInputImageRowByteAlignment = 64;
    const int expected_stride = static_cast<int>(
        fbl::round_up(width * input_image_bytes_per_pixel, kInputImageRowByteAlignment));

    CompareConfig compare_config = GetCompareConfigForCurrentTestEnvironment(width, height);

    int64_t num_pixels_different = 0;
    for (int captured_row = compare_config.start_row; captured_row < compare_config.end_row;
         ++captured_row) {
      for (int captured_column = compare_config.start_column;
           captured_column < compare_config.end_column; ++captured_column) {
        int expected_row = captured_row - compare_config.capture_offset_row;
        int expected_column = captured_column - compare_config.capture_offset_column;
        bool pixel_same = true;
        for (int channel = 0; channel < 3; channel++) {
          // On the expected image, the fourth byte per pixel is alpha channel, so we ignore it.
          int expected_byte_index = expected_row * expected_stride + expected_column * 4 + channel;
          int captured_byte_index = captured_row * capture_stride + captured_column * 3 + channel;
          if (abs(static_cast<int>(expected_rgba_image[expected_byte_index]) -
                  captured_image[captured_byte_index]) >
              compare_config.color_difference_threshold) {
            pixel_same = false;
            break;
          }
        }
        if (!pixel_same) {
          const int expected_rgb_begin_index = expected_row * expected_stride + expected_column * 4;
          const std::string expected_rgb =
              fxl::StringPrintf("%02x%02x%02x", expected_rgba_image[expected_rgb_begin_index + 0],
                                expected_rgba_image[expected_rgb_begin_index + 1],
                                expected_rgba_image[expected_rgb_begin_index + 2]);
          const int captured_rgb_begin_index = captured_row * capture_stride + captured_column * 3;
          const std::string captured_rgb =
              fxl::StringPrintf("%02x%02x%02x", captured_image[captured_rgb_begin_index + 0],
                                captured_image[captured_rgb_begin_index + 1],
                                captured_image[captured_rgb_begin_index + 2]);
          if (num_pixels_different < 1000) {
            FX_LOGS(ERROR) << "(" << expected_row << ", " << expected_column
                           << ") Expected RGB: " << expected_rgb
                           << " Captured RGB: " << captured_rgb;
          }
          ++num_pixels_different;
        }
      }
    }

    if (num_pixels_different != 0U) {
      FX_LOGS(ERROR) << "Capture Compare number of pixels different: " << num_pixels_different;
    }
    return num_pixels_different == 0U;
  }
#endif  // FAKE_DISPLAY
};

namespace {

/** DIRECTIONS FOR WRITING TESTS
----------------------------------
When tests run on environments with a virtual gpu, please include this line in the top of the
test body:
    SKIP_TEST_IF_ESCHER_USES_DEVICE(VirtualGpu);

Furthermore, please make sure to use GTEST_SKIP() when appropriate to prevent display-coordinator
related failures that may happen when using fake display or on certain devices where some
display-coordinator functionality may not be implemented, and release the imported buffer collection
when the test ends:

For example, when using display capture:

  if (capture_collection_result.is_error() &&
      capture_collection_result.error() == ZX_ERR_NOT_SUPPORTED) {
    GTEST_SKIP();
  }
  auto release_capture_collection = fit::defer(
      [this, kCaptureCollectionId] { ReleaseCaptureBufferCollection(kCaptureCollectionId); });

And when importing textures to the display compositor:

  auto texture_collection =
      SetupClientTextures(display_compositor.get(), kTextureCollectionId, GetParam(), kTextureWidth,
                          kTextureHeight, 1, &texture_collection_info);
  if (!texture_collection.is_valid()) {
    GTEST_SKIP();
  }
  auto release_texture_collection =
      fit::defer([display_compositor = display_compositor.get(), kTextureCollectionId] {
        ReleaseClientTextureBufferCollection(display_compositor.get(), kTextureCollectionId);
      });

If you are developing a test specifically for the DisplayCoordinator that does NOT need the
Vulkan Renderer, try creating a DisplayCompositor with the NullRenderer

  auto renderer = NewNullRenderer();
  auto display_compositor = std::make_shared<flatland::DisplayCompositor>(
      dispatcher(), display_manager_->coordinator_proxy(), renderer,
      utils::CreateSysmemAllocatorClient(dispatcher(), "display_compositor_pixeltest"),
      flatland::DisplayCompositorConfig{});

Lastly, if you are specifically testing the Vulkan Renderer and do not need Display Compositing, try
creating a DisplayCompositor with enable_direct_to_display=false:

   auto display_compositor = std::make_shared<flatland::DisplayCompositor>(
      dispatcher(), display_manager_->coordinator_proxy(), renderer,
      utils::CreateSysmemAllocatorClient(dispatcher(), "display_compositor_pixeltest"),
      flatland::DisplayCompositorConfig{.enable_direct_to_display = false});

When uploading a CL that makes changes to these tests, also make sure that they run on NUC
environments with basic envs. This should happen automatically because this is specified in
the build files but if it does not please add manually.
*/

class DisplayCompositorParameterizedPixelTest
    : public DisplayCompositorPixelTest,
      public ::testing::WithParamInterface<fuchsia_images2::PixelFormat> {};

// Renders a fullscreen green rectangle to the provided display. This
// tests the engine's ability to properly read in flatland uberstruct
// data and then pass the data along to the display-coordinator interface
// to be composited directly in hardware. The Astro display coordinator
// only handles full screen rects.
VK_TEST_P(DisplayCompositorParameterizedPixelTest, FullscreenRectangleTest) {
  auto renderer = NewNullRenderer();
  auto display_compositor = std::make_shared<flatland::DisplayCompositor>(
      dispatcher(), display_manager_->coordinator_proxy(), renderer,
      utils::CreateSysmemAllocatorClient(dispatcher(), "display_compositor_pixeltest"),
      flatland::DisplayCompositorConfig{});

  auto display = display_manager_->default_display();
  auto& display_coordinator = raw_display_coordinator();

  const uint64_t kTextureCollectionId = allocation::GenerateUniqueBufferCollectionId();
  const uint64_t kCaptureCollectionId = allocation::GenerateUniqueBufferCollectionId();

  // Set up buffer collection and image for display_coordinator capture.
  allocation::GlobalImageId capture_image_id = allocation::GenerateUniqueImageId();
  fuchsia_sysmem2::BufferCollectionInfo capture_info;
  auto capture_collection_result =
      SetupCapture(kCaptureCollectionId, GetParam(), &capture_info, capture_image_id);
  if (capture_collection_result.is_error() &&
      capture_collection_result.error() == ZX_ERR_NOT_SUPPORTED) {
    GTEST_SKIP();
  }
  EXPECT_TRUE(capture_collection_result.is_ok());
  auto capture_collection = std::move(capture_collection_result.value());
  auto release_capture_collection = fit::defer(
      [this, kCaptureCollectionId] { ReleaseCaptureBufferCollection(kCaptureCollectionId); });

  // Setup the collection for the texture. Due to display coordinator limitations, the size of
  // the texture needs to match the size of the rect. So since we have a fullscreen rect, we
  // must also have a fullscreen texture to match.
  const uint32_t kRectWidth = display->width_in_px(), kTextureWidth = display->width_in_px();
  const uint32_t kRectHeight = display->height_in_px(), kTextureHeight = display->height_in_px();
  fuchsia_sysmem2::BufferCollectionInfo texture_collection_info;
  auto texture_collection =
      SetupClientTextures(display_compositor.get(), kTextureCollectionId, GetParam(), kTextureWidth,
                          kTextureHeight, 1, &texture_collection_info);
  if (!texture_collection.is_valid()) {
    GTEST_SKIP();
  }

  const uint32_t texture_bytes_per_row =
      utils::GetBytesPerRow(texture_collection_info.settings().value(), kTextureWidth);

  auto release_texture_collection =
      fit::defer([display_compositor = display_compositor.get(), kTextureCollectionId] {
        ReleaseClientTextureBufferCollection(display_compositor, kTextureCollectionId);
      });

  // Get a raw pointer for the texture's vmo and make it blue.
  // We use blue instead of other colors, since it's easy to distinguish
  // incorrect byte ordering of RGBA / BGRA.
  std::vector<uint8_t> write_bytes =
      FillVmoWithColor(texture_collection_info, /*vmo_index=*/0, /*pixel_format=*/GetParam(),
                       kTextureWidth, kTextureHeight, /*rgba=*/{0U, 0U, 255U, 255U});

  // Import the texture to the engine.
  auto image_metadata = ImageMetadata{.collection_id = kTextureCollectionId,
                                      .identifier = allocation::GenerateUniqueImageId(),
                                      .vmo_index = 0,
                                      .width = kTextureWidth,
                                      .height = kTextureHeight};
  EXPECT_TRUE(RunPromise(
      display_compositor->ImportBufferImage(image_metadata, BufferCollectionUsage::kClientImage)));

  // We cannot send to display because it is not supported in allocations.
  if (!IsDisplaySupported(display_compositor.get(), kTextureCollectionId)) {
    GTEST_SKIP();
  }

  DisplayInfo display_info{
      .dimensions = glm::uvec2(display->width_in_px(), display->height_in_px()),
      .formats = {kDisplayPixelFormat},
      .max_layer_count = display->max_layer_count()};
  auto promise = display_compositor->AddDisplay(display, display_info, /*num_vmos*/ 0,
                                                /*out_buffer_collection*/ nullptr);
  EXPECT_TRUE(RunPromise(std::move(promise)));

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
  std::span<const RenderData> render_data_list(&render_data, 1);

  display_compositor->RenderFrame(1, zx::time(1), render_data_list, {}, {}, {},
                                  [](const scheduling::Timestamps&) {});

  bool images_are_same = [&]() -> bool {
    // The amlogic capture IP block can sometimes be flaky.
    // If the images aren't the same, retry a single time.
    static constexpr int kNumTries = 2;

    for (int i = 0; i < kNumTries; i++) {
      // Grab the capture vmo data.
      std::vector<uint8_t> read_values;
      // We do not release the capture image as it's used in future attempts.
      // The capture image will be cleaned up after all attempts are finished.
      CaptureDisplayOutput(*display_compositor->GetDisplayCoordinatorForTest(), capture_info,
                           capture_image_id, &read_values,
                           /* release_capture_image= */ false);

      // Compare the capture vmo data to the texture data above.
      if (CaptureCompare(read_values, write_bytes, GetParam(), display->height_in_px(),
                         display->width_in_px())) {
        return true;
      }
    }
    return false;
  }();
  EXPECT_TRUE(images_are_same);

  // Manually clean up the capture since we didn't in CaptureDisplayOutput.
  const display::WireImageId fidl_capture_image_id = capture_image_id.ToFidl();
  const fidl::OneWayStatus release_result =
      display_coordinator.sync()->ReleaseImage(fidl_capture_image_id);
  EXPECT_TRUE(release_result.ok())
      << "Failed to call FIDL ReleaseImage: " << release_result.status_string();
}

// Test color conversion on the display hardware.
//
// TODO(https://fxbug.dev/42076344): Currently this test is skipped on all of the
// display platforms Fuchsia supports, because none of the display drivers
// fully supports the features required by the test.
VK_TEST_P(DisplayCompositorParameterizedPixelTest, ColorConversionTest) {
  auto renderer = NewNullRenderer();
  auto display_compositor = std::make_shared<flatland::DisplayCompositor>(
      dispatcher(), display_manager_->coordinator_proxy(), renderer,
      utils::CreateSysmemAllocatorClient(dispatcher(), "display_compositor_pixeltest"),
      flatland::DisplayCompositorConfig{});

  auto display = display_manager_->default_display();

  const uint64_t kCompareCollectionId = allocation::GenerateUniqueBufferCollectionId();
  const uint64_t kCaptureCollectionId = allocation::GenerateUniqueBufferCollectionId();

  // Set up buffer collection and image for display_coordinator capture.
  allocation::GlobalImageId capture_image_id = allocation::GenerateUniqueImageId();
  fuchsia_sysmem2::BufferCollectionInfo capture_info;
  auto capture_collection_result =
      SetupCapture(kCaptureCollectionId, GetParam(), &capture_info, capture_image_id);
  if (capture_collection_result.is_error() &&
      capture_collection_result.error() == ZX_ERR_NOT_SUPPORTED) {
    GTEST_SKIP();
  }
  EXPECT_TRUE(capture_collection_result.is_ok());
  auto capture_collection = std::move(capture_collection_result.value());
  auto release_capture_collection = fit::defer(
      [this, kCaptureCollectionId] { ReleaseCaptureBufferCollection(kCaptureCollectionId); });

  // Setup the collection for the texture. Due to display coordinator limitations, the size of
  // the texture needs to match the size of the rect. So since we have a fullscreen rect, we
  // must also have a fullscreen texture to match.
  const uint32_t kRectWidth = display->width_in_px(), kTextureWidth = display->width_in_px();
  const uint32_t kRectHeight = display->height_in_px(), kTextureHeight = display->height_in_px();
  fuchsia_sysmem2::BufferCollectionInfo compare_collection_info;
  auto compare_collection =
      SetupClientTextures(display_compositor.get(), kCompareCollectionId, GetParam(), kTextureWidth,
                          kTextureHeight, 1, &compare_collection_info);
  if (!compare_collection) {
    GTEST_SKIP();
  }
  auto release_compare_collection =
      fit::defer([display_compositor = display_compositor.get(), kCompareCollectionId] {
        ReleaseClientTextureBufferCollection(display_compositor, kCompareCollectionId);
      });

  // Set up the values that will be used to test against the display capture. We are making
  // a green rect with color correcion to multiply it by 0.2 so we should get (1 * 0.2) * 255 = 51
  // as the green value.
  std::vector<uint8_t> write_values =
      FillVmoWithColor(compare_collection_info, /*vmo_index=*/0, /*pixel_format=*/GetParam(),
                       kTextureWidth, kTextureHeight, /*rgba=*/{0U, 51U, 0U, 255U});

  // We cannot send to display because it is not supported in allocations.
  if (!IsDisplaySupported(display_compositor.get(), kCompareCollectionId)) {
    GTEST_SKIP();
  }

  DisplayInfo display_info{
      .dimensions = glm::uvec2(display->width_in_px(), display->height_in_px()),
      .formats = {kDisplayPixelFormat},
      .max_layer_count = display->max_layer_count()};
  auto promise = display_compositor->AddDisplay(display, display_info, /*num_vmos*/ 0,
                                                /*out_buffer_collection*/ nullptr);
  EXPECT_TRUE(RunPromise(std::move(promise)));

  ResolvedLayer layer = {
      .geometry = SrcToDest(types::RectangleF({.x = 0,
                                               .y = 0,
                                               .width = static_cast<float>(kRectWidth),
                                               .height = static_cast<float>(kRectHeight)})),
      .multiply_color = {0.f, 1.f, 0.f, 1.f},
      .blend_mode = BlendMode::kReplace(),
      .content = ResolvedLayer::SolidColorContent{.color = {0.f, 1.f, 0.f, 1.f}},
  };
  RenderData render_data = {.display_id = display->display_id(),
                            .layers = std::span<const ResolvedLayer>(&layer, 1)};
  std::span<const RenderData> render_data_list(&render_data, 1);

  display_compositor->SetColorConversionValues({0.2f, 0, 0, 0, 0.2f, 0, 0, 0, 0.2f}, {0, 0, 0},
                                               {0, 0, 0});

  // Now we can finally render. Do it a few times to make sure the color correction persists.
  for (uint32_t i = 0; i < 3; i++) {
    display_compositor->RenderFrame(1, zx::time(1), render_data_list, {}, {}, {},
                                    [](const scheduling::Timestamps&) {});

    // Grab the capture vmo data.
    std::vector<uint8_t> read_values;
    CaptureDisplayOutput(*display_compositor->GetDisplayCoordinatorForTest(), capture_info,
                         capture_image_id, &read_values);

    // Compare the capture vmo data to the texture data above.
    bool images_are_same =
        CaptureCompare(read_values, cpp20::span(write_values.data(), write_values.size()),
                       GetParam(), display->height_in_px(), display->width_in_px());
    EXPECT_TRUE(images_are_same);
  }
}

// Renders a fullscreen blue rectangle to the provided display using a solid color rect
// instead of an image. Use the NullRenderer to confirm this is being rendered through
// the display hardware.
//
// TODO(https://fxbug.dev/42076344): Currently this test is skipped on all of the
// display platforms Fuchsia supports, because none of the display drivers
// fully supports the features required by the test.
VK_TEST_P(DisplayCompositorParameterizedPixelTest, FullscreenSolidColorRectangleTest) {
  auto renderer = NewNullRenderer();
  auto display_compositor = std::make_shared<flatland::DisplayCompositor>(
      dispatcher(), display_manager_->coordinator_proxy(), renderer,
      utils::CreateSysmemAllocatorClient(dispatcher(), "display_compositor_pixeltest"),
      flatland::DisplayCompositorConfig{});

  auto display = display_manager_->default_display();

  const uint64_t kCompareCollectionId = allocation::GenerateUniqueBufferCollectionId();
  const uint64_t kCaptureCollectionId = allocation::GenerateUniqueBufferCollectionId();

  // Set up buffer collection and image for display_coordinator capture.
  allocation::GlobalImageId capture_image_id = allocation::GenerateUniqueImageId();
  fuchsia_sysmem2::BufferCollectionInfo capture_info;
  auto capture_collection_result =
      SetupCapture(kCaptureCollectionId, GetParam(), &capture_info, capture_image_id);
  if (capture_collection_result.is_error() &&
      capture_collection_result.error() == ZX_ERR_NOT_SUPPORTED) {
    GTEST_SKIP();
  }
  EXPECT_TRUE(capture_collection_result.is_ok());
  auto capture_collection = std::move(capture_collection_result.value());
  auto release_capture_collection = fit::defer(
      [this, kCaptureCollectionId] { ReleaseCaptureBufferCollection(kCaptureCollectionId); });

  // Setup the collection for the texture. Due to display coordinator limitations, the size of
  // the texture needs to match the size of the rect. So since we have a fullscreen rect, we
  // must also have a fullscreen texture to match.
  const uint32_t kRectWidth = display->width_in_px(), kTextureWidth = display->width_in_px();
  const uint32_t kRectHeight = display->height_in_px(), kTextureHeight = display->height_in_px();
  fuchsia_sysmem2::BufferCollectionInfo compare_collection_info;
  auto compare_collection =
      SetupClientTextures(display_compositor.get(), kCompareCollectionId, GetParam(), kTextureWidth,
                          kTextureHeight, 1, &compare_collection_info);
  if (!compare_collection) {
    GTEST_SKIP();
  }
  auto release_compare_collection =
      fit::defer([display_compositor = display_compositor.get(), kCompareCollectionId] {
        ReleaseClientTextureBufferCollection(display_compositor, kCompareCollectionId);
      });

  std::vector<uint8_t> write_bytes =
      FillVmoWithColor(compare_collection_info, /*vmo_index=*/0, /*pixel_format=*/GetParam(),
                       kTextureWidth, kTextureHeight, /*rgba=*/{0U, 0U, 51U, 255U});

  // We cannot send to display because it is not supported in allocations.
  if (!IsDisplaySupported(display_compositor.get(), kCompareCollectionId)) {
    GTEST_SKIP();
  }

  DisplayInfo display_info{
      .dimensions = glm::uvec2(display->width_in_px(), display->height_in_px()),
      .formats = {kDisplayPixelFormat},
      .max_layer_count = display->max_layer_count()};
  auto promise = display_compositor->AddDisplay(display, display_info, /*num_vmos*/ 0,
                                                /*out_buffer_collection*/ nullptr);
  EXPECT_TRUE(RunPromise(std::move(promise)));

  ResolvedLayer layer = {
      .geometry = SrcToDest(types::RectangleF({.x = 0,
                                               .y = 0,
                                               .width = static_cast<float>(kRectWidth),
                                               .height = static_cast<float>(kRectHeight)})),
      .multiply_color = {1.f, 1.f, 1.f, 1.f},
      .blend_mode = BlendMode::kReplace(),
      .content = ResolvedLayer::SolidColorContent{.color = {0.f, 0.2f, 0.f, 1.f}},
  };
  RenderData render_data = {.display_id = display->display_id(),
                            .layers = std::span<const ResolvedLayer>(&layer, 1)};
  std::span<const RenderData> render_data_list(&render_data, 1);

  display_compositor->RenderFrame(1, zx::time(1), render_data_list, {}, {}, {},
                                  [](const scheduling::Timestamps&) {});

  // Grab the capture vmo data.
  std::vector<uint8_t> read_values;
  CaptureDisplayOutput(*display_compositor->GetDisplayCoordinatorForTest(), capture_info,
                       capture_image_id, &read_values);

  // Compare the capture vmo data to the texture data above.
  bool images_are_same = CaptureCompare(read_values, write_bytes, GetParam(),
                                        display->height_in_px(), display->width_in_px());
  EXPECT_TRUE(images_are_same);
}

// TODO(https://fxbug.dev/42076344): Currently this test is skipped on all of the
// display platforms Fuchsia supports, because none of the display drivers
// fully supports the features required by the test.
VK_TEST_P(DisplayCompositorParameterizedPixelTest, SetMinimumRGBTest) {
  auto renderer = NewNullRenderer();
  auto display_compositor = std::make_shared<flatland::DisplayCompositor>(
      dispatcher(), display_manager_->coordinator_proxy(), renderer,
      utils::CreateSysmemAllocatorClient(dispatcher(), "display_compositor_pixeltest"),
      flatland::DisplayCompositorConfig{});

  auto display = display_manager_->default_display();

  const uint64_t kCompareCollectionId = allocation::GenerateUniqueBufferCollectionId();
  const uint64_t kCaptureCollectionId = allocation::GenerateUniqueBufferCollectionId();

  // Set up buffer collection and image for display_coordinator capture.
  allocation::GlobalImageId capture_image_id = allocation::GenerateUniqueImageId();
  fuchsia_sysmem2::BufferCollectionInfo capture_info;
  auto capture_collection_result =
      SetupCapture(kCaptureCollectionId, GetParam(), &capture_info, capture_image_id);
  if (capture_collection_result.is_error() &&
      capture_collection_result.error() == ZX_ERR_NOT_SUPPORTED) {
    GTEST_SKIP();
  }
  EXPECT_TRUE(capture_collection_result.is_ok());
  auto capture_collection = std::move(capture_collection_result.value());
  auto release_capture_collection = fit::defer(
      [this, kCaptureCollectionId] { ReleaseCaptureBufferCollection(kCaptureCollectionId); });

  // Setup the collection for the texture. Due to display coordinator limitations, the size of
  // the texture needs to match the size of the rect. So since we have a fullscreen rect, we
  // must also have a fullscreen texture to match.
  const uint32_t kRectWidth = display->width_in_px(), kTextureWidth = display->width_in_px();
  const uint32_t kRectHeight = display->height_in_px(), kTextureHeight = display->height_in_px();
  fuchsia_sysmem2::BufferCollectionInfo compare_collection_info;
  auto compare_collection =
      SetupClientTextures(display_compositor.get(), kCompareCollectionId, GetParam(), kTextureWidth,
                          kTextureHeight, 1, &compare_collection_info);
  if (!compare_collection) {
    GTEST_SKIP();
  }
  auto release_compare_collection =
      fit::defer([display_compositor = display_compositor.get(), kCompareCollectionId] {
        ReleaseClientTextureBufferCollection(display_compositor, kCompareCollectionId);
      });

  const uint8_t kMinimum = 10U;

  // Get a raw pointer for the texture's vmo and make it the minimum color.
  const uint32_t num_pixels = kTextureWidth * kTextureHeight;
  std::vector<uint8_t> expected_values;
  expected_values.assign(num_pixels * 4, kMinimum);
  switch (GetParam()) {
    case fuchsia_images2::PixelFormat::kB8G8R8A8:
    case fuchsia_images2::PixelFormat::kR8G8B8A8: {
      MapHostPointer(compare_collection_info, /*vmo_index*/ 0, HostPointerAccessMode::kWriteOnly,
                     [&expected_values](uint8_t* vmo_host, uint32_t num_bytes) {
                       EXPECT_GE(num_bytes, sizeof(uint8_t) * expected_values.size());
                       memcpy(vmo_host, expected_values.data(),
                              sizeof(uint8_t) * expected_values.size());
                     });
      break;
    }
    default:
      FX_NOTREACHED();
  }

  // We cannot send to display because it is not supported in allocations.
  if (!IsDisplaySupported(display_compositor.get(), kCompareCollectionId)) {
    GTEST_SKIP();
  }

  DisplayInfo display_info{
      .dimensions = glm::uvec2(display->width_in_px(), display->height_in_px()),
      .formats = {kDisplayPixelFormat},
      .max_layer_count = display->max_layer_count()};
  auto promise = display_compositor->AddDisplay(display, display_info, /*num_vmos*/ 0,
                                                /*out_buffer_collection*/ nullptr);
  EXPECT_TRUE(RunPromise(std::move(promise)));

  ResolvedLayer layer = {
      .geometry = SrcToDest(types::RectangleF({.x = 0,
                                               .y = 0,
                                               .width = static_cast<float>(kRectWidth),
                                               .height = static_cast<float>(kRectHeight)})),
      .multiply_color = {1.f, 1.f, 1.f, 1.f},
      .blend_mode = BlendMode::kReplace(),
      .content = ResolvedLayer::SolidColorContent{.color = {0.f, 0.f, 0.f, 0.f}},
  };
  RenderData render_data = {.display_id = display->display_id(),
                            .layers = std::span<const ResolvedLayer>(&layer, 1)};
  std::span<const RenderData> render_data_list(&render_data, 1);

  display_compositor->SetMinimumRgb(kMinimum);

  display_compositor->RenderFrame(1, zx::time(1), render_data_list, {}, {}, {},
                                  [](const scheduling::Timestamps&) {});

  // Grab the capture vmo data.
  std::vector<uint8_t> readback_values;
  CaptureDisplayOutput(*display_compositor->GetDisplayCoordinatorForTest(), capture_info,
                       capture_image_id, &readback_values);

  // Compare the capture vmo data to the expected data above.
  bool images_are_same = CaptureCompare(readback_values, expected_values, GetParam(),
                                        display->height_in_px(), display->width_in_px());
  EXPECT_TRUE(images_are_same);
}

// TODO(https://fxbug.dev/42154038): Add YUV formats when they are supported by fake or real
// display.
INSTANTIATE_TEST_SUITE_P(PixelFormats, DisplayCompositorParameterizedPixelTest,
                         ::testing::Values(fuchsia_images2::PixelFormat::kB8G8R8A8,
                                           fuchsia_images2::PixelFormat::kR8G8B8A8));

class DisplayCompositorFallbackParameterizedPixelTest
    : public DisplayCompositorPixelTest,
      public ::testing::WithParamInterface<fuchsia_images2::PixelFormat> {};

// Test the software path of the engine. Render 2 rectangles, each taking up half of the
// display's screen, so that the left half is blue and the right half is red.
VK_TEST_P(DisplayCompositorFallbackParameterizedPixelTest, SoftwareRenderingTest) {
  SKIP_TEST_IF_ESCHER_USES_DEVICE(VirtualGpu);
  if (GetParam() == fuchsia_images2::PixelFormat::kNv12 ||
      GetParam() == fuchsia_images2::PixelFormat::kI420) {
    // TODO(https://fxbug.dev/321072153)
    SKIP_TEST_IF_ESCHER_USES_DEVICE(SoftwareGpu);
  }

  auto display = display_manager_->default_display();

  const uint64_t kTextureCollectionId = allocation::GenerateUniqueBufferCollectionId();
  const uint64_t kCaptureCollectionId = allocation::GenerateUniqueBufferCollectionId();

  // Set up buffer collection and image for display_coordinator capture.
  allocation::GlobalImageId capture_image_id = allocation::GenerateUniqueImageId();
  fuchsia_sysmem2::BufferCollectionInfo capture_info;
  auto capture_collection_result =
      SetupCapture(kCaptureCollectionId, fuchsia_images2::PixelFormat::kB8G8R8A8, &capture_info,
                   capture_image_id);
  if (capture_collection_result.is_error() &&
      capture_collection_result.error() == ZX_ERR_NOT_SUPPORTED) {
    GTEST_SKIP();
  }
  EXPECT_TRUE(capture_collection_result.is_ok());
  auto capture_collection = std::move(capture_collection_result.value());
  auto release_capture_collection = fit::defer(
      [this, kCaptureCollectionId] { ReleaseCaptureBufferCollection(kCaptureCollectionId); });

  // Setup the collection for the textures. Since we're rendering in software, we don't have to
  // deal with display limitations.
  const uint32_t kTextureWidth = 32, kTextureHeight = 32;
  fuchsia_sysmem2::BufferCollectionInfo texture_collection_info;

  // Create the image metadatas.
  ImageMetadata image_metadatas[2];
  for (uint32_t i = 0; i < 2; i++) {
    image_metadatas[i] = {.collection_id = kTextureCollectionId,
                          .identifier = allocation::GenerateUniqueImageId(),
                          .vmo_index = i,
                          .width = kTextureWidth,
                          .height = kTextureHeight};
  }

  // Use the VK renderer here so we can make use of software rendering.
  auto [escher, renderer] = NewVkRenderer();
  auto display_compositor = std::make_shared<flatland::DisplayCompositor>(
      dispatcher(), display_manager_->coordinator_proxy(), renderer,
      utils::CreateSysmemAllocatorClient(dispatcher(), "display_compositor_pixeltest"),
      flatland::DisplayCompositorConfig{});

  auto texture_collection = SetupClientTextures(display_compositor.get(), kTextureCollectionId,
                                                GetParam(), kTextureWidth, kTextureHeight,
                                                /*num_vmos*/ 2, &texture_collection_info);
  auto release_texture_collection =
      fit::defer([display_compositor = display_compositor.get(), kTextureCollectionId] {
        ReleaseClientTextureBufferCollection(display_compositor, kTextureCollectionId);
      });

  // Write to the two textures. Make the first blue and the second red.
  const uint32_t num_pixels = kTextureWidth * kTextureHeight;
  for (uint32_t i = 0; i < 2; i++) {
    MapHostPointer(texture_collection_info, /*vmo_index*/ i, HostPointerAccessMode::kWriteOnly,
                   [i](uint8_t* vmo_host, uint32_t num_bytes) {
                     switch (GetParam()) {
                       case fuchsia_images2::PixelFormat::kB8G8R8A8: {
                         const uint8_t kBlueBgraValues[] = {255U, 0U, 0U, 255U};
                         const uint8_t kRedBgraValues[] = {0U, 0U, 255U, 255U};
                         const uint8_t* cols = i == 0 ? kBlueBgraValues : kRedBgraValues;
                         for (uint32_t p = 0; p < num_pixels * 4; ++p)
                           vmo_host[p] = cols[p % 4];
                         break;
                       }
                       case fuchsia_images2::PixelFormat::kR8G8B8A8: {
                         const uint8_t kBlueRgbaValues[] = {0U, 0U, 255U, 255U};
                         const uint8_t kRedRgbaValues[] = {255U, 0U, 0U, 255U};
                         const uint8_t* cols = i == 0 ? kBlueRgbaValues : kRedRgbaValues;
                         for (uint32_t p = 0; p < num_pixels * 4; ++p)
                           vmo_host[p] = cols[p % 4];
                         break;
                       }
                       case fuchsia_images2::PixelFormat::kNv12: {
                         const uint8_t kBlueYuvValues[] = {29U, 255U, 107U};
                         const uint8_t kRedYuvValues[] = {76U, 84U, 255U};
                         const uint8_t* cols = i == 0 ? kBlueYuvValues : kRedYuvValues;
                         for (uint32_t p = 0; p < num_pixels; ++p)
                           vmo_host[p] = cols[0];
                         for (uint32_t p = num_pixels; p < num_pixels + num_pixels / 2; p += 2) {
                           vmo_host[p] = cols[1];
                           vmo_host[p + 1] = cols[2];
                         }
                         break;
                       }
                       case fuchsia_images2::PixelFormat::kI420: {
                         const uint8_t kBlueYuvValues[] = {29U, 255U, 107U};
                         const uint8_t kRedYuvValues[] = {76U, 84U, 255U};
                         const uint8_t* cols = i == 0 ? kBlueYuvValues : kRedYuvValues;
                         for (uint32_t p = 0; p < num_pixels; ++p)
                           vmo_host[p] = cols[0];
                         for (uint32_t p = num_pixels; p < num_pixels + num_pixels / 4; ++p)
                           vmo_host[p] = cols[1];
                         for (uint32_t p = num_pixels + num_pixels / 4;
                              p < num_pixels + num_pixels / 2; ++p)
                           vmo_host[p] = cols[2];
                         break;
                       }
                       default:
                         FX_NOTREACHED();
                     }
                   });
  }

  // We now have to import the textures to the engine and the renderer.
  for (uint32_t i = 0; i < 2; i++) {
    EXPECT_TRUE(RunPromise(display_compositor->ImportBufferImage(
        image_metadatas[i], BufferCollectionUsage::kClientImage)));
  }

  fuchsia_sysmem2::BufferCollectionInfo render_target_info;
  DisplayInfo display_info{
      .dimensions = glm::uvec2(display->width_in_px(), display->height_in_px()),
      .formats = {kDisplayPixelFormat},
      .max_layer_count = display->max_layer_count()};
  auto promise =
      display_compositor->AddDisplay(display, display_info, /*num_vmos*/ 2, &render_target_info);
  EXPECT_TRUE(RunPromise(std::move(promise)));

  // Now we can finally render.
  const uint32_t width = display->width_in_px() / 2;
  const uint32_t height = display->height_in_px();

  ResolvedLayer layer1 = {
      .geometry = SrcToDest(types::RectangleF({.x = 0,
                                               .y = 0,
                                               .width = static_cast<float>(kTextureWidth),
                                               .height = static_cast<float>(kTextureHeight)}),
                            types::RectangleF({.x = 0,
                                               .y = 0,
                                               .width = static_cast<float>(width),
                                               .height = static_cast<float>(height)}),
                            types::RotateFlip::kIdentity()),
      .content =
          ResolvedLayer::ImageContent{
              .image_id = image_metadatas[0].identifier,
              .width = image_metadatas[0].width,
              .height = image_metadatas[0].height,
          },
  };
  ResolvedLayer layer2 = {
      .geometry = SrcToDest(types::RectangleF({.x = 0,
                                               .y = 0,
                                               .width = static_cast<float>(kTextureWidth),
                                               .height = static_cast<float>(kTextureHeight)}),
                            types::RectangleF({.x = static_cast<float>(width),
                                               .y = 0,
                                               .width = static_cast<float>(width),
                                               .height = static_cast<float>(height)}),
                            types::RotateFlip::kIdentity()),
      .content =
          ResolvedLayer::ImageContent{
              .image_id = image_metadatas[1].identifier,
              .width = image_metadatas[1].width,
              .height = image_metadatas[1].height,
          },
  };
  std::array layers{layer1, layer2};
  RenderData render_data = {
      .display_id = display->display_id(),
      .layers = layers,
  };
  display_compositor->RenderFrame(1, zx::time(1), std::span<const RenderData>(&render_data, 1), {},
                                  {}, {}, [](const scheduling::Timestamps&) {});
  renderer->WaitIdle();

  // Make sure the render target has the same data as what's being put on the display.
  MapHostPointer(render_target_info, /*vmo_index*/ 0, HostPointerAccessMode::kReadOnly,
                 [&](const uint8_t* vmo_host, uint32_t num_bytes) {
                   // Grab the capture vmo data.
                   std::vector<uint8_t> read_values;
                   CaptureDisplayOutput(*display_compositor->GetDisplayCoordinatorForTest(),
                                        capture_info, capture_image_id, &read_values);

                   // Compare the capture vmo data to the values we are expecting.
                   const fuchsia_images2::PixelFormat render_target_pixel_format =
                       render_target_info.settings()
                           .value()
                           .image_format_constraints()
                           .value()
                           .pixel_format()
                           .value();
                   bool images_are_same = CaptureCompare(
                       read_values, cpp20::span(vmo_host, num_bytes), render_target_pixel_format,
                       display->height_in_px(), display->width_in_px());
                   EXPECT_TRUE(images_are_same);

                   // Make sure that the vmo_host has the right amount of blue and red colors, so
                   // that we know that even if the display matches the render target, that its not
                   // just because both are black or some other wrong colors.
                   uint32_t num_blue = 0, num_red = 0;
                   uint32_t num_pixels = num_bytes / 4;
                   for (uint32_t i = 0; i < num_pixels; i++) {
                     // |vmo_host| has BGRA sequence in pixel values.
                     if (vmo_host[4 * i] == 255U) {
                       num_blue++;
                     } else if (vmo_host[4 * i + 2] == 255U) {
                       num_red++;
                     }
                   }

                   // Due to image formating, the number of "pixels" in the image above might not be
                   // the same as the number of pixels that are actually on the screen. So here we
                   // make sure that exactly half the screen is blue, and the other half is red.
                   uint32_t num_screen_pixels = display->width_in_px() * display->height_in_px();
                   EXPECT_EQ(num_blue, num_screen_pixels / 2);
                   EXPECT_EQ(num_red, num_screen_pixels / 2);
                 });
}

INSTANTIATE_TEST_SUITE_P(PixelFormats, DisplayCompositorFallbackParameterizedPixelTest,
                         ::testing::Values(fuchsia_images2::PixelFormat::kB8G8R8A8,
                                           fuchsia_images2::PixelFormat::kR8G8B8A8,
                                           fuchsia_images2::PixelFormat::kNv12,
                                           fuchsia_images2::PixelFormat::kI420));

class DisplayCompositorTransparencyPixelTest : public DisplayCompositorPixelTest,
                                               public ::testing::WithParamInterface<BlendMode> {};

// Test to make sure that the engine can handle rendering a transparent object overlapping an
// opaque one.
VK_TEST_P(DisplayCompositorTransparencyPixelTest, OverlappingTransparencyTest) {
  SKIP_TEST_IF_ESCHER_USES_DEVICE(VirtualGpu);
  auto blend_mode_param = GetParam();
  auto display = display_manager_->default_display();

  const uint64_t kTextureCollectionId = allocation::GenerateUniqueBufferCollectionId();
  const uint64_t kCaptureCollectionId = allocation::GenerateUniqueBufferCollectionId();

  // Set up buffer collection and image for display_coordinator capture.
  allocation::GlobalImageId capture_image_id = allocation::GenerateUniqueImageId();
  fuchsia_sysmem2::BufferCollectionInfo capture_info;
  auto capture_collection_result =
      SetupCapture(kCaptureCollectionId, fuchsia_images2::PixelFormat::kB8G8R8A8, &capture_info,
                   capture_image_id);
  if (capture_collection_result.is_error() &&
      capture_collection_result.error() == ZX_ERR_NOT_SUPPORTED) {
    GTEST_SKIP();
  }
  EXPECT_TRUE(capture_collection_result.is_ok());
  auto capture_collection = std::move(capture_collection_result.value());
  auto release_capture_collection = fit::defer(
      [this, kCaptureCollectionId] { ReleaseCaptureBufferCollection(kCaptureCollectionId); });

  // Setup the collection for the textures. Since we're rendering in software, we don't have to
  // deal with display limitations.
  const uint32_t kTextureWidth = 1, kTextureHeight = 1;
  fuchsia_sysmem2::BufferCollectionInfo texture_collection_info;

  // Create the image metadatas.
  ImageMetadata image_metadatas[2];
  for (uint32_t i = 0; i < 2; i++) {
    image_metadatas[i] = {.collection_id = kTextureCollectionId,
                          .identifier = allocation::GenerateUniqueImageId(),
                          .vmo_index = i,
                          .width = kTextureWidth,
                          .height = kTextureHeight};
  }

  // Use the VK renderer here so we can make use of software rendering.
  auto [escher, renderer] = NewVkRenderer();
  auto display_compositor = std::make_shared<flatland::DisplayCompositor>(
      dispatcher(), display_manager_->coordinator_proxy(), renderer,
      utils::CreateSysmemAllocatorClient(dispatcher(), "display_compositor_pixeltest"),
      flatland::DisplayCompositorConfig{
          // Force GPU composition for this test.
          .enable_direct_to_display = false,
      });

  auto texture_collection =
      SetupClientTextures(display_compositor.get(), kTextureCollectionId,
                          fuchsia_images2::PixelFormat::kB8G8R8A8, kTextureWidth, kTextureHeight,
                          /*num_vmos*/ 2, &texture_collection_info);
  auto release_texture_collection =
      fit::defer([display_compositor = display_compositor.get(), kTextureCollectionId] {
        ReleaseClientTextureBufferCollection(display_compositor, kTextureCollectionId);
      });

  // Write to the two textures. Make the first blue and opaque and the second red and
  // half transparent. Format is BGRA32 in sRGB color space.
  constexpr uint32_t kBgraBlue = (255 << 24) | (255U << 0);
  constexpr uint32_t kBgraRedTranslucent = (128 << 24) | (188U << 16);
  constexpr uint32_t kBgraRedTranslucentNonPremultiplied = (128 << 24) | (255U << 16);
  std::vector<uint32_t> kBgraColors = {
      kBgraBlue,
      (blend_mode_param == BlendMode::kPremultipliedAlpha() ? kBgraRedTranslucent
                                                            : kBgraRedTranslucentNonPremultiplied)};
  for (uint32_t i = 0; i < 2; i++) {
    std::vector<uint32_t> write_values;
    write_values.assign(kTextureWidth * kTextureHeight, kBgraColors[i]);
    MapHostPointer(texture_collection_info, /*vmo_index*/ i, HostPointerAccessMode::kWriteOnly,
                   [write_values](uint8_t* vmo_host, uint32_t num_bytes) {
                     EXPECT_TRUE(num_bytes >= sizeof(uint32_t) * write_values.size());
                     memcpy(vmo_host, write_values.data(), sizeof(uint32_t) * write_values.size());
                   });
  }

  // We now have to import the textures to the engine and the renderer.
  for (uint32_t i = 0; i < 2; i++) {
    EXPECT_TRUE(RunPromise(display_compositor->ImportBufferImage(
        image_metadatas[i], BufferCollectionUsage::kClientImage)));
  }

  fuchsia_sysmem2::BufferCollectionInfo render_target_info;
  DisplayInfo display_info{
      .dimensions = glm::uvec2(display->width_in_px(), display->height_in_px()),
      .formats = {kDisplayPixelFormat},
      .max_layer_count = display->max_layer_count()};
  auto promise =
      display_compositor->AddDisplay(display, display_info, /*num_vmos*/ 2, &render_target_info);
  EXPECT_TRUE(RunPromise(std::move(promise)));

  // Now we can finally render.
  const uint32_t kNumOverlappingColumns = 25;
  const uint32_t width = display->width_in_px() / 2;
  const uint32_t height = display->height_in_px();

  ResolvedLayer layer1 = {
      .geometry =
          SrcToDest(types::RectangleF({.x = 0,
                                       .y = 0,
                                       .width = static_cast<float>(kTextureWidth),
                                       .height = static_cast<float>(kTextureHeight)}),
                    types::RectangleF({.x = 0,
                                       .y = 0,
                                       .width = static_cast<float>(width + kNumOverlappingColumns),
                                       .height = static_cast<float>(height)}),
                    types::RotateFlip::kIdentity()),
      .content =
          ResolvedLayer::ImageContent{
              .image_id = image_metadatas[0].identifier,
              .width = image_metadatas[0].width,
              .height = image_metadatas[0].height,
          },
  };
  ResolvedLayer layer2 = {
      .geometry =
          SrcToDest(types::RectangleF({.x = 0,
                                       .y = 0,
                                       .width = static_cast<float>(kTextureWidth),
                                       .height = static_cast<float>(kTextureHeight)}),
                    types::RectangleF({.x = static_cast<float>(width - kNumOverlappingColumns),
                                       .y = 0,
                                       .width = static_cast<float>(width + kNumOverlappingColumns),
                                       .height = static_cast<float>(height)}),
                    types::RotateFlip::kIdentity()),
      .blend_mode = blend_mode_param,
      .content =
          ResolvedLayer::ImageContent{
              .image_id = image_metadatas[1].identifier,
              .width = image_metadatas[1].width,
              .height = image_metadatas[1].height,
          },
  };
  std::array layers{layer1, layer2};
  RenderData render_data = {
      .display_id = display->display_id(),
      .layers = layers,
  };
  display_compositor->RenderFrame(1, zx::time(1), std::span<const RenderData>(&render_data, 1), {},
                                  {}, {}, [](const scheduling::Timestamps&) {});
  renderer->WaitIdle();

  // Make sure the render target has the same data as what's being put on the display.
  MapHostPointer(
      render_target_info, /*vmo_index*/ 0, HostPointerAccessMode::kReadOnly,
      [&](const uint8_t* vmo_host, uint32_t num_bytes) {
        // Each pixel is 4 bytes, so the total memory used must be at least 4 * number of pixels, or
        // more if there is e.g. padding at the end of rows.
        EXPECT_GE(num_bytes, 4 * display->width_in_px() * display->height_in_px());

        // Grab the capture vmo data.
        std::vector<uint8_t> read_values;
        CaptureDisplayOutput(*display_compositor->GetDisplayCoordinatorForTest(), capture_info,
                             capture_image_id, &read_values);

        const fuchsia_images2::PixelFormat render_target_pixel_format_type =
            render_target_info.settings()
                .value()
                .image_format_constraints()
                .value()
                .pixel_format()
                .value();

        // Compare the capture vmo data to the values we are expecting.
        bool images_are_same = CaptureCompare(read_values, cpp20::span(vmo_host, num_bytes),
                                              render_target_pixel_format_type,
                                              display->height_in_px(), display->width_in_px());
        EXPECT_TRUE(images_are_same);

        // Make sure that the vmo_host has the right amount of blue and red colors, so
        // that we know that even if the display matches the render target, that its not
        // just because both are black or some other wrong colors.
        uint32_t num_blue = 0, num_red = 0, num_overlap = 0;
        uint32_t num_pixels = num_bytes / 4;
        const uint32_t* host_ptr = reinterpret_cast<const uint32_t*>(vmo_host);
        for (uint32_t i = 0; i < num_pixels; i++) {
          const uint32_t current_color_render_target_format = host_ptr[i];
          uint32_t current_color_bgra = 0;
          switch (render_target_pixel_format_type) {
            case fuchsia_images2::PixelFormat::kB8G8R8A8:
              current_color_bgra = current_color_render_target_format;
              break;
            case fuchsia_images2::PixelFormat::kR8G8B8A8: {
              cpp20::span<const uint8_t> rgba(
                  reinterpret_cast<const uint8_t*>(&current_color_render_target_format), 4);
              current_color_bgra =
                  (rgba[3] << 24) | (rgba[0] << 16) | (rgba[1] << 8) | (rgba[2] << 0);
              break;
            }
            default:
              GTEST_FAIL() << "Unsupported pixel format "
                           << static_cast<uint32_t>(render_target_pixel_format_type);
          }
          if (current_color_bgra == kBgraBlue) {
            num_blue++;
          } else if (current_color_bgra == kBgraRedTranslucent) {
            num_red++;
          } else if (current_color_bgra != 0) {
            num_overlap++;
          }
        }

        // Due to image formating, the number of "pixels" in the image above might not be
        // the same as the number of pixels that are actually on the screen.
        uint32_t num_screen_pixels =
            (display->width_in_px() / 2 - kNumOverlappingColumns) * display->height_in_px();
        EXPECT_EQ(num_blue, num_screen_pixels);
        EXPECT_EQ(num_red, num_screen_pixels);
        EXPECT_EQ(num_overlap,
                  (display->width_in_px() * display->height_in_px()) - 2 * num_screen_pixels);
      });
}

INSTANTIATE_TEST_SUITE_P(BlendModesOverlappingTransparency, DisplayCompositorTransparencyPixelTest,
                         ::testing::Values(BlendMode::kPremultipliedAlpha(),
                                           BlendMode::kStraightAlpha()));

class DisplayCompositorParameterizedTest
    : public DisplayCompositorPixelTest,
      public ::testing::WithParamInterface<fuchsia_images2::PixelFormat> {};

// TODO(https://fxbug.dev/42154038): Add YUV formats when they are supported by fake or real
// display.
INSTANTIATE_TEST_SUITE_P(PixelFormats, DisplayCompositorParameterizedTest,
                         ::testing::Values(fuchsia_images2::PixelFormat::kB8G8R8A8));

// Pixel test for making sure that multiparented transforms render properly.
// This is for A11Y Magnification.
//
// For this test we are going to render the same colored square twice: once on the left side of
// the screen at regular resolution and once on the right at a magnified resolution. The original
// will be (2,2) and the magnified one will have a scale factor of 2 applied, so it will become
// (4,4). However both squares will in actuality be the same transform/image in the flatland scene
// graph and uber struct. It is simply that the transform has two parents, which causes it to be
// duplicated in the topology vector. The top-left corner of the square has been marked a different
// color from the rest of the square in order to guarantee the orientation of the magnified render.
//
// NOTE: the magnified square uses a linear magnification filter, so not all of the pixels are
//       100% blue or white.
//
// NOTE: the name is a misnomer, after the test was rewritten to use a flat display list instead
//       of an UberStruct scene.
//
// - - - - - - - - - -     where i: rgba(137, 137, 255, 255)
// - B W - - B i j W -           j: rgba(225, 225, 255, 255)
// - W W - - i k l W -           k: rgba(177, 177, 255, 255)
// - - - - - j l m W -           l: rgba(233, 233, 255, 255)
// - - - - - W W W W -           m: rgba(248, 248, 255, 255)
// - - - - - - - - - -
//
VK_TEST_P(DisplayCompositorParameterizedTest, MultipleParentPixelTest) {
  SKIP_TEST_IF_ESCHER_USES_DEVICE(VirtualGpu);
  auto display = display_manager_->default_display();
  auto& display_coordinator = raw_display_coordinator();

  // Use the VK renderer here so we can make use of software rendering.
  auto [escher, renderer] = NewVkRenderer();
  auto display_compositor = std::make_shared<flatland::DisplayCompositor>(
      dispatcher(), display_manager_->coordinator_proxy(), renderer,
      utils::CreateSysmemAllocatorClient(dispatcher(), "display_compositor_pixeltest"),
      flatland::DisplayCompositorConfig{.enable_direct_to_display = false});

  const uint64_t kTextureCollectionId = allocation::GenerateUniqueBufferCollectionId();
  const uint64_t kCaptureCollectionId = allocation::GenerateUniqueBufferCollectionId();

  // Set up buffer collection and image for display_coordinator capture.
  allocation::GlobalImageId capture_image_id = allocation::GenerateUniqueImageId();
  fuchsia_sysmem2::BufferCollectionInfo capture_info;
  auto capture_collection_result =
      SetupCapture(kCaptureCollectionId, GetParam(), &capture_info, capture_image_id);
  if (capture_collection_result.is_error() &&
      capture_collection_result.error() == ZX_ERR_NOT_SUPPORTED) {
    GTEST_SKIP();
  }

  EXPECT_TRUE(capture_collection_result.is_ok());
  auto capture_collection = std::move(capture_collection_result.value());
  auto release_capture_collection = fit::defer(
      [this, kCaptureCollectionId] { ReleaseCaptureBufferCollection(kCaptureCollectionId); });

  // Setup the collection for the textures. Since we're rendering in software, we don't have to
  // deal with display limitations.
  const uint32_t kTextureWidth = 2, kTextureHeight = 2;
  fuchsia_sysmem2::BufferCollectionInfo texture_collection_info;

  // Create the texture's metadata.
  ImageMetadata image_metadata = {.collection_id = kTextureCollectionId,
                                  .identifier = allocation::GenerateUniqueImageId(),
                                  .vmo_index = 0,
                                  .width = kTextureWidth,
                                  .height = kTextureHeight};

  auto texture_collection =
      SetupClientTextures(display_compositor.get(), kTextureCollectionId, GetParam(), 60, 40,
                          /*num_vmos*/ 1, &texture_collection_info);
  auto release_texture_collection =
      fit::defer([display_compositor = display_compositor.get(), kTextureCollectionId] {
        ReleaseClientTextureBufferCollection(display_compositor, kTextureCollectionId);
      });

  switch (GetParam()) {
    case fuchsia_images2::PixelFormat::kB8G8R8A8: {
      MapHostPointer(texture_collection_info, /*vmo_index*/ 0, HostPointerAccessMode::kWriteOnly,
                     [](uint8_t* vmo_host, uint32_t num_bytes) {
                       const uint8_t kBlueBgraValues[] = {255U, 0U, 0U, 255U};
                       const uint8_t kWhiteBgraValues[] = {255U, 255U, 255U, 255U};

                       for (uint32_t p = 0; p < num_bytes; ++p) {
                         // Make the first pixel blue, and the rest white.
                         const uint8_t* cols = (p < 4) ? kBlueBgraValues : kWhiteBgraValues;
                         vmo_host[p] = cols[p % 4];
                       }
                     });

      break;
    }
    default:
      FX_NOTREACHED();
  }

  EXPECT_TRUE(RunPromise(
      display_compositor->ImportBufferImage(image_metadata, BufferCollectionUsage::kClientImage)));

  fuchsia_sysmem2::BufferCollectionInfo render_target_info;
  DisplayInfo display_info{
      .dimensions = glm::uvec2(display->width_in_px(), display->height_in_px()),
      .formats = {kDisplayPixelFormat},
      .max_layer_count = display->max_layer_count()};
  auto promise =
      display_compositor->AddDisplay(display, display_info, /*num_vmos*/ 2, &render_target_info);
  EXPECT_TRUE(RunPromise(std::move(promise)));

  ResolvedLayer layer1 = {
      .geometry = SrcToDest(types::RectangleF({.x = 0,
                                               .y = 0,
                                               .width = static_cast<float>(kTextureWidth),
                                               .height = static_cast<float>(kTextureHeight)}),
                            types::RectangleF({.x = 0, .y = 0, .width = 2, .height = 2}),
                            types::RotateFlip::kIdentity()),
      .multiply_color = {1.f, 1.f, 1.f, 1.f},
      .blend_mode = BlendMode::kReplace(),
      .content = ResolvedLayer::ImageContent{.image_id = image_metadata.identifier,
                                             .width = image_metadata.width,
                                             .height = image_metadata.height},
  };
  ResolvedLayer layer2 = {
      .geometry = SrcToDest(types::RectangleF({.x = 0,
                                               .y = 0,
                                               .width = static_cast<float>(kTextureWidth),
                                               .height = static_cast<float>(kTextureHeight)}),
                            types::RectangleF({.x = 10, .y = 0, .width = 4, .height = 4}),
                            types::RotateFlip::kIdentity()),
      .multiply_color = {1.f, 1.f, 1.f, 1.f},
      .blend_mode = BlendMode::kReplace(),
      .content = ResolvedLayer::ImageContent{.image_id = image_metadata.identifier,
                                             .width = image_metadata.width,
                                             .height = image_metadata.height},
  };
  std::array layers{layer1, layer2};
  RenderData render_data = {.display_id = display->display_id(), .layers = layers};
  std::span<const RenderData> render_data_list(&render_data, 1);

  auto render_frame_result = display_compositor->RenderFrame(
      1, zx::time(1), render_data_list, {}, {}, {}, [](const scheduling::Timestamps&) {},
      // NOTE: this is somewhat redundant, since we also pass enable_direct_to_display=false into
      // the DisplayCompositor constructor.  But, no harm is done.
      DisplayCompositor::RenderFrameTestArgs{.force_gpu_composition = true});
  EXPECT_EQ(render_frame_result, DisplayCompositor::RenderFrameResult::kGpuComposition);
  renderer->WaitIdle();

  // Make sure the render target has the same data as what's being put on the display.
  MapHostPointer(
      render_target_info, /*vmo_index*/ 0, HostPointerAccessMode::kReadOnly,
      [&](const uint8_t* vmo_host, uint32_t num_bytes) {
        const uint32_t display_bytes_per_row =
            utils::GetBytesPerRow(render_target_info.settings().value(), display->width_in_px());
        EXPECT_EQ(0U, display_bytes_per_row % 4);
        const uint32_t display_width_including_padding = display_bytes_per_row / 4;

        bool images_are_same = [&]() -> bool {
          // The amlogic capture IP block can sometimes be flaky.
          // If the images aren't the same, retry a single time.
          static constexpr int kNumTries = 2;

          for (int i = 0; i < kNumTries; i++) {
            // Grab the capture vmo data.
            std::vector<uint8_t> read_values;
            // We do not release the capture image as it's used in future attempts.
            // The capture image will be cleaned up after all attempts are finished.
            CaptureDisplayOutput(*display_compositor->GetDisplayCoordinatorForTest(), capture_info,
                                 capture_image_id, &read_values,
                                 /* release_capture_image= */ false);

            // Compare the capture vmo data to the values we are expecting.
            if (CaptureCompare(read_values, cpp20::span(vmo_host, num_bytes), GetParam(),
                               display->height_in_px(), display->width_in_px())) {
              return true;
            }
          }
          return false;
        }();
        EXPECT_TRUE(images_are_same);

        // Manually clean up the capture since we didn't in CaptureDisplayOutput.
        const display::WireImageId fidl_capture_image_id = capture_image_id.ToFidl();
        const fidl::OneWayStatus result =
            display_coordinator.sync()->ReleaseImage(fidl_capture_image_id);
        EXPECT_TRUE(result.ok()) << "Failed to call FIDL ReleaseImage: " << result.status_string();

        // |vmo_host| has BGRA sequence in pixel values.
        auto get_pixel = [&display, display_bytes_per_row](const uint8_t* vmo_host, uint32_t x,
                                                           uint32_t y) -> uint32_t {
          EXPECT_LT(x, display->width_in_px());
          EXPECT_LT(y, display->height_in_px());

          uint32_t index = y * display_bytes_per_row + x * 4;
          auto b = vmo_host[index];
          auto g = vmo_host[index + 1];
          auto r = vmo_host[index + 2];
          auto a = vmo_host[index + 3];
          return (b << 24) | (g << 16) | (r << 8) | a;
        };

        // Pack a BGRA pixel into a uint32_t.
        auto make_bgra_pixel = [](uint32_t r, uint32_t g, uint32_t b, uint32_t a) {
          return (b << 24) | (g << 16) | (r << 8) | a;
        };

        // There should be a total of 20 white pixels (4 for the normal white square and
        // 16 for the magnified white square).
        uint32_t num_pixels = num_bytes / 4;
        const uint32_t kWhiteColor = 0xFFFFFFFF;
        const uint32_t kBlueColor = 0xFF0000FF;

        // Verify the colors of the 4 pixels in the unmagnified rect.
        EXPECT_HEX_EQ(kBlueColor, get_pixel(vmo_host, 0, 0));
        EXPECT_HEX_EQ(kWhiteColor, get_pixel(vmo_host, 1, 0));
        EXPECT_HEX_EQ(kWhiteColor, get_pixel(vmo_host, 0, 1));
        EXPECT_HEX_EQ(kWhiteColor, get_pixel(vmo_host, 1, 1));

        // Verify the colors of the 16 pixels in the magnified rect.
        // (top row)
        EXPECT_HEX_EQ(kBlueColor, get_pixel(vmo_host, 10, 0));
        EXPECT_HEX_EQ(make_bgra_pixel(137, 137, 255, 255), get_pixel(vmo_host, 11, 0));
        EXPECT_HEX_EQ(make_bgra_pixel(225, 225, 255, 255), get_pixel(vmo_host, 12, 0));
        EXPECT_HEX_EQ(kWhiteColor, get_pixel(vmo_host, 13, 0));
        // (2nd row)
        EXPECT_HEX_EQ(make_bgra_pixel(137, 137, 255, 255), get_pixel(vmo_host, 10, 1));
        EXPECT_HEX_EQ(make_bgra_pixel(177, 177, 255, 255), get_pixel(vmo_host, 11, 1));
        EXPECT_HEX_EQ(make_bgra_pixel(233, 233, 255, 255), get_pixel(vmo_host, 12, 1));
        EXPECT_HEX_EQ(kWhiteColor, get_pixel(vmo_host, 13, 1));
        // (3nd row)
        EXPECT_HEX_EQ(make_bgra_pixel(225, 225, 255, 255), get_pixel(vmo_host, 10, 2));
        EXPECT_HEX_EQ(make_bgra_pixel(233, 233, 255, 255), get_pixel(vmo_host, 11, 2));
        EXPECT_HEX_EQ(make_bgra_pixel(248, 248, 255, 255), get_pixel(vmo_host, 12, 2));
        EXPECT_HEX_EQ(kWhiteColor, get_pixel(vmo_host, 13, 2));
        // (bottom row)
        EXPECT_HEX_EQ(kWhiteColor, get_pixel(vmo_host, 10, 3));
        EXPECT_HEX_EQ(kWhiteColor, get_pixel(vmo_host, 11, 3));
        EXPECT_HEX_EQ(kWhiteColor, get_pixel(vmo_host, 12, 3));
        EXPECT_HEX_EQ(kWhiteColor, get_pixel(vmo_host, 13, 3));

        // Verify that all of the rest of the pixels (except for the 20 above) are black.
        uint32_t num_black = 0;
        const uint32_t kBlackColor = 0x00000000;
        for (uint32_t x = 0; x < display->width_in_px(); x++) {
          for (uint32_t y = 0; y < display->height_in_px(); y++) {
            const uint32_t i = y * display_width_including_padding + x;

            // |vmo_host| has BGRA sequence in pixel values.
            auto b = vmo_host[(i * 4)];
            auto g = vmo_host[(i * 4) + 1];
            auto r = vmo_host[(i * 4) + 2];
            auto a = vmo_host[(i * 4) + 3];
            uint32_t val = (b << 24) | (g << 16) | (r << 8) | a;
            if (val == kBlackColor) {
              num_black++;
            }
          }
        }
        EXPECT_EQ(num_black, display->width_in_px() * display->height_in_px() - 20U);
      });
}

// Pixeltest for ensuring rotation and flipping are applied correctly.
//
// This test creates a 2x2 texture, with the top-left pixel colored blue and the rest of the pixels
// colored white.
//
// B W ----------
// W W ----------
// --------------
// --------------
//
// This image is flipped up-down and rotated 180 degrees CCW and translated to reposition to
// display. The resulting display image should be:
//
// W B ----------
// W W ----------
// --------------
// --------------
//
VK_TEST_P(DisplayCompositorParameterizedTest, ImageFlipRotate180DegreesPixelTest) {
  SKIP_TEST_IF_ESCHER_USES_DEVICE(VirtualGpu);
  auto display = display_manager_->default_display();

  // Use the VK renderer here so we can make use of software rendering.
  auto [escher, renderer] = NewVkRenderer();
  auto display_compositor = std::make_shared<flatland::DisplayCompositor>(
      dispatcher(), display_manager_->coordinator_proxy(), renderer,
      utils::CreateSysmemAllocatorClient(dispatcher(), "display_compositor_pixeltest"),
      flatland::DisplayCompositorConfig{.enable_direct_to_display = false});

  const uint64_t kTextureCollectionId = allocation::GenerateUniqueBufferCollectionId();
  const uint64_t kCaptureCollectionId = allocation::GenerateUniqueBufferCollectionId();

  // Set up buffer collection and image for display_coordinator capture.
  allocation::GlobalImageId capture_image_id = allocation::GenerateUniqueImageId();
  fuchsia_sysmem2::BufferCollectionInfo capture_info;
  auto capture_collection_result =
      SetupCapture(kCaptureCollectionId, GetParam(), &capture_info, capture_image_id);
  if (capture_collection_result.is_error() &&
      capture_collection_result.error() == ZX_ERR_NOT_SUPPORTED) {
    GTEST_SKIP();
  }

  EXPECT_TRUE(capture_collection_result.is_ok());
  auto capture_collection = std::move(capture_collection_result.value());
  auto release_capture_collection = fit::defer(
      [this, kCaptureCollectionId] { ReleaseCaptureBufferCollection(kCaptureCollectionId); });

  // Setup the collection for the textures. Since we're rendering in software, we don't have to
  // deal with display limitations.
  const uint32_t kTextureWidth = 2, kTextureHeight = 2;
  fuchsia_sysmem2::BufferCollectionInfo texture_collection_info;

  // Create the texture's metadata.
  ImageMetadata image_metadata = {.collection_id = kTextureCollectionId,
                                  .identifier = allocation::GenerateUniqueImageId(),
                                  .vmo_index = 0,
                                  .width = kTextureWidth,
                                  .height = kTextureHeight};

  auto texture_collection =
      SetupClientTextures(display_compositor.get(), kTextureCollectionId, GetParam(), 60, 40,
                          /*num_vmos*/ 1, &texture_collection_info);
  auto release_texture_collection =
      fit::defer([display_compositor = display_compositor.get(), kTextureCollectionId] {
        ReleaseClientTextureBufferCollection(display_compositor, kTextureCollectionId);
      });

  switch (GetParam()) {
    case fuchsia_images2::PixelFormat::kB8G8R8A8: {
      MapHostPointer(texture_collection_info, /*vmo_index*/ 0, HostPointerAccessMode::kWriteOnly,
                     [](uint8_t* vmo_host, uint32_t num_bytes) {
                       const uint8_t kBlueBgraValues[] = {255U, 0U, 0U, 255U};
                       const uint8_t kWhiteBgraValues[] = {255U, 255U, 255U, 255U};

                       for (uint32_t p = 0; p < num_bytes; ++p) {
                         // Make the first pixel blue, and the rest white.
                         const uint8_t* cols = (p < 4) ? kBlueBgraValues : kWhiteBgraValues;
                         vmo_host[p] = cols[p % 4];
                       }
                     });

      break;
    }
    default:
      FX_NOTREACHED();
  }

  EXPECT_TRUE(RunPromise(
      display_compositor->ImportBufferImage(image_metadata, BufferCollectionUsage::kClientImage)));

  fuchsia_sysmem2::BufferCollectionInfo render_target_info;
  DisplayInfo display_info{
      .dimensions = glm::uvec2(display->width_in_px(), display->height_in_px()),
      .formats = {kDisplayPixelFormat},
      .max_layer_count = display->max_layer_count()};
  auto promise =
      display_compositor->AddDisplay(display, display_info, /*num_vmos*/ 2, &render_target_info);
  EXPECT_TRUE(RunPromise(std::move(promise)));

  ResolvedLayer layer = {
      .geometry =
          SrcToDest(types::RectangleF({.x = 0,
                                       .y = 0,
                                       .width = static_cast<float>(image_metadata.width),
                                       .height = static_cast<float>(image_metadata.height)}),
                    types::RectangleF({.x = 0, .y = 0, .width = 2, .height = 2}),
                    types::RotateFlip::From(fuchsia_ui_composition::Orientation::kCcw180Degrees,
                                            fuchsia_ui_composition::ImageFlip::kUpDown)),
      .multiply_color = {1.f, 1.f, 1.f, 1.f},
      .blend_mode = BlendMode::kReplace(),
      .content = ResolvedLayer::ImageContent{.image_id = image_metadata.identifier,
                                             .width = image_metadata.width,
                                             .height = image_metadata.height},
  };
  RenderData render_data = {.display_id = display->display_id(),
                            .layers = std::span<const ResolvedLayer>(&layer, 1)};
  std::span<const RenderData> render_data_list(&render_data, 1);

  display_compositor->RenderFrame(1, zx::time(1), render_data_list, {}, {}, {},
                                  [](const scheduling::Timestamps&) {});
  renderer->WaitIdle();

  // Make sure the render target has the same data as what's being put on the display.
  MapHostPointer(
      render_target_info, /*vmo_index*/ 0, HostPointerAccessMode::kReadOnly,
      [&](const uint8_t* vmo_host, uint32_t num_bytes) {
        // Grab the capture vmo data.
        std::vector<uint8_t> read_values;
        CaptureDisplayOutput(*display_compositor->GetDisplayCoordinatorForTest(), capture_info,
                             capture_image_id, &read_values);

        const fuchsia_images2::PixelFormat render_target_pixel_format_type =
            render_target_info.settings()
                .value()
                .image_format_constraints()
                .value()
                .pixel_format()
                .value();

        // Compare the capture vmo data to the values we are expecting.
        bool images_are_same = CaptureCompare(read_values, cpp20::span(vmo_host, num_bytes),
                                              render_target_pixel_format_type,
                                              display->height_in_px(), display->width_in_px());
        EXPECT_TRUE(images_are_same);

        // There should be a total of 3 white pixels and 1 blue pixel.
        uint32_t num_white = 0, num_blue = 0;
        uint32_t num_pixels = num_bytes / 4;
        const uint32_t kWhiteColorBgra = 0xFFFFFFFF;
        const uint32_t kBlueColorBgra = 0xFF0000FF;
        for (uint32_t i = 0; i < num_pixels; i += 4) {
          // |vmo_host| has BGRA sequence in pixel values.
          uint32_t bgra = 0;
          switch (render_target_pixel_format_type) {
            case fuchsia_images2::PixelFormat::kR8G8B8A8:
              bgra = (vmo_host[i + 3] << 24) | (vmo_host[i + 0] << 16) | (vmo_host[i + 1] << 8) |
                     vmo_host[i + 2];
              break;
            case fuchsia_images2::PixelFormat::kB8G8R8A8:
              bgra = (vmo_host[i + 3] << 24) | (vmo_host[i + 2] << 16) | (vmo_host[i + 1] << 8) |
                     vmo_host[i + 0];
              break;
            default:
              GTEST_FAIL() << "Unexpected pixel format "
                           << static_cast<uint32_t>(render_target_pixel_format_type);
          }
          if (bgra == kWhiteColorBgra) {
            num_white++;
          } else if (bgra == kBlueColorBgra) {
            num_blue++;
          }
        }
        EXPECT_EQ(num_white, 3U);
        EXPECT_EQ(num_blue, 1U);

        auto get_bgra_pixel = [&display, pixel_format_type = render_target_info.settings()
                                                                 .value()
                                                                 .image_format_constraints()
                                                                 .value()
                                                                 .pixel_format()
                                                                 .value()](
                                  const uint8_t* vmo_host, uint32_t x, uint32_t y) -> uint32_t {
          uint32_t index = y * display->width_in_px() * 4 + x * 4;

          switch (pixel_format_type) {
            case fuchsia_images2::PixelFormat::kR8G8B8A8:
              return (vmo_host[index + 3] << 24) | (vmo_host[index + 0] << 16) |
                     (vmo_host[index + 1] << 8) | vmo_host[index + 2];
            case fuchsia_images2::PixelFormat::kB8G8R8A8:
              return (vmo_host[index + 3] << 24) | (vmo_host[index + 2] << 16) |
                     (vmo_host[index + 1] << 8) | vmo_host[index + 0];
            default:
              EXPECT_TRUE(false) << "Unexpected pixel format "
                                 << static_cast<uint32_t>(pixel_format_type);
              return 0;
          }
        };

        // Expect the top-right corner of the rect to be blue.
        EXPECT_HEX_EQ(get_bgra_pixel(vmo_host, 0, 0), kWhiteColorBgra);
        EXPECT_HEX_EQ(get_bgra_pixel(vmo_host, 1, 0), kBlueColorBgra);
      });
}

// Verify that we can switch between GPU-composited and direct-to-display frames.
VK_TEST_F(DisplayCompositorPixelTest, SwitchDisplayMode) {
  SKIP_TEST_IF_ESCHER_USES_DEVICE(VirtualGpu);

  auto display = display_manager_->default_display();
  const auto kPixelFormat = fuchsia_images2::PixelFormat::kB8G8R8A8;

  auto [escher, renderer] = NewVkRenderer();
  auto display_compositor = std::make_shared<flatland::DisplayCompositor>(
      dispatcher(), display_manager_->coordinator_proxy(), renderer,
      utils::CreateSysmemAllocatorClient(dispatcher(), "display_compositor_pixeltest"),
      flatland::DisplayCompositorConfig{});

  const uint64_t kTextureCollectionId = allocation::GenerateUniqueBufferCollectionId();
  const uint64_t kCaptureCollectionId = allocation::GenerateUniqueBufferCollectionId();

  // Set up buffer collection and image for display_coordinator capture.
  allocation::GlobalImageId capture_image_id = allocation::GenerateUniqueImageId();
  fuchsia_sysmem2::BufferCollectionInfo capture_info;
  auto capture_collection_result =
      SetupCapture(kCaptureCollectionId, kPixelFormat, &capture_info, capture_image_id);
  if (capture_collection_result.is_error() &&
      capture_collection_result.error() == ZX_ERR_NOT_SUPPORTED) {
    GTEST_SKIP();
  }
  EXPECT_TRUE(capture_collection_result.is_ok());
  auto capture_collection1 = std::move(capture_collection_result.value());
  auto release_capture_collection = fit::defer(
      [this, kCaptureCollectionId] { ReleaseCaptureBufferCollection(kCaptureCollectionId); });

  // Setup the collection for the texture. Due to display coordinator limitations, the size of
  // the texture needs to match the size of the rect. So since we have a fullscreen rect, we
  // must also have a fullscreen texture to match.
  const uint32_t kRectWidth = display->width_in_px(), kTextureWidth = display->width_in_px();
  const uint32_t kRectHeight = display->height_in_px(), kTextureHeight = display->height_in_px();

  fuchsia_sysmem2::BufferCollectionInfo texture_collection_info;
  auto texture_collection =
      SetupClientTextures(display_compositor.get(), kTextureCollectionId, kPixelFormat,
                          kTextureWidth, kTextureHeight, 2, &texture_collection_info);
  if (!texture_collection.is_valid()) {
    GTEST_SKIP();
  }
  auto release_texture_collection =
      fit::defer([display_compositor = display_compositor.get(), kTextureCollectionId] {
        ReleaseClientTextureBufferCollection(display_compositor, kTextureCollectionId);
      });

  std::vector<uint8_t> blue_write_values =
      FillVmoWithColor(texture_collection_info, /*vmo_index=*/0, /*pixel_format=*/kPixelFormat,
                       kTextureWidth, kTextureHeight, /*rgba=*/{0U, 0U, 255U, 255U});
  std::vector<uint8_t> green_write_values =
      FillVmoWithColor(texture_collection_info, /*vmo_index=*/1, /*pixel_format=*/kPixelFormat,
                       kTextureWidth, kTextureHeight, /*rgba=*/{0U, 255U, 0U, 255U});

  // Import the texture to the engine. Set blue/green to 0.2, which when converted to an
  // unnormalized uint8 value in the range [0,255] will be 51U.
  ImageMetadata image_metadatas[2];
  for (uint32_t i = 0; i < 2; i++) {
    image_metadatas[i] = {.collection_id = kTextureCollectionId,
                          .identifier = allocation::GenerateUniqueImageId(),
                          .vmo_index = i,
                          .width = kTextureWidth,
                          .height = kTextureHeight};
  }

  auto& blue_image_metadata = image_metadatas[0];
  auto& green_image_metadata = image_metadatas[1];

  {
    EXPECT_TRUE(RunPromise(display_compositor->ImportBufferImage(
        blue_image_metadata, BufferCollectionUsage::kClientImage)));
    EXPECT_TRUE(RunPromise(display_compositor->ImportBufferImage(
        green_image_metadata, BufferCollectionUsage::kClientImage)));
  }

  // Set up display render targets.
  //
  // Other tests use the buffer collection info to obtain the pixel format when comparing the
  // captured display contents to the expected values, but here we always use kDisplayPixelFormat.
  fuchsia_sysmem2::BufferCollectionInfo unused_render_target_info;
  DisplayInfo display_info{
      .dimensions = glm::uvec2(display->width_in_px(), display->height_in_px()),
      .formats = {kDisplayPixelFormat},
      .max_layer_count = display->max_layer_count()};
  auto promise =
      display_compositor->AddDisplay(display, display_info, /*num_vmos*/ 2,
                                     /*out_buffer_collection*/ &unused_render_target_info);
  EXPECT_TRUE(RunPromise(std::move(promise)));

  const ResolvedLayer blue_layer = {
      .geometry =
          SrcToDest(types::RectangleF({.x = 0,
                                       .y = 0,
                                       .width = static_cast<float>(blue_image_metadata.width),
                                       .height = static_cast<float>(blue_image_metadata.height)}),
                    types::RectangleF({.x = 0,
                                       .y = 0,
                                       .width = static_cast<float>(kRectWidth),
                                       .height = static_cast<float>(kRectHeight)}),
                    types::RotateFlip::kIdentity()),
      .multiply_color = {1.f, 1.f, 1.f, 1.f},
      .blend_mode = BlendMode::kReplace(),
      .content = ResolvedLayer::ImageContent{.image_id = blue_image_metadata.identifier,
                                             .width = blue_image_metadata.width,
                                             .height = blue_image_metadata.height},
  };
  const RenderData blue_render_data = {.display_id = display->display_id(),
                                       .layers = std::span<const ResolvedLayer>(&blue_layer, 1)};
  std::span<const RenderData> blue_display_list(&blue_render_data, 1);

  const ResolvedLayer green_layer = {
      .geometry =
          SrcToDest(types::RectangleF({.x = 0,
                                       .y = 0,
                                       .width = static_cast<float>(green_image_metadata.width),
                                       .height = static_cast<float>(green_image_metadata.height)}),
                    types::RectangleF({.x = 0,
                                       .y = 0,
                                       .width = static_cast<float>(kRectWidth),
                                       .height = static_cast<float>(kRectHeight)}),
                    types::RotateFlip::kIdentity()),
      .multiply_color = {1.f, 1.f, 1.f, 1.f},
      .blend_mode = BlendMode::kReplace(),
      .content = ResolvedLayer::ImageContent{.image_id = green_image_metadata.identifier,
                                             .width = green_image_metadata.width,
                                             .height = green_image_metadata.height},
  };
  const RenderData green_render_data = {.display_id = display->display_id(),
                                        .layers = std::span<const ResolvedLayer>(&green_layer, 1)};
  std::span<const RenderData> green_display_list(&green_render_data, 1);

  // FRAME 1, BLUE, GPU-COMPOSITED //////////////////////////////////////////////////////

  auto render_frame_result = display_compositor->RenderFrame(
      1, zx::time(1), blue_display_list, {}, {}, {}, [](const scheduling::Timestamps&) {},
      {.force_gpu_composition = true});
  EXPECT_EQ(render_frame_result, DisplayCompositor::RenderFrameResult::kGpuComposition);

  // Grab the capture vmo data, and compare to the texture data.
  std::vector<uint8_t> read_values;
  CaptureDisplayOutput(*display_compositor->GetDisplayCoordinatorForTest(), capture_info,
                       capture_image_id, &read_values,
                       /*release_capture_image=*/false);
  bool images_are_same = CaptureCompare(read_values, blue_write_values, kPixelFormat,
                                        display->height_in_px(), display->width_in_px());
  EXPECT_TRUE(images_are_same);

  // FRAME 2, GREEN, GPU-COMPOSITED //////////////////////////////////////////////////////

  render_frame_result = display_compositor->RenderFrame(2, zx::time(1), green_display_list, {}, {},
                                                        {}, [](const scheduling::Timestamps&) {},
                                                        {.force_gpu_composition = true});
  EXPECT_EQ(render_frame_result, DisplayCompositor::RenderFrameResult::kGpuComposition);

  // Grab the capture vmo data, and compare to the texture data.
  CaptureDisplayOutput(*display_compositor->GetDisplayCoordinatorForTest(), capture_info,
                       capture_image_id, &read_values,
                       /*release_capture_image=*/false);
  images_are_same = CaptureCompare(read_values, green_write_values, kPixelFormat,
                                   display->height_in_px(), display->width_in_px());
  EXPECT_TRUE(images_are_same);

  // FRAME 3, BLUE, DIRECT-TO-DISPLAY //////////////////////////////////////////////////////

  render_frame_result = display_compositor->RenderFrame(3, zx::time(1), blue_display_list, {}, {},
                                                        {}, [](const scheduling::Timestamps&) {},
                                                        {.force_gpu_composition = false});
  EXPECT_EQ(render_frame_result, DisplayCompositor::RenderFrameResult::kDirectToDisplay);

  // Grab the capture vmo data, and compare to the texture data.
  CaptureDisplayOutput(*display_compositor->GetDisplayCoordinatorForTest(), capture_info,
                       capture_image_id, &read_values,
                       /*release_capture_image=*/false);
  images_are_same = CaptureCompare(read_values, blue_write_values, kPixelFormat,
                                   display->height_in_px(), display->width_in_px());
  EXPECT_TRUE(images_are_same);

  // FRAME 4, GREEN, DIRECT-TO-DISPLAY //////////////////////////////////////////////////////

  render_frame_result = display_compositor->RenderFrame(4, zx::time(1), green_display_list, {}, {},
                                                        {}, [](const scheduling::Timestamps&) {},
                                                        {.force_gpu_composition = false});
  EXPECT_EQ(render_frame_result, DisplayCompositor::RenderFrameResult::kDirectToDisplay);

  // Grab the capture vmo data, and compare to the texture data.
  CaptureDisplayOutput(*display_compositor->GetDisplayCoordinatorForTest(), capture_info,
                       capture_image_id, &read_values,
                       /*release_capture_image=*/false);
  images_are_same = CaptureCompare(read_values, green_write_values, kPixelFormat,
                                   display->height_in_px(), display->width_in_px());
  EXPECT_TRUE(images_are_same);

  // FRAME 5, BLUE, GPU-COMPOSITED //////////////////////////////////////////////////////

  render_frame_result = display_compositor->RenderFrame(5, zx::time(1), blue_display_list, {}, {},
                                                        {}, [](const scheduling::Timestamps&) {},
                                                        {.force_gpu_composition = true});
  EXPECT_EQ(render_frame_result, DisplayCompositor::RenderFrameResult::kGpuComposition);

  // Grab the capture vmo data, and compare to the texture data.
  CaptureDisplayOutput(*display_compositor->GetDisplayCoordinatorForTest(), capture_info,
                       capture_image_id, &read_values,
                       /*release_capture_image=*/false);
  images_are_same = CaptureCompare(read_values, blue_write_values, kPixelFormat,
                                   display->height_in_px(), display->width_in_px());
  EXPECT_TRUE(images_are_same);

  // FRAME 6, GREEN, DIRECT-TO-DISPLAY //////////////////////////////////////////////////////

  render_frame_result = display_compositor->RenderFrame(6, zx::time(1), green_display_list, {}, {},
                                                        {}, [](const scheduling::Timestamps&) {},
                                                        {.force_gpu_composition = false});
  EXPECT_EQ(render_frame_result, DisplayCompositor::RenderFrameResult::kDirectToDisplay);

  // Grab the capture vmo data, and compare to the texture data.
  CaptureDisplayOutput(*display_compositor->GetDisplayCoordinatorForTest(), capture_info,
                       capture_image_id, &read_values,
                       /*release_capture_image=*/false);
  images_are_same = CaptureCompare(read_values, green_write_values, kPixelFormat,
                                   display->height_in_px(), display->width_in_px());
  EXPECT_TRUE(images_are_same);

  // Cleanup.
  display::WireImageId image_id = capture_image_id.ToFidl();
  const fidl::OneWayStatus release_image_result =
      raw_display_coordinator().sync()->ReleaseImage(image_id);
  EXPECT_TRUE(release_image_result.ok())
      << "Failed to call FIDL ReleaseImage: " << release_image_result.status_string();
}

// Tests that the DisplayCompositor can render an empty scene (black background) correctly.
VK_TEST_F(DisplayCompositorPixelTest, EmptySceneLayerTest) {
  SKIP_TEST_IF_ESCHER_USES_DEVICE(VirtualGpu);

  // Create a display realm and DisplayCompositor.
  auto renderer = NewNullRenderer();
  auto display_compositor = std::make_shared<flatland::DisplayCompositor>(
      dispatcher(), display_manager_->coordinator_proxy(), renderer,
      utils::CreateSysmemAllocatorClient(dispatcher(), "display_compositor_pixeltest"),
      flatland::DisplayCompositorConfig{});

  auto display = display_manager_->default_display();
  const auto kPixelFormat = fuchsia_images2::PixelFormat::kB8G8R8A8;
  const uint64_t kCaptureCollectionId = allocation::GenerateUniqueBufferCollectionId();

  // Set up buffer collection and image for display_coordinator capture.
  allocation::GlobalImageId capture_image_id = allocation::GenerateUniqueImageId();
  fuchsia_sysmem2::BufferCollectionInfo capture_info;
  auto capture_collection_result =
      SetupCapture(kCaptureCollectionId, kPixelFormat, &capture_info, capture_image_id);
  if (capture_collection_result.is_error() &&
      capture_collection_result.error() == ZX_ERR_NOT_SUPPORTED) {
    GTEST_SKIP();
  }
  EXPECT_TRUE(capture_collection_result.is_ok());
  auto capture_collection = std::move(capture_collection_result.value());
  auto release_capture_collection = fit::defer(
      [this, kCaptureCollectionId] { ReleaseCaptureBufferCollection(kCaptureCollectionId); });

  fuchsia_sysmem2::BufferCollectionInfo unused_render_target_info;
  DisplayInfo display_info{
      .dimensions = glm::uvec2(display->width_in_px(), display->height_in_px()),
      .formats = {kDisplayPixelFormat},
      .max_layer_count = display->max_layer_count()};
  // Create display VMOs for the boards where color layers are not supported.
  auto promise = display_compositor->AddDisplay(display, display_info, /*num_vmos*/ 2,
                                                &unused_render_target_info);
  EXPECT_TRUE(RunPromise(std::move(promise)));

  // Render an empty frame.
  RenderData render_data = {.display_id = display->display_id(), .layers = {}};
  std::span<const RenderData> render_data_list(&render_data, 1);
  auto render_frame_result = display_compositor->RenderFrame(
      1, zx::time(1), render_data_list, {}, {}, {}, [](const scheduling::Timestamps&) {},
      {.force_gpu_composition = false});
  EXPECT_NE(render_frame_result, DisplayCompositor::RenderFrameResult::kFailure);

  // Grab the capture vmo data.
  std::vector<uint8_t> read_values;
  CaptureDisplayOutput(*display_compositor->GetDisplayCoordinatorForTest(), capture_info,
                       capture_image_id, &read_values);

  // Verify the output is black.
  // We expect a black image.
  const uint32_t width = display->width_in_px();
  const uint32_t height = display->height_in_px();
  PixelFormatAndModifier pixel_format_and_modifier(
      static_cast<fuchsia_images2::wire::PixelFormat>(kPixelFormat),
      fuchsia_images2::PixelFormatModifier::kLinear);
  const uint32_t expected_image_bytes_per_pixel =
      ImageFormatStrideBytesPerWidthPixel(pixel_format_and_modifier);
  constexpr uint32_t kExpectedImageRowByteAlignment = 64;
  std::vector<uint8_t> expected_black_image =
      GetColoredPixels(static_cast<int>(fbl::round_up(width * expected_image_bytes_per_pixel,
                                                      kExpectedImageRowByteAlignment)),
                       width, height, kPixelFormat, {0, 0, 0, 255});

  bool images_are_same =
      CaptureCompare(read_values, expected_black_image, kPixelFormat, height, width);
  EXPECT_TRUE(images_are_same);
}

}  // namespace
}  // namespace test
}  // namespace flatland

// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/element/cpp/fidl.h>
#include <fuchsia/logger/cpp/fidl.h>
#include <fuchsia/sysmem/cpp/fidl.h>
#include <fuchsia/sysmem2/cpp/fidl.h>
#include <fuchsia/tracing/provider/cpp/fidl.h>
#include <fuchsia/ui/composition/cpp/fidl.h>
#include <fuchsia/virtualization/hardware/cpp/fidl.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/sys/component/cpp/testing/realm_builder_types.h>
#include <lib/zx/result.h>

#include <cmath>
#include <numeric>

#include <virtio/gpu.h>

#include "src/ui/scenic/lib/utils/pixel.h"
#include "src/ui/testing/ui_test_manager/ui_test_manager.h"
#include "src/ui/testing/ui_test_realm/ui_test_realm.h"
#include "src/virtualization/bin/vmm/device/tests/test_with_device.h"
#include "src/virtualization/bin/vmm/device/tests/virtio_queue_fake.h"

namespace {

enum Virtqueues : uint16_t {
  kControlQ,
  kCursorQ,
  kNumQueues,
};

constexpr uint16_t kQueueSize = 16;

const uint32_t kPageSize = zx_system_get_page_size();

constexpr uint32_t kPixelFormat = VIRTIO_GPU_FORMAT_B8G8R8A8_UNORM;
constexpr size_t kPixelSizeInBytes = 4;

constexpr uint32_t kCursorWidth = 64;
constexpr uint32_t kCursorHeight = 64;

const std::array<uint32_t, kNumQueues> kQueueDataSizes = {
    // A single full framebuffer for a 1280x800 scanout requires almost 4MiB. A cursor resource
    // (64x64) requires 16KiB. Ensure the controlq can safely accommodate these amounts as well as
    // any miscellaneous descriptors
    8 * 1024 * 1024,
    kPageSize,
};

// Resource IDs are client allocated, so any value here is fine except for 0. Some GPU commands (ex
// SET_SCANOUT) use resource_id == 0 to mean no resource so some implementations may fail to create
// a resource with resource_id == 0.
//
// Section 5.7.6.8: controlq: ...The driver can use resource_id = 0 to disable a scanout.
constexpr uint32_t kResourceId = 1;
constexpr uint32_t kScanoutId = 0;

static constexpr uint32_t kGpuStartupWidth = 1280;
static constexpr uint32_t kGpuStartupHeight = 720;

constexpr auto kComponentUrl = "#meta/virtio_gpu.cm";
constexpr auto kGraphicalPresenterUrl = "#meta/test_graphical_presenter.cm";

struct VirtioGpuTestParam {
  std::string test_name;
  bool configure_cursor_queue;
};

class VirtioGpuTest : public TestWithDevice,
                      public ::testing::WithParamInterface<VirtioGpuTestParam> {
 protected:
  VirtioGpuTest()
      :  // Place both queues after the miscellaneous data segment
        control_queue_(phys_mem_,
                       std::accumulate(kQueueDataSizes.begin(), kQueueDataSizes.end(), 0),
                       kQueueSize),
        cursor_queue_(phys_mem_, control_queue_.end(), kQueueSize) {}

  void SetUp() override {
    using component_testing::ChildRef;
    using component_testing::ParentRef;
    using component_testing::Protocol;
    using component_testing::RealmBuilder;
    using component_testing::RealmRoot;
    using component_testing::Route;

    ui_testing::UITestRealm::Config ui_config;
    ui_config.use_scene_owner = true;
    ui_config.accessibility_owner = ui_testing::UITestRealm::AccessibilityOwnerType::FAKE;
    ui_config.ui_to_client_services = {fuchsia::ui::composition::Flatland::Name_,
                                       fuchsia::ui::composition::Allocator::Name_};
    ui_config.exposed_client_services = {fuchsia::virtualization::hardware::VirtioGpu::Name_};

    ui_test_manager_.emplace(std::move(ui_config));

    constexpr auto kComponentName = "virtio_gpu";
    constexpr auto kGraphicalPresenterComponentName = "graphical_presenter";

    auto realm_builder = ui_test_manager_->AddSubrealm();
    realm_builder.AddChild(kComponentName, kComponentUrl);
    realm_builder.AddChild(kGraphicalPresenterComponentName, kGraphicalPresenterUrl);

    realm_builder
        .AddRoute(Route{
            .capabilities =
                {
                    Protocol{fuchsia::logger::LogSink::Name_},
                    Protocol{fuchsia::ui::composition::Flatland::Name_},
                },
            .source = ParentRef(),
            .targets = {ChildRef{kComponentName}, ChildRef{kGraphicalPresenterComponentName}}})
        .AddRoute(Route{.capabilities =
                            {
                                Protocol{fuchsia::sysmem::Allocator::Name_},
                                Protocol{fuchsia::sysmem2::Allocator::Name_},
                                Protocol{fuchsia::tracing::provider::Registry::Name_},
                                Protocol{fuchsia::ui::composition::Allocator::Name_},
                            },
                        .source = ParentRef(),
                        .targets = {ChildRef{kComponentName}}})
        .AddRoute(Route{.capabilities =
                            {
                                Protocol{fuchsia::virtualization::hardware::VirtioGpu::Name_},
                            },
                        .source = ChildRef{kComponentName},
                        .targets = {ParentRef()}})
        .AddRoute(Route{.capabilities =
                            {
                                Protocol{fuchsia::ui::app::ViewProvider::Name_},
                            },
                        .source = ChildRef{kGraphicalPresenterComponentName},
                        .targets = {ParentRef()}})
        .AddRoute(Route{.capabilities =
                            {
                                Protocol{fuchsia::element::GraphicalPresenter::Name_},
                            },
                        .source = ChildRef{kGraphicalPresenterComponentName},
                        .targets = {ChildRef{kComponentName}}});

    ui_test_manager_->BuildRealm();
    exposed_client_services_ = ui_test_manager_->CloneExposedServicesDirectory();

    ui_test_manager_->InitializeScene();

    fuchsia::virtualization::hardware::StartInfo start_info;
    // The cursor queue is placed at the very end of the guest memory range.
    zx_status_t status = MakeStartInfo(cursor_queue_.end(), &start_info);
    ASSERT_EQ(ZX_OK, status);

    status = exposed_client_services_->Connect(gpu_.NewRequest());
    ASSERT_EQ(ZX_OK, status);

    status = gpu_->Start(std::move(start_info), nullptr, nullptr);
    ASSERT_EQ(ZX_OK, status);

    // Configure device queues.
    VirtioQueueFake* queues[kNumQueues] = {&control_queue_, &cursor_queue_};
    for (uint16_t i = 0; i < kNumQueues; i++) {
      auto q = queues[i];
      q->Configure(std::accumulate(kQueueDataSizes.begin(), kQueueDataSizes.begin() + i, 0),
                   kQueueDataSizes[i]);
      status = gpu_->ConfigureQueue(i, q->size(), q->desc(), q->avail(), q->used());
      ASSERT_EQ(ZX_OK, status);
      if (!GetParam().configure_cursor_queue) {
        break;
      }
    }

    // Finish negotiating features.
    status = gpu_->Ready(0);
    ASSERT_EQ(ZX_OK, status);
  }

  void TearDown() override {
    bool complete = false;
    ui_test_manager_->TeardownRealm(
        [&](fit::result<fuchsia::component::Error> result) { complete = true; });
    RunLoopUntil([&]() { return complete; });
  }

  std::optional<fuchsia::ui::observation::geometry::ViewDescriptor> FindGpuView() {
    auto presenter_koid = ui_test_manager_->ClientViewRefKoid();
    if (!presenter_koid) {
      return {};
    }
    auto presenter = ui_test_manager_->FindViewFromSnapshotByKoid(*presenter_koid);
    if (!presenter || !presenter->has_children() || presenter->children().empty()) {
      return {};
    }
    return ui_test_manager_->FindViewFromSnapshotByKoid(presenter->children()[0]);
  }

  zx::result<std::pair<uint32_t, uint32_t>> WaitForScanout() {
    bool view_created =
        RunLoopWithTimeoutOrUntil([this] { return FindGpuView().has_value(); }, zx::sec(20));
    if (!view_created) {
      return zx::error(ZX_ERR_TIMED_OUT);
    }
    auto gpu_view = *FindGpuView();
    const auto& extent = gpu_view.layout().extent;
    return zx::ok(std::make_pair(std::round(extent.max.x - extent.min.x),
                                 std::round(extent.max.y - extent.min.y)));
  }

  template <typename T>
  zx_status_t SendRequest(uint16_t queue_id, VirtioQueueFake& queue, const T& request,
                          virtio_gpu_ctrl_hdr_t** response) {
    zx_status_t status = DescriptorChainBuilder(queue)
                             .AppendReadableDescriptor(&request, sizeof(request))
                             .AppendWritableDescriptor(response, sizeof(virtio_gpu_ctrl_hdr_t))
                             .Build();
    if (status != ZX_OK) {
      return status;
    }

    status = gpu_->NotifyQueue(queue_id);
    if (status != ZX_OK) {
      return status;
    }

    return WaitOnInterrupt();
  }

  template <typename T>
  zx_status_t SendControlRequest(const T& request, virtio_gpu_ctrl_hdr_t** response) {
    return SendRequest(kControlQ, control_queue_, request, response);
  }

  template <typename T>
  zx_status_t SendCursorRequest(const T& request, virtio_gpu_ctrl_hdr_t** response) {
    return SendRequest(kCursorQ, cursor_queue_, request, response);
  }

  void ResourceCreate2d() {
    virtio_gpu_ctrl_hdr_t* response;
    ASSERT_EQ(SendControlRequest(
                  virtio_gpu_resource_create_2d_t{
                      .hdr = {.type = VIRTIO_GPU_CMD_RESOURCE_CREATE_2D},
                      .resource_id = kResourceId,
                      .format = kPixelFormat,
                      .width = kGpuStartupWidth,
                      .height = kGpuStartupHeight,
                  },
                  &response),
              ZX_OK);
    ASSERT_EQ(VIRTIO_GPU_RESP_OK_NODATA, response->type);
  }

  void ResourceAttachBacking() {
    virtio_gpu_ctrl_hdr_t* response;
    ASSERT_EQ(SendControlRequest(
                  virtio_gpu_resource_attach_backing_t{
                      .hdr = {.type = VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING},
                      .resource_id = kResourceId,
                      .nr_entries = 0,
                  },
                  &response),
              ZX_OK);
    ASSERT_EQ(VIRTIO_GPU_RESP_OK_NODATA, response->type);
  }

  void SetScanout(uint32_t resource_id, uint32_t response_type) {
    virtio_gpu_ctrl_hdr_t* response;
    ASSERT_EQ(SendControlRequest(
                  virtio_gpu_set_scanout_t{
                      .hdr = {.type = VIRTIO_GPU_CMD_SET_SCANOUT},
                      .r = {.x = 0, .y = 0, .width = kGpuStartupWidth, .height = kGpuStartupHeight},
                      .scanout_id = kScanoutId,
                      .resource_id = resource_id,
                  },
                  &response),
              ZX_OK);
    EXPECT_EQ(response_type, response->type);
  }

  std::optional<ui_testing::UITestManager> ui_test_manager_;

  // Note: use of sync can be problematic here if the test environment needs to handle
  // some incoming FIDL requests.
  fuchsia::virtualization::hardware::VirtioGpuSyncPtr gpu_;
  VirtioQueueFake control_queue_;
  VirtioQueueFake cursor_queue_;
  std::unique_ptr<sys::ServiceDirectory> exposed_client_services_;
};

TEST_P(VirtioGpuTest, GetDisplayInfo) {
  auto geometry = WaitForScanout();
  ASSERT_TRUE(geometry.is_ok());
  auto [gpu_width, gpu_height] = *geometry;

  virtio_gpu_ctrl_hdr_t request = {
      .type = VIRTIO_GPU_CMD_GET_DISPLAY_INFO,
  };
  virtio_gpu_resp_display_info_t* response;
  zx_status_t status = DescriptorChainBuilder(control_queue_)
                           .AppendReadableDescriptor(&request, sizeof(request))
                           .AppendWritableDescriptor(&response, sizeof(*response))
                           .Build();
  ASSERT_EQ(ZX_OK, status);

  status = gpu_->NotifyQueue(0);
  ASSERT_EQ(ZX_OK, status);
  status = WaitOnInterrupt();
  ASSERT_EQ(ZX_OK, status);

  EXPECT_EQ(response->hdr.type, VIRTIO_GPU_RESP_OK_DISPLAY_INFO);
  EXPECT_EQ(response->pmodes[0].r.x, 0u);
  EXPECT_EQ(response->pmodes[0].r.y, 0u);
  EXPECT_EQ(response->pmodes[0].r.width, gpu_width);
  EXPECT_EQ(response->pmodes[0].r.height, gpu_height);
}

TEST_P(VirtioGpuTest, SetScanout) {
  ASSERT_TRUE(WaitForScanout().is_ok());
  ResourceCreate2d();
  ResourceAttachBacking();
  SetScanout(kResourceId, VIRTIO_GPU_RESP_OK_NODATA);
}

TEST_P(VirtioGpuTest, SetScanoutWithInvalidResourceId) {
  ASSERT_TRUE(WaitForScanout().is_ok());
  ResourceCreate2d();
  ResourceAttachBacking();
  SetScanout(UINT32_MAX, VIRTIO_GPU_RESP_ERR_INVALID_RESOURCE_ID);
}

TEST_P(VirtioGpuTest, CreateLargeResource) {
  virtio_gpu_ctrl_hdr_t* response;
  ASSERT_EQ(SendControlRequest(
                virtio_gpu_resource_create_2d_t{
                    .hdr = {.type = VIRTIO_GPU_CMD_RESOURCE_CREATE_2D},
                    .resource_id = kResourceId,
                    .width = UINT32_MAX,
                    .height = UINT32_MAX,
                },
                &response),
            ZX_OK);
  EXPECT_EQ(response->type, VIRTIO_GPU_RESP_ERR_OUT_OF_MEMORY);
}

TEST_P(VirtioGpuTest, InvalidTransferToHostParams) {
  ResourceCreate2d();
  ResourceAttachBacking();

  // Select a x/y/width/height values that overflow in a way that (x+width)
  // and (y+height) are within the buffer, but other calculations will not be.
  constexpr virtio_gpu_rect_t kBadRectangle = {
      .x = 0x0004'c000,
      .y = 0x0000'0008,
      .width = 0xfffb'4500,
      .height = 0x0000'02c8,
  };
  static_assert(kBadRectangle.width + kBadRectangle.x <= kGpuStartupWidth);
  static_assert(kBadRectangle.height + kBadRectangle.y <= kGpuStartupHeight);

  virtio_gpu_ctrl_hdr_t* response;
  ASSERT_EQ(
      SendControlRequest(
          virtio_gpu_transfer_to_host_2d_t{
              .hdr = {.type = VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D},
              .r = kBadRectangle,
              .offset = (kBadRectangle.y * kGpuStartupWidth + kBadRectangle.x) * kPixelSizeInBytes,
              .resource_id = kResourceId,
          },
          &response),
      ZX_OK);
  EXPECT_EQ(response->type, VIRTIO_GPU_RESP_ERR_INVALID_PARAMETER);
}

TEST_P(VirtioGpuTest, UpdateCursor) {
  if (!GetParam().configure_cursor_queue)
    return;

  auto geometry = WaitForScanout();
  ASSERT_TRUE(geometry.is_ok());
  auto [display_width, display_height] = *geometry;

  constexpr uint32_t kBackgroundResourceId = 10;
  constexpr uint32_t kCursorResourceId = 20;

  // Cursor hotspot position, relative to the cursor resource's top-left corner.
  constexpr uint32_t kCursorHotX = 63;
  constexpr uint32_t kCursorHotY = 63;

  // Position at which to draw the cursor resource.
  constexpr uint32_t kCursorX = 100;
  constexpr uint32_t kCursorY = 100;

  const auto& kWhite = utils::Pixel(0xff, 0xff, 0xff, 0xff);
  const auto& kBlack = utils::kBlack;
  const auto& kMagenta = utils::kMagenta;

  // Create background resource
  {
    virtio_gpu_ctrl_hdr_t* response;
    ASSERT_EQ(SendControlRequest(
                  virtio_gpu_resource_create_2d_t{
                      .hdr = {.type = VIRTIO_GPU_CMD_RESOURCE_CREATE_2D},
                      .resource_id = kBackgroundResourceId,
                      .format = kPixelFormat,
                      .width = display_width,
                      .height = display_height,
                  },
                  &response),
              ZX_OK);
    ASSERT_EQ(VIRTIO_GPU_RESP_OK_NODATA, response->type);
  }

  // Initialize backing memory for background resource
  uint32_t background_data_size = display_width * display_height * kPixelSizeInBytes;
  auto background_allocation = control_queue_.AllocData(background_data_size);
  std::memset(background_allocation.device_mem, 0xff, background_data_size);

  // Attach backing memory to background resource
  {
    struct {
      virtio_gpu_resource_attach_backing_t attach_backing;
      virtio_gpu_mem_entry_t entry;
    } __PACKED request = {
        .attach_backing =
            {
                .hdr = {.type = VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING},
                .resource_id = kBackgroundResourceId,
                .nr_entries = 1,
            },
        .entry =
            {
                .addr = background_allocation.driver_mem,
                .length = background_data_size,
            },
    };

    virtio_gpu_ctrl_hdr_t* response;
    ASSERT_EQ(SendControlRequest(request, &response), ZX_OK);
    ASSERT_EQ(VIRTIO_GPU_RESP_OK_NODATA, response->type);
  }

  const virtio_gpu_rect_t full_screen_rect = {
      .x = 0,
      .y = 0,
      .width = display_width,
      .height = display_height,
  };
  ASSERT_LE(full_screen_rect.width + full_screen_rect.x, display_width);
  ASSERT_LE(full_screen_rect.height + full_screen_rect.y, display_height);

  // Set scanout to resource
  {
    virtio_gpu_ctrl_hdr_t* response;
    ASSERT_EQ(SendControlRequest(
                  virtio_gpu_set_scanout_t{
                      .hdr = {.type = VIRTIO_GPU_CMD_SET_SCANOUT},
                      .r = full_screen_rect,
                      .scanout_id = kScanoutId,
                      .resource_id = kBackgroundResourceId,
                  },
                  &response),
              ZX_OK);
    ASSERT_EQ(VIRTIO_GPU_RESP_OK_NODATA, response->type);
  }

  // Transfer background resource to host
  {
    virtio_gpu_ctrl_hdr_t* response;
    ASSERT_EQ(SendControlRequest(
                  virtio_gpu_transfer_to_host_2d_t{
                      .hdr = {.type = VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D},
                      .r = full_screen_rect,
                      .offset = 0,
                      .resource_id = kBackgroundResourceId,
                  },
                  &response),
              ZX_OK);
    ASSERT_EQ(VIRTIO_GPU_RESP_OK_NODATA, response->type);
  }

  // Flush resource to display
  {
    virtio_gpu_ctrl_hdr_t* response;
    ASSERT_EQ(SendControlRequest(
                  virtio_gpu_resource_flush_t{
                      .hdr = {.type = VIRTIO_GPU_CMD_RESOURCE_FLUSH},
                      .r = full_screen_rect,
                      .resource_id = kBackgroundResourceId,
                  },
                  &response),
              ZX_OK);
    ASSERT_EQ(VIRTIO_GPU_RESP_OK_NODATA, response->type);
  }

  // We should have an opaque white background at this point.
  {
    std::map<utils::Pixel, uint32_t> expected_histogram{
        std::tuple{kWhite, display_width * display_height}};

    RunLoopUntil(
        [&]() { return ui_test_manager_->TakeScreenshot().Histogram() == expected_histogram; });

    auto screenshot = ui_test_manager_->TakeScreenshot();
    EXPECT_EQ(screenshot.width(), display_width);
    EXPECT_EQ(screenshot.height(), display_height);
    ASSERT_EQ(screenshot.Histogram(), expected_histogram);
  }

  // Create cursor resource
  {
    virtio_gpu_ctrl_hdr_t* response;
    ASSERT_EQ(SendControlRequest(
                  virtio_gpu_resource_create_2d_t{
                      .hdr = {.type = VIRTIO_GPU_CMD_RESOURCE_CREATE_2D},
                      .resource_id = kCursorResourceId,
                      .format = kPixelFormat,
                      .width = kCursorWidth,
                      .height = kCursorHeight,
                  },
                  &response),
              ZX_OK);
    ASSERT_EQ(VIRTIO_GPU_RESP_OK_NODATA, response->type);
  }

  // Initialize backing memory for cursor resource. It will be all black except for the
  // cursor hotspot pixel which will be magenta.
  uint32_t cursor_data_size = kCursorWidth * kCursorHeight * kPixelSizeInBytes;
  auto cursor_allocation = control_queue_.AllocData(cursor_data_size);
  uint8_t* cursor_data = static_cast<uint8_t*>(cursor_allocation.device_mem);
  // Pixel format is BGRA
  for (size_t i = 0; i < cursor_data_size; i += kPixelSizeInBytes) {
    cursor_data[i + 0] = kBlack.blue;
    cursor_data[i + 1] = kBlack.green;
    cursor_data[i + 2] = kBlack.red;
    cursor_data[i + 3] = kBlack.alpha;
  }
  size_t hotspot_idx =
      kCursorWidth * kPixelSizeInBytes * kCursorHotX + kPixelSizeInBytes * kCursorHotY;
  cursor_data[hotspot_idx + 0] = kMagenta.blue;
  cursor_data[hotspot_idx + 1] = kMagenta.green;
  cursor_data[hotspot_idx + 2] = kMagenta.red;
  cursor_data[hotspot_idx + 3] = kMagenta.alpha;

  // Attach backing memory to cursor resource
  {
    struct {
      virtio_gpu_resource_attach_backing_t attach_backing;
      virtio_gpu_mem_entry_t entry;
    } __PACKED request = {
        .attach_backing =
            {
                .hdr = {.type = VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING},
                .resource_id = kCursorResourceId,
                .nr_entries = 1,
            },
        .entry =
            {
                .addr = cursor_allocation.driver_mem,
                .length = cursor_data_size,
            },
    };

    virtio_gpu_ctrl_hdr_t* response;
    ASSERT_EQ(SendControlRequest(request, &response), ZX_OK);
    ASSERT_EQ(VIRTIO_GPU_RESP_OK_NODATA, response->type);
  }

  // Transfer cursor resource to host
  {
    virtio_gpu_ctrl_hdr_t* response;
    ASSERT_EQ(SendControlRequest(
                  virtio_gpu_transfer_to_host_2d_t{
                      .hdr = {.type = VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D},
                      .r =
                          {
                              .x = 0,
                              .y = 0,
                              .width = kCursorWidth,
                              .height = kCursorHeight,
                          },
                      .offset = 0,
                      .resource_id = kCursorResourceId,
                  },
                  &response),
              ZX_OK);
    ASSERT_EQ(VIRTIO_GPU_RESP_OK_NODATA, response->type);
  }

  // Try to update the cursor. (hot_x, hot_y) is the active pixel's coordinate within the cursor
  // image and should be the pixel eventually found at the (x, y) coordinate.
  {
    virtio_gpu_ctrl_hdr_t* response;
    ASSERT_EQ(SendCursorRequest(
                  virtio_gpu_update_cursor_t{
                      .hdr = {.type = VIRTIO_GPU_CMD_UPDATE_CURSOR},
                      .pos =
                          {
                              .scanout_id = kScanoutId,
                              .x = kCursorX,
                              .y = kCursorY,
                          },
                      .resource_id = kCursorResourceId,
                      .hot_x = kCursorHotX,
                      .hot_y = kCursorHotY,
                  },
                  &response),
              ZX_OK);
    ASSERT_EQ(VIRTIO_GPU_RESP_OK_NODATA, response->type);
  }

  // Verify the screenshot is as expected.
  {
    std::map<utils::Pixel, uint32_t> expected_histogram{
        std::tuple{kWhite, display_width * display_height - kCursorWidth * kCursorHeight},
        std::tuple{kBlack, kCursorWidth * kCursorHeight - 1}, std::tuple{kMagenta, 1}};

    RunLoopUntil(
        [&]() { return ui_test_manager_->TakeScreenshot().Histogram() == expected_histogram; });

    auto screen = ui_test_manager_->TakeScreenshot();
    EXPECT_EQ(screen.Histogram(), expected_histogram);

    auto screenshot = screen.screenshot();
    ASSERT_EQ(screenshot.size(), display_height);
    ASSERT_EQ(screenshot[0].size(), display_width);

    size_t below_cursor = kCursorY + kCursorHeight;
    size_t right_of_cursor = kCursorX + kCursorWidth;

    std::vector<utils::Pixel> scanline{display_width, kWhite};
    for (size_t row_idx = 0; row_idx < kCursorY; row_idx++) {
      EXPECT_EQ(screenshot[row_idx], scanline);
    }
    for (size_t row_idx = below_cursor; row_idx < display_height; row_idx++) {
      EXPECT_EQ(screenshot[row_idx], scanline);
    }

    for (size_t col_idx = kCursorX; col_idx < right_of_cursor; col_idx++) {
      scanline[col_idx] = kBlack;
    }
    for (size_t row_idx = kCursorY; row_idx < below_cursor; row_idx++) {
      if (row_idx != kCursorY + kCursorHotY) {
        EXPECT_EQ(screenshot[row_idx], scanline);
      }
    }

    scanline[kCursorX + kCursorHotX] = kMagenta;
    EXPECT_EQ(screenshot[kCursorY + kCursorHotY], scanline);
  }

  // Clear the cursor with resource_id 0.
  {
    virtio_gpu_ctrl_hdr_t* response;
    ASSERT_EQ(SendCursorRequest(
                  virtio_gpu_update_cursor_t{
                      .hdr = {.type = VIRTIO_GPU_CMD_UPDATE_CURSOR},
                      .pos =
                          {
                              .scanout_id = kScanoutId,
                              .x = kCursorX,
                              .y = kCursorY,
                          },
                      .resource_id = 0,
                      .hot_x = kCursorHotX,
                      .hot_y = kCursorHotY,
                  },
                  &response),
              ZX_OK);
    ASSERT_EQ(VIRTIO_GPU_RESP_OK_NODATA, response->type);
  }

  // Once more we should have just a white background.
  {
    std::map<utils::Pixel, uint32_t> expected_histogram{
        std::tuple{kWhite, display_width * display_height}};

    RunLoopUntil(
        [&]() { return ui_test_manager_->TakeScreenshot().Histogram() == expected_histogram; });

    auto screenshot = ui_test_manager_->TakeScreenshot();
    EXPECT_EQ(screenshot.width(), display_width);
    EXPECT_EQ(screenshot.height(), display_height);
    ASSERT_EQ(screenshot.Histogram(), expected_histogram);
  }
}

INSTANTIATE_TEST_SUITE_P(VirtioGpuComponentsTest, VirtioGpuTest,
                         testing::Values(VirtioGpuTestParam{"cursorq", true},
                                         VirtioGpuTestParam{"nocursorq", false}),
                         [](const testing::TestParamInfo<VirtioGpuTestParam>& info) {
                           return info.param.test_name;
                         });
}  // namespace

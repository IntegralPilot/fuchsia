// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <endian.h>
#include <errno.h>
#include <fidl/fuchsia.hardware.display.types/cpp/wire.h>
#include <fidl/fuchsia.hardware.display/cpp/fidl.h>
#include <fidl/fuchsia.hardware.display/cpp/wire.h>
#include <fidl/fuchsia.images2/cpp/wire.h>
#include <fidl/fuchsia.sysinfo/cpp/wire.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/loop.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/incoming/cpp/service_member_watcher.h>
#include <lib/fzl/vmo-mapper.h>
#include <lib/sysmem-version/sysmem-version.h>
#include <lib/zircon-internal/align.h>
#include <zircon/process.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>
#include <zircon/types.h>

#include <algorithm>
#include <cinttypes>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <limits>
#include <memory>
#include <span>
#include <string_view>
#include <vector>

#include <fbl/string_buffer.h>
#include <fbl/vector.h>

#include "src/graphics/display/bin/display-test/display.h"
#include "src/graphics/display/bin/display-test/virtual-layer.h"
#include "src/graphics/display/lib/api-types/cpp/buffer-collection-id.h"
#include "src/graphics/display/lib/api-types/cpp/config-stamp.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/event-id.h"
#include "src/graphics/display/lib/api-types/cpp/image-id.h"
#include "src/graphics/display/lib/api-types/cpp/layer-id.h"
#include "src/graphics/display/lib/api-types/cpp/vsync-ack-cookie.h"

using display_test::ColorLayer;
using display_test::Display;
using display_test::PrimaryLayer;
using display_test::VirtualLayer;

namespace {

zx::result<fidl::ClientEnd<fuchsia_hardware_display::Provider>> ConnectToDisplayProvider() {
  component::SyncServiceMemberWatcher<fuchsia_hardware_display::Service::Provider> watcher;
  static constexpr zx::duration kTimeoutSeconds = zx::sec(30);
  zx::result<fidl::ClientEnd<fuchsia_hardware_display::Provider>> watch_result =
      watcher.GetNextInstance(/*stop_at_idle=*/false, zx::deadline_after(kTimeoutSeconds));
  if (watch_result.status_value() == ZX_ERR_TIMED_OUT) {
    fprintf(stderr, "Timed out after %" PRId64 "seconds waiting for display Provider",
            kTimeoutSeconds.to_secs());
    return watch_result.take_error();
  }
  return watch_result;
}

// Listens to [`fuchsia.hardware.display/CoordinatorListener`] requests.
//
// This class is thread-unsafe. It must be accessed from a single thread or a
// dispatcher running on that thread.
class CoordinatorListener final
    : public fidl::WireServer<fuchsia_hardware_display::CoordinatorListener> {
 public:
  CoordinatorListener() = default;

  CoordinatorListener(const CoordinatorListener&) = delete;
  CoordinatorListener(CoordinatorListener&&) = delete;
  CoordinatorListener& operator=(const CoordinatorListener&) = delete;
  CoordinatorListener& operator=(CoordinatorListener&&) = delete;

  void Bind(fidl::ServerEnd<fuchsia_hardware_display::CoordinatorListener> server,
            async_dispatcher_t& dispatcher) {
    ZX_DEBUG_ASSERT(!binding_.has_value());
    binding_ = fidl::BindServer(&dispatcher, std::move(server), this);
  }

  // fidl::Server<fuchsia_hardware_display::CoordinatorListener>:
  void OnDisplaysChanged(OnDisplaysChangedRequestView request,
                         OnDisplaysChangedCompleter::Sync& completer) override {
    fidl::Arena arena;
    for (const fuchsia_hardware_display::wire::Info& display : request->added) {
      displays_.emplace(display::DisplayId(display.id), display);
    }
    for (const fuchsia_hardware_display_types::wire::DisplayId& display_id : request->removed) {
      displays_.erase(display::DisplayId(display_id));
    }
  }
  void OnVsync(OnVsyncRequestView request, OnVsyncCompleter::Sync& completer) override {
    vsync_count_++;
    latest_config_stamp_ = display::ConfigStamp(request->displayed_config_stamp);
    if (display::VsyncAckCookie(request->cookie) != display::kInvalidVsyncAckCookie) {
      pending_vsync_cookie_ = display::VsyncAckCookie(request->cookie);
    }
  }
  void OnClientOwnershipChange(OnClientOwnershipChangeRequestView request,
                               OnClientOwnershipChangeCompleter::Sync& completer) override {
    has_ownership_ = request->has_ownership;
  }
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_display::CoordinatorListener> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  const std::unordered_map<display::DisplayId, Display>& displays() { return displays_; }
  bool has_ownership() const { return has_ownership_; }
  int64_t vsync_count() const { return vsync_count_; }
  display::ConfigStamp latest_config_stamp() const { return latest_config_stamp_; }

  display::VsyncAckCookie TakePendingVsyncCookie() {
    display::VsyncAckCookie to_take = pending_vsync_cookie_;
    pending_vsync_cookie_ = display::kInvalidVsyncAckCookie;
    return to_take;
  }

 private:
  std::unordered_map<display::DisplayId, Display> displays_;
  bool has_ownership_ = false;
  int64_t vsync_count_ = 0;
  display::ConfigStamp latest_config_stamp_ = display::kInvalidConfigStamp;
  display::VsyncAckCookie pending_vsync_cookie_ = display::kInvalidVsyncAckCookie;

  std::optional<fidl::ServerBindingRef<fuchsia_hardware_display::CoordinatorListener>> binding_;
};

static CoordinatorListener g_coordinator_listener;
static zx_handle_t device_handle;
static fidl::WireSyncClient<fuchsia_hardware_display::Coordinator> dc;

constexpr display::EventId kEventId(13);
constexpr display::BufferCollectionId kBufferCollectionId(12);
// Use a large ID to avoid conflict with Image IDs allocated by VirtualLayers.
constexpr display::ImageId kCaptureImageId(std::numeric_limits<uint64_t>::max());
zx::event client_event_;
fidl::SyncClient<fuchsia_sysmem2::BufferCollection> collection_;
zx::vmo capture_vmo;

enum TestBundle {
  SIMPLE = 0,
  FLIP,
  BLANK,
  BUNDLE_COUNT,
};

static constexpr const char* testbundle_names[] = {"SIMPLE", "FLIP", "BLANK"};

enum Platforms {
  INTEL_PLATFORM = 0,
  AMLOGIC_PLATFORM,
  AEMU_PLATFORM,
  QEMU_PLATFORM,
  UNKNOWN_PLATFORM,
  PLATFORM_COUNT,
};

Platforms platform = UNKNOWN_PLATFORM;
fbl::StringBuffer<fuchsia_sysinfo::wire::kBoardNameLen> board_name;

bool BindDisplay(fidl::ClientEnd<fuchsia_hardware_display::Provider> provider,
                 async::Loop& coordinator_listener_loop, fbl::Vector<Display>* displays) {
  printf("Opening coordinator\n");

  auto [coordinator_client, coordinator_server] =
      fidl::Endpoints<fuchsia_hardware_display::Coordinator>::Create();
  auto [listener_client, listener_server] =
      fidl::Endpoints<fuchsia_hardware_display::CoordinatorListener>::Create();

  fidl::Arena arena;
  auto open_coordinator_request =
      fuchsia_hardware_display::wire::ProviderOpenCoordinatorRequest::Builder(arena)
          .coordinator(std::move(coordinator_server))
          .coordinator_listener(std::move(listener_client))
          .priority(fuchsia_hardware_display::wire::ClientPriority{
              .value = fuchsia_hardware_display::wire::kTestUtilityClientPriorityValue})
          .Build();
  fidl::WireResult open_response =
      fidl::WireCall(provider)->OpenCoordinator(open_coordinator_request);
  if (!open_response.ok()) {
    printf("Failed to call service handle: %s\n", open_response.FormatDescription().c_str());
    return false;
  }
  if (open_response.value().is_error()) {
    printf("Failed to open coordinator %d (%s)\n", open_response.value().error_value(),
           zx_status_get_string(open_response.value().error_value()));
    return false;
  }

  dc = fidl::WireSyncClient(std::move(coordinator_client));
  g_coordinator_listener.Bind(std::move(listener_server), *coordinator_listener_loop.dispatcher());

  while (g_coordinator_listener.displays().empty()) {
    printf("Waiting for display\n");
    zx_status_t status =
        coordinator_listener_loop.Run(zx::time_monotonic::infinite(), /*once=*/true);
    if (status != ZX_OK || g_coordinator_listener.vsync_count() > 0) {
      printf("Got unexpected message\n");
      return false;
    }
  }

  *displays = {};
  for (const auto& [id, display] : g_coordinator_listener.displays()) {
    displays->push_back(display);
  }

  return true;
}

Display* find_display(fbl::Vector<Display>& displays, const char* id_str) {
  errno = 0;
  uint64_t id_value = strtoul(id_str, nullptr, 10);
  // strtoul() sets `errno` to non-zero on failure.
  if (errno != 0) {
    fprintf(stderr, "Failed to convert display ID \"%s\": %s\n", id_str, strerror(errno));
    errno = 0;
    return nullptr;
  }
  display::DisplayId id(id_value);
  if (id != display::kInvalidDisplayId) {
    for (auto& d : displays) {
      if (d.id() == id) {
        return &d;
      }
    }
  }
  return nullptr;
}

bool update_display_layers(const fbl::Vector<std::unique_ptr<VirtualLayer>>& layers,
                           const Display& display, fbl::Vector<display::LayerId>* current_layers) {
  fbl::Vector<display::LayerId> new_layers;

  for (auto& layer : layers) {
    display::LayerId id = layer->id(display.id());
    if (id.value() != fuchsia_hardware_display_types::wire::kInvalidDispId) {
      new_layers.push_back(id);
    }
  }

  bool layer_change = new_layers.size() != current_layers->size();
  if (!layer_change) {
    for (unsigned i = 0; i < new_layers.size(); i++) {
      if (new_layers[i] != (*current_layers)[i]) {
        layer_change = true;
        break;
      }
    }
  }

  if (layer_change) {
    current_layers->swap(new_layers);

    std::vector<fuchsia_hardware_display::wire::LayerId> current_layers_fidl_id;
    current_layers_fidl_id.reserve(current_layers->size());
    for (const display::LayerId& layer_id : *current_layers) {
      current_layers_fidl_id.push_back(layer_id.ToFidl());
    }
    if (!dc->SetDisplayLayers(
               display.id().ToFidl(),
               fidl::VectorView<fuchsia_hardware_display::wire::LayerId>::FromExternal(
                   current_layers_fidl_id))
             .ok()) {
      printf("Failed to set layers\n");
      return false;
    }
  }
  return true;
}

zx_status_t apply_config(fuchsia_hardware_display::wire::ConfigStamp stamp) {
  auto check_result = dc->CheckConfig();
  if (!check_result.ok()) {
    printf("Failed to make check call: %s\n", check_result.FormatDescription().c_str());
    return check_result.error().status();
  }

  if (check_result.value().res != fuchsia_hardware_display_types::wire::ConfigResult::kOk) {
    printf("Config not valid (%d)\n", static_cast<uint32_t>(check_result.value().res));
    return ZX_ERR_INVALID_ARGS;
  }

  fidl::Arena arena;
  auto builder = fuchsia_hardware_display::wire::CoordinatorCommitConfigRequest::Builder(arena);
  builder.stamp(stamp);
  auto apply_result = dc->CommitConfig(builder.Build());
  if (!apply_result.ok()) {
    printf("Apply failed\n");
    return apply_result.error().status();
  }

  return ZX_OK;
}

zx_status_t wait_for_vsync(async::Loop& coordinator_listener_loop,
                           fuchsia_hardware_display::wire::ConfigStamp expected_stamp) {
  zx_status_t status = coordinator_listener_loop.Run(zx::time_monotonic::infinite(), /*once=*/true);
  if (status != ZX_OK) {
    printf("wait_for_vsync(): Failed to run coordinator listener loop: %s",
           zx_status_get_string(status));
    return status;
  }
  if (!g_coordinator_listener.has_ownership()) {
    return ZX_ERR_NEXT;
  }
  if (g_coordinator_listener.displays().empty()) {
    printf("wait_for_vsync(): Display disconnected\n");
    return ZX_ERR_STOP;
  }

  // Received a Vsync event
  display::VsyncAckCookie pending_vsync_cookie = g_coordinator_listener.TakePendingVsyncCookie();
  if (pending_vsync_cookie != display::kInvalidVsyncAckCookie) {
    (void)dc->AcknowledgeVsync(pending_vsync_cookie.value());
  }

  if (g_coordinator_listener.latest_config_stamp() < display::ConfigStamp(expected_stamp)) {
    return ZX_ERR_NEXT;
  }
  return ZX_OK;
}

zx_status_t set_minimum_rgb(uint8_t min_rgb) {
  auto resp = dc->SetMinimumRgb(min_rgb);
  return resp.status();
}

zx_status_t capture_setup(Display& display) {
  // TODO(https://fxbug.dev/42117494): Pull common image setup code into a library

  // First make sure capture is supported on this platform
  auto support_resp = dc->IsCaptureSupported();
  if (!support_resp.ok()) {
    printf("%s: %s\n", __func__, support_resp.FormatDescription().c_str());
    return ZX_ERR_NOT_SUPPORTED;
  }
  if (!support_resp->value()->supported) {
    return ZX_ERR_NOT_SUPPORTED;
  }
  // Import event used to get notified once capture is completed
  auto status = zx::event::create(0, &client_event_);
  if (status != ZX_OK) {
    printf("Could not create event %d\n", status);
    return status;
  }
  zx::event e2;
  status = client_event_.duplicate(ZX_RIGHT_SAME_RIGHTS, &e2);
  if (status != ZX_OK) {
    printf("Could not duplicate event %d\n", status);
    return status;
  }
  auto event_status = dc->ImportEvent(std::move(e2), kEventId.ToFidl());
  if (event_status.status() != ZX_OK) {
    printf("Could not import event: %s\n", event_status.FormatDescription().c_str());
    return event_status.status();
  }

  // get connection to sysmem
  zx::result sysmem_client = component::Connect<fuchsia_sysmem2::Allocator>();
  if (sysmem_client.is_error()) {
    printf("Could not connect to sysmem Allocator %s\n", sysmem_client.status_string());
    return sysmem_client.status_value();
  }
  auto sysmem_allocator = fidl::SyncClient(std::move(sysmem_client.value()));

  // Create and import token
  zx::result token_endpoints = fidl::CreateEndpoints<fuchsia_sysmem2::BufferCollectionToken>();
  if (token_endpoints.is_error()) {
    printf("Could not create token channel %d\n", token_endpoints.error_value());
    return token_endpoints.error_value();
  }
  auto token = fidl::SyncClient(std::move(token_endpoints->client));

  // pass token server to sysmem allocator
  fuchsia_sysmem2::AllocatorAllocateSharedCollectionRequest allocate_shared_request;
  allocate_shared_request.token_request() = std::move(token_endpoints->server);
  auto allocate_shared_result =
      sysmem_allocator->AllocateSharedCollection(std::move(allocate_shared_request));
  if (!allocate_shared_result.is_ok()) {
    printf("Could not pass token to sysmem allocator: %s\n",
           allocate_shared_result.error_value().FormatDescription().c_str());
    return allocate_shared_result.error_value().status();
  }

  // duplicate the token and pass to display driver
  zx::result token_dup_endpoints = fidl::CreateEndpoints<fuchsia_sysmem2::BufferCollectionToken>();
  if (token_dup_endpoints.is_error()) {
    printf("Could not create duplicate token channel %d\n", token_dup_endpoints.error_value());
    return token_dup_endpoints.error_value();
  }
  fidl::SyncClient display_token(std::move(token_dup_endpoints->client));
  fuchsia_sysmem2::BufferCollectionTokenDuplicateRequest dup_request;
  dup_request.rights_attenuation_mask() = ZX_RIGHT_SAME_RIGHTS;
  dup_request.token_request() = std::move(token_dup_endpoints->server);
  auto dup_res = token->Duplicate(std::move(dup_request));
  if (!dup_res.is_ok()) {
    printf("Could not duplicate token: %s\n", dup_res.error_value().FormatDescription().c_str());
    return dup_res.error_value().status();
  }
  // TODO(https://fxbug.dev/42180237) Consider handling the error instead of ignoring it.
  (void)token->Sync();
  auto import_resp =
      dc->ImportBufferCollection(kBufferCollectionId.ToFidl(), display_token.TakeClientEnd());
  if (import_resp.status() != ZX_OK) {
    printf("Could not import token: %s\n", import_resp.FormatDescription().c_str());
    return import_resp.status();
  }

  // set buffer constraints
  fuchsia_hardware_display_types::wire::ImageBufferUsage image_buffer_usage = {
      .tiling_type = fuchsia_hardware_display_types::wire::kImageTilingTypeCapture,
  };
  auto constraints_resp =
      dc->SetBufferCollectionConstraints(kBufferCollectionId.ToFidl(), image_buffer_usage);
  if (constraints_resp.status() != ZX_OK) {
    printf("Could not set capture constraints %s\n", constraints_resp.FormatDescription().c_str());
    return constraints_resp.status();
  }

  // setup our our constraints for buffer to be allocated
  zx::result collection_endpoints = fidl::CreateEndpoints<fuchsia_sysmem2::BufferCollection>();
  if (collection_endpoints.is_error()) {
    printf("Could not create collection channel %d\n", collection_endpoints.error_value());
    return collection_endpoints.error_value();
  }
  // let's return token
  fuchsia_sysmem2::AllocatorBindSharedCollectionRequest bind_shared_request;
  bind_shared_request.token() = token.TakeClientEnd();
  bind_shared_request.buffer_collection_request() = std::move(collection_endpoints->server);
  auto bind_resp = sysmem_allocator->BindSharedCollection(std::move(bind_shared_request));
  if (!bind_resp.is_ok()) {
    printf("Could not bind to shared collection: %s\n",
           bind_resp.error_value().FormatDescription().c_str());
    return bind_resp.error_value().status();
  }

  // finally setup our constraints
  fuchsia_sysmem2::BufferCollectionSetConstraintsRequest set_constraints_request;
  auto& constraints = set_constraints_request.constraints().emplace();
  constraints.usage().emplace().cpu() =
      fuchsia_sysmem2::kCpuUsageReadOften | fuchsia_sysmem2::kCpuUsageWriteOften;
  constraints.min_buffer_count_for_camping() = 1;
  constraints.buffer_memory_constraints().emplace().ram_domain_supported() = true;
  fuchsia_sysmem2::ImageFormatConstraints& image_constraints =
      constraints.image_format_constraints().emplace().emplace_back();
  if (platform == AMLOGIC_PLATFORM) {
    image_constraints.pixel_format() = fuchsia_images2::PixelFormat::kB8G8R8;
  } else {
    image_constraints.pixel_format() = fuchsia_images2::PixelFormat::kB8G8R8A8;
  }
  image_constraints.color_spaces().emplace().emplace_back(fuchsia_images2::ColorSpace::kSrgb);

  collection_ = fidl::SyncClient(std::move(collection_endpoints->client));
  auto set_constraints_result = collection_->SetConstraints(std::move(set_constraints_request));
  if (!set_constraints_result.is_ok()) {
    printf("Could not set buffer constraints: %s\n",
           set_constraints_result.error_value().FormatDescription().c_str());
    return set_constraints_result.error_value().status();
  }

  // wait for allocation
  auto wait_resp = collection_->WaitForAllBuffersAllocated();
  if (!wait_resp.is_ok()) {
    printf("Wait for buffer allocation failed: %s\n",
           wait_resp.error_value().FormatDescription().c_str());
    zx_status_t status;
    if (wait_resp.error_value().is_framework_error()) {
      status = ZX_ERR_INTERNAL;
    } else {
      status = sysmem::V1CopyFromV2Error(wait_resp.error_value().domain_error());
    }
    return status;
  }

  capture_vmo =
      std::move(wait_resp.value().buffer_collection_info()->buffers()->at(0).vmo().value());

  // import image for capture
  // TODO(https://fxbug.dev/332521780): Display clients will be required to
  // pass the captured display's mode information.
  fuchsia_hardware_display_types::wire::ImageMetadata capture_metadata = {
      .dimensions = {.width = display.mode().active_area.width,
                     .height = display.mode().active_area.height},
      .tiling_type = fuchsia_hardware_display_types::wire::kImageTilingTypeCapture,
  };
  fidl::WireResult import_capture_result =
      dc->ImportImage(capture_metadata, kBufferCollectionId.ToFidl(), 0, kCaptureImageId.ToFidl());
  if (import_capture_result.status() != ZX_OK) {
    printf("Failed to start capture: %s\n", import_capture_result.FormatDescription().c_str());
    return import_capture_result.status();
  }
  return ZX_OK;
}

zx_status_t capture_start() {
  // start capture
  fidl::WireResult start_capture_result =
      dc->StartCapture(kEventId.ToFidl(), kCaptureImageId.ToFidl());
  if (start_capture_result.status() != ZX_OK) {
    printf("Could not start capture: %s\n", start_capture_result.FormatDescription().c_str());
    return start_capture_result.status();
  }
  // wait for capture to complete
  uint32_t observed;
  auto event_res =
      client_event_.wait_one(ZX_EVENT_SIGNALED, zx::deadline_after(zx::sec(1)), &observed);
  if (event_res == ZX_OK) {
    client_event_.signal(ZX_EVENT_SIGNALED, 0);
  } else {
    printf("capture failed %d\n", event_res);
    return event_res;
  }
  return ZX_OK;
}

bool AmlogicCompareCapturedImage(std::span<const uint8_t> captured_image,
                                 std::span<const uint8_t> input_image,
                                 fuchsia_images2::wire::PixelFormat input_image_pixel_format,
                                 int height, int width) {
  assert(input_image_pixel_format == fuchsia_images2::wire::PixelFormat::kB8G8R8A8 ||
         input_image_pixel_format == fuchsia_images2::wire::PixelFormat::kR8G8B8A8);

  auto expected_image = std::vector<uint8_t>(input_image.begin(), input_image.end());

  // Amlogic captured images are always in packed (least-significant) B8G8R8
  // (most-siginificant) format. To avoid out-of-order data access, we convert
  // the endianness of the input image, if they are in (least-significant)
  // R8G8B8A8 (most-significant) order.
  if (input_image_pixel_format == fuchsia_images2::wire::PixelFormat::kB8G8R8A8) {
    for (size_t i = 0; i + 3 < expected_image.size(); i += 4) {
      std::swap(expected_image[i], expected_image[i + 3]);
      std::swap(expected_image[i + 1], expected_image[i + 2]);
    }
  }

  // Capture image are of RGB888 formats; each pixel has 3 bytes.
  constexpr int kCaptureImageBytesPerPixel = 3;
  const int capture_stride = ZX_ALIGN(width * kCaptureImageBytesPerPixel, 64);
  // Input image are of R8G8B8A8 or B8R8G8A8 formats; each pixel has 4 bytes.
  constexpr int kInputImageByetsPerPixel = 4;
  const int expected_stride = ZX_ALIGN(width * kInputImageByetsPerPixel, 64);

  // Ignore the first row. It sometimes contains junk (hardware bug).
  int start_row = 1;
  int end_row = height;

  int start_column = 0;
  // Ignore the last column for Astro only. It contains junk bytes (hardware bug).
  const bool board_is_astro =
      std::string_view(board_name.data(), board_name.size()).find("astro") !=
      std::string_view::npos;
  int end_column = board_is_astro ? (width - 1) : width;

  for (int row = start_row; row < end_row; row++) {
    for (int column = start_column; column < end_column; column++) {
      for (int channel = 0; channel < 3; channel++) {
        // On expected image, the first byte is alpha channel, so we ignore it.
        int expected_byte_index = row * expected_stride + column * 4 + 1 + channel;
        int captured_byte_index = row * capture_stride + column * 3 + channel;

        constexpr int kColorDifferenceThreshold = 2;
        if (abs(expected_image[expected_byte_index] - captured_image[captured_byte_index]) >
            kColorDifferenceThreshold) {
          printf("Pixel different: (row=%d, col=%d, channel=%d) expected 0x%02x captured 0x%02x\n",
                 row, column, channel, expected_image[expected_byte_index],
                 captured_image[captured_byte_index]);
          return false;
        }
      }
    }
  }
  return true;
}

bool CompareCapturedImage(std::span<const uint8_t> input_image,
                          fuchsia_images2::wire::PixelFormat input_image_pixel_format, int height,
                          int width) {
  if (input_image.data() == nullptr) {
    printf("%s: input image is null\n", __func__);
    return false;
  }

  fzl::VmoMapper mapped_capture_vmo;
  size_t capture_vmo_size;
  auto status = capture_vmo.get_size(&capture_vmo_size);
  if (status != ZX_OK) {
    printf("capture vmo get size failed %d\n", status);
    return status;
  }
  status =
      mapped_capture_vmo.Map(capture_vmo, 0, capture_vmo_size, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE);
  if (status != ZX_OK) {
    printf("Could not map capture vmo %d\n", status);
    return status;
  }
  auto* ptr = reinterpret_cast<uint8_t*>(mapped_capture_vmo.start());
  zx_cache_flush(ptr, capture_vmo_size, ZX_CACHE_FLUSH_INVALIDATE);

  if (platform == AMLOGIC_PLATFORM) {
    return AmlogicCompareCapturedImage(
        std::span(reinterpret_cast<const uint8_t*>(mapped_capture_vmo.start()), capture_vmo_size),
        input_image, input_image_pixel_format, height, width);
  }

  return !memcmp(input_image.data(), mapped_capture_vmo.start(), capture_vmo_size);
}

void capture_release() {
  // TODO(https://fxbug.dev/42180237) Consider handling the error instead of ignoring it.
  (void)dc->ReleaseImage(kCaptureImageId.ToFidl());
  // TODO(https://fxbug.dev/42180237) Consider handling the error instead of ignoring it.
  (void)dc->ReleaseBufferCollection(kBufferCollectionId.ToFidl());
}

void usage(void) {
  printf(
      "Usage: display-test [OPTIONS]\n\n"
      "--dump                   : print properties of attached display\n"
      "--mode-set D N           : Set Display D to mode N (use dump option for choices)\n"
      "--format-set D N         : Set Display D to format N (use dump option for choices)\n"
      "--grayscale              : Display images in grayscale mode (default off)\n"
      "--num-frames N           : Run test in N number of frames (default 120)\n"
      "                           N can be an integer or 'infinite'\n"
      "--delay N                : Add delay (ms) between Vsync complete and next configuration\n"
      "--capture                : Capture each display frame and verify\n"
      "--fgcolor 0xaarrggbb     : Set foreground color\n"
      "--bgcolor 0xaarrggbb     : Set background color\n"
      "--preoffsets x,y,z       : set preoffsets for color correction\n"
      "--postoffsets x,y,z      : set postoffsets for color correction\n"
      "--coeff c00,c01,...,,c22 : 3x3 coefficient matrix for color correction\n"
      "--enable-alpha           : Enable per-pixel alpha blending.\n"
      "--opacity o              : Set the opacity of the screen\n"
      "                           <o> is a value between [0 1] inclusive\n"
      "--enable-compression     : Enable framebuffer compression.\n"
      "--apply-config-once      : Apply configuration once in single buffer mode.\n"
      "--clamp-rgb c            : Set minimum RGB value [0 255].\n"
      "--configs-per-vsync n    : Number of configs applied per vsync\n"
      "--pattern pattern        : Image pattern to use - 'checkerboard' (default) or 'border'\n"
      "\nTest Modes:\n\n"
      "--bundle N       : Run test from test bundle N as described below\n\n"
      "                   bundle %d: Display a single pattern using single buffer\n"
      "                   bundle %d: Flip between two buffers to display a pattern\n"
      "                   bundle %d: Blank the screen and sleep for --num-frames.\n"
      "                   (default: %d on Intel / Amlogic platforms, %d otherwise)\n\n"
      "--help           : Show this help message\n",
      SIMPLE, FLIP, BLANK, FLIP, SIMPLE);
}

Platforms GetPlatform() {
  zx::result channel = component::Connect<fuchsia_sysinfo::SysInfo>();
  if (channel.is_error()) {
    return UNKNOWN_PLATFORM;
  }

  const fidl::WireResult result = fidl::WireCall(channel.value())->GetBoardName();
  if (!result.ok()) {
    return UNKNOWN_PLATFORM;
  }
  const fidl::WireResponse response = result.value();
  if (response.status != ZX_OK) {
    return UNKNOWN_PLATFORM;
  };

  board_name.Clear();
  board_name.Append(response.name.get());

  printf("Found board %s\n", board_name.c_str());

  std::string_view board_name_cmp(board_name);
  if (board_name_cmp == "x64" || board_name_cmp == "chromebook-x64" || board_name_cmp == "Eve" ||
      board_name_cmp.find("Nocturne") != std::string_view::npos ||
      board_name_cmp.find("NUC") != std::string_view::npos) {
    return INTEL_PLATFORM;
  }
  if (board_name_cmp.find("astro") != std::string_view::npos ||
      board_name_cmp.find("sherlock") != std::string_view::npos ||
      board_name_cmp.find("vim2") != std::string_view::npos ||
      board_name_cmp.find("vim3") != std::string_view::npos ||
      board_name_cmp.find("nelson") != std::string_view::npos ||
      board_name_cmp.find("luis") != std::string_view::npos) {
    return AMLOGIC_PLATFORM;
  }
  if (board_name_cmp.find("qemu") != std::string_view::npos ||
      board_name_cmp.find("Standard PC (Q35 + ICH9, 2009)") != std::string_view::npos) {
    return QEMU_PLATFORM;
  }
  return UNKNOWN_PLATFORM;
}

}  // namespace

int main(int argc, const char* argv[]) {
  printf("Running display test\n");

  fbl::Vector<Display> displays;
  fbl::Vector<fbl::Vector<display::LayerId>> display_layers;
  fbl::Vector<std::unique_ptr<VirtualLayer>> layers;
  std::optional<int32_t> num_frames = 120;  // default to 120 frames. std::nullopt means infinite
  int32_t delay = 0;
  bool capture = false;
  bool verify_capture = false;
  async::Loop coordinator_listener_loop(&kAsyncLoopConfigNeverAttachToThread);

  platform = GetPlatform();

  // Select a "test bundle" based on the platform.  This bundle is a configuration flag which
  // controls the initialization of various test objects.
  TestBundle testbundle;
  switch (platform) {
    case INTEL_PLATFORM:
    case AMLOGIC_PLATFORM:
      testbundle = FLIP;
      break;
    default:
      testbundle = SIMPLE;
  }

  zx::result<fidl::ClientEnd<fuchsia_hardware_display::Provider>> provider_result =
      ConnectToDisplayProvider();
  if (provider_result.is_error()) {
    fprintf(stderr, "Failed to connect to display provider: %s\n", provider_result.status_string());
    return -1;
  }
  fidl::ClientEnd<fuchsia_hardware_display::Provider> provider = std::move(provider_result.value());

  if (!BindDisplay(std::move(provider), coordinator_listener_loop, &displays)) {
    usage();
    return -1;
  }

  if (displays.is_empty()) {
    printf("No displays available\n");
    return 0;
  }

  for (unsigned i = 0; i < displays.size(); i++) {
    display_layers.push_back(fbl::Vector<display::LayerId>());
  }

  argc--;
  argv++;

  display_test::Image::Pattern image_pattern = display_test::Image::Pattern::kCheckerboard;
  uint32_t fgcolor_rgba = 0xffff0000;  // red (default)
  uint32_t bgcolor_rgba = 0xffffffff;  // white (default)
  bool use_color_correction = false;
  int clamp_rgb = -1;

  display_test::ColorCorrectionArgs color_correction_args;

  float alpha_val = std::nanf("");
  bool enable_alpha = false;
  bool enable_compression = false;
  bool apply_config_once = false;
  uint32_t configs_per_vsync = 1;

  while (argc) {
    if (strcmp(argv[0], "--dump") == 0) {
      for (auto& display : displays) {
        display.Dump();
      }
      return 0;
    }
    if (strcmp(argv[0], "--mode-set") == 0 || strcmp(argv[0], "--format-set") == 0) {
      Display* display = find_display(displays, argv[1]);
      if (!display) {
        printf("Invalid display \"%s\" for %s\n", argv[1], argv[0]);
        return -1;
      }
      if (strcmp(argv[0], "--mode-set") == 0) {
        if (!display->set_mode_idx(atoi(argv[2]))) {
          printf("Invalid mode id\n");
          return -1;
        }
      } else {
        if (!display->set_format_idx(atoi(argv[2]))) {
          printf("Invalid format id\n");
          return -1;
        }
      }
      argv += 3;
      argc -= 3;
    } else if (strcmp(argv[0], "--grayscale") == 0) {
      for (auto& d : displays) {
        d.set_grayscale(true);
      }
      argv++;
      argc--;
    } else if (strcmp(argv[0], "--num-frames") == 0) {
      if (strcmp(argv[1], "infinite") == 0) {
        num_frames = std::nullopt;
      } else {
        num_frames = atoi(argv[1]);
      }
      argv += 2;
      argc -= 2;
    } else if (strcmp(argv[0], "--controller") == 0) {
      // We already processed this, skip it.
      argv += 2;
      argc -= 2;
    } else if (strcmp(argv[0], "--delay") == 0) {
      delay = atoi(argv[1]);
      argv += 2;
      argc -= 2;
    } else if (strcmp(argv[0], "--bundle") == 0) {
      testbundle = static_cast<TestBundle>(atoi(argv[1]));
      if (testbundle >= BUNDLE_COUNT || testbundle < 0) {
        printf("Invalid test bundle selected\n");
        usage();
        return -1;
      }
      argv += 2;
      argc -= 2;
    } else if (strcmp(argv[0], "--capture") == 0) {
      capture = true;
      verify_capture = true;
      argv += 1;
      argc -= 1;
    } else if (strcmp(argv[0], "--clamp-rgb") == 0) {
      clamp_rgb = atoi(argv[1]);
      if (clamp_rgb < 0 || clamp_rgb > 255) {
        usage();
        return -1;
      }
      argv += 2;
      argc -= 2;
    } else if (strcmp(argv[0], "--fgcolor") == 0) {
      fgcolor_rgba = static_cast<uint32_t>(strtoul(argv[1], nullptr, 16));
      argv += 2;
      argc -= 2;
    } else if (strcmp(argv[0], "--bgcolor") == 0) {
      bgcolor_rgba = static_cast<uint32_t>(strtoul(argv[1], nullptr, 16));
      argv += 2;
      argc -= 2;
    } else if (strcmp(argv[0], "--preoffsets") == 0) {
      sscanf(argv[1], "%f,%f,%f", &color_correction_args.preoffsets[0],
             &color_correction_args.preoffsets[1], &color_correction_args.preoffsets[2]);
      use_color_correction = true;
      argv += 2;
      argc -= 2;
    } else if (strcmp(argv[0], "--postoffsets") == 0) {
      sscanf(argv[1], "%f,%f,%f", &color_correction_args.postoffsets[0],
             &color_correction_args.postoffsets[1], &color_correction_args.postoffsets[2]);
      use_color_correction = true;
      argv += 2;
      argc -= 2;
    } else if (strcmp(argv[0], "--coeff") == 0) {
      sscanf(argv[1], "%f,%f,%f,%f,%f,%f,%f,%f,%f", &color_correction_args.coeff[0],
             &color_correction_args.coeff[1], &color_correction_args.coeff[2],
             &color_correction_args.coeff[3], &color_correction_args.coeff[4],
             &color_correction_args.coeff[5], &color_correction_args.coeff[6],
             &color_correction_args.coeff[7], &color_correction_args.coeff[8]);
      use_color_correction = true;
      argv += 2;
      argc -= 2;
    } else if (strcmp(argv[0], "--enable-alpha") == 0) {
      enable_alpha = true;
      argv += 1;
      argc -= 1;
    } else if (strcmp(argv[0], "--opacity") == 0) {
      enable_alpha = true;
      alpha_val = std::stof(argv[1]);
      if (alpha_val < 0 || alpha_val > 1) {
        printf("Invalid alpha value. Must be between 0 and 1\n");
        usage();
        return -1;
      }
      argv += 2;
      argc -= 2;
    } else if (strcmp(argv[0], "--enable-compression") == 0) {
      enable_compression = true;
      argv += 1;
      argc -= 1;
    } else if (strcmp(argv[0], "--apply-config-once") == 0) {
      apply_config_once = true;
      argv += 1;
      argc -= 1;
    } else if (strcmp(argv[0], "--configs-per-vsync") == 0) {
      configs_per_vsync = atoi(argv[1]);
      argv += 2;
      argc -= 2;
    } else if (strcmp(argv[0], "--pattern") == 0) {
      if (strcmp(argv[1], "checkerboard") == 0) {
        image_pattern = display_test::Image::Pattern::kCheckerboard;
      } else if (strcmp(argv[1], "border") == 0) {
        image_pattern = display_test::Image::Pattern::kBorder;
      } else {
        printf("Invalid image pattern \"%s\".\n", argv[1]);
        usage();
        return 0;
      }
      argv += 2;
      argc -= 2;

    } else if (strcmp(argv[0], "--help") == 0) {
      usage();
      return 0;
    } else {
      printf("Unrecognized argument \"%s\"\n", argv[0]);
      usage();
      return -1;
    }
  }

  // TODO(https://fxbug.dev/42076494): AFBC compression test doesn't work on
  // amlogic-display; the AFBC encoding format supported by amlogic-display
  // driver is not compatible with the AFBC buffer formats and generation logic
  // used by this test. Once amlogic-display supports non-tiled-header formats
  // and AFBC format switching on the fly, this test will work again and we can
  // delete the error message below.
  if (enable_compression) {
    fprintf(stderr,
            "AFBC compression test is not working for amlogic-display. "
            "The --enable-compression option will be re-enabled once "
            "https://fxbug.dev/42076494 is fixed.\n");
    return -1;
  }

  if (use_color_correction) {
    for (auto& d : displays) {
      d.apply_color_correction(true);
    }
  }

  if (capture && capture_setup(displays[0]) != ZX_OK) {
    printf("Could not setup capture\n");
    capture = false;
  }

  if (clamp_rgb != -1) {
    if (set_minimum_rgb(static_cast<uint8_t>(clamp_rgb)) != ZX_OK) {
      printf("Warning: RGB Clamping Not Supported!\n");
    }
  }

  // Call apply_config for each frame by default.
  std::optional<int32_t> max_apply_configs(num_frames);

  fbl::AllocChecker ac;

  printf("Using TestBundle: %s\n", testbundle_names[testbundle]);
  if (testbundle == FLIP) {
    // Amlogic display test
    std::unique_ptr<PrimaryLayer> layer1 = fbl::make_unique_checked<PrimaryLayer>(
        &ac, displays, image_pattern, fgcolor_rgba, bgcolor_rgba);
    if (!ac.check()) {
      return ZX_ERR_NO_MEMORY;
    }
    if (enable_alpha) {
      layer1->SetAlpha(true, alpha_val);
    }
    layer1->SetLayerFlipping(true);
    if (enable_compression) {
      layer1->SetFormatModifier(fuchsia_images2::wire::PixelFormatModifier::kArmAfbc16X16);
    }
    layers.push_back(std::move(layer1));
  } else if (testbundle == SIMPLE) {
    // Simple display test
    bool mirrors = true;
    std::unique_ptr<PrimaryLayer> layer1 = fbl::make_unique_checked<PrimaryLayer>(
        &ac, displays, image_pattern, fgcolor_rgba, bgcolor_rgba, mirrors);
    if (!ac.check()) {
      return ZX_ERR_NO_MEMORY;
    }

    if (enable_compression) {
      layer1->SetFormatModifier(fuchsia_images2::wire::PixelFormatModifier::kArmAfbc16X16);
    }
    if (apply_config_once) {
      max_apply_configs = 1;
    }
    layers.push_back(std::move(layer1));
  } else if (testbundle == BLANK) {
    // 0 layers, applied one time
    max_apply_configs = 1;
  }

  printf("Initializing layers\n");
  for (auto& layer : layers) {
    if (!layer->Init(dc)) {
      printf("Layer init failed\n");
      return -1;
    }
  }

  for (auto& display : displays) {
    display.Init(dc, color_correction_args);
  }

  if (capture && layers.size() != 1) {
    printf("Capture disabled: verification only works for single-layer display tests\n");
    verify_capture = false;
  }

  printf("Starting rendering\n");
  if (capture) {
    printf("Capturing every frame. Verification is %s\n", verify_capture ? "enabled" : "disabled");
  }

  fuchsia_hardware_display::wire::ConfigStamp last_applied_stamp = {
      .value = fuchsia_hardware_display::wire::kInvalidConfigStampValue};
  bool capture_result = true;
  for (int i = 0; !num_frames || i < num_frames; i++) {
    for (auto& layer : layers) {
      // Step before waiting, since not every layer is used every frame
      // so we won't necessarily need to wait.
      layer->StepLayout(i);

      // Wait for previous config to be applied before deciding whether images are OK for rendering.
      if (last_applied_stamp.value > 0) {
        while (true) {
          zx_status_t vsync_status = wait_for_vsync(coordinator_listener_loop, last_applied_stamp);

          if (vsync_status == ZX_OK) {
            ZX_ASSERT(g_coordinator_listener.latest_config_stamp() ==
                      display::ConfigStamp(last_applied_stamp));
            break;
          } else if (vsync_status == ZX_ERR_NEXT) {
            zx::nanosleep(zx::deadline_after(zx::msec(100)));
            continue;
          } else {
            ZX_PANIC("Failed to wait for Vsync: %s", zx_status_get_string(vsync_status));
          }
        }
      }

      if (!layer->ReadyToRender(display::ConfigStamp(last_applied_stamp))) {
        printf("Buffer failed to become free\n");
        return -1;
      }

      layer->clear_done();
      layer->SendLayout(dc);
    }

    for (unsigned i = 0; i < displays.size(); i++) {
      if (!update_display_layers(layers, displays[i], &display_layers[i])) {
        return -1;
      }
    }

    // This delay is used to skew the timing between vsync and SubmitConfiguration
    // in order to observe any tearing effects
    zx_nanosleep(zx_deadline_after(ZX_MSEC(delay)));

    if (!max_apply_configs || i < max_apply_configs) {
      for (uint32_t cpv = 0; cpv < configs_per_vsync; cpv++) {
        last_applied_stamp.value++;
        if (zx_status_t status; (status = apply_config(last_applied_stamp)) != ZX_OK) {
          printf("apply_config() failed with status: %s\n", zx_status_get_string(status));
          return -1;
        }
      }
    }

    for (auto& layer : layers) {
      layer->Render(i);
    }

    zx_status_t status = ZX_OK;
    while (layers.size() != 0 && (status = wait_for_vsync(coordinator_listener_loop,
                                                          last_applied_stamp)) == ZX_ERR_NEXT) {
    }
    ZX_ASSERT(status == ZX_OK);
    if (capture) {
      // capture has been requested.
      status = capture_start();
      if (status != ZX_OK) {
        printf("Capture start failed %d\n", status);
        capture_release();
        capture = false;
        break;
      }
      if (verify_capture && !CompareCapturedImage(std::span(reinterpret_cast<const uint8_t*>(
                                                                layers[0]->GetCurrentImageBuf()),
                                                            layers[0]->GetCurrentImageSize()),
                                                  /*input_image_pixel_format=*/displays[0].format(),
                                                  /*height=*/displays[0].mode().active_area.height,
                                                  /*width=*/displays[0].mode().active_area.width)) {
        capture_result = false;
        break;
      }
    }
  }

  printf("Done rendering\n");

  if (capture) {
    printf("Capture completed\n");
    if (verify_capture) {
      if (capture_result) {
        printf("Capture Verification Passed\n");
      } else {
        printf("Capture Verification Failed!\n");
      }
    }
    capture_release();
  }
  zx_handle_close(device_handle);

  return 0;
}

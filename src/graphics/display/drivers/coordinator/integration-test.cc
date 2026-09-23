// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.display/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-testing/test_loop.h>
#include <lib/async/cpp/task.h>
#include <lib/async/default.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fdf/cpp/arena.h>
#include <lib/fdf/dispatcher.h>
#include <lib/fidl/cpp/wire/array.h>
#include <lib/fit/result.h>
#include <lib/sysmem-version/sysmem-version.h>
#include <lib/zx/clock.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>
#include <zircon/assert.h>
#include <zircon/compiler.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <cstdint>
#include <format>
#include <memory>
#include <optional>
#include <string>
#include <vector>

#include <fbl/alloc_checker.h>
#include <fbl/auto_lock.h>
#include <gtest/gtest.h>

#include "src/graphics/display/drivers/coordinator/client-vsync-queue.h"
#include "src/graphics/display/drivers/coordinator/post-display-task.h"
#include "src/graphics/display/drivers/coordinator/testing/base.h"
#include "src/graphics/display/drivers/coordinator/testing/mock-coordinator-listener.h"
#include "src/graphics/display/lib/api-types/cpp/buffer-collection-id.h"
#include "src/graphics/display/lib/api-types/cpp/client-priority.h"
#include "src/graphics/display/lib/api-types/cpp/config-check-result.h"
#include "src/graphics/display/lib/api-types/cpp/config-stamp.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-config-stamp.h"
#include "src/graphics/display/lib/api-types/cpp/event-id.h"
#include "src/graphics/display/lib/api-types/cpp/image-id.h"
#include "src/graphics/display/lib/api-types/cpp/image-metadata.h"
#include "src/graphics/display/lib/api-types/cpp/image-tiling-type.h"
#include "src/graphics/display/lib/api-types/cpp/layer-id.h"
#include "src/graphics/display/lib/api-types/cpp/mode.h"
#include "src/graphics/display/lib/api-types/cpp/vsync-ack-cookie.h"
#include "src/graphics/display/lib/driver-utils/post-task.h"
#include "src/graphics/display/lib/fake-display-stack/fake-display.h"
#include "src/lib/fsl/handles/object_info.h"
#include "src/lib/testing/predicates/status.h"

namespace display_coordinator {

namespace {

// Cached information about a display reported by the coordinator.
struct TestDisplayInfo {
 public:
  static TestDisplayInfo From(const fuchsia_hardware_display::wire::Info& fidl_display_info);

  display::DisplayId id;

  // Represents an image that covers the entire display.
  display::ImageMetadata fullscreen_image_metadata;

  uint32_t max_layer_count;
};

// static
TestDisplayInfo TestDisplayInfo::From(
    const fuchsia_hardware_display::wire::Info& fidl_display_info) {
  const display::DisplayId display_id = display::DisplayId(fidl_display_info.id);
  ZX_ASSERT(display_id != display::kInvalidDisplayId);

  ZX_ASSERT(!fidl_display_info.modes.empty());
  display::Mode display_mode = display::Mode::From(fidl_display_info.modes[0]);

  const display::ImageMetadata fullscreen_image_metadata = display::ImageMetadata({
      .width = display_mode.active_area().width(),
      .height = display_mode.active_area().height(),
      .tiling_type = display::ImageTilingType::kLinear,
  });

  return TestDisplayInfo{
      .id = display_id,
      .fullscreen_image_metadata = fullscreen_image_metadata,
      .max_layer_count = fidl_display_info.max_layer_count,
  };
}

// Coordinator client state updated by the listener protocol.
//
// This class is thread-safe.
class TestClientState {
 public:
  TestClientState() = default;
  TestClientState(const TestClientState&) = delete;
  TestClientState& operator=(const TestClientState&) = delete;
  ~TestClientState() = default;

  // The returned count is guaranteed to be monotonically increasing across the
  // instance's lifetime.
  uint64_t vsync_count() const;

  bool has_display_ownership() const;
  display::ConfigStamp last_vsync_config_stamp() const;
  display::VsyncAckCookie last_vsync_ack_cookie() const;

  bool HasConnectedDisplay() const;

  // The first connected display's ID.
  //
  // Crashes if no display is connected.
  display::DisplayId display_id() const;

  // Metadata for an image that fully covers the first connected display.
  //
  // Crashes if no display is connected.
  display::ImageMetadata FullscreenImageMetadata() const;

  // Returns the cached information for the first connected display.
  //
  // Crashes if no display is connected.
  const TestDisplayInfo& GetDisplayInfo() const;

  // MockCoordinatorListener implementation
  void OnDisplaysChanged(std::span<const fuchsia_hardware_display::wire::Info> added_displays,
                         std::span<const display::DisplayId> removed_display_ids);
  void OnClientOwnershipChange(bool has_ownership);
  void OnVsync(display::DisplayId display_id, zx::time_monotonic timestamp,
               display::ConfigStamp displayed_config_stamp,
               display::VsyncAckCookie vsync_ack_cookie);

 private:
  // Locks all the state in this class.
  mutable std::mutex mutex_;

  std::vector<TestDisplayInfo> connected_displays_ __TA_GUARDED(mutex_);
  bool has_display_ownership_ __TA_GUARDED(mutex_) = false;
  uint64_t vsync_count_ TA_GUARDED(mutex_) = 0;
  display::VsyncAckCookie last_vsync_ack_cookie_ __TA_GUARDED(mutex_) =
      display::kInvalidVsyncAckCookie;
  display::ConfigStamp last_vsync_config_stamp_ __TA_GUARDED(mutex_);
};

uint64_t TestClientState::vsync_count() const {
  std::lock_guard lock(mutex_);
  return vsync_count_;
}

bool TestClientState::has_display_ownership() const {
  std::lock_guard lock(mutex_);
  return has_display_ownership_;
}

display::ConfigStamp TestClientState::last_vsync_config_stamp() const {
  std::lock_guard lock(mutex_);
  return last_vsync_config_stamp_;
}

display::VsyncAckCookie TestClientState::last_vsync_ack_cookie() const {
  std::lock_guard lock(mutex_);
  return last_vsync_ack_cookie_;
}

bool TestClientState::HasConnectedDisplay() const {
  std::lock_guard lock(mutex_);
  return !connected_displays_.empty();
}

display::DisplayId TestClientState::display_id() const {
  std::lock_guard lock(mutex_);
  ZX_ASSERT(!connected_displays_.empty());
  return connected_displays_[0].id;
}

display::ImageMetadata TestClientState::FullscreenImageMetadata() const {
  std::lock_guard lock(mutex_);
  ZX_ASSERT(!connected_displays_.empty());
  return connected_displays_[0].fullscreen_image_metadata;
}

const TestDisplayInfo& TestClientState::GetDisplayInfo() const {
  std::lock_guard lock(mutex_);
  ZX_ASSERT(!connected_displays_.empty());
  return connected_displays_[0];
}

void TestClientState::OnDisplaysChanged(
    std::span<const fuchsia_hardware_display::wire::Info> added_displays,
    std::span<const display::DisplayId> removed_display_ids) {
  ZX_ASSERT(removed_display_ids.empty());

  std::lock_guard lock(mutex_);
  for (const fuchsia_hardware_display::wire::Info& added_display : added_displays) {
    connected_displays_.push_back(TestDisplayInfo::From(added_display));
  }
}

void TestClientState::OnClientOwnershipChange(bool has_ownership) {
  std::lock_guard lock(mutex_);
  has_display_ownership_ = has_ownership;
}

void TestClientState::OnVsync(display::DisplayId display_id, zx::time_monotonic timestamp,
                              display::ConfigStamp displayed_config_stamp,
                              display::VsyncAckCookie vsync_ack_cookie) {
  std::lock_guard lock(mutex_);
  ++vsync_count_;
  last_vsync_config_stamp_ = displayed_config_stamp;
  if (vsync_ack_cookie != display::kInvalidVsyncAckCookie) {
    last_vsync_ack_cookie_ = vsync_ack_cookie;
  }
}

// Convenience method for creating a B8R8G8A8 color.
constexpr display::Color BgraColor(uint32_t bgra_color) {
  const uint8_t blue = static_cast<uint8_t>(bgra_color);
  const uint8_t green = static_cast<uint8_t>(bgra_color >> 8);
  const uint8_t red = static_cast<uint8_t>(bgra_color >> 16);
  const uint8_t alpha = static_cast<uint8_t>(bgra_color >> 24);
  return display::Color(
      {.format = display::PixelFormat::kB8G8R8A8,
       .bytes = std::initializer_list<uint8_t>{blue, green, red, alpha, 0, 0, 0, 0}});
}

constexpr display::Color kFuchsiaBgra = BgraColor(0xffff00ff);

// Encapsulates boilerplate for driving the Coordinator via FIDL.
//
// This class is not thead-safe. Instances must be accessed on a single thread,
// or on a single synchronized dispatcher. Exception: both `state()` and the
// returned `TestClientState` instance can be accessed from any thread.
class TestFidlClient {
 public:
  struct EventInfo {
    display::EventId id;
    zx::event event;
  };

  // One layer's configuration.
  //
  // This structure only covers the information that varies across integration
  // tests.
  struct LayerConfig {
    // Valid layer configurations must have valid layer IDs.
    display::LayerId layer_id;

    // Invalid if the layer does not include an image.
    display::ImageId image_id = display::kInvalidImageId;

    // Invalid if the layer does not specify a wait event.
    display::EventId image_ready_wait_event_id = display::kInvalidEventId;
  };

  // `sysmem` must outlive this instance.
  explicit TestFidlClient(const fidl::WireSyncClient<fuchsia_sysmem2::Allocator>* sysmem);
  TestFidlClient(const TestFidlClient&) = delete;
  TestFidlClient& operator=(const TestFidlClient&) = delete;
  ~TestFidlClient();

  // Thread-safe.
  TestClientState& state() { return state_; }

  // `coordinator_listener_dispatcher` must be non-null and must be running
  // throughout the test.
  zx::result<> OpenCoordinator(
      const fidl::WireSyncClient<fuchsia_hardware_display::Provider>& provider,
      display::ClientPriority client_priority, async_dispatcher_t* coordinator_listener_dispatcher);

  zx::result<display::LayerId> CreateLayer();
  zx::result<> ImportBufferCollection(
      display::BufferCollectionId buffer_collection_id,
      fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> buffer_token);
  zx::result<> ImportImage(const display::ImageMetadata& image_metadata,
                           display::BufferCollectionId buffer_collection_id, uint32_t buffer_index,
                           display::ImageId image_id);
  zx::result<> ImportEvent(zx::event event, display::EventId event_id);
  zx::result<> SetBufferCollectionConstraints(display::BufferCollectionId buffer_collection_id,
                                              display::ImageBufferUsage image_buffer_usage);

  // The std::vector can be converted to std::span once we adopt C++23, which has
  // more ergonoic span handling.
  zx::result<> SetDisplayLayers(display::DisplayId display_id,
                                const std::vector<LayerConfig>& layer_configs);

  zx::result<> SetLayerPrimaryConfig(display::LayerId layer_id,
                                     const display::ImageMetadata& image_metadata);
  zx::result<> SetLayerImage(display::LayerId layer_id, display::ImageId image_id,
                             display::EventId event_id);
  zx::result<> SetLayerColor(display::LayerId layer_id, const display::Color& fallback_color);
  zx::result<display::ConfigCheckResult> CheckConfig();
  zx::result<> CommitConfig(display::ConfigStamp config_stamp);
  zx::result<> AcknowledgeVsync(display::VsyncAckCookie vsync_ack_cookie);
  zx::result<> SetMinimumRgb(uint8_t minimum_rgb);
  zx::result<display::ConfigStamp> GetLastCommittedConfigStamp();

  zx::result<fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken>>
  SysmemAllocateSharedCollection();
  zx::result<fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken>> SysmemTokenDuplicateSync(
      const fidl::WireSyncClient<fuchsia_sysmem2::BufferCollectionToken>& token);
  zx::result<fidl::ClientEnd<fuchsia_sysmem2::BufferCollection>> SysmemTokenBind(
      fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token);
  zx::result<> SysmemBufferCollectionRelease(
      fidl::ClientEnd<fuchsia_sysmem2::BufferCollection> buffer_collection);

  // Returns the number of allocated buffers.
  zx::result<size_t> SysmemWaitForAllBuffersAllocated(
      const fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection>& buffer_collection);

  zx::result<> SetSysmemConstraintsForImage(
      const fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection>& buffer_collection);

  zx::result<display::ImageId> ImportImageWithSysmem(const display::ImageMetadata& image_metadata);

  // Imports an image that covers the first connected display.
  //
  // Crashes if no display is connected.
  zx::result<display::ImageId> CreateFullscreenImage();

  // Creates a layer that covers the first connected display.
  //
  // Crashes if no display is connected.
  zx::result<display::LayerId> CreateFullscreenImageLayer();

  // Creates a solid color fill layer that covers the first connected display.
  //
  // Crashes if no display is connected.
  zx::result<display::LayerId> CreateFullscreenColorLayer(display::Color color);

  zx::result<EventInfo> CreateEvent();

  // Returns a one-layer configuration that covers the first connected display.
  //
  // Crashes if no display is connected.
  std::vector<LayerConfig> CreateFullscreenLayerConfig();

  // Applies a configuration to the first connected display.
  //
  // Blocks until the Display Coordinator processes the underlying CommitConfig()
  // call. Crashes if no display is connected.
  //
  // The std::vector can be converted to std::span once we adopt C++23, which has
  // more ergonoic span handling.
  zx::result<> ApplyLayers(display::ConfigStamp config_stamp,
                           const std::vector<LayerConfig>& layer_configs);

 private:
  display::BufferCollectionId next_buffer_collection_id_{1};
  display::ImageId next_imported_image_id_{1};
  display::LayerId next_layer_id_{1};

  fidl::WireSyncClient<fuchsia_hardware_display::Coordinator> coordinator_fidl_client_;
  const fidl::WireSyncClient<fuchsia_sysmem2::Allocator>& sysmem_;

  // Must outlive `coordinator_listener_`.
  TestClientState state_;

  // Must outlive `coordinator_listener_binding_`.
  MockCoordinatorListener coordinator_listener_{
      fit::bind_member<&TestClientState::OnDisplaysChanged>(&state_),
      fit::bind_member<&TestClientState::OnVsync>(&state_),
      fit::bind_member<&TestClientState::OnClientOwnershipChange>(&state_)};
  async_dispatcher_t* coordinator_listener_dispatcher_ = nullptr;
  std::optional<fidl::ServerBindingRef<fuchsia_hardware_display::CoordinatorListener>>
      coordinator_listener_binding_;
};

TestFidlClient::TestFidlClient(const fidl::WireSyncClient<fuchsia_sysmem2::Allocator>* sysmem)
    : sysmem_(*sysmem) {
  ZX_ASSERT(sysmem != nullptr);
}

TestFidlClient::~TestFidlClient() {
  if (coordinator_listener_binding_.has_value()) {
    ZX_ASSERT(coordinator_listener_dispatcher_ != nullptr);
    // We can call Unbind() on any thread, but it's async and previously-started dispatches can
    // still be in-flight after this call.
    coordinator_listener_binding_->Unbind();
    // The Unbind() above will prevent starting any new dispatches, but previously-started
    // dispatches can still be in-flight. For this reason we must fence the Bind's dispatcher thread
    // before we delete stuff used during dispatch such as on_vsync_callback_.
    libsync::Completion done;
    zx::result<> post_task_result = display::PostTask<display_coordinator::kDisplayTaskTargetSize>(
        *coordinator_listener_dispatcher_, [&done] { done.Signal(); });
    ZX_ASSERT(post_task_result.is_ok());
    done.Wait();
    // Now it's safe to delete on_vsync_callback_ (for example).
  }
}

zx::result<> TestFidlClient::OpenCoordinator(
    const fidl::WireSyncClient<fuchsia_hardware_display::Provider>& provider,
    display::ClientPriority client_priority, async_dispatcher_t* coordinator_listener_dispatcher) {
  ZX_DEBUG_ASSERT(client_priority != display::ClientPriority::kInvalid);
  ZX_ASSERT(coordinator_listener_dispatcher != nullptr);
  ZX_ASSERT_MSG(!coordinator_listener_binding_.has_value(), "OpenCoordinator() already called");
  ZX_ASSERT_MSG(coordinator_listener_dispatcher_ == nullptr, "OpenCoordinator() already called");

  auto [coordinator_client, coordinator_server] =
      fidl::Endpoints<fuchsia_hardware_display::Coordinator>::Create();
  auto [coordinator_listener_client, coordinator_listener_server] =
      fidl::Endpoints<fuchsia_hardware_display::CoordinatorListener>::Create();
  fdf::info("Opening coordinator");
  fidl::Arena arena;
  auto request =
      fidl::WireRequest<fuchsia_hardware_display::Provider::OpenCoordinator>::Builder(arena)
          .coordinator(std::move(coordinator_server))
          .coordinator_listener(std::move(coordinator_listener_client))
          .priority(client_priority.ToFidl())
          .Build();
  fidl::WireResult<fuchsia_hardware_display::Provider::OpenCoordinator> fidl_transport_result =
      provider->OpenCoordinator(std::move(request));
  if (!fidl_transport_result.ok()) {
    fdf::error("FIDL error calling OpenCoordinator: {}", fidl_transport_result.error());
    return zx::make_result(fidl_transport_result.status());
  }
  fit::result<zx_status_t>& fidl_domain_result = fidl_transport_result.value();
  if (fidl_domain_result.is_error()) {
    fdf::error("OpenCoordinator failed: {}", zx::make_result(fidl_domain_result.error_value()));
    return zx::make_result(fidl_domain_result.error_value());
  }

  coordinator_fidl_client_.Bind(std::move(coordinator_client));
  coordinator_listener_dispatcher_ = coordinator_listener_dispatcher;
  coordinator_listener_binding_.emplace(fidl::BindServer(coordinator_listener_dispatcher,
                                                         std::move(coordinator_listener_server),
                                                         &coordinator_listener_));
  return zx::ok();
}

zx::result<> TestFidlClient::ImportEvent(zx::event event, display::EventId event_id) {
  ZX_ASSERT(coordinator_fidl_client_.is_valid());

  const fuchsia_hardware_display::wire::EventId fidl_event_id = event_id.ToFidl();

  fidl::OneWayStatus fidl_transport_status =
      coordinator_fidl_client_->ImportEvent(std::move(event), fidl_event_id);
  if (!fidl_transport_status.ok()) {
    fdf::error("FIDL error calling ImportEvent: {}", fidl_transport_status.error());
    return zx::error(fidl_transport_status.status());
  }
  return zx::ok();
}

zx::result<display::LayerId> TestFidlClient::CreateLayer() {
  ZX_ASSERT(coordinator_fidl_client_.is_valid());

  const display::LayerId layer_id = next_layer_id_;
  ++next_layer_id_;

  fidl::WireResult<fuchsia_hardware_display::Coordinator::CreateLayer> fidl_transport_result =
      coordinator_fidl_client_->CreateLayer(layer_id.ToFidl());
  if (!fidl_transport_result.ok()) {
    fdf::error("FIDL error calling CreateLayer: {}", fidl_transport_result.error());
    return zx::error(fidl_transport_result.status());
  }

  fit::result<zx_status_t>& fidl_domain_result = fidl_transport_result.value();
  if (fidl_domain_result.is_error()) {
    fdf::warn("CreateLayer failed: {}", zx::make_result(fidl_domain_result.error_value()));
    return zx::error(fidl_domain_result.error_value());
  }

  return zx::ok(layer_id);
}

zx::result<> TestFidlClient::ImportImage(const display::ImageMetadata& image_metadata,
                                         display::BufferCollectionId buffer_collection_id,
                                         uint32_t buffer_index, display::ImageId image_id) {
  ZX_ASSERT(coordinator_fidl_client_.is_valid());

  const fuchsia_hardware_display::wire::BufferCollectionId fidl_buffer_collection_id =
      buffer_collection_id.ToFidl();
  const fuchsia_hardware_display::wire::ImageId fidl_image_id = image_id.ToFidl();

  fidl::WireResult<fuchsia_hardware_display::Coordinator::ImportImage> fidl_transport_result =
      coordinator_fidl_client_->ImportImage(image_metadata.ToFidl(), fidl_buffer_collection_id,
                                            buffer_index, fidl_image_id);
  if (!fidl_transport_result.ok()) {
    fdf::error("FIDL error calling ImportImage: {}", fidl_transport_result.error());
    return zx::error(fidl_transport_result.status());
  }

  fit::result<zx_status_t>& fidl_domain_result = fidl_transport_result.value();
  if (fidl_domain_result.is_error()) {
    fdf::warn("ImportImage failed: {}", zx::make_result(fidl_domain_result.error_value()));
    return zx::error(fidl_domain_result.error_value());
  }
  return zx::ok();
}

zx::result<> TestFidlClient::SetDisplayLayers(display::DisplayId display_id,
                                              const std::vector<LayerConfig>& layer_configs) {
  ZX_ASSERT_MSG(!layer_configs.empty(), "Empty configurations are not supported");

  ZX_ASSERT(coordinator_fidl_client_.is_valid());

  std::vector<fuchsia_hardware_display::wire::LayerId> fidl_layer_ids;
  fidl_layer_ids.reserve(layer_configs.size());
  for (const LayerConfig& layer_config : layer_configs) {
    ZX_ASSERT(layer_config.layer_id != display::kInvalidLayerId);
    const fuchsia_hardware_display::wire::LayerId fidl_layer_id = layer_config.layer_id.ToFidl();
    fidl_layer_ids.push_back(fidl_layer_id);
  }

  fidl::OneWayStatus fidl_transport_status = coordinator_fidl_client_->SetDisplayLayers(
      display_id.ToFidl(),
      fidl::VectorView<fuchsia_hardware_display::wire::LayerId>::FromExternal(fidl_layer_ids));
  if (!fidl_transport_status.ok()) {
    fdf::error("FIDL error calling SetDisplayLayers: {}", fidl_transport_status.error());
    return zx::error(fidl_transport_status.status());
  }
  return zx::ok();
}

zx::result<> TestFidlClient::SetLayerPrimaryConfig(display::LayerId layer_id,
                                                   const display::ImageMetadata& image_metadata) {
  ZX_ASSERT(coordinator_fidl_client_.is_valid());

  const fuchsia_hardware_display::wire::LayerId fidl_layer_id = layer_id.ToFidl();
  const fuchsia_hardware_display_types::wire::ImageMetadata fidl_image_metadata =
      image_metadata.ToFidl();

  fidl::OneWayStatus fidl_transport_status =
      coordinator_fidl_client_->SetLayerPrimaryConfig(fidl_layer_id, fidl_image_metadata);
  if (!fidl_transport_status.ok()) {
    fdf::error("FIDL error calling SetLayerPrimaryConfig: {}", fidl_transport_status.error());
    return zx::error(fidl_transport_status.status());
  }
  return zx::ok();
}

zx::result<> TestFidlClient::SetLayerImage(display::LayerId layer_id, display::ImageId image_id,
                                           display::EventId event_id) {
  ZX_ASSERT(coordinator_fidl_client_.is_valid());

  const fuchsia_hardware_display::wire::LayerId fidl_layer_id = layer_id.ToFidl();
  const fuchsia_hardware_display::wire::ImageId fidl_image_id = image_id.ToFidl();
  const fuchsia_hardware_display::wire::EventId fidl_event_id = event_id.ToFidl();

  fidl::OneWayStatus fidl_transport_status =
      coordinator_fidl_client_->SetLayerImage2(fidl_layer_id, fidl_image_id, fidl_event_id);
  if (!fidl_transport_status.ok()) {
    fdf::error("FIDL error calling SetLayerImage2: {}", fidl_transport_status.error());
    return zx::error(fidl_transport_status.status());
  }
  return zx::ok();
}

zx::result<> TestFidlClient::SetLayerColor(display::LayerId layer_id,
                                           const display::Color& fallback_color) {
  ZX_ASSERT(coordinator_fidl_client_.is_valid());

  const fuchsia_hardware_display::wire::LayerId fidl_layer_id = layer_id.ToFidl();
  const fuchsia_hardware_display_types::wire::Color fidl_fallback_color = fallback_color.ToFidl();
  const display::ImageMetadata fullscreen_metadata = state_.FullscreenImageMetadata();
  const fuchsia_math::wire::RectU display_destination = {
      .x = 0,
      .y = 0,
      .width = static_cast<uint32_t>(fullscreen_metadata.width()),
      .height = static_cast<uint32_t>(fullscreen_metadata.height()),
  };

  fidl::OneWayStatus fidl_transport_status = coordinator_fidl_client_->SetLayerColorConfig(
      fidl_layer_id, fidl_fallback_color, display_destination);
  if (!fidl_transport_status.ok()) {
    fdf::error("FIDL error calling SetLayerColorConfig: {}", fidl_transport_status.error());
    return zx::error(fidl_transport_status.status());
  }
  return zx::ok();
}

zx::result<display::ConfigCheckResult> TestFidlClient::CheckConfig() {
  ZX_ASSERT(coordinator_fidl_client_.is_valid());

  fidl::WireResult<fuchsia_hardware_display::Coordinator::CheckConfig> fidl_transport_result =
      coordinator_fidl_client_->CheckConfig();
  if (!fidl_transport_result.ok()) {
    fdf::error("FIDL error calling CheckConfig: {}", fidl_transport_result.error());
    return zx::error(fidl_transport_result.status());
  }
  fuchsia_hardware_display::wire::CoordinatorCheckConfigResponse& fidl_domain_result =
      fidl_transport_result.value();
  fuchsia_hardware_display_types::wire::ConfigResult fidl_config_check_result =
      fidl_domain_result.res;
  if (!display::ConfigCheckResult::IsValid(fidl_config_check_result)) {
    fdf::error("CheckConfig() returned unrecognized code: {}",
               static_cast<uint32_t>(fidl_config_check_result));
    return zx::error(ZX_ERR_INTERNAL);
  }

  return zx::ok(display::ConfigCheckResult(fidl_domain_result.res));
}

zx::result<> TestFidlClient::CommitConfig(display::ConfigStamp config_stamp) {
  ZX_ASSERT(coordinator_fidl_client_.is_valid());

  const fuchsia_hardware_display::wire::ConfigStamp fidl_config_stamp = config_stamp.ToFidl();
  fidl::Arena arena;
  fuchsia_hardware_display::wire::CoordinatorCommitConfigRequest request =
      fidl::WireRequest<fuchsia_hardware_display::Coordinator::CommitConfig>::Builder(arena)
          .stamp(fidl_config_stamp)
          .Build();

  fidl::OneWayStatus fidl_transport_status =
      coordinator_fidl_client_->CommitConfig(std::move(request));
  if (!fidl_transport_status.ok()) {
    fdf::error("FIDL error calling CommitConfig: {}", fidl_transport_status.error());
    return zx::error(fidl_transport_status.status());
  }
  return zx::ok();
}

zx::result<> TestFidlClient::AcknowledgeVsync(display::VsyncAckCookie vsync_ack_cookie) {
  ZX_ASSERT(coordinator_fidl_client_.is_valid());

  const fuchsia_hardware_display::wire::VsyncAckCookie fidl_vsync_ack_cookie =
      vsync_ack_cookie.ToFidl();
  fidl::OneWayStatus fidl_transport_status =
      coordinator_fidl_client_->AcknowledgeVsync(fidl_vsync_ack_cookie.value);
  if (!fidl_transport_status.ok()) {
    fdf::error("FIDL error calling AcknowledgeVsync: {}", fidl_transport_status.error());
    return zx::error(fidl_transport_status.status());
  }
  return zx::ok();
}

zx::result<> TestFidlClient::SetMinimumRgb(uint8_t minimum_rgb) {
  ZX_ASSERT(coordinator_fidl_client_.is_valid());

  fidl::WireResult<fuchsia_hardware_display::Coordinator::SetMinimumRgb> fidl_transport_result =
      coordinator_fidl_client_->SetMinimumRgb(minimum_rgb);
  if (!fidl_transport_result.ok()) {
    fdf::error("FIDL error calling SetMinimumRgb: {}", fidl_transport_result.error());
    return zx::error(fidl_transport_result.status());
  }

  fit::result<zx_status_t>& fidl_domain_result = fidl_transport_result.value();
  if (fidl_domain_result.is_error()) {
    fdf::warn("SetMinimumRgb failed: {}", zx::make_result(fidl_domain_result.error_value()));
    return zx::error(fidl_domain_result.error_value());
  }
  return zx::ok();
}

zx::result<display::ImageId> TestFidlClient::CreateFullscreenImage() {
  return ImportImageWithSysmem(state_.FullscreenImageMetadata());
}

zx::result<display::LayerId> TestFidlClient::CreateFullscreenImageLayer() {
  ZX_ASSERT(coordinator_fidl_client_.is_valid());

  zx::result<display::LayerId> layer_id_result = CreateLayer();
  if (layer_id_result.is_error()) {
    // CreateLayer() has already logged the error.
    return layer_id_result;
  }
  const display::LayerId layer_id = layer_id_result.value();
  zx::result<> set_primary_config_result =
      SetLayerPrimaryConfig(layer_id, state_.FullscreenImageMetadata());
  if (set_primary_config_result.is_error()) {
    // SetLayerPrimaryConfig() has already logged the error.
    return set_primary_config_result.take_error();
  }

  return zx::ok(layer_id);
}

zx::result<display::LayerId> TestFidlClient::CreateFullscreenColorLayer(display::Color color) {
  ZX_ASSERT(coordinator_fidl_client_.is_valid());

  zx::result<display::LayerId> layer_id_result = CreateLayer();
  if (layer_id_result.is_error()) {
    // CreateLayer() has already logged the error.
    return layer_id_result;
  }
  const display::LayerId layer_id = layer_id_result.value();
  zx::result<> set_color_result = SetLayerColor(layer_id, color);
  if (set_color_result.is_error()) {
    // SetLayerColor() has already logged the error.
    return set_color_result.take_error();
  }

  return zx::ok(layer_id);
}

zx::result<TestFidlClient::EventInfo> TestFidlClient::CreateEvent() {
  zx::event event;
  zx_status_t create_status = zx::event::create(0u, &event);
  if (create_status != ZX_OK) {
    fdf::error("zx::event::create() failed: {}", zx::make_result(create_status));
    return zx::error(create_status);
  }

  zx_info_handle_basic_t event_handle_info;
  zx_status_t get_info_status = event.get_info(ZX_INFO_HANDLE_BASIC, &event_handle_info,
                                               sizeof(event_handle_info), nullptr, nullptr);
  if (get_info_status != ZX_OK) {
    fdf::error("zx::event::get_info() failed: {}", zx::make_result(get_info_status));
    return zx::error(get_info_status);
  }

  zx::event event_duplicate;
  zx_status_t duplicate_status = event.duplicate(ZX_RIGHT_SAME_RIGHTS, &event_duplicate);
  if (duplicate_status != ZX_OK) {
    fdf::error("zx::event::duplicate() failed: {}", zx::make_result(duplicate_status));
    return zx::error(duplicate_status);
  }

  const display::EventId event_id(event_handle_info.koid);
  zx::result<> import_result = ImportEvent(std::move(event), event_id);
  if (import_result.is_error()) {
    // ImportEvent() has already logged the error.
    return import_result.take_error();
  }

  return zx::ok(EventInfo{
      .id = event_id,
      .event = std::move(event_duplicate),
  });
}

zx::result<> TestFidlClient::ApplyLayers(display::ConfigStamp config_stamp,
                                         const std::vector<LayerConfig>& layer_configs) {
  ZX_ASSERT_MSG(!layer_configs.empty(), "Empty configurations are not supported");

  zx::result<> set_display_layers_result = SetDisplayLayers(state_.display_id(), layer_configs);
  if (set_display_layers_result.is_error()) {
    // SetDisplayLayers() has already logged the error.
    return set_display_layers_result;
  }

  for (const LayerConfig& layer_config : layer_configs) {
    if (layer_config.image_id == display::kInvalidImageId) {
      // The layer is configured as a solid color fill layer.
      continue;
    }

    zx::result<> set_layer_image_result = SetLayerImage(
        layer_config.layer_id, layer_config.image_id, layer_config.image_ready_wait_event_id);
    if (set_layer_image_result.is_error()) {
      // SetLayerImage() has already logged the error.
      return set_layer_image_result;
    }
  }

  zx::result<display::ConfigCheckResult> check_config_result = CheckConfig();
  if (check_config_result.is_error()) {
    // CheckConfig() has already logged the error.
    return check_config_result.take_error();
  }
  if (check_config_result.value() != display::ConfigCheckResult::kOk) {
    fdf::error("CheckConfig() rejected the config: code {}",
               check_config_result.value().ValueForLogging());
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  zx::result<> commit_config_result = CommitConfig(config_stamp);
  if (commit_config_result.is_error()) {
    // CommitConfig() has already logged the error.
    return commit_config_result;
  }

  zx::result<display::ConfigStamp> get_last_config_stamp_result = GetLastCommittedConfigStamp();
  if (get_last_config_stamp_result.is_error()) {
    // GetLastCommittedConfigStamp() has already logged the error.
    return get_last_config_stamp_result.take_error();
  }

  if (get_last_config_stamp_result.value() != config_stamp) {
    fdf::error("GetLastCommittedConfigStamp() returned {}, expected {}",
               get_last_config_stamp_result->value(), config_stamp.value());
    return zx::error(ZX_ERR_INTERNAL);
  }
  return zx::ok();
}

zx::result<display::ConfigStamp> TestFidlClient::GetLastCommittedConfigStamp() {
  EXPECT_TRUE(coordinator_fidl_client_);
  fidl::WireResult<fuchsia_hardware_display::Coordinator::GetLatestCommittedConfigStamp>
      fidl_transport_result = coordinator_fidl_client_->GetLatestCommittedConfigStamp();
  if (!fidl_transport_result.ok()) {
    fdf::error("FIDL error calling GetLatestCommittedConfigStamp: {}",
               fidl_transport_result.error());
    return zx::error(fidl_transport_result.status());
  }
  fuchsia_hardware_display::wire::CoordinatorGetLatestCommittedConfigStampResponse&
      fidl_domain_result = fidl_transport_result.value();
  return zx::ok(display::ConfigStamp(fidl_domain_result.stamp));
}

std::vector<TestFidlClient::LayerConfig> TestFidlClient::CreateFullscreenLayerConfig() {
  zx::result<display::LayerId> layer_id_result = CreateFullscreenImageLayer();
  ZX_ASSERT_MSG(layer_id_result.is_ok(), "%s", layer_id_result.status_string());

  zx::result<display::ImageId> image_id_result =
      ImportImageWithSysmem(state_.FullscreenImageMetadata());
  ZX_ASSERT_MSG(image_id_result.is_ok(), "%s", image_id_result.status_string());

  return {
      LayerConfig{.layer_id = layer_id_result.value(), .image_id = image_id_result.value()},
  };
}

zx::result<> TestFidlClient::ImportBufferCollection(
    display::BufferCollectionId buffer_collection_id,
    fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> buffer_token) {
  const fuchsia_hardware_display::wire::BufferCollectionId fidl_buffer_collection_id =
      buffer_collection_id.ToFidl();

  fidl::WireResult<fuchsia_hardware_display::Coordinator::ImportBufferCollection>
      fidl_transport_result = coordinator_fidl_client_->ImportBufferCollection(
          fidl_buffer_collection_id, std::move(buffer_token));
  if (!fidl_transport_result.ok()) {
    fdf::error("FIDL error calling ImportBufferCollection: {}", fidl_transport_result.error());
    return zx::error(fidl_transport_result.status());
  }

  fit::result<zx_status_t>& fidl_domain_result = fidl_transport_result.value();
  if (fidl_domain_result.is_error()) {
    fdf::warn("ImportBufferCollection failed: {}",
              zx::make_result(fidl_domain_result.error_value()));
    return zx::error(fidl_domain_result.error_value());
  }
  return zx::ok();
}

zx::result<> TestFidlClient::SetBufferCollectionConstraints(
    display::BufferCollectionId buffer_collection_id,
    display::ImageBufferUsage image_buffer_usage) {
  const fuchsia_hardware_display::wire::BufferCollectionId fidl_buffer_collection_id =
      buffer_collection_id.ToFidl();
  const fuchsia_hardware_display_types::wire::ImageBufferUsage fidl_image_buffer_usage =
      image_buffer_usage.ToFidl();

  fidl::WireResult<fuchsia_hardware_display::Coordinator::SetBufferCollectionConstraints>
      fidl_transport_result = coordinator_fidl_client_->SetBufferCollectionConstraints(
          fidl_buffer_collection_id, fidl_image_buffer_usage);
  if (!fidl_transport_result.ok()) {
    fdf::error("FIDL error calling SetBufferCollectionConstraints: {}",
               fidl_transport_result.error());
    return zx::error(fidl_transport_result.status());
  }

  fit::result<zx_status_t>& fidl_domain_result = fidl_transport_result.value();
  if (fidl_domain_result.is_error()) {
    fdf::warn("SetBufferCollectionConstraints failed: {}",
              zx::make_result(fidl_domain_result.error_value()));
    return zx::error(fidl_domain_result.error_value());
  }
  return zx::ok();
}

zx::result<fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken>>
TestFidlClient::SysmemAllocateSharedCollection() {
  auto [token_client, token_server] =
      fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();
  fidl::Arena arena;
  fidl::OneWayStatus fidl_transport_status = sysmem_->AllocateSharedCollection(
      fuchsia_sysmem2::wire::AllocatorAllocateSharedCollectionRequest::Builder(arena)
          .token_request(std::move(token_server))
          .Build());
  if (!fidl_transport_status.ok()) {
    fdf::error("FIDL error calling AllocateSharedCollection: {}", fidl_transport_status.error());
    return zx::error(fidl_transport_status.status());
  }
  return zx::ok(std::move(token_client));
}

zx::result<fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken>>
TestFidlClient::SysmemTokenDuplicateSync(
    const fidl::WireSyncClient<fuchsia_sysmem2::BufferCollectionToken>& token) {
  auto [new_token_client, new_token_server] =
      fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();

  fidl::Arena arena;
  static constexpr zx_rights_t kRightAttenunationMasks[] = {ZX_RIGHT_SAME_RIGHTS};
  fidl::WireResult<fuchsia_sysmem2::BufferCollectionToken::DuplicateSync> fidl_transport_result =
      token->DuplicateSync(
          fuchsia_sysmem2::wire::BufferCollectionTokenDuplicateSyncRequest::Builder(arena)
              .rights_attenuation_masks(kRightAttenunationMasks)
              .Build());
  if (!fidl_transport_result.ok()) {
    fdf::error("FIDL error calling BufferCollectionToken.DuplicateSync: {}",
               fidl_transport_result.error());
    return zx::error(fidl_transport_result.status());
  }

  fuchsia_sysmem2::wire::BufferCollectionTokenDuplicateSyncResponse& fidl_domain_result =
      fidl_transport_result.value();
  ZX_ASSERT(fidl_domain_result.has_tokens());
  ZX_ASSERT(fidl_domain_result.tokens().size() == 1);

  return zx::ok(std::move(fidl_domain_result.tokens()[0]));
}

zx::result<fidl::ClientEnd<fuchsia_sysmem2::BufferCollection>> TestFidlClient::SysmemTokenBind(
    fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token) {
  auto [buffer_collection_client, buffer_collection_server] =
      fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();

  fidl::Arena arena;
  fidl::OneWayStatus fidl_transport_status = sysmem_->BindSharedCollection(
      fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
          .token(std::move(token))
          .buffer_collection_request(std::move(buffer_collection_server))
          .Build());
  if (!fidl_transport_status.ok()) {
    fdf::error("FIDL error calling BindSharedCollection: {}", fidl_transport_status.error());
    return zx::error(fidl_transport_status.status());
  }
  return zx::ok(std::move(buffer_collection_client));
}

zx::result<> TestFidlClient::SysmemBufferCollectionRelease(
    fidl::ClientEnd<fuchsia_sysmem2::BufferCollection> buffer_collection) {
  fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection> buffer_collection_client(
      std::move(buffer_collection));

  fidl::OneWayStatus fidl_transport_status = buffer_collection_client->Release();
  if (!fidl_transport_status.ok()) {
    fdf::error("FIDL error calling BufferCollection.Release: {}", fidl_transport_status.error());
    return zx::error(fidl_transport_status.status());
  }
  return zx::ok();
}

zx::result<size_t> TestFidlClient::SysmemWaitForAllBuffersAllocated(
    const fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection>& buffer_collection) {
  fidl::WireResult<fuchsia_sysmem2::BufferCollection::WaitForAllBuffersAllocated>
      fidl_transport_result = buffer_collection->WaitForAllBuffersAllocated();
  if (!fidl_transport_result.ok()) {
    fdf::error("FIDL error calling WaitForAllBuffersAllocated: {}", fidl_transport_result.error());
    return zx::error(fidl_transport_result.status());
  }

  fit::result<fuchsia_sysmem2::wire::Error,
              fuchsia_sysmem2::wire::BufferCollectionWaitForAllBuffersAllocatedResponse*>&
      fidl_domain_result = fidl_transport_result.value();
  if (fidl_domain_result.is_error()) {
    fdf::warn("WaitForAllBufferAllocated failed with sysmem error: {}",
              static_cast<uint32_t>(fidl_domain_result.error_value()));
    return zx::error(ZX_ERR_INTERNAL);
  }
  ZX_DEBUG_ASSERT_MSG(fidl_domain_result.value()->has_buffer_collection_info(),
                      "Sysmem deviated from its contract");
  ZX_DEBUG_ASSERT_MSG(fidl_domain_result.value()->buffer_collection_info().has_buffers(),
                      "Sysmem deviated from its contract");
  return zx::ok(fidl_domain_result.value()->buffer_collection_info().buffers().size());
}

zx::result<> TestFidlClient::SetSysmemConstraintsForImage(
    const fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection>& buffer_collection) {
  {
    fidl::Arena arena;
    fidl::OneWayStatus set_name_transport_status =
        buffer_collection->SetName(fuchsia_sysmem2::wire::NodeSetNameRequest::Builder(arena)
                                       .priority(10000u)
                                       .name("display-coordintator-tests")
                                       .Build());
    if (!set_name_transport_status.ok()) {
      fdf::error("FIDL error calling BufferCollection.SetName: {}",
                 set_name_transport_status.error());
      return zx::error(set_name_transport_status.status());
    }
  }

  fidl::Arena arena;
  fidl::WireTableBuilder<fuchsia_sysmem2::wire::BufferCollectionConstraints> constraints_builder =
      fuchsia_sysmem2::wire::BufferCollectionConstraints::Builder(arena);
  constraints_builder.min_buffer_count(1)
      .usage(fuchsia_sysmem2::wire::BufferUsage::Builder(arena)
                 .none(fuchsia_sysmem2::wire::kNoneUsage)
                 .Build())
      // We specify min_size_bytes 1 so that something is specifying a minimum size. More typically
      // the display client would specify ImageFormatConstraints that implies a non-zero
      // min_size_bytes.
      .buffer_memory_constraints(fuchsia_sysmem2::wire::BufferMemoryConstraints::Builder(arena)
                                     .min_size_bytes(1)
                                     .ram_domain_supported(true)
                                     .Build());

  fidl::OneWayStatus set_constraints_transport_status = buffer_collection->SetConstraints(
      fuchsia_sysmem2::wire::BufferCollectionSetConstraintsRequest::Builder(arena)
          .constraints(constraints_builder.Build())
          .Build());
  if (!set_constraints_transport_status.ok()) {
    fdf::error("FIDL error calling BufferCollection.SetConstraints: {}",
               set_constraints_transport_status.error());
    return zx::error(set_constraints_transport_status.status());
  }
  return zx::ok();
}

zx::result<display::ImageId> TestFidlClient::ImportImageWithSysmem(
    const display::ImageMetadata& image_metadata) {
  zx::result<fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken>> local_buffer_token_result =
      SysmemAllocateSharedCollection();
  if (local_buffer_token_result.is_error()) {
    // SysmemAllocateSharedCollection() already logged the error.
    return local_buffer_token_result.take_error();
  }
  fidl::WireSyncClient<fuchsia_sysmem2::BufferCollectionToken> local_buffer_token(
      std::move(local_buffer_token_result).value());

  // We use DuplicateSync() to ensure that the buffer token is created before
  // passing it to the driver.
  zx::result<fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken>> driver_buffer_token_result =
      SysmemTokenDuplicateSync(local_buffer_token);
  if (driver_buffer_token_result.is_error()) {
    // SysmemTokenDuplicateSync() already logged the error.
    return driver_buffer_token_result.take_error();
  }
  fidl::WireSyncClient<fuchsia_sysmem2::BufferCollectionToken> driver_buffer_token(
      std::move(driver_buffer_token_result).value());

  const display::BufferCollectionId buffer_collection_id = next_buffer_collection_id_;
  ++next_buffer_collection_id_;
  zx::result<> import_buffer_collection_result =
      ImportBufferCollection(buffer_collection_id, driver_buffer_token.TakeClientEnd());
  if (import_buffer_collection_result.is_error()) {
    // ImportBufferCollection() has already logged the error.
    return import_buffer_collection_result.take_error();
  }

  const display::ImageBufferUsage image_buffer_usage({
      .tiling_type = image_metadata.tiling_type(),
  });
  zx::result<> set_buffer_constraints_result =
      SetBufferCollectionConstraints(buffer_collection_id, image_buffer_usage);
  if (set_buffer_constraints_result.is_error()) {
    // SetBufferCollectionConstraints() has already logged the error.
    return set_buffer_constraints_result.take_error();
  }

  // Ensure that the collection's buffers are allocated before using them in
  // ImportImage() calls.
  zx::result<fidl::ClientEnd<fuchsia_sysmem2::BufferCollection>> buffer_collection_result =
      SysmemTokenBind(local_buffer_token.TakeClientEnd());
  if (buffer_collection_result.is_error()) {
    // SysmemTokenBind() has already logged the error.
    return buffer_collection_result.take_error();
  }
  fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection> buffer_collection(
      std::move(buffer_collection_result).value());

  zx::result<> set_image_constraints_result = SetSysmemConstraintsForImage(buffer_collection);
  if (set_image_constraints_result.is_error()) {
    // SetSysmemConstraintsForImage() has already logged the error.
    return set_image_constraints_result.take_error();
  }

  zx::result<size_t> wait_for_all_buffers_allocated_result =
      SysmemWaitForAllBuffersAllocated(buffer_collection);
  if (wait_for_all_buffers_allocated_result.is_error()) {
    // SysmemWaitForAllBuffersAllocated() has already logged the error.
    return wait_for_all_buffers_allocated_result.take_error();
  }
  if (wait_for_all_buffers_allocated_result.value() < 1) {
    fdf::error("WaitForAllBuffersAllocated() only allocated {} buffers",
               wait_for_all_buffers_allocated_result.value());
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  const display::ImageId image_id = next_imported_image_id_;
  ++next_imported_image_id_;

  zx::result<> import_image_result =
      ImportImage(image_metadata, buffer_collection_id, /*buffer_index=*/0, image_id);
  if (import_image_result.is_error()) {
    // ImportImage() has already logged the error.
    return import_image_result.take_error();
  }

  zx::result<> buffer_collection_release_result =
      SysmemBufferCollectionRelease(buffer_collection.TakeClientEnd());
  if (buffer_collection_release_result.is_error()) {
    // SysmemBufferCollectionRelease() has already logged the error.
    return buffer_collection_release_result.take_error();
  }

  return zx::ok(image_id);
}

class IntegrationTest : public TestBase {
 public:
  void TriggerDisplayEngineVsync() { FakeDisplayEngine().TriggerVsync(); }

  display::DriverConfigStamp DisplayEngineSubmittedConfigStamp() {
    return FakeDisplayEngine().LastSubmittedConfigStamp();
  }

  // Sets up a Display Coordinator client connection that delivers VSync events.
  //
  // The returned client is guaranteed to have a connected display. However, the
  // client is not guaranteed to own the displays.
  //
  // The returned client has requested VSync event delivery. However, the request
  // may not have been processed by the Display Coordinator.
  //
  // Crashes if any error occurs during the setup.
  std::unique_ptr<TestFidlClient> OpenCoordinatorTestFidlClient(
      const fidl::WireSyncClient<fuchsia_sysmem2::Allocator>* sysmem_client,
      const fidl::WireSyncClient<fuchsia_hardware_display::Provider>& display_provider_client,
      display::ClientPriority client_priority) {
    ZX_ASSERT(sysmem_client != nullptr);
    ZX_ASSERT(sysmem_client->is_valid());
    ZX_ASSERT(display_provider_client.is_valid());
    ZX_DEBUG_ASSERT(client_priority != display::ClientPriority::kInvalid);

    auto coordinator_client = std::make_unique<TestFidlClient>(&sysmem_client_);
    zx::result<> open_coordinator_result =
        coordinator_client->OpenCoordinator(display_provider_client, client_priority, dispatcher());
    ZX_ASSERT_MSG(open_coordinator_result.is_ok(), "Failed to open coordinator: %s",
                  open_coordinator_result.status_string());

    WaitUntil([&]() { return coordinator_client->state().HasConnectedDisplay(); });

    return coordinator_client;
  }

  // |TestBase|
  void SetUp() override {
    TestBase::SetUp();

    sysmem_client_ = fidl::WireSyncClient<fuchsia_sysmem2::Allocator>(ConnectToSysmemAllocatorV2());

    zx_koid_t koid = fsl::GetCurrentProcessKoid();
    std::string debug_name = std::format("display-coordinator-unittests[{}]", koid);

    fidl::Arena arena;
    fidl::OneWayStatus fidl_transport_status = sysmem_client_->SetDebugClientInfo(
        fuchsia_sysmem2::wire::AllocatorSetDebugClientInfoRequest::Builder(arena)
            .name(fidl::StringView::FromExternal(debug_name))
            .id(koid)
            .Build());
    EXPECT_TRUE(fidl_transport_status.ok()) << fidl_transport_status.status_string();
  }

 protected:
  fidl::WireSyncClient<fuchsia_sysmem2::Allocator> sysmem_client_;
};

TEST_F(IntegrationTest, DISABLED_ClientsCanBail) {
  for (size_t i = 0; i < 100; i++) {
    std::unique_ptr<TestFidlClient> client = OpenCoordinatorTestFidlClient(
        &sysmem_client_, DisplayProviderClient(), display::ClientPriority::kCompositor);
  }
}

TEST_F(IntegrationTest, RejectSecondPrimaryClient) {
  std::unique_ptr<TestFidlClient> primary_client = OpenCoordinatorTestFidlClient(
      &sysmem_client_, DisplayProviderClient(), display::ClientPriority::kCompositor);

  TestFidlClient second_client(&sysmem_client_);
  zx::result<> open_coordinator_result = second_client.OpenCoordinator(
      DisplayProviderClient(), display::ClientPriority::kCompositor, dispatcher());
  EXPECT_FALSE(open_coordinator_result.is_ok());
  EXPECT_EQ(open_coordinator_result.error_value(), ZX_ERR_ALREADY_BOUND);
}

TEST_F(IntegrationTest, MustUseUniqueEventIDs) {
  std::unique_ptr<TestFidlClient> client = OpenCoordinatorTestFidlClient(
      &sysmem_client_, DisplayProviderClient(), display::ClientPriority::kCompositor);
  zx::event event_a, event_b, event_c;
  ASSERT_OK(zx::event::create(0, &event_a));
  ASSERT_OK(zx::event::create(0, &event_b));
  ASSERT_OK(zx::event::create(0, &event_c));
  {
    static constexpr display::EventId kEventId(123);
    ASSERT_OK(client->ImportEvent(std::move(event_a), kEventId));
    ASSERT_OK(client->ImportEvent(std::move(event_b), kEventId));
    // This test passes if it closes without deadlocking.
  }
  // TODO: Use LLCPP epitaphs when available to detect ZX_ERR_PEER_CLOSED.
}

TEST_F(IntegrationTest, MaxLayerCountPropagatedToClient) {
  std::unique_ptr<TestFidlClient> client = OpenCoordinatorTestFidlClient(
      &sysmem_client_, DisplayProviderClient(), display::ClientPriority::kCompositor);

  // The integration test uses a fake display engine. In the test setup,
  // the fake engine reports its max_layer_count (currently 1 in
  // testing/base.cc).
  static constexpr uint32_t kEngineMaxLayerCount = 1;

  WaitUntil([&]() { return client->state().HasConnectedDisplay(); });
  const TestDisplayInfo& display_info = client->state().GetDisplayInfo();

  // Verify that the max_layer_count field we added in Part 1 is correctly
  // propagated from the engine driver to the FIDL client.
  EXPECT_EQ(display_info.max_layer_count, kEngineMaxLayerCount);
}
TEST_F(IntegrationTest, VsyncEventForImageConfig) {
  std::unique_ptr<TestFidlClient> primary_client = OpenCoordinatorTestFidlClient(
      &sysmem_client_, DisplayProviderClient(), display::ClientPriority::kCompositor);
  WaitUntil([&]() { return primary_client->state().has_display_ownership(); });

  static constexpr display::ConfigStamp kInitialConfigStamp(42);
  ASSERT_EQ(display::kInvalidDriverConfigStamp, DisplayEngineSubmittedConfigStamp());
  ASSERT_OK(primary_client->ApplyLayers(kInitialConfigStamp,
                                        primary_client->CreateFullscreenLayerConfig()));
  WaitUntil(
      [&]() { return DisplayEngineSubmittedConfigStamp() != display::kInvalidDriverConfigStamp; });

  // Wait for a VSync acknowledging the displayed configuration.
  ASSERT_EQ(0u, primary_client->state().vsync_count());
  TriggerDisplayEngineVsync();
  WaitUntil([&]() { return primary_client->state().vsync_count() >= 1; });
  EXPECT_EQ(kInitialConfigStamp, primary_client->state().last_vsync_config_stamp());
  EXPECT_EQ(1u, primary_client->state().vsync_count());
}

TEST_F(IntegrationTest, VsyncEventForImagelessConfig) {
  std::unique_ptr<TestFidlClient> primary_client = OpenCoordinatorTestFidlClient(
      &sysmem_client_, DisplayProviderClient(), display::ClientPriority::kCompositor);
  WaitUntil([&]() { return primary_client->state().has_display_ownership(); });

  zx::result<display::LayerId> create_color_layer_result =
      primary_client->CreateFullscreenColorLayer(kFuchsiaBgra);
  ASSERT_OK(create_color_layer_result);
  display::LayerId color_layer_id = create_color_layer_result.value();

  static constexpr display::ConfigStamp kInitialConfigStamp(42);
  ASSERT_EQ(display::kInvalidDriverConfigStamp, DisplayEngineSubmittedConfigStamp());
  ASSERT_OK(primary_client->ApplyLayers(kInitialConfigStamp, {{.layer_id = color_layer_id}}));
  WaitUntil(
      [&]() { return DisplayEngineSubmittedConfigStamp() != display::kInvalidDriverConfigStamp; });

  // Wait for a VSync acknowledging the displayed configuration.
  ASSERT_EQ(0u, primary_client->state().vsync_count());
  TriggerDisplayEngineVsync();
  WaitUntil([&]() { return primary_client->state().vsync_count() >= 1; });
  EXPECT_EQ(kInitialConfigStamp, primary_client->state().last_vsync_config_stamp());
  EXPECT_EQ(1u, primary_client->state().vsync_count());
}

TEST_F(IntegrationTest, VsyncEventAfterImageLayerConvertsToColorLayer) {
  std::unique_ptr<TestFidlClient> primary_client = OpenCoordinatorTestFidlClient(
      &sysmem_client_, DisplayProviderClient(), display::ClientPriority::kCompositor);
  WaitUntil([&]() { return primary_client->state().has_display_ownership(); });

  static constexpr display::ConfigStamp kInitialConfigStamp(1);
  ASSERT_EQ(display::kInvalidDriverConfigStamp, DisplayEngineSubmittedConfigStamp());
  ASSERT_OK(primary_client->ApplyLayers(kInitialConfigStamp,
                                        primary_client->CreateFullscreenLayerConfig()));
  WaitUntil(
      [&]() { return DisplayEngineSubmittedConfigStamp() != display::kInvalidDriverConfigStamp; });
  const display::DriverConfigStamp initial_driver_config_stamp =
      DisplayEngineSubmittedConfigStamp();

  // Wait for a VSync acknowledging the displayed configuration.
  ASSERT_EQ(0u, primary_client->state().vsync_count());
  TriggerDisplayEngineVsync();
  WaitUntil([&]() { return primary_client->state().vsync_count() >= 1; });
  EXPECT_EQ(kInitialConfigStamp, primary_client->state().last_vsync_config_stamp());
  EXPECT_EQ(1u, primary_client->state().vsync_count());

  // Replace the image layer with a color layer.
  zx::result<display::LayerId> create_color_layer_result =
      primary_client->CreateFullscreenColorLayer(kFuchsiaBgra);
  ASSERT_OK(create_color_layer_result);
  display::LayerId color_layer_id = create_color_layer_result.value();

  static constexpr display::ConfigStamp kSecondConfigStamp(2);
  ASSERT_EQ(initial_driver_config_stamp, DisplayEngineSubmittedConfigStamp());
  ASSERT_OK(primary_client->ApplyLayers(kSecondConfigStamp, {{.layer_id = color_layer_id}}));
  WaitUntil([&]() { return DisplayEngineSubmittedConfigStamp() > initial_driver_config_stamp; });

  // Wait for a VSync acknowledging the configuration with a layer change.
  ASSERT_EQ(1u, primary_client->state().vsync_count());
  TriggerDisplayEngineVsync();
  WaitUntil([&]() { return primary_client->state().vsync_count() >= 2; });
  EXPECT_EQ(kSecondConfigStamp, primary_client->state().last_vsync_config_stamp());
  EXPECT_EQ(2u, primary_client->state().vsync_count());
}

TEST_F(IntegrationTest, DisplayOwnershipChangeEventsWithTwoClients) {
  std::unique_ptr<TestFidlClient> virtcon_client = OpenCoordinatorTestFidlClient(
      &sysmem_client_, DisplayProviderClient(), display::ClientPriority::kVirtcon);
  WaitUntil([&]() { return virtcon_client->state().has_display_ownership(); });

  std::unique_ptr<TestFidlClient> primary_client = OpenCoordinatorTestFidlClient(
      &sysmem_client_, DisplayProviderClient(), display::ClientPriority::kCompositor);
  WaitUntil([&]() { return primary_client->state().has_display_ownership(); });
  EXPECT_FALSE(virtcon_client->state().has_display_ownership());

  primary_client.reset();
  WaitUntil([&]() { return virtcon_client->state().has_display_ownership(); });
}

TEST_F(IntegrationTest, DisplayOwnershipChangeEventsWithThreeClients) {
  std::unique_ptr<TestFidlClient> virtcon_client = OpenCoordinatorTestFidlClient(
      &sysmem_client_, DisplayProviderClient(), display::ClientPriority::kVirtcon);
  WaitUntil([&]() { return virtcon_client->state().has_display_ownership(); });

  std::unique_ptr<TestFidlClient> primary_client = OpenCoordinatorTestFidlClient(
      &sysmem_client_, DisplayProviderClient(), display::ClientPriority::kCompositor);
  WaitUntil([&]() { return primary_client->state().has_display_ownership(); });
  EXPECT_FALSE(virtcon_client->state().has_display_ownership());

  std::unique_ptr<TestFidlClient> tool_client = OpenCoordinatorTestFidlClient(
      &sysmem_client_, DisplayProviderClient(), display::ClientPriority(300));
  WaitUntil([&]() { return tool_client->state().has_display_ownership(); });
  EXPECT_FALSE(primary_client->state().has_display_ownership());
  EXPECT_FALSE(virtcon_client->state().has_display_ownership());

  tool_client.reset();
  WaitUntil([&]() { return primary_client->state().has_display_ownership(); });
  EXPECT_FALSE(virtcon_client->state().has_display_ownership());

  primary_client.reset();
  WaitUntil([&]() { return virtcon_client->state().has_display_ownership(); });
}

TEST_F(IntegrationTest, CommitConfigAfterOwnerChangeWithImageLayers) {
  std::unique_ptr<TestFidlClient> virtcon_client = OpenCoordinatorTestFidlClient(
      &sysmem_client_, DisplayProviderClient(), display::ClientPriority::kVirtcon);
  WaitUntil([&]() { return virtcon_client->state().has_display_ownership(); });

  static constexpr display::ConfigStamp kVirtconConfigStamp(1);
  ASSERT_EQ(display::kInvalidDriverConfigStamp, DisplayEngineSubmittedConfigStamp());
  ASSERT_OK(virtcon_client->ApplyLayers(kVirtconConfigStamp,
                                        virtcon_client->CreateFullscreenLayerConfig()));
  WaitUntil(
      [&]() { return DisplayEngineSubmittedConfigStamp() != display::kInvalidDriverConfigStamp; });
  const display::DriverConfigStamp virtcon_driver_config_stamp =
      DisplayEngineSubmittedConfigStamp();

  std::unique_ptr<TestFidlClient> primary_client = OpenCoordinatorTestFidlClient(
      &sysmem_client_, DisplayProviderClient(), display::ClientPriority::kCompositor);
  WaitUntil([&]() { return primary_client->state().has_display_ownership(); });

  static constexpr display::ConfigStamp kPrimaryConfigStamp(2);
  ASSERT_EQ(virtcon_driver_config_stamp, DisplayEngineSubmittedConfigStamp());
  ASSERT_OK(primary_client->ApplyLayers(kPrimaryConfigStamp,
                                        primary_client->CreateFullscreenLayerConfig()));
  WaitUntil([&]() { return DisplayEngineSubmittedConfigStamp() > virtcon_driver_config_stamp; });
}

TEST_F(IntegrationTest, CommitConfigAfterOwnerChangeWithColorLayers) {
  std::unique_ptr<TestFidlClient> virtcon_client = OpenCoordinatorTestFidlClient(
      &sysmem_client_, DisplayProviderClient(), display::ClientPriority::kVirtcon);
  WaitUntil([&]() { return virtcon_client->state().has_display_ownership(); });

  zx::result<display::LayerId> create_virtcon_color_layer_result =
      virtcon_client->CreateFullscreenColorLayer(kFuchsiaBgra);
  ASSERT_OK(create_virtcon_color_layer_result);
  display::LayerId virtcon_color_layer_id = create_virtcon_color_layer_result.value();

  static constexpr display::ConfigStamp kVirtconConfigStamp(1);
  ASSERT_EQ(display::kInvalidDriverConfigStamp, DisplayEngineSubmittedConfigStamp());
  ASSERT_OK(
      virtcon_client->ApplyLayers(kVirtconConfigStamp, {{.layer_id = virtcon_color_layer_id}}));
  WaitUntil(
      [&]() { return DisplayEngineSubmittedConfigStamp() != display::kInvalidDriverConfigStamp; });
  const display::DriverConfigStamp virtcon_driver_config_stamp =
      DisplayEngineSubmittedConfigStamp();

  std::unique_ptr<TestFidlClient> primary_client = OpenCoordinatorTestFidlClient(
      &sysmem_client_, DisplayProviderClient(), display::ClientPriority::kCompositor);
  WaitUntil([&]() { return primary_client->state().has_display_ownership(); });

  zx::result<display::LayerId> create_primary_color_layer_result =
      primary_client->CreateFullscreenColorLayer(kFuchsiaBgra);
  ASSERT_OK(create_primary_color_layer_result);
  display::LayerId primary_color_layer_id = create_primary_color_layer_result.value();

  static constexpr display::ConfigStamp kPrimaryConfigStamp(2);
  ASSERT_EQ(virtcon_driver_config_stamp, DisplayEngineSubmittedConfigStamp());
  ASSERT_OK(
      primary_client->ApplyLayers(kPrimaryConfigStamp, {{.layer_id = primary_color_layer_id}}));
  WaitUntil([&]() { return DisplayEngineSubmittedConfigStamp() > virtcon_driver_config_stamp; });
}

TEST_F(IntegrationTest, VsyncEventAfterOwnerChangeWithImageLayers) {
  std::unique_ptr<TestFidlClient> virtcon_client = OpenCoordinatorTestFidlClient(
      &sysmem_client_, DisplayProviderClient(), display::ClientPriority::kVirtcon);
  WaitUntil([&]() { return virtcon_client->state().has_display_ownership(); });

  static constexpr display::ConfigStamp kVirtconConfigStamp(1);
  ASSERT_EQ(display::kInvalidDriverConfigStamp, DisplayEngineSubmittedConfigStamp());
  ASSERT_OK(virtcon_client->ApplyLayers(kVirtconConfigStamp,
                                        virtcon_client->CreateFullscreenLayerConfig()));
  WaitUntil(
      [&]() { return DisplayEngineSubmittedConfigStamp() != display::kInvalidDriverConfigStamp; });
  const display::DriverConfigStamp virtcon_driver_config_stamp =
      DisplayEngineSubmittedConfigStamp();

  ASSERT_EQ(0u, virtcon_client->state().vsync_count());
  TriggerDisplayEngineVsync();
  WaitUntil([&]() { return virtcon_client->state().vsync_count() >= 1; });
  EXPECT_EQ(kVirtconConfigStamp, virtcon_client->state().last_vsync_config_stamp());
  EXPECT_EQ(1u, virtcon_client->state().vsync_count());

  std::unique_ptr<TestFidlClient> primary_client = OpenCoordinatorTestFidlClient(
      &sysmem_client_, DisplayProviderClient(), display::ClientPriority::kCompositor);
  WaitUntil([&]() { return primary_client->state().has_display_ownership(); });

  static constexpr display::ConfigStamp kPrimaryConfigStamp(2);
  ASSERT_EQ(virtcon_driver_config_stamp, DisplayEngineSubmittedConfigStamp());
  ASSERT_OK(primary_client->ApplyLayers(kPrimaryConfigStamp,
                                        primary_client->CreateFullscreenLayerConfig()));
  WaitUntil([&]() { return DisplayEngineSubmittedConfigStamp() > virtcon_driver_config_stamp; });

  EXPECT_EQ(1u, virtcon_client->state().vsync_count());
  EXPECT_EQ(kVirtconConfigStamp, virtcon_client->state().last_vsync_config_stamp());

  ASSERT_EQ(0u, primary_client->state().vsync_count());
  TriggerDisplayEngineVsync();
  WaitUntil([&]() { return primary_client->state().vsync_count() >= 1; });
  EXPECT_EQ(kPrimaryConfigStamp, primary_client->state().last_vsync_config_stamp());
  EXPECT_EQ(1u, primary_client->state().vsync_count());

  EXPECT_EQ(1u, virtcon_client->state().vsync_count());
  EXPECT_EQ(kVirtconConfigStamp, virtcon_client->state().last_vsync_config_stamp());
}

TEST_F(IntegrationTest, VsyncEventAfterOwnerChangeWithColorLayers) {
  std::unique_ptr<TestFidlClient> virtcon_client = OpenCoordinatorTestFidlClient(
      &sysmem_client_, DisplayProviderClient(), display::ClientPriority::kVirtcon);
  WaitUntil([&]() { return virtcon_client->state().has_display_ownership(); });

  zx::result<display::LayerId> create_virtcon_color_layer_result =
      virtcon_client->CreateFullscreenColorLayer(kFuchsiaBgra);
  ASSERT_OK(create_virtcon_color_layer_result);
  display::LayerId virtcon_color_layer_id = create_virtcon_color_layer_result.value();

  static constexpr display::ConfigStamp kVirtconConfigStamp(1);
  ASSERT_EQ(display::kInvalidDriverConfigStamp, DisplayEngineSubmittedConfigStamp());
  ASSERT_OK(
      virtcon_client->ApplyLayers(kVirtconConfigStamp, {{.layer_id = virtcon_color_layer_id}}));
  WaitUntil(
      [&]() { return DisplayEngineSubmittedConfigStamp() != display::kInvalidDriverConfigStamp; });
  const display::DriverConfigStamp virtcon_driver_config_stamp =
      DisplayEngineSubmittedConfigStamp();

  ASSERT_EQ(0u, virtcon_client->state().vsync_count());
  TriggerDisplayEngineVsync();
  WaitUntil([&]() { return virtcon_client->state().vsync_count() >= 1; });
  EXPECT_EQ(kVirtconConfigStamp, virtcon_client->state().last_vsync_config_stamp());
  EXPECT_EQ(1u, virtcon_client->state().vsync_count());

  std::unique_ptr<TestFidlClient> primary_client = OpenCoordinatorTestFidlClient(
      &sysmem_client_, DisplayProviderClient(), display::ClientPriority::kCompositor);
  WaitUntil([&]() { return primary_client->state().has_display_ownership(); });

  zx::result<display::LayerId> create_primary_color_layer_result =
      primary_client->CreateFullscreenColorLayer(kFuchsiaBgra);
  ASSERT_OK(create_primary_color_layer_result);
  display::LayerId primary_color_layer_id = create_primary_color_layer_result.value();

  static constexpr display::ConfigStamp kPrimaryConfigStamp(2);
  ASSERT_EQ(virtcon_driver_config_stamp, DisplayEngineSubmittedConfigStamp());
  ASSERT_OK(
      primary_client->ApplyLayers(kPrimaryConfigStamp, {{.layer_id = primary_color_layer_id}}));
  WaitUntil([&]() { return DisplayEngineSubmittedConfigStamp() > virtcon_driver_config_stamp; });

  EXPECT_EQ(1u, virtcon_client->state().vsync_count());
  EXPECT_EQ(kVirtconConfigStamp, virtcon_client->state().last_vsync_config_stamp());

  ASSERT_EQ(0u, primary_client->state().vsync_count());
  TriggerDisplayEngineVsync();
  WaitUntil([&]() { return primary_client->state().vsync_count() >= 1; });
  EXPECT_EQ(kPrimaryConfigStamp, primary_client->state().last_vsync_config_stamp());
  EXPECT_EQ(1u, primary_client->state().vsync_count());

  EXPECT_EQ(1u, virtcon_client->state().vsync_count());
  EXPECT_EQ(kVirtconConfigStamp, virtcon_client->state().last_vsync_config_stamp());
}

TEST_F(IntegrationTest, VsyncEventsAfterClientChange) {
  std::unique_ptr<TestFidlClient> virtcon_client = OpenCoordinatorTestFidlClient(
      &sysmem_client_, DisplayProviderClient(), display::ClientPriority::kVirtcon);
  WaitUntil([&]() { return virtcon_client->state().has_display_ownership(); });

  // The Virtcon client does not apply any configuration, so it will never be
  // eligible for VSync events.

  std::unique_ptr<TestFidlClient> primary_client = OpenCoordinatorTestFidlClient(
      &sysmem_client_, DisplayProviderClient(), display::ClientPriority::kCompositor);
  WaitUntil([&]() { return primary_client->state().has_display_ownership(); });

  // Display an image.
  static constexpr display::ConfigStamp kPrimary1InitialConfigStamp(2);
  ASSERT_EQ(display::kInvalidDriverConfigStamp, DisplayEngineSubmittedConfigStamp());
  ASSERT_OK(primary_client->ApplyLayers(kPrimary1InitialConfigStamp,
                                        primary_client->CreateFullscreenLayerConfig()));
  WaitUntil(
      [&]() { return DisplayEngineSubmittedConfigStamp() != display::kInvalidDriverConfigStamp; });
  const display::DriverConfigStamp primary1_initial_driver_config_stamp =
      DisplayEngineSubmittedConfigStamp();

  ASSERT_EQ(0u, primary_client->state().vsync_count());
  TriggerDisplayEngineVsync();
  WaitUntil([&]() { return primary_client->state().vsync_count() >= 1; });
  EXPECT_EQ(kPrimary1InitialConfigStamp, primary_client->state().last_vsync_config_stamp());
  EXPECT_EQ(1u, primary_client->state().vsync_count());

  // Replace the image layer with a color layer.
  zx::result<display::LayerId> create_primary_color_layer_result =
      primary_client->CreateFullscreenColorLayer(kFuchsiaBgra);
  ASSERT_OK(create_primary_color_layer_result);
  display::LayerId primary_color_layer_id = create_primary_color_layer_result.value();

  static constexpr display::ConfigStamp kPrimary1SecondConfigStamp(3);
  ASSERT_EQ(primary1_initial_driver_config_stamp, DisplayEngineSubmittedConfigStamp());
  ASSERT_OK(primary_client->ApplyLayers(kPrimary1SecondConfigStamp,
                                        {{.layer_id = primary_color_layer_id}}));
  WaitUntil(
      [&]() { return DisplayEngineSubmittedConfigStamp() > primary1_initial_driver_config_stamp; });
  const display::DriverConfigStamp primary1_second_driver_config_stamp =
      DisplayEngineSubmittedConfigStamp();

  // The primary client disconnects, and the Virtcon client receives display
  // ownership. The old primary client's config remains applied, because the
  // Virtcon client did not apply any config.
  primary_client.reset();
  WaitUntil([&]() { return virtcon_client->state().has_display_ownership(); });
  EXPECT_EQ(primary1_second_driver_config_stamp, DisplayEngineSubmittedConfigStamp());

  // A new primary client connects.
  primary_client = OpenCoordinatorTestFidlClient(&sysmem_client_, DisplayProviderClient(),
                                                 display::ClientPriority::kCompositor);
  WaitUntil([&]() { return primary_client->state().has_display_ownership(); });

  // The VSync must be routed to the client that applied the configuration,
  // which is now disconnected. Nothing should be sent to the new client.
  EXPECT_EQ(primary1_second_driver_config_stamp, DisplayEngineSubmittedConfigStamp());
  TriggerDisplayEngineVsync();

  static constexpr display::ConfigStamp kPrimary2InitialConfigStamp(4);
  ASSERT_EQ(primary1_second_driver_config_stamp, DisplayEngineSubmittedConfigStamp());
  ASSERT_OK(primary_client->ApplyLayers(kPrimary2InitialConfigStamp,
                                        primary_client->CreateFullscreenLayerConfig()));
  WaitUntil(
      [&]() { return DisplayEngineSubmittedConfigStamp() > primary1_second_driver_config_stamp; });

  // Send a VSync using the config the client applied.
  EXPECT_EQ(0u, virtcon_client->state().vsync_count());
  ASSERT_EQ(0u, primary_client->state().vsync_count());
  TriggerDisplayEngineVsync();
  WaitUntil([&]() { return primary_client->state().vsync_count() >= 1; });
  EXPECT_EQ(kPrimary2InitialConfigStamp, primary_client->state().last_vsync_config_stamp());
  EXPECT_EQ(1u, primary_client->state().vsync_count());
  EXPECT_EQ(0u, virtcon_client->state().vsync_count());
}

TEST_F(IntegrationTest, AcknowledgeVsyncAfterQueueFull) {
  std::unique_ptr<TestFidlClient> primary_client = OpenCoordinatorTestFidlClient(
      &sysmem_client_, DisplayProviderClient(), display::ClientPriority::kCompositor);
  WaitUntil([&]() { return primary_client->state().has_display_ownership(); });

  zx::result<display::LayerId> create_color_layer_result =
      primary_client->CreateFullscreenColorLayer(kFuchsiaBgra);
  ASSERT_OK(create_color_layer_result);
  display::LayerId color_layer_id = create_color_layer_result.value();

  // Generate VSync messages with unique configuration stamps.
  ASSERT_EQ(0u, primary_client->state().vsync_count());
  int64_t generated_vsync_count =
      ClientVsyncQueue::kThrottleWatermark + ClientVsyncQueue::kThrottleBufferSize;
  for (int64_t index = 0; index < generated_vsync_count; ++index) {
    const display::DriverConfigStamp old_driver_stamp = DisplayEngineSubmittedConfigStamp();
    const display::ConfigStamp config_stamp(1 + index);
    ASSERT_OK(primary_client->ApplyLayers(config_stamp, {{.layer_id = color_layer_id}}));

    // Wait until the engine driver receives the new configuration.
    WaitUntil([&]() { return DisplayEngineSubmittedConfigStamp() != old_driver_stamp; });
    TriggerDisplayEngineVsync();
  }

  // Collect the throttled VSync messages and the ack cookie.
  {
    static constexpr uint64_t expected_vsync_count = ClientVsyncQueue::kThrottleWatermark;
    WaitUntil([&]() { return (primary_client->state().vsync_count() >= expected_vsync_count); });
    EXPECT_EQ(expected_vsync_count, primary_client->state().vsync_count());
    EXPECT_EQ(display::ConfigStamp(expected_vsync_count),
              primary_client->state().last_vsync_config_stamp());
  }
  ASSERT_NE(display::kInvalidVsyncAckCookie, primary_client->state().last_vsync_ack_cookie());

  // Acknowledge VSync to unblock the throttled messages.
  ASSERT_OK(primary_client->AcknowledgeVsync(primary_client->state().last_vsync_ack_cookie()));

  {
    static constexpr uint64_t expected_vsync_count =
        ClientVsyncQueue::kThrottleWatermark + ClientVsyncQueue::kThrottleBufferSize;
    WaitUntil([&]() { return primary_client->state().vsync_count() >= expected_vsync_count; });
    EXPECT_EQ(expected_vsync_count, primary_client->state().vsync_count());
    EXPECT_EQ(display::ConfigStamp(expected_vsync_count),
              primary_client->state().last_vsync_config_stamp());
  }
}

TEST_F(IntegrationTest, CreateImageLayer) {
  std::unique_ptr<TestFidlClient> client = OpenCoordinatorTestFidlClient(
      &sysmem_client_, DisplayProviderClient(), display::ClientPriority::kCompositor);

  EXPECT_OK(client->CreateFullscreenImageLayer());
}

TEST_F(IntegrationTest, CreateColorLayer) {
  std::unique_ptr<TestFidlClient> client = OpenCoordinatorTestFidlClient(
      &sysmem_client_, DisplayProviderClient(), display::ClientPriority::kCompositor);

  EXPECT_OK(client->CreateFullscreenColorLayer(kFuchsiaBgra));
}

TEST_F(IntegrationTest, ImportImageWithInvalidImageIdFails) {
  std::unique_ptr<TestFidlClient> client = OpenCoordinatorTestFidlClient(
      &sysmem_client_, DisplayProviderClient(), display::ClientPriority::kCompositor);

  constexpr display::ImageId image_id = display::kInvalidImageId;
  constexpr display::BufferCollectionId buffer_collection_id(0xffeeeedd);

  zx::result<> import_image_result =
      client->ImportImage(client->state().FullscreenImageMetadata(), buffer_collection_id,
                          /*buffer_index=*/0, image_id);
  EXPECT_NE(ZX_OK, import_image_result.status_value()) << import_image_result.status_string();
}

TEST_F(IntegrationTest, ImportImageWithNonExistentBufferCollectionId) {
  std::unique_ptr<TestFidlClient> client = OpenCoordinatorTestFidlClient(
      &sysmem_client_, DisplayProviderClient(), display::ClientPriority::kCompositor);

  constexpr display::BufferCollectionId kNonExistentCollectionId(0xffeeeedd);
  constexpr display::ImageId image_id(1);
  zx::result<> import_image_result =
      client->ImportImage(client->state().FullscreenImageMetadata(), kNonExistentCollectionId,
                          /*buffer_index=*/0, image_id);
  EXPECT_NE(ZX_OK, import_image_result.status_value()) << import_image_result.status_string();
}

TEST_F(IntegrationTest, ClampRgb) {
  std::unique_ptr<TestFidlClient> virtcon_client = OpenCoordinatorTestFidlClient(
      &sysmem_client_, DisplayProviderClient(), display::ClientPriority::kVirtcon);
  WaitUntil([&]() { return virtcon_client->state().has_display_ownership(); });

  {
    // Clamp RGB to a minimum value
    ASSERT_OK(virtcon_client->SetMinimumRgb(32));
    WaitUntil([&]() { return FakeDisplayEngine().GetClampRgbValue() == 32; });
  }

  // Create a primary client
  std::unique_ptr<TestFidlClient> primary_client = OpenCoordinatorTestFidlClient(
      &sysmem_client_, DisplayProviderClient(), display::ClientPriority::kCompositor);
  WaitUntil([&]() { return primary_client->state().has_display_ownership(); });
  // Clamp RGB to a new value
  ASSERT_OK(primary_client->SetMinimumRgb(1));
  WaitUntil([&]() { return FakeDisplayEngine().GetClampRgbValue() == 1; });

  // Close the primary client, wait for the virtcon client to regain display
  // ownership.
  primary_client.reset(nullptr);
  WaitUntil([&]() { return virtcon_client->state().has_display_ownership(); });

  zx::result<display::LayerId> create_virtcon_color_layer_result =
      virtcon_client->CreateFullscreenColorLayer(kFuchsiaBgra);
  ASSERT_OK(create_virtcon_color_layer_result);
  display::LayerId virtcon_color_layer_id = create_virtcon_color_layer_result.value();

  // Apply a config so the virtcon client's RGB clamp is set.
  static constexpr display::ConfigStamp kVirtconInitialConfigStamp(1);
  ASSERT_EQ(display::kInvalidDriverConfigStamp, DisplayEngineSubmittedConfigStamp());
  ASSERT_OK(virtcon_client->ApplyLayers(kVirtconInitialConfigStamp,
                                        {{.layer_id = virtcon_color_layer_id}}));
  WaitUntil(
      [&]() { return DisplayEngineSubmittedConfigStamp() != display::kInvalidDriverConfigStamp; });

  TriggerDisplayEngineVsync();
  // TODO(https://fxbug.dev/388885807): This test is racy. There's no guarantee
  // that the VSync has reached the Display Coordinator. Figure out the right
  // contract for RGB clamp application, and write the test against that
  // contract.

  // make sure clamp value was restored
  WaitUntil([&]() { return FakeDisplayEngine().GetClampRgbValue() == 32; });
}

TEST_F(IntegrationTest, VsyncGoesToClientWhoAppliedConfig) {
  // Create and bind virtcon client.
  std::unique_ptr<TestFidlClient> virtcon_client = OpenCoordinatorTestFidlClient(
      &sysmem_client_, DisplayProviderClient(), display::ClientPriority::kVirtcon);
  WaitUntil([&]() { return virtcon_client->state().has_display_ownership(); });

  zx::result<display::LayerId> create_virtcon_color_layer_result =
      virtcon_client->CreateFullscreenColorLayer(kFuchsiaBgra);
  ASSERT_OK(create_virtcon_color_layer_result);
  display::LayerId virtcon_color_layer_id = create_virtcon_color_layer_result.value();

  static constexpr display::ConfigStamp kVirtconInitialConfigStamp(1);
  ASSERT_EQ(display::kInvalidDriverConfigStamp, DisplayEngineSubmittedConfigStamp());
  ASSERT_OK(virtcon_client->ApplyLayers(kVirtconInitialConfigStamp,
                                        {{.layer_id = virtcon_color_layer_id}}));
  WaitUntil(
      [&]() { return DisplayEngineSubmittedConfigStamp() != display::kInvalidDriverConfigStamp; });
  const display::DriverConfigStamp virtcon_initial_driver_config_stamp =
      DisplayEngineSubmittedConfigStamp();

  // Create and bind primary client.
  std::unique_ptr<TestFidlClient> primary_client = OpenCoordinatorTestFidlClient(
      &sysmem_client_, DisplayProviderClient(), display::ClientPriority::kCompositor);
  WaitUntil([&]() { return primary_client->state().has_display_ownership(); });

  // The Virtcon client should receive VSync events while its config is applied.
  // This is the case until the primary client applies a config.
  EXPECT_EQ(0u, primary_client->state().vsync_count());
  ASSERT_EQ(0u, virtcon_client->state().vsync_count());
  TriggerDisplayEngineVsync();
  WaitUntil([&]() { return virtcon_client->state().vsync_count() >= 1; });
  EXPECT_EQ(kVirtconInitialConfigStamp, virtcon_client->state().last_vsync_config_stamp());
  EXPECT_EQ(0u, primary_client->state().vsync_count());
  EXPECT_EQ(1u, virtcon_client->state().vsync_count());

  // Present an image from the primary client.
  static constexpr display::ConfigStamp kPrimaryInitialConfigStamp(2);
  ASSERT_EQ(virtcon_initial_driver_config_stamp, DisplayEngineSubmittedConfigStamp());
  ASSERT_OK(primary_client->ApplyLayers(kPrimaryInitialConfigStamp,
                                        primary_client->CreateFullscreenLayerConfig()));
  WaitUntil(
      [&]() { return DisplayEngineSubmittedConfigStamp() > virtcon_initial_driver_config_stamp; });

  // Primary client should receive VSync events after applying a config.
  ASSERT_EQ(0u, primary_client->state().vsync_count());
  EXPECT_EQ(1u, virtcon_client->state().vsync_count());
  TriggerDisplayEngineVsync();
  WaitUntil([&]() { return primary_client->state().vsync_count() >= 1; });
  EXPECT_EQ(kPrimaryInitialConfigStamp, primary_client->state().last_vsync_config_stamp());
  EXPECT_EQ(1u, primary_client->state().vsync_count());
  EXPECT_EQ(1u, virtcon_client->state().vsync_count());
}

// This test case covers the basic interaction between CommitConfig() and VSync
// events.
//
// The test uses configurations with images without any wait fences. These
// images are ready for use when the Coordinator receives the CommitConfig()
// call. In this case, each VSync event should report the ConfigStamp used
// in the last CommitConfig() call.
//
// Here we test the following case:
//
//  * CommitConfig({layer1: image1}) ==> kNoFence1ConfigStamp
//  - Vsync now should have kNoFence1ConfigStamp
//  * CommitConfig({layer1: image2}) ==> kNoFence2ConfigStamp
//  - Vsync now should have kNoFence2ConfigStamp
//  * CommitConfig({}) ==> kNoImageConfigStamp
//  - Vsync now should have kNoImageConfigStamp
TEST_F(IntegrationTest, VsyncReflectsAppliedConfig) {
  // Create and bind primary client.
  std::unique_ptr<TestFidlClient> primary_client = OpenCoordinatorTestFidlClient(
      &sysmem_client_, DisplayProviderClient(), display::ClientPriority::kCompositor);
  WaitUntil([&]() { return primary_client->state().has_display_ownership(); });

  zx::result<display::LayerId> create_color_layer_result =
      primary_client->CreateFullscreenColorLayer(kFuchsiaBgra);
  ASSERT_OK(create_color_layer_result);
  display::LayerId color_layer_id = create_color_layer_result.value();

  // Apply a config so the client starts receiving VSync events.
  static constexpr display::ConfigStamp kInitialConfigStamp(1);
  ASSERT_EQ(display::kInvalidDriverConfigStamp, DisplayEngineSubmittedConfigStamp());
  ASSERT_OK(primary_client->ApplyLayers(kInitialConfigStamp, {{.layer_id = color_layer_id}}));
  WaitUntil(
      [&]() { return DisplayEngineSubmittedConfigStamp() != display::kInvalidDriverConfigStamp; });
  const display::DriverConfigStamp initial_driver_config_stamp =
      DisplayEngineSubmittedConfigStamp();

  ASSERT_EQ(0u, primary_client->state().vsync_count());
  TriggerDisplayEngineVsync();
  WaitUntil([&]() { return primary_client->state().vsync_count() >= 1; });
  EXPECT_EQ(kInitialConfigStamp, primary_client->state().last_vsync_config_stamp());
  EXPECT_EQ(1u, primary_client->state().vsync_count());

  zx::result<display::LayerId> create_layer1_result = primary_client->CreateFullscreenImageLayer();
  zx::result<display::ImageId> create_image1_result = primary_client->CreateFullscreenImage();
  zx::result<display::ImageId> create_image2_result = primary_client->CreateFullscreenImage();

  ASSERT_OK(create_layer1_result);
  ASSERT_OK(create_image1_result);
  ASSERT_OK(create_image2_result);

  display::LayerId layer1_id = create_layer1_result.value();
  display::ImageId image1_id = create_image1_result.value();
  display::ImageId image2_id = create_image2_result.value();

  // Present one single image without wait.
  static constexpr display::ConfigStamp kNoFence1ConfigStamp(2);
  ASSERT_EQ(initial_driver_config_stamp, DisplayEngineSubmittedConfigStamp());
  ASSERT_OK(primary_client->ApplyLayers(kNoFence1ConfigStamp,
                                        {{.layer_id = layer1_id, .image_id = image1_id}}));
  WaitUntil([&]() { return DisplayEngineSubmittedConfigStamp() > initial_driver_config_stamp; });
  const display::DriverConfigStamp no_fence1_driver_config_stamp =
      DisplayEngineSubmittedConfigStamp();

  ASSERT_EQ(1u, primary_client->state().vsync_count());
  TriggerDisplayEngineVsync();
  WaitUntil([&]() { return primary_client->state().vsync_count() >= 2; });
  EXPECT_EQ(kNoFence1ConfigStamp, primary_client->state().last_vsync_config_stamp());
  EXPECT_EQ(2u, primary_client->state().vsync_count());

  // Present another image layer without a wait fence.
  static constexpr display::ConfigStamp kNoFence2ConfigStamp(3);
  ASSERT_EQ(no_fence1_driver_config_stamp, DisplayEngineSubmittedConfigStamp());
  ASSERT_OK(primary_client->ApplyLayers(kNoFence2ConfigStamp,
                                        {{.layer_id = layer1_id, .image_id = image2_id}}));
  WaitUntil([&]() { return DisplayEngineSubmittedConfigStamp() > no_fence1_driver_config_stamp; });
  const display::DriverConfigStamp no_fence2_driver_config_stamp =
      DisplayEngineSubmittedConfigStamp();

  ASSERT_EQ(2u, primary_client->state().vsync_count());
  TriggerDisplayEngineVsync();
  WaitUntil([&]() { return primary_client->state().vsync_count() >= 3; });
  EXPECT_EQ(kNoFence2ConfigStamp, primary_client->state().last_vsync_config_stamp());
  EXPECT_EQ(3u, primary_client->state().vsync_count());

  // Hide the image layer and replace it with a color layer.
  static constexpr display::ConfigStamp kNoImageConfigStamp(4);
  ASSERT_EQ(no_fence2_driver_config_stamp, DisplayEngineSubmittedConfigStamp());
  ASSERT_OK(primary_client->ApplyLayers(kNoImageConfigStamp, {{.layer_id = color_layer_id}}));
  WaitUntil([&]() { return DisplayEngineSubmittedConfigStamp() > no_fence2_driver_config_stamp; });

  ASSERT_EQ(3u, primary_client->state().vsync_count());
  TriggerDisplayEngineVsync();
  WaitUntil([&]() { return primary_client->state().vsync_count() >= 4; });
  EXPECT_EQ(kNoImageConfigStamp, primary_client->state().last_vsync_config_stamp());
  EXPECT_EQ(4u, primary_client->state().vsync_count());
}

// This test case covers CommitConfig() with configurations that include waiting
// images. This matches the usage pattern of Scenic with GPU composition.
//
// When applying configurations with waiting images, the ConfigStamp reported by
// VSync events should match the latest committed configuration that doesn't have
// any waiting image.
//
// Here we test the following case:
//
//  * CommitConfig({layer1: image_without_fence}) ==> kImageWithoutFenceConfigStamp
//  - Vsync now should have kImageWithoutFenceConfigStamp
//  * CommitConfig({layer1: image_with_fence}) ==> kImageWithFenceConfigStamp
//  - Vsync now should have kImageWithoutFenceConfigStamp
//  * Signal kImageWithoutFenceConfigStamp
//  - Vsync now should have kImageWithFenceConfigStamp
TEST_F(IntegrationTest, CommitConfigWithWaitingImage) {
  // Create and bind primary client.
  std::unique_ptr<TestFidlClient> primary_client = OpenCoordinatorTestFidlClient(
      &sysmem_client_, DisplayProviderClient(), display::ClientPriority::kCompositor);
  WaitUntil([&]() { return primary_client->state().has_display_ownership(); });

  zx::result<display::LayerId> create_color_layer_result =
      primary_client->CreateFullscreenColorLayer(kFuchsiaBgra);
  ASSERT_OK(create_color_layer_result);
  display::LayerId color_layer_id = create_color_layer_result.value();

  // Apply a config so the client starts receiving VSync events.
  static constexpr display::ConfigStamp kInitialConfigStamp(1);
  ASSERT_EQ(display::kInvalidDriverConfigStamp, DisplayEngineSubmittedConfigStamp());
  ASSERT_OK(primary_client->ApplyLayers(kInitialConfigStamp, {{.layer_id = color_layer_id}}));
  WaitUntil(
      [&]() { return DisplayEngineSubmittedConfigStamp() != display::kInvalidDriverConfigStamp; });
  const display::DriverConfigStamp initial_driver_config_stamp =
      DisplayEngineSubmittedConfigStamp();

  ASSERT_EQ(0u, primary_client->state().vsync_count());
  TriggerDisplayEngineVsync();
  WaitUntil([&]() { return primary_client->state().vsync_count() >= 1; });
  EXPECT_EQ(kInitialConfigStamp, primary_client->state().last_vsync_config_stamp());
  EXPECT_EQ(1u, primary_client->state().vsync_count());

  zx::result<display::LayerId> create_layer1_result = primary_client->CreateFullscreenImageLayer();
  zx::result<display::ImageId> create_image_without_fence_result =
      primary_client->CreateFullscreenImage();
  zx::result<display::ImageId> create_image_with_fence_result =
      primary_client->CreateFullscreenImage();
  zx::result<TestFidlClient::EventInfo> create_image_ready_fence_result =
      primary_client->CreateEvent();

  ASSERT_OK(create_layer1_result);
  ASSERT_OK(create_image_without_fence_result);
  ASSERT_OK(create_image_with_fence_result);
  ASSERT_OK(create_image_ready_fence_result);

  display::LayerId layer1_id = create_layer1_result.value();
  display::ImageId image_without_fence_id = create_image_without_fence_result.value();
  display::ImageId image_with_fence_id = create_image_with_fence_result.value();
  TestFidlClient::EventInfo image_ready_fence = std::move(create_image_ready_fence_result.value());

  // Present one image layer without a wait event.
  static constexpr display::ConfigStamp kImageWithoutFenceConfigStamp(2);
  ASSERT_EQ(initial_driver_config_stamp, DisplayEngineSubmittedConfigStamp());
  ASSERT_OK(
      primary_client->ApplyLayers(kImageWithoutFenceConfigStamp,
                                  {{.layer_id = layer1_id, .image_id = image_without_fence_id}}));
  WaitUntil([&]() { return DisplayEngineSubmittedConfigStamp() > initial_driver_config_stamp; });
  const display::DriverConfigStamp image_without_fence_driver_config_stamp =
      DisplayEngineSubmittedConfigStamp();

  ASSERT_EQ(1u, primary_client->state().vsync_count());
  TriggerDisplayEngineVsync();
  WaitUntil([&]() { return primary_client->state().vsync_count() >= 2; });
  EXPECT_EQ(kImageWithoutFenceConfigStamp, primary_client->state().last_vsync_config_stamp());
  EXPECT_EQ(2u, primary_client->state().vsync_count());

  // Present an image layer whose image is not ready yet. The Display
  // Coordinator must wait on the fence. VSync events must report the previous
  // configuration.
  static constexpr display::ConfigStamp kImageWithFenceConfigStamp(3);
  ASSERT_OK(primary_client->ApplyLayers(kImageWithFenceConfigStamp,
                                        {{.layer_id = layer1_id,
                                          .image_id = image_with_fence_id,
                                          .image_ready_wait_event_id = image_ready_fence.id}}));

  ASSERT_EQ(2u, primary_client->state().vsync_count());
  TriggerDisplayEngineVsync();
  WaitUntil([&]() { return primary_client->state().vsync_count() >= 3; });
  EXPECT_EQ(kImageWithoutFenceConfigStamp, primary_client->state().last_vsync_config_stamp());
  EXPECT_EQ(3u, primary_client->state().vsync_count());

  // Signal the event, marking the image ready. The Coordinator must apply the
  // configuration kImageWithFenceConfigStamp, which includes the image that is
  // now ready. Once the configuration is applied, the next VSync must reflect
  // it.
  ASSERT_EQ(image_without_fence_driver_config_stamp, DisplayEngineSubmittedConfigStamp());
  image_ready_fence.event.signal(0u, ZX_EVENT_SIGNALED);
  WaitUntil([&]() {
    return DisplayEngineSubmittedConfigStamp() > image_without_fence_driver_config_stamp;
  });

  ASSERT_EQ(3u, primary_client->state().vsync_count());
  TriggerDisplayEngineVsync();
  WaitUntil([&]() { return primary_client->state().vsync_count() >= 4; });
  EXPECT_EQ(kImageWithFenceConfigStamp, primary_client->state().last_vsync_config_stamp());
  EXPECT_EQ(4u, primary_client->state().vsync_count());
}

// This test case covers CommitConfig() when a committed configuration removes a layer
// with a waiting image from a previously committed configuration.
//
// VSync events should never include the ConfigStamp of the configuration with the
// waiting image, because that image never becomes ready.
//
// Here we test the following case:
//
//  * CommitConfig({layer1: image_without_fence}) ==> kImageWithoutFenceConfigStamp
//  - Vsync now should have kImageWithoutFenceConfigStamp
//  * CommitConfig({layerA: img1, waiting on fence}) ==> kImageWithFenceConfigStamp
//  - Vsync now should have kImageWithoutFenceConfigStamp
//  * CommitConfig({}) ==> kNoImageConfigStamp
//  - Vsync now should have kNoImageConfigStamp
//
// The fence is never signaled.
TEST_F(IntegrationTest, CommitConfigRemovesLayerWithWaitingImage) {
  // Create and bind primary client.
  std::unique_ptr<TestFidlClient> primary_client = OpenCoordinatorTestFidlClient(
      &sysmem_client_, DisplayProviderClient(), display::ClientPriority::kCompositor);
  WaitUntil([&]() { return primary_client->state().has_display_ownership(); });

  zx::result<display::LayerId> create_color_layer_result =
      primary_client->CreateFullscreenColorLayer(kFuchsiaBgra);
  ASSERT_OK(create_color_layer_result);
  display::LayerId color_layer_id = create_color_layer_result.value();

  // Apply a config so the client starts receiving VSync events.
  static constexpr display::ConfigStamp kInitialConfigStamp(1);
  ASSERT_EQ(display::kInvalidDriverConfigStamp, DisplayEngineSubmittedConfigStamp());
  ASSERT_OK(primary_client->ApplyLayers(kInitialConfigStamp, {{.layer_id = color_layer_id}}));
  WaitUntil(
      [&]() { return DisplayEngineSubmittedConfigStamp() != display::kInvalidDriverConfigStamp; });
  const display::DriverConfigStamp initial_driver_config_stamp =
      DisplayEngineSubmittedConfigStamp();

  ASSERT_EQ(0u, primary_client->state().vsync_count());
  TriggerDisplayEngineVsync();
  WaitUntil([&]() { return primary_client->state().vsync_count() >= 1; });
  EXPECT_EQ(kInitialConfigStamp, primary_client->state().last_vsync_config_stamp());
  EXPECT_EQ(1u, primary_client->state().vsync_count());

  zx::result<display::LayerId> create_layer1_result = primary_client->CreateFullscreenImageLayer();
  zx::result<display::ImageId> create_image_without_fence_result =
      primary_client->CreateFullscreenImage();
  zx::result<display::ImageId> create_image_with_fence_result =
      primary_client->CreateFullscreenImage();
  zx::result<TestFidlClient::EventInfo> create_image_ready_fence_result =
      primary_client->CreateEvent();

  ASSERT_OK(create_layer1_result);
  ASSERT_OK(create_image_without_fence_result);
  ASSERT_OK(create_image_with_fence_result);
  ASSERT_OK(create_image_ready_fence_result);

  display::LayerId layer1_id = create_layer1_result.value();
  display::ImageId image_without_fence_id = create_image_without_fence_result.value();
  display::ImageId image_with_fence_id = create_image_with_fence_result.value();
  TestFidlClient::EventInfo image_ready_fence = std::move(create_image_ready_fence_result.value());

  // Present an image layer.
  static constexpr display::ConfigStamp kImageWithoutFenceConfigStamp(2);
  ASSERT_EQ(initial_driver_config_stamp, DisplayEngineSubmittedConfigStamp());
  ASSERT_OK(
      primary_client->ApplyLayers(kImageWithoutFenceConfigStamp,
                                  {{.layer_id = layer1_id, .image_id = image_without_fence_id}}));
  WaitUntil([&]() { return DisplayEngineSubmittedConfigStamp() > initial_driver_config_stamp; });
  const display::DriverConfigStamp image_without_fence_driver_config_stamp =
      DisplayEngineSubmittedConfigStamp();

  ASSERT_EQ(1u, primary_client->state().vsync_count());
  TriggerDisplayEngineVsync();
  WaitUntil([&]() { return primary_client->state().vsync_count() >= 2; });
  EXPECT_EQ(kImageWithoutFenceConfigStamp, primary_client->state().last_vsync_config_stamp());
  EXPECT_EQ(2u, primary_client->state().vsync_count());

  // Present an image layer whose image is not ready yet. The Display
  // Coordinator must wait on the event. VSync events must report the previous
  // configuration.
  static constexpr display::ConfigStamp kImageWithFenceConfigStamp(3);
  ASSERT_EQ(image_without_fence_driver_config_stamp, DisplayEngineSubmittedConfigStamp());
  ASSERT_OK(primary_client->ApplyLayers(kImageWithFenceConfigStamp,
                                        {{.layer_id = layer1_id,
                                          .image_id = image_with_fence_id,
                                          .image_ready_wait_event_id = image_ready_fence.id}}));

  ASSERT_EQ(2u, primary_client->state().vsync_count());
  TriggerDisplayEngineVsync();
  WaitUntil([&]() { return primary_client->state().vsync_count() >= 3; });
  EXPECT_EQ(kImageWithoutFenceConfigStamp, primary_client->state().last_vsync_config_stamp());
  EXPECT_EQ(3u, primary_client->state().vsync_count());

  // Replace the image layer with a solid color fill layer. The Coordinator must
  // "skip over" the image layer that is not ready, and apply the configuration
  // with the color layer.
  static constexpr display::ConfigStamp kNoImageConfigStamp(4);
  ASSERT_EQ(image_without_fence_driver_config_stamp, DisplayEngineSubmittedConfigStamp());
  ASSERT_OK(primary_client->ApplyLayers(kNoImageConfigStamp, {{.layer_id = color_layer_id}}));
  WaitUntil([&]() {
    return DisplayEngineSubmittedConfigStamp() > image_without_fence_driver_config_stamp;
  });

  // On Vsync, the configuration stamp client receives on Vsync event message
  // will be the latest one applied to the display controller, since the waiting
  // image has been removed from the configuration.
  ASSERT_EQ(3u, primary_client->state().vsync_count());
  TriggerDisplayEngineVsync();
  WaitUntil([&]() { return primary_client->state().vsync_count() >= 4; });
  EXPECT_EQ(kNoImageConfigStamp, primary_client->state().last_vsync_config_stamp());
  EXPECT_EQ(4u, primary_client->state().vsync_count());
}

// This test case covers CommitConfig() assigning two different waiting images to
// the same layer in two different committed configs. The second image becomes
// ready at some point, while the first image remains waiting forever.
//
// VSync events should never include the ConfigStamp of the configuration with
// the first waiting image. After the second image's waiting fence is signaled,
// the ConfigStamp for the configuration using that second image should be
// included in VSync events.
//
// Here we test the following case:
//
//  * CommitConfig({layer1: image_without_fence}) ==> kImageWithoutFenceConfigStamp
//  - Vsync now should have kImageWithoutFenceConfigStamp
//  * CommitConfig({layer1: image_with_fence1, waiting on fence1}) ==> kImageWithFence1ConfigStamp
//  - Vsync now should have kImageWithoutFenceConfigStamp since fence1 is not signaled
//  * CommitConfig({layerA: image_with_fence2, waiting on fence2}) ==> kImageWithFence2ConfigStamp
//  - Vsync now should have kImageWithoutFenceConfigStamp since fence1 and fence2 are not
//  signaled
//  * Signal fence2
//  - Vsync now should have kImageWithFence2ConfigStamp.
//  * Signal fence1
//  - Vsync should still have kImageWithFence2ConfigStamp.
//
// fence1, the first fence, is never signaled.
TEST_F(IntegrationTest, CommitConfigSkipsConfigWithWaitingImage) {
  // Create and bind primary client.
  std::unique_ptr<TestFidlClient> primary_client = OpenCoordinatorTestFidlClient(
      &sysmem_client_, DisplayProviderClient(), display::ClientPriority::kCompositor);
  WaitUntil([&]() { return primary_client->state().has_display_ownership(); });

  zx::result<display::LayerId> create_layer1_result = primary_client->CreateFullscreenImageLayer();
  zx::result<display::ImageId> create_image_without_fence_result =
      primary_client->CreateFullscreenImage();
  zx::result<display::ImageId> create_image_with_fence1_result =
      primary_client->CreateFullscreenImage();
  zx::result<display::ImageId> create_image_with_fence2_result =
      primary_client->CreateFullscreenImage();
  zx::result<TestFidlClient::EventInfo> create_image_ready_fence1_result =
      primary_client->CreateEvent();
  zx::result<TestFidlClient::EventInfo> create_image_ready_fence2_result =
      primary_client->CreateEvent();

  ASSERT_OK(create_layer1_result);
  ASSERT_OK(create_image_without_fence_result);
  ASSERT_OK(create_image_with_fence1_result);
  ASSERT_OK(create_image_with_fence2_result);
  ASSERT_OK(create_image_ready_fence1_result);
  ASSERT_OK(create_image_ready_fence2_result);

  display::LayerId layer1_id = create_layer1_result.value();
  display::ImageId image_without_fence_id = create_image_without_fence_result.value();
  display::ImageId image_with_fence1_id = create_image_with_fence1_result.value();
  display::ImageId image_with_fence2_id = create_image_with_fence2_result.value();
  TestFidlClient::EventInfo image_ready_fence1 =
      std::move(create_image_ready_fence1_result.value());
  TestFidlClient::EventInfo image_ready_fence2 =
      std::move(create_image_ready_fence2_result.value());

  // Apply a config so the client starts receiving VSync events.
  static constexpr display::ConfigStamp kImageWithoutFenceConfigStamp(2);
  ASSERT_EQ(display::kInvalidDriverConfigStamp, DisplayEngineSubmittedConfigStamp());
  ASSERT_OK(
      primary_client->ApplyLayers(kImageWithoutFenceConfigStamp,
                                  {{.layer_id = layer1_id, .image_id = image_without_fence_id}}));
  WaitUntil(
      [&]() { return DisplayEngineSubmittedConfigStamp() != display::kInvalidDriverConfigStamp; });
  const display::DriverConfigStamp image_without_fence_driver_config_stamp =
      DisplayEngineSubmittedConfigStamp();

  ASSERT_EQ(0u, primary_client->state().vsync_count());
  TriggerDisplayEngineVsync();
  WaitUntil([&]() { return primary_client->state().vsync_count() >= 1; });
  EXPECT_EQ(kImageWithoutFenceConfigStamp, primary_client->state().last_vsync_config_stamp());
  EXPECT_EQ(1u, primary_client->state().vsync_count());

  // Present an image layer whose image is not ready yet. The Display
  // Coordinator must wait on the event. VSync events must report the previous
  // configuration.
  static constexpr display::ConfigStamp kImageWithFence1ConfigStamp(3);
  ASSERT_OK(primary_client->ApplyLayers(kImageWithFence1ConfigStamp,
                                        {{.layer_id = layer1_id,
                                          .image_id = image_with_fence1_id,
                                          .image_ready_wait_event_id = image_ready_fence1.id}}));

  ASSERT_EQ(1u, primary_client->state().vsync_count());
  TriggerDisplayEngineVsync();
  WaitUntil([&]() { return primary_client->state().vsync_count() >= 2; });
  EXPECT_EQ(kImageWithoutFenceConfigStamp, primary_client->state().last_vsync_config_stamp());
  EXPECT_EQ(2u, primary_client->state().vsync_count());

  // Present another image layer whose image is not ready. By the same
  // reasoning as above, VSync events must still report the same
  // configuration as above.
  static constexpr display::ConfigStamp kImageWithFence2ConfigStamp(4);
  ASSERT_OK(primary_client->ApplyLayers(kImageWithFence2ConfigStamp,
                                        {{.layer_id = layer1_id,
                                          .image_id = image_with_fence2_id,
                                          .image_ready_wait_event_id = image_ready_fence2.id}}));

  ASSERT_EQ(2u, primary_client->state().vsync_count());
  TriggerDisplayEngineVsync();
  WaitUntil([&]() { return primary_client->state().vsync_count() >= 3; });
  EXPECT_EQ(kImageWithoutFenceConfigStamp, primary_client->state().last_vsync_config_stamp());
  EXPECT_EQ(3u, primary_client->state().vsync_count());

  // Signal the second image's event. The Coordinator must "skip over" the
  // configuration that includes the first image, and apply the configuration
  // that includes the second image. Once the configuration is applied, the next
  // VSync must reflect it.
  ASSERT_EQ(image_without_fence_driver_config_stamp, DisplayEngineSubmittedConfigStamp());
  image_ready_fence2.event.signal(0u, ZX_EVENT_SIGNALED);
  WaitUntil([&]() {
    return DisplayEngineSubmittedConfigStamp() > image_without_fence_driver_config_stamp;
  });
  const display::DriverConfigStamp image_with_fence2_driver_config_stamp =
      DisplayEngineSubmittedConfigStamp();

  ASSERT_EQ(3u, primary_client->state().vsync_count());
  TriggerDisplayEngineVsync();
  WaitUntil([&]() { return primary_client->state().vsync_count() >= 4; });
  EXPECT_EQ(kImageWithFence2ConfigStamp, primary_client->state().last_vsync_config_stamp());
  EXPECT_EQ(4u, primary_client->state().vsync_count());

  // Signal the first image's event. Since we have displayed a newer image, signaling the
  // old event associated with the old image shouldn't trigger ReapplyConfig().
  // We should still see |apply_config_stamp_2| as the latest presented config
  // stamp in the client.
  image_ready_fence1.event.signal(0u, ZX_EVENT_SIGNALED);

  // TODO(https://fxbug.dev/388885807): This check can have a false positive
  // pass, due to using a hard-coded timeout.
  {
    zx::time_monotonic deadline = zx::deadline_after(zx::sec(1));
    WaitUntil([&]() {
      if (zx::clock::get_monotonic() >= deadline)
        return true;
      return DisplayEngineSubmittedConfigStamp() > image_with_fence2_driver_config_stamp;
    });
  }
  EXPECT_EQ(image_with_fence2_driver_config_stamp, DisplayEngineSubmittedConfigStamp());

  ASSERT_EQ(4u, primary_client->state().vsync_count());
  TriggerDisplayEngineVsync();
  WaitUntil([&]() { return primary_client->state().vsync_count() >= 5; });
  EXPECT_EQ(kImageWithFence2ConfigStamp, primary_client->state().last_vsync_config_stamp());
  EXPECT_EQ(5u, primary_client->state().vsync_count());
}

// TODO(https://fxbug.dev/42171874): Currently the fake-display driver only supports one
// primary layer. In order to better test CommitConfig() / OnVsync() behavior,
// we should make fake-display driver support multi-layer configurations and
// then we could add more multi-layer tests.

TEST_F(IntegrationTest, CheckConfigFullscreenImageLayer) {
  // Create and bind primary client.
  std::unique_ptr<TestFidlClient> primary_client = OpenCoordinatorTestFidlClient(
      &sysmem_client_, DisplayProviderClient(), display::ClientPriority::kCompositor);
  WaitUntil([&]() { return primary_client->state().has_display_ownership(); });

  zx::result<display::LayerId> create_layer_result = primary_client->CreateLayer();
  ASSERT_OK(create_layer_result);
  display::LayerId layer_id = create_layer_result.value();
  ASSERT_OK(primary_client->SetDisplayLayers(primary_client->state().display_id(),
                                             {{.layer_id = layer_id}}));

  // TODO(https://fxbug.dev/42068661): This test would be easier to understand
  // if the fake display resolution was hardcoded into the test.
  const display::ImageMetadata image_metadata = primary_client->state().FullscreenImageMetadata();
  ASSERT_OK(primary_client->SetLayerPrimaryConfig(layer_id, image_metadata));

  zx::result<display::ConfigCheckResult> check_config_result = primary_client->CheckConfig();
  ASSERT_OK(check_config_result);
  EXPECT_EQ(display::ConfigCheckResult::kOk, check_config_result.value());
}

TEST_F(IntegrationTest, CheckConfigLargeLayer) {
  std::unique_ptr<TestFidlClient> primary_client = OpenCoordinatorTestFidlClient(
      &sysmem_client_, DisplayProviderClient(), display::ClientPriority::kCompositor);
  WaitUntil([&]() { return primary_client->state().has_display_ownership(); });

  zx::result<display::LayerId> create_layer_result = primary_client->CreateLayer();
  ASSERT_OK(create_layer_result);
  display::LayerId layer_id = create_layer_result.value();
  ASSERT_OK(primary_client->SetDisplayLayers(primary_client->state().display_id(),
                                             {{.layer_id = layer_id}}));

  // The ImageMetadata instance below is larger than the display size.
  // SetLayerPrimaryConfig() uses the metadata to size the layer's destination,
  // resulting in a layer that cannot possibly fit the display area. So, the
  // configuration that uses the layer is guaranteed to be invalid.
  //
  // TODO(https://fxbug.dev/42068661): This test would be easier to understand
  // if the fake display resolution was hardcoded into the test.
  const display::ImageMetadata large_image_metadata({
      .width = primary_client->state().FullscreenImageMetadata().width() * 2,
      .height = primary_client->state().FullscreenImageMetadata().height() * 2,
      .tiling_type = display::ImageTilingType::kLinear,
  });
  ASSERT_OK(primary_client->SetLayerPrimaryConfig(layer_id, large_image_metadata));

  zx::result<display::ConfigCheckResult> check_config_result = primary_client->CheckConfig();
  ASSERT_OK(check_config_result);
  EXPECT_EQ(display::ConfigCheckResult::kInvalidConfig, check_config_result.value());
}

TEST_F(IntegrationTest, CheckConfigRepeatedLayer) {
  // Create and bind primary client.
  std::unique_ptr<TestFidlClient> primary_client = OpenCoordinatorTestFidlClient(
      &sysmem_client_, DisplayProviderClient(), display::ClientPriority::kCompositor);
  WaitUntil([&]() { return primary_client->state().has_display_ownership(); });

  zx::result<display::LayerId> create_layer_result = primary_client->CreateLayer();
  ASSERT_OK(create_layer_result);
  display::LayerId layer_id = create_layer_result.value();
  ASSERT_OK(primary_client->SetDisplayLayers(primary_client->state().display_id(),
                                             {{.layer_id = layer_id}, {.layer_id = layer_id}}));

  // TODO(https://fxbug.dev/42068661): This test would be easier to understand
  // if the fake display resolution was hardcoded into the test.
  const display::ImageMetadata image_metadata = primary_client->state().FullscreenImageMetadata();
  ASSERT_OK(primary_client->SetLayerPrimaryConfig(layer_id, image_metadata));

  zx::result<display::ConfigCheckResult> check_config_result = primary_client->CheckConfig();
  EXPECT_STATUS(zx::error(ZX_ERR_PEER_CLOSED), check_config_result);
}

TEST_F(IntegrationTest, SetDisplayLayersTooManyLayersDisconnects) {
  // Create and bind primary client.
  std::unique_ptr<TestFidlClient> primary_client = OpenCoordinatorTestFidlClient(
      &sysmem_client_, DisplayProviderClient(), display::ClientPriority::kCompositor);
  WaitUntil([&]() { return primary_client->state().has_display_ownership(); });

  ASSERT_EQ(1u, primary_client->state().GetDisplayInfo().max_layer_count)
      << "This test assumes that fake-display is configured to support 1 layer";

  zx::result<display::LayerId> create_layer1_result = primary_client->CreateLayer();
  ASSERT_OK(create_layer1_result);
  zx::result<display::LayerId> create_layer2_result = primary_client->CreateLayer();
  ASSERT_OK(create_layer2_result);

  // The Coordinator closes the channel after this call, because it violates the
  // layer count precondition.
  zx::result<> result = primary_client->SetDisplayLayers(
      primary_client->state().display_id(),
      {{.layer_id = create_layer1_result.value()}, {.layer_id = create_layer2_result.value()}});
  ASSERT_OK(result);

  // Issue a synchronous call to verify that the Coordinator closed the channel.
  zx::result<display::ConfigStamp> get_stamp_result = primary_client->GetLastCommittedConfigStamp();
  EXPECT_STATUS(zx::error(ZX_ERR_PEER_CLOSED), get_stamp_result);
}

}  // namespace

}  // namespace display_coordinator

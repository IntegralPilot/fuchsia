// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_CLIENT_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_CLIENT_H_

#include <fidl/fuchsia.hardware.display.types/cpp/wire.h>
#include <fidl/fuchsia.hardware.display/cpp/wire.h>
#include <lib/inspect/cpp/vmo/types.h>
#include <lib/zx/time.h>
#include <zircon/types.h>

#include <cstdint>
#include <map>
#include <span>

#include <fbl/auto_lock.h>
#include <fbl/intrusive_double_list.h>
#include <fbl/ref_ptr.h>
#include <fbl/ring_buffer.h>
#include <fbl/vector.h>

#include "src/graphics/display/drivers/coordinator/capture-image.h"
#include "src/graphics/display/drivers/coordinator/client-id.h"
#include "src/graphics/display/drivers/coordinator/client-vsync-queue.h"
#include "src/graphics/display/drivers/coordinator/controller.h"
#include "src/graphics/display/drivers/coordinator/display-config.h"
#include "src/graphics/display/drivers/coordinator/fence.h"
#include "src/graphics/display/drivers/coordinator/id-map.h"
#include "src/graphics/display/drivers/coordinator/image.h"
#include "src/graphics/display/drivers/coordinator/layer.h"
#include "src/graphics/display/lib/api-types/cpp/buffer-collection-id.h"
#include "src/graphics/display/lib/api-types/cpp/client-priority.h"
#include "src/graphics/display/lib/api-types/cpp/config-check-result.h"
#include "src/graphics/display/lib/api-types/cpp/config-stamp.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-buffer-collection-id.h"
#include "src/graphics/display/lib/api-types/cpp/event-id.h"
#include "src/graphics/display/lib/api-types/cpp/image-id.h"
#include "src/graphics/display/lib/api-types/cpp/layer-id.h"
#include "src/graphics/display/lib/api-types/cpp/vsync-ack-cookie.h"

namespace display_coordinator {

// Manages the state associated with a display coordinator client connection.
//
// This class is not thread-safe. The constructor, destructor and all methods
// must run on the coordinator driver dispatcher.
class Client final : public fidl::WireServer<fuchsia_hardware_display::Coordinator>,
                     public FenceListener {
 public:
  // `controller` must outlive the newly created client.
  // `priority`, `client_id`, `coordinator_server_end` and
  // `coordinator_listener_client_end` must be valid.
  Client(Controller* controller, display::ClientPriority priority, ClientId client_id,
         fidl::ServerEnd<fuchsia_hardware_display::Coordinator> coordinator_server_end,
         fidl::ClientEnd<fuchsia_hardware_display::CoordinatorListener>
             coordinator_listener_client_end);

  Client(const Client&) = delete;
  Client& operator=(const Client&) = delete;

  ~Client() override;

  // Must be called exactly once.
  void AttachInspectNode(inspect::Node client_node);

  void OnDisplayVsync(display::DisplayId display_id, zx_instant_mono_t timestamp,
                      display::DriverConfigStamp driver_config_stamp);
  void OnCaptureComplete();
  void SubmitSpecialConfigs();

  inspect::Node& node() { return node_; }

  struct ConfigStampPair {
    display::DriverConfigStamp driver_stamp;
    display::ConfigStamp client_stamp;
  };
  std::list<ConfigStampPair>& pending_displayed_config_stamps() {
    return pending_displayed_config_stamps_;
  }
  void UpdateConfigStampMapping(ConfigStampPair stamps);

  static constexpr uint32_t kMaxImageHandles = 8;

  void OnDisplaysChanged(std::span<const display::DisplayId> added_display_ids,
                         std::span<const display::DisplayId> removed_display_ids);
  void SetOwnership(bool is_owner);

  void NotifyDisplayChanges(
      std::span<const fuchsia_hardware_display::wire::Info> added_display_infos,
      std::span<const fuchsia_hardware_display_types::wire::DisplayId> removed_display_ids);
  void NotifyOwnershipChange(bool client_has_ownership);
  void NotifyVsync(display::DisplayId display_id, zx::time_monotonic timestamp,
                   display::ConfigStamp config_stamp, display::VsyncAckCookie vsync_ack_cookie);

  // Submits the latest committed configuration.
  //
  // Called when the client gains ownership of the displays.
  //
  // This method is a no-op if the Client instance has not committed any
  // configuration.
  void SubmitLastCommittedConfig();

  // `FenceListener`:
  void OnFenceSignaled(Fence& fence) override;

  // Closes the FIDL connection.
  //
  // Called when the Coordinator is shutting down. Under normal operation, the Coordinator
  // services a client until it closes the FIDL connection.
  void CloseFidlConnection(zx_status_t epitaph);

  // Releases resources allocated for this client.
  //
  // Must be called before the Client instance is destroyed.
  void ReleaseResources();

  ClientId id() const { return id_; }
  display::ClientPriority priority() const { return priority_; }
  void CaptureCompleted();

  uint8_t GetMinimumRgb() const { return client_minimum_rgb_; }

  // fidl::WireServer<fuchsia_hardware_display::Coordinator> overrides:
  void ImportImage(ImportImageRequestView request, ImportImageCompleter::Sync& completer) override;
  void ReleaseImage(ReleaseImageRequestView request,
                    ReleaseImageCompleter::Sync& completer) override;
  void ImportEvent(ImportEventRequestView request, ImportEventCompleter::Sync& completer) override;
  void ReleaseEvent(ReleaseEventRequestView request,
                    ReleaseEventCompleter::Sync& completer) override;
  void CreateLayer(CreateLayerRequestView request, CreateLayerCompleter::Sync& completer) override;
  void DestroyLayer(DestroyLayerRequestView request,
                    DestroyLayerCompleter::Sync& completer) override;
  void SetDisplayMode(SetDisplayModeRequestView request,
                      SetDisplayModeCompleter::Sync& completer) override;
  void SetDisplayColorConversion(SetDisplayColorConversionRequestView request,
                                 SetDisplayColorConversionCompleter::Sync& completer) override;
  void SetDisplayLayers(SetDisplayLayersRequestView request,
                        SetDisplayLayersCompleter::Sync& completer) override;
  void SetLayerPrimaryConfig(SetLayerPrimaryConfigRequestView request,
                             SetLayerPrimaryConfigCompleter::Sync& completer) override;
  void SetLayerPrimaryPosition(SetLayerPrimaryPositionRequestView request,
                               SetLayerPrimaryPositionCompleter::Sync& completer) override;
  void SetLayerPrimaryAlpha(SetLayerPrimaryAlphaRequestView request,
                            SetLayerPrimaryAlphaCompleter::Sync& completer) override;
  void SetLayerColorConfig(SetLayerColorConfigRequestView request,
                           SetLayerColorConfigCompleter::Sync& completer) override;
  void SetLayerImage2(SetLayerImage2RequestView request,
                      SetLayerImage2Completer::Sync& completer) override;
  void CheckConfig(CheckConfigCompleter::Sync& completer) override;
  void DiscardConfig(DiscardConfigCompleter::Sync& completer) override;
  void CommitConfig(CommitConfigRequestView request,
                    CommitConfigCompleter::Sync& completer) override;
  void GetLatestCommittedConfigStamp(
      GetLatestCommittedConfigStampCompleter::Sync& completer) override;

  void ImportBufferCollection(ImportBufferCollectionRequestView request,
                              ImportBufferCollectionCompleter::Sync& completer) override;
  void SetBufferCollectionConstraints(
      SetBufferCollectionConstraintsRequestView request,
      SetBufferCollectionConstraintsCompleter::Sync& completer) override;
  void ReleaseBufferCollection(ReleaseBufferCollectionRequestView request,
                               ReleaseBufferCollectionCompleter::Sync& completer) override;

  void IsCaptureSupported(IsCaptureSupportedCompleter::Sync& completer) override;

  void StartCapture(StartCaptureRequestView request,
                    StartCaptureCompleter::Sync& completer) override;

  void AcknowledgeVsync(AcknowledgeVsyncRequestView request,
                        AcknowledgeVsyncCompleter::Sync& completer) override;

  void SetMinimumRgb(SetMinimumRgbRequestView request,
                     SetMinimumRgbCompleter::Sync& completer) override;

  void SetDisplayPowerMode(SetDisplayPowerModeRequestView request,
                           SetDisplayPowerModeCompleter::Sync& completer) override;

 private:
  display::ConfigCheckResult CheckConfigImpl();

  // Called when the client closes the FIDL channel to the Coordinator.
  void OnClientFidlBindingClosed(fidl::UnbindInfo unbind_info);

  // Submits the client's current (most recent) committed configuration.
  //
  // The client must have a committed configuration.
  void SubmitConfig();

  // CheckConfig() implementation for a single display configuration.
  //
  // `display_config`'s draft configuration must have a non-empty layer list.
  display::ConfigCheckResult CheckConfigForDisplay(
      const DisplayConfig& display_config, std::span<const display::ModeAndId> preferred_modes);

  // Cleans up states of all current Images.
  // Returns true if any current layer has been modified.
  bool CleanUpAllImages();

  // Cleans up layer state associated with an Image. `image` must be valid.
  // Returns true if a current layer has been modified.
  bool CleanUpImage(Image& image);
  void CleanUpCaptureImage(display::ImageId id);

  // Displays' draft layers list may have been changed by SetDisplayLayers().
  //
  // Restores the draft layer lists of all the displays to their applied layer
  // list state respectively, undoing all draft changes to the layer lists.
  void SetAllConfigDraftLayersToCommittedLayers();

  // `fuchsia.hardware.display/Coordinator.ImportImage()` helper for display
  // images.
  //
  // `image_id` must be unused and `image_metadata` contains metadata for an
  // image used for display.
  zx_status_t ImportImageForDisplay(const display::ImageMetadata& image_metadata,
                                    display::BufferCollectionId buffer_collection_id,
                                    uint32_t buffer_index, display::ImageId image_id);

  // `fuchsia.hardware.display/Coordinator.ImportImage()` helper for capture
  // images.
  //
  // `image_id` must be unused and `image_metadata` contains metadata for an
  // image used for capture.
  zx_status_t ImportImageForCapture(const display::ImageMetadata& image_metadata,
                                    display::BufferCollectionId buffer_collection_id,
                                    uint32_t buffer_index, display::ImageId image_id);

  // Discards all the draft configs on all displays and layers.
  void DiscardConfig();

  void DrainVsyncQueue();

  ClientVsyncQueue vsync_queue_;
  bool enable_capture_ = false;
  std::list<ConfigStampPair> pending_displayed_config_stamps_;
  inspect::Node node_;
  inspect::BoolProperty is_owner_property_;

  Controller& controller_;
  const display::ClientPriority priority_;
  const ClientId id_;

  bool attach_inspect_node_called_ = false;
  bool release_resources_called_ = false;

  Image::Map images_;
  CaptureImage::Map capture_images_;

  // Maps each known display ID to this client's display config.
  //
  // The client's knowledge of the connected displays can fall out of sync with
  // this map. This is because the map is modified when the Coordinator
  // processes display change events from display engine drivers, which happens
  // before the client receives the display driver.
  DisplayConfig::Map display_configs_;

  // True iff CheckConfig() succeeded on the draft configuration.
  //
  // Set to false any time when the client modifies the draft configuration. Set
  // to true when the client calls CheckConfig() and the check passes.
  bool draft_display_config_was_validated_ = false;

  bool is_owner_ = false;

  // A counter for the number of times the client has successfully applied
  // a configuration. This does not account for changes due to waiting images.
  display::ConfigStamp latest_config_stamp_ = display::kInvalidConfigStamp;

  // This is the client's clamped RGB value.
  uint8_t client_minimum_rgb_ = 0;

  struct Collections {
    // The BufferCollection ID used in fuchsia.hardware.display.Controller
    // protocol.
    display::DriverBufferCollectionId driver_buffer_collection_id;
  };
  std::map<display::BufferCollectionId, Collections> collection_map_;

  FenceCollection fences_;

  Layer::Map layers_;

  fidl::ServerBinding<fuchsia_hardware_display::Coordinator> binding_;
  fidl::WireSyncClient<fuchsia_hardware_display::CoordinatorListener> coordinator_listener_;

  // Capture related bookkeeping.
  display::EventId capture_fence_id_ = display::kInvalidEventId;

  // Points to the image whose contents is modified by the current capture.
  //
  // Invalid when no is capture in progress.
  display::ImageId current_capture_image_id_ = display::kInvalidImageId;

  // Tracks an image released by the client while used by a capture.
  //
  // The coordinator must ensure that an image remains valid while a display
  // engine is writing to it. If a client attempts to release the image used by
  // an in-progress capture, we defer the release operation until the capture
  // completes. The deferred release is tracked here.
  display::ImageId pending_release_capture_image_id_ = display::kInvalidImageId;
};

}  // namespace display_coordinator

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_CLIENT_H_

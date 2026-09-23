// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_DISPLAY_TESTS_MOCK_DISPLAY_COORDINATOR_H_
#define SRC_UI_SCENIC_LIB_DISPLAY_TESTS_MOCK_DISPLAY_COORDINATOR_H_

#include <fidl/fuchsia.hardware.display.types/cpp/fidl.h>
#include <fidl/fuchsia.hardware.display/cpp/wire_test_base.h>
#include <fidl/fuchsia.math/cpp/fidl.h>
#include <lib/fidl/cpp/binding_set.h>
#include <lib/syslog/cpp/macros.h>

#include <unordered_set>

#include "src/ui/scenic/lib/display/fidl_typedefs.h"

namespace display::test {

class MockDisplayCoordinator
    : public fidl::testing::WireTestBase<fuchsia_hardware_display::Coordinator> {
 public:
  using ImportImageFn =
      std::function<void(fuchsia_hardware_display::wire::CoordinatorImportImageRequest*)>;
  using ReleaseImageFn =
      std::function<void(fuchsia_hardware_display::wire::CoordinatorReleaseImageRequest*)>;
  using ImportEventFn = std::function<void(zx::event event, WireEventId event_id)>;
  using ReleaseEventFn =
      std::function<void(fuchsia_hardware_display::wire::CoordinatorReleaseEventRequest*)>;
  using CreateLayerFn = std::function<void(WireLayerId)>;
  using DestroyLayerFn =
      std::function<void(fuchsia_hardware_display::wire::CoordinatorDestroyLayerRequest*)>;
  using SetDisplayModeFn = std::function<void(WireDisplayId, WireDisplayMode)>;
  using SetDisplayColorConversionFn = std::function<void(
      display::WireDisplayId, fidl::Array<float, 3>, fidl::Array<float, 9>, fidl::Array<float, 3>)>;
  using SetDisplayLayersFn = std::function<void(WireDisplayId, fidl::VectorView<WireLayerId>)>;
  using SetLayerPrimaryConfigFn = std::function<void(WireLayerId, WireImageMetadata)>;
  using SetLayerPrimaryPositionFn =
      std::function<void(WireLayerId, WireCoordinateTransformation, WireRectU, WireRectU)>;
  using SetLayerPrimaryAlphaFn = std::function<void(WireLayerId, WireAlphaMode, float)>;
  using SetLayerColorConfigFn = std::function<void(WireLayerId, WireColor, WireRectU)>;
  using SetLayerImage2Fn = std::function<void(WireLayerId, WireImageId, WireEventId)>;
  using CheckConfigFn = std::function<void(WireConfigResult*)>;
  using DiscardConfigFn = std::function<void()>;
  using AcknowledgeVsyncFn = std::function<void(uint64_t cookie)>;
  using SetMinimumRgbFn = std::function<void(uint8_t)>;
  using SetDisplayPowerModeFn =
      std::function<void(fuchsia_hardware_display::wire::CoordinatorSetDisplayPowerModeRequest*)>;

  explicit MockDisplayCoordinator(WireDisplayInfo display_info);
  ~MockDisplayCoordinator() override;

  // `fidl::testing::TestBase<fuchsia_hardware_display::Coordinator>`:
  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) final {}

  // Methods in FIDL order
  void ImportImage(fuchsia_hardware_display::wire::CoordinatorImportImageRequest* request,
                   ImportImageCompleter::Sync& completer) override;
  void ReleaseImage(fuchsia_hardware_display::wire::CoordinatorReleaseImageRequest* request,
                    ReleaseImageCompleter::Sync& completer) override;
  void ImportEvent(fuchsia_hardware_display::wire::CoordinatorImportEventRequest* request,
                   ImportEventCompleter::Sync& completer) override;
  void ReleaseEvent(fuchsia_hardware_display::wire::CoordinatorReleaseEventRequest* request,
                    ReleaseEventCompleter::Sync& completer) override;
  void CreateLayer(CreateLayerRequestView request, CreateLayerCompleter::Sync& completer) override;
  void DestroyLayer(fuchsia_hardware_display::wire::CoordinatorDestroyLayerRequest* request,
                    DestroyLayerCompleter::Sync& completer) override;
  void SetDisplayMode(fuchsia_hardware_display::wire::CoordinatorSetDisplayModeRequest* request,
                      SetDisplayModeCompleter::Sync& completer) override;
  void SetDisplayColorConversion(
      fuchsia_hardware_display::wire::CoordinatorSetDisplayColorConversionRequest* request,
      SetDisplayColorConversionCompleter::Sync& completer) override;
  void SetDisplayLayers(fuchsia_hardware_display::wire::CoordinatorSetDisplayLayersRequest* request,
                        SetDisplayLayersCompleter::Sync& completer) override;
  void SetLayerPrimaryConfig(
      fuchsia_hardware_display::wire::CoordinatorSetLayerPrimaryConfigRequest* request,
      SetLayerPrimaryConfigCompleter::Sync& completer) override;
  void SetLayerPrimaryPosition(
      fuchsia_hardware_display::wire::CoordinatorSetLayerPrimaryPositionRequest* request,
      SetLayerPrimaryPositionCompleter::Sync& completer) override;
  void SetLayerPrimaryAlpha(
      fuchsia_hardware_display::wire::CoordinatorSetLayerPrimaryAlphaRequest* request,
      SetLayerPrimaryAlphaCompleter::Sync& completer) override;
  void SetLayerColorConfig(
      fuchsia_hardware_display::wire::CoordinatorSetLayerColorConfigRequest* request,
      SetLayerColorConfigCompleter::Sync& completer) override;
  void SetLayerImage2(fuchsia_hardware_display::wire::CoordinatorSetLayerImage2Request* request,
                      SetLayerImage2Completer::Sync& completer) override;
  void CheckConfig(CheckConfigCompleter::Sync& completer) override;
  void DiscardConfig(DiscardConfigCompleter::Sync& completer) override;
  void AcknowledgeVsync(fuchsia_hardware_display::wire::CoordinatorAcknowledgeVsyncRequest* request,
                        AcknowledgeVsyncCompleter::Sync& completer) override;
  void SetMinimumRgb(fuchsia_hardware_display::wire::CoordinatorSetMinimumRgbRequest* request,
                     SetMinimumRgbCompleter::Sync& completer) override;
  void SetDisplayPowerMode(
      fuchsia_hardware_display::wire::CoordinatorSetDisplayPowerModeRequest* request,
      SetDisplayPowerModeCompleter::Sync& completer) override;
  void GetLatestCommittedConfigStamp(
      GetLatestCommittedConfigStampCompleter::Sync& completer) override;

  // `listener_client` is allowed to be null.
  void Bind(fidl::ServerEnd<fuchsia_hardware_display::Coordinator> coordinator_server,
            fidl::ClientEnd<fuchsia_hardware_display::CoordinatorListener> listener_client,
            async_dispatcher_t* dispatcher = nullptr);

  void ResetCoordinatorBinding();

  // Sends an `OnDisplayChanged()` event to the display CoordinatorListener server
  // with the default display being added.
  //
  // Must be called only after the MockDisplayCoordinator is bound to a channel.
  void SendOnDisplayChangedRequest();

  fidl::ServerBindingRef<fuchsia_hardware_display::Coordinator>& binding() {
    ZX_DEBUG_ASSERT(binding_.has_value());
    return *binding_;
  }

  fidl::WireSharedClient<fuchsia_hardware_display::CoordinatorListener>& listener() {
    return listener_;
  }

  const WireDisplayInfo& display_info() const { return display_info_; }

  void set_import_image_fn(ImportImageFn fn) { import_image_fn_ = std::move(fn); }
  void set_release_image_fn(ReleaseImageFn fn) { release_image_fn_ = std::move(fn); }
  void set_import_event_fn(ImportEventFn fn) { import_event_fn_ = std::move(fn); }
  void set_release_event_fn(ReleaseEventFn fn) { release_event_fn_ = std::move(fn); }
  void set_create_layer_fn(CreateLayerFn fn) { create_layer_fn_ = std::move(fn); }
  void set_destroy_layer_fn(DestroyLayerFn fn) { destroy_layer_fn_ = std::move(fn); }
  void set_display_mode_fn(SetDisplayModeFn fn) { set_display_mode_fn_ = std::move(fn); }
  void set_display_color_conversion_fn(SetDisplayColorConversionFn fn) {
    set_display_color_conversion_fn_ = std::move(fn);
  }
  void set_set_display_layers_fn(SetDisplayLayersFn fn) { set_display_layers_fn_ = std::move(fn); }
  void set_set_layer_primary_config_fn(SetLayerPrimaryConfigFn fn) {
    set_layer_primary_config_fn_ = std::move(fn);
  }
  void set_layer_primary_position_fn(SetLayerPrimaryPositionFn fn) {
    set_layer_primary_position_fn_ = std::move(fn);
  }
  void set_set_layer_primary_alpha_fn(SetLayerPrimaryAlphaFn fn) {
    set_layer_primary_alpha_fn_ = std::move(fn);
  }
  void set_set_layer_image_fn(SetLayerImage2Fn fn) { set_layer_image_fn_ = std::move(fn); }
  void set_set_layer_color_config_fn(SetLayerColorConfigFn fn) {
    set_layer_color_config_fn_ = std::move(fn);
  }
  void set_check_config_fn(CheckConfigFn fn) { check_config_fn_ = std::move(fn); }
  void set_discard_config_fn(DiscardConfigFn fn) { discard_config_fn_ = std::move(fn); }
  void set_acknowledge_vsync_fn(AcknowledgeVsyncFn fn) { acknowledge_vsync_fn_ = std::move(fn); }
  void set_minimum_rgb_fn(SetMinimumRgbFn fn) { set_minimum_rgb_fn_ = std::move(fn); }
  void set_set_display_power_mode_fn(SetDisplayPowerModeFn fn) {
    set_display_power_mode_fn_ = std::move(fn);
  }
  void set_set_display_power_mode_result(zx_status_t result) {
    set_display_power_mode_result_ = result;
  }
  bool display_power_on() const { return display_power_on_; }

  // Number of times each function has been called.
  uint32_t import_image_count() const { return import_image_count_; }
  uint32_t release_image_count() const { return release_image_count_; }
  uint32_t import_event_count() const { return import_event_count_; }
  uint32_t release_event_count() const { return release_event_count_; }
  uint32_t create_layer_count() const { return create_layer_count_; }
  uint32_t destroy_layer_count() const { return destroy_layer_count_; }
  uint32_t set_display_mode_count() const { return set_display_mode_count_; }
  uint32_t set_display_color_conversion_count() const {
    return set_display_color_conversion_count_;
  }
  uint32_t set_display_layers_count() const { return set_display_layers_count_; }
  uint32_t set_layer_primary_config_count() const { return set_layer_primary_config_count_; }
  uint32_t set_layer_primary_position_count() const { return set_layer_primary_position_count_; }
  uint32_t set_layer_primary_alpha_count() const { return set_layer_primary_alpha_count_; }
  uint32_t set_layer_image_count() const { return set_layer_image_count_; }
  uint32_t check_config_count() const { return check_config_count_; }
  uint32_t discard_config_count() const { return discard_config_count_; }
  uint32_t acknowledge_vsync_count() const { return acknowledge_vsync_count_; }
  uint32_t set_minimum_rgb_count() const { return set_minimum_rgb_count_; }
  uint32_t set_display_power_count() const { return set_display_power_count_; }
  uint32_t illegal_action_count() const { return illegal_action_count_; }

 private:
  // Callback functions in FIDL order.
  ImportImageFn import_image_fn_;
  ReleaseImageFn release_image_fn_;
  ImportEventFn import_event_fn_;
  ReleaseEventFn release_event_fn_;
  CreateLayerFn create_layer_fn_;
  DestroyLayerFn destroy_layer_fn_;
  SetDisplayModeFn set_display_mode_fn_;
  SetDisplayColorConversionFn set_display_color_conversion_fn_;
  SetDisplayLayersFn set_display_layers_fn_;
  SetLayerPrimaryConfigFn set_layer_primary_config_fn_;
  SetLayerPrimaryPositionFn set_layer_primary_position_fn_;
  SetLayerPrimaryAlphaFn set_layer_primary_alpha_fn_;
  SetLayerColorConfigFn set_layer_color_config_fn_;
  SetLayerImage2Fn set_layer_image_fn_;
  CheckConfigFn check_config_fn_;
  DiscardConfigFn discard_config_fn_;
  AcknowledgeVsyncFn acknowledge_vsync_fn_;
  SetMinimumRgbFn set_minimum_rgb_fn_;
  SetDisplayPowerModeFn set_display_power_mode_fn_;

  std::unordered_set<uint64_t> imported_image_ids_;
  std::unordered_map<uint64_t, zx::event> imported_events_;
  std::unordered_set<uint64_t> created_layer_ids_;

  // FIDL method invocation counts (in FIDL order).
  uint32_t illegal_action_count_ = 0;
  uint32_t import_image_count_ = 0;
  uint32_t release_image_count_ = 0;
  uint32_t import_event_count_ = 0;
  uint32_t release_event_count_ = 0;
  uint32_t create_layer_count_ = 0;
  uint32_t destroy_layer_count_ = 0;
  uint32_t set_display_mode_count_ = 0;
  uint32_t set_display_color_conversion_count_ = 0;
  uint32_t set_display_layers_count_ = 0;
  uint32_t set_layer_primary_config_count_ = 0;
  uint32_t set_layer_primary_position_count_ = 0;
  uint32_t set_layer_primary_alpha_count_ = 0;
  uint32_t set_layer_image_count_ = 0;
  uint32_t set_layer_color_config_count_ = 0;
  uint32_t check_config_count_ = 0;
  uint32_t discard_config_count_ = 0;
  uint32_t acknowledge_vsync_count_ = 0;
  uint32_t set_minimum_rgb_count_ = 0;
  uint32_t set_display_power_count_ = 0;

  zx_status_t set_display_power_mode_result_ = ZX_OK;
  bool display_power_on_ = true;

  const WireDisplayInfo display_info_;

  std::optional<fidl::ServerBindingRef<fuchsia_hardware_display::Coordinator>> binding_;
  fidl::WireSharedClient<fuchsia_hardware_display::CoordinatorListener> listener_;
};

}  // namespace display::test

#endif  // SRC_UI_SCENIC_LIB_DISPLAY_TESTS_MOCK_DISPLAY_COORDINATOR_H_

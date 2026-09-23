// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/display/tests/mock_display_coordinator.h"

#include <fidl/fuchsia.hardware.display.types/cpp/fidl.h>
#include <fidl/fuchsia.hardware.display/cpp/fidl.h>
#include <fidl/fuchsia.math/cpp/fidl.h>
#include <lib/async/default.h>
#include <lib/syslog/cpp/macros.h>

#include <algorithm>

namespace display::test {

MockDisplayCoordinator::MockDisplayCoordinator(WireDisplayInfo display_info)
    : display_info_(display_info) {}

MockDisplayCoordinator::~MockDisplayCoordinator() = default;

void MockDisplayCoordinator::Bind(
    fidl::ServerEnd<fuchsia_hardware_display::Coordinator> coordinator_server,
    fidl::ClientEnd<fuchsia_hardware_display::CoordinatorListener> listener_client,
    async_dispatcher_t* dispatcher) {
  if (dispatcher == nullptr) {
    dispatcher = async_get_default_dispatcher();
  }
  binding_ = fidl::BindServer(dispatcher, std::move(coordinator_server), this);

  if (listener_client) {
    listener_.Bind(std::move(listener_client), dispatcher);
  }
}

void MockDisplayCoordinator::ResetCoordinatorBinding() {
  if (binding_.has_value()) {
    binding_->Close(ZX_ERR_INTERNAL);
    binding_ = std::nullopt;
  }
  listener_ = {};
}

// Methods in FIDL order
void MockDisplayCoordinator::ImportImage(
    fuchsia_hardware_display::wire::CoordinatorImportImageRequest* request,
    ImportImageCompleter::Sync& completer) {
  ++import_image_count_;
  if (import_image_fn_) {
    import_image_fn_(request);
  }

  if (imported_image_ids_.contains(request->image_id.value)) {
    ++illegal_action_count_;
    completer.Reply(fit::error(ZX_ERR_INVALID_ARGS));
    return;
  }
  imported_image_ids_.insert(request->image_id.value);
  completer.Reply(fit::ok());
}

void MockDisplayCoordinator::ReleaseImage(
    fuchsia_hardware_display::wire::CoordinatorReleaseImageRequest* request,
    ReleaseImageCompleter::Sync& completer) {
  ++release_image_count_;
  if (release_image_fn_) {
    release_image_fn_(request);
  }

  if (!imported_image_ids_.contains(request->image_id.value)) {
    ++illegal_action_count_;
    return;
  }
  imported_image_ids_.erase(request->image_id.value);
}

void MockDisplayCoordinator::ImportEvent(
    fuchsia_hardware_display::wire::CoordinatorImportEventRequest* request,
    ImportEventCompleter::Sync& completer) {
  ++import_event_count_;
  if (import_event_fn_) {
    zx::event callback_event;
    zx_status_t status = request->event.duplicate(ZX_RIGHT_SAME_RIGHTS, &callback_event);
    FX_CHECK(status == ZX_OK) << "Failed to duplicate event handle for callback: "
                              << zx_status_get_string(status);
    import_event_fn_(std::move(callback_event), request->id);
  }

  if (imported_events_.contains(request->id.value)) {
    ++illegal_action_count_;
    return;
  }
  imported_events_[request->id.value] = std::move(request->event);
}

void MockDisplayCoordinator::ReleaseEvent(
    fuchsia_hardware_display::wire::CoordinatorReleaseEventRequest* request,
    ReleaseEventCompleter::Sync& completer) {
  ++release_event_count_;
  if (release_event_fn_) {
    release_event_fn_(request);
  }

  if (!imported_events_.contains(request->id.value)) {
    ++illegal_action_count_;
    return;
  }
  imported_events_.erase(request->id.value);
}

void MockDisplayCoordinator::CreateLayer(CreateLayerRequestView request,
                                         CreateLayerCompleter::Sync& completer) {
  ++create_layer_count_;
  if (create_layer_fn_) {
    create_layer_fn_(request->layer_id);
  }

  if (created_layer_ids_.contains(request->layer_id.value)) {
    ++illegal_action_count_;
    completer.Reply(fit::error(ZX_ERR_ALREADY_EXISTS));
    return;
  }
  created_layer_ids_.insert(request->layer_id.value);
  completer.Reply(fit::ok());
}

void MockDisplayCoordinator::DestroyLayer(
    fuchsia_hardware_display::wire::CoordinatorDestroyLayerRequest* request,
    DestroyLayerCompleter::Sync& completer) {
  ++destroy_layer_count_;
  if (destroy_layer_fn_) {
    destroy_layer_fn_(request);
  }
}

void MockDisplayCoordinator::SetDisplayMode(
    fuchsia_hardware_display::wire::CoordinatorSetDisplayModeRequest* request,
    SetDisplayModeCompleter::Sync& completer) {
  ++set_display_mode_count_;
  if (set_display_mode_fn_) {
    set_display_mode_fn_(request->display_id, request->mode);
  }
}

void MockDisplayCoordinator::SetDisplayColorConversion(
    fuchsia_hardware_display::wire::CoordinatorSetDisplayColorConversionRequest* request,
    SetDisplayColorConversionCompleter::Sync& completer) {
  ++set_display_color_conversion_count_;
  if (set_display_color_conversion_fn_) {
    set_display_color_conversion_fn_(request->display_id, request->preoffsets,
                                     request->coefficients, request->postoffsets);
  }
}

void MockDisplayCoordinator::SetDisplayLayers(
    fuchsia_hardware_display::wire::CoordinatorSetDisplayLayersRequest* request,
    SetDisplayLayersCompleter::Sync& completer) {
  ++set_display_layers_count_;
  if (set_display_layers_fn_) {
    set_display_layers_fn_(request->display_id, request->layer_ids);
  }
}

void MockDisplayCoordinator::SetLayerPrimaryConfig(
    fuchsia_hardware_display::wire::CoordinatorSetLayerPrimaryConfigRequest* request,
    SetLayerPrimaryConfigCompleter::Sync& completer) {
  ++set_layer_primary_config_count_;
  if (set_layer_primary_config_fn_) {
    set_layer_primary_config_fn_(request->layer_id, request->image_metadata);
  }
  // completer.ReplySuccess();
}

void MockDisplayCoordinator::SetLayerPrimaryPosition(
    fuchsia_hardware_display::wire::CoordinatorSetLayerPrimaryPositionRequest* request,
    SetLayerPrimaryPositionCompleter::Sync& completer) {
  ++set_layer_primary_position_count_;
  if (set_layer_primary_position_fn_) {
    set_layer_primary_position_fn_(request->layer_id, request->image_source_transformation,
                                   request->image_source, request->display_destination);
  }
}

void MockDisplayCoordinator::SetLayerPrimaryAlpha(
    fuchsia_hardware_display::wire::CoordinatorSetLayerPrimaryAlphaRequest* request,
    SetLayerPrimaryAlphaCompleter::Sync& completer) {
  ++set_layer_primary_alpha_count_;
  if (set_layer_primary_alpha_fn_) {
    set_layer_primary_alpha_fn_(request->layer_id, request->mode, request->val);
  }
  // completer.ReplySuccess();
}

void MockDisplayCoordinator::SetLayerColorConfig(
    fuchsia_hardware_display::wire::CoordinatorSetLayerColorConfigRequest* request,
    SetLayerColorConfigCompleter::Sync& completer) {
  ++set_layer_color_config_count_;
  if (set_layer_color_config_fn_) {
    set_layer_color_config_fn_(request->layer_id, request->color, request->display_destination);
  }
  // completer.ReplySuccess();
}

void MockDisplayCoordinator::SetLayerImage2(
    fuchsia_hardware_display::wire::CoordinatorSetLayerImage2Request* request,
    SetLayerImage2Completer::Sync& completer) {
  ++set_layer_image_count_;
  if (set_layer_image_fn_) {
    set_layer_image_fn_(request->layer_id, request->image_id, request->wait_event_id);
  }
  // completer.ReplySuccess();
}

void MockDisplayCoordinator::CheckConfig(CheckConfigCompleter::Sync& completer) {
  fuchsia_hardware_display_types::ConfigResult result =
      fuchsia_hardware_display_types::ConfigResult::kOk;
  ++check_config_count_;
  if (check_config_fn_) {
    check_config_fn_(&result);
  }

  completer.Reply(result);
}

void MockDisplayCoordinator::DiscardConfig(DiscardConfigCompleter::Sync& completer) {
  ++discard_config_count_;
  if (discard_config_fn_) {
    discard_config_fn_();
  }
  // completer.ReplySuccess();
}

void MockDisplayCoordinator::AcknowledgeVsync(
    fuchsia_hardware_display::wire::CoordinatorAcknowledgeVsyncRequest* request,
    AcknowledgeVsyncCompleter::Sync& completer) {
  ++acknowledge_vsync_count_;
  if (acknowledge_vsync_fn_) {
    acknowledge_vsync_fn_(request->cookie);
  }
}

void MockDisplayCoordinator::SetMinimumRgb(
    fuchsia_hardware_display::wire::CoordinatorSetMinimumRgbRequest* request,
    SetMinimumRgbCompleter::Sync& completer) {
  ++set_minimum_rgb_count_;
  if (set_minimum_rgb_fn_) {
    set_minimum_rgb_fn_(request->minimum_rgb);
  }

  completer.Reply(fit::ok());
}

void MockDisplayCoordinator::SetDisplayPowerMode(
    fuchsia_hardware_display::wire::CoordinatorSetDisplayPowerModeRequest* request,
    SetDisplayPowerModeCompleter::Sync& completer) {
  // ++set_display_power_mode_count_;
  if (set_display_power_mode_fn_) {
    set_display_power_mode_fn_(request);
  }

  if (set_display_power_mode_result_ == ZX_OK) {
    display_power_on_ = request->power_mode == fuchsia_hardware_display_types::wire::PowerMode::kOn;
    completer.Reply(fit::ok());
  } else {
    completer.Reply(fit::error(set_display_power_mode_result_));
  }
}

void MockDisplayCoordinator::GetLatestCommittedConfigStamp(
    GetLatestCommittedConfigStampCompleter::Sync& completer) {
  completer.Reply({.value = 0});
}

void MockDisplayCoordinator::SendOnDisplayChangedRequest() {
  FX_CHECK(binding_.has_value());

  fidl::OneWayStatus result =
      listener().sync()->OnDisplaysChanged(fidl::VectorView<WireDisplayInfo>::FromExternal(
                                               const_cast<WireDisplayInfo*>(&display_info_), 1),
                                           {});
  FX_CHECK(result.ok());
}

}  // namespace display::test

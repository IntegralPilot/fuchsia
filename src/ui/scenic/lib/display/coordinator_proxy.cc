// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/display/coordinator_proxy.h"

#include <lib/trace/event.h>

#include <algorithm>

#include "src/ui/scenic/lib/utils/fidl_array_cast.h"
#include "src/ui/scenic/lib/utils/logging.h"

// Allows us to manually change this to enable logging without *all* Flatland verbose logging.
#define CP_VERBOSE_LOG FLATLAND_VERBOSE_LOG

// At time of writing, `sizeof(DisplayEquivalence) == 104` and `sizeof(DisplayEquivalence) == 56`,
// so we can estimate a 2-layer cache entry to be ~250 bytes; 20*250 == 5KB is a reasonable cost.
static constexpr size_t kCheckConfigCacheSize = 20;

namespace display {

CoordinatorProxy::CoordinatorProxy(
    fidl::ClientEnd<fuchsia_hardware_display::Coordinator> coordinator,
    async_dispatcher_t* dispatcher, CheckConfigHeuristics check_config_heuristics,
    inspect::Node inspect_node)
    : CoordinatorProxy(fidl::WireSharedClient<fuchsia_hardware_display::Coordinator>(
                           std::move(coordinator), dispatcher),
                       check_config_heuristics, std::move(inspect_node)) {}

CoordinatorProxy::CoordinatorProxy(
    fidl::WireSharedClient<fuchsia_hardware_display::Coordinator> coordinator,
    CheckConfigHeuristics check_config_heuristics, inspect::Node inspect_node)
    : coordinator_(std::move(coordinator)),
      check_config_cache_(kCheckConfigCacheSize),
      check_config_heuristics_(check_config_heuristics),
      inspect_node_(std::move(inspect_node)) {
  FX_DCHECK(coordinator_);
  inspect_api_calls_received_ = inspect_node_.CreateUint("API Calls Received", 0);
  inspect_api_calls_sent_ = inspect_node_.CreateUint("API Calls Sent", 0);
  inspect_apply_config_calls_received_ = inspect_node_.CreateUint("ApplyConfig Calls Received", 0);
  inspect_apply_config_calls_sent_ = inspect_node_.CreateUint("ApplyConfig Calls Sent", 0);
  inspect_check_config_calls_skipped_ = inspect_node_.CreateUint("CheckConfig Calls Skipped", 0);
  inspect_check_config_calls_sent_ = inspect_node_.CreateUint("CheckConfig Calls Sent", 0);
  inspect_check_config_cache_size_ = inspect_node_.CreateUint("CheckConfig Cache Size", 0);
  inspect_import_image_count_ = inspect_node_.CreateUint("Imported Images (total)", 0);
  inspect_current_image_count_ = inspect_node_.CreateUint("Imported Images (current)", 0);
  inspect_import_event_count_ = inspect_node_.CreateUint("Imported Events (total)", 0);
  inspect_current_event_count_ = inspect_node_.CreateUint("Imported Events (current)", 0);

// Nobody cares about the specific cache values except when debugging.
#ifndef NDEBUG
  inspect_config_cache_dump_ =
      inspect_node_.CreateLazyValues("CheckConfig Cache values (DEBUG only)", [this] {
        inspect::Inspector inspector;

        std::ostringstream str;
        size_t i = 0;
        for (const auto& kv : check_config_cache_) {
          str << "\n" << i++ << (kv.value ? ": valid " : ": invalid ") << kv.key;
        }
        str << "\n";

        inspector.GetRoot().CreateString("CheckConfig Cache Values", str.str(), &inspector);

        return fpromise::make_ok_promise(std::move(inspector));
      });
#endif
}

zx::result<> CoordinatorProxy::ImportImage(types::Extent2 image_dimensions,
                                           uint32_t image_tiling_type,
                                           WireBufferCollectionId buffer_collection_id,
                                           uint32_t buffer_index, ImageId image_id) {
  CP_VERBOSE_LOG << "CoordinatorProxy::ImportImage(image_dimensions=" << image_dimensions << ")";

  IncrementImportImageCallsSent();
  FX_DCHECK(!images_.contains(image_id)) << "Not expecting to find image_id=" << image_id.value();

  fuchsia_hardware_display_types::wire::ImageMetadata image_metadata{
      .dimensions = image_dimensions.ToWire(),
      .tiling_type = image_tiling_type,
  };

  const auto import_image_result = coordinator_.sync()->ImportImage(
      image_metadata, buffer_collection_id, buffer_index, image_id.ToFidl());
  if (!import_image_result.ok()) {
    FX_LOGS(ERROR) << "ImportImage transport error: " << import_image_result.status_string();
    return zx::error(import_image_result.status());
  }
  if (import_image_result->is_error()) {
    const zx_status_t error_value = import_image_result->error_value();
    FX_LOGS(ERROR) << "ImportImage method error: " << zx_status_get_string(error_value);
    return zx::error(error_value);
  }

  images_.insert(image_id);
  UpdateCurrentImageCount();
  return zx::ok();
}

void CoordinatorProxy::ReleaseImage(const ImageId& image_id) {
  CP_VERBOSE_LOG << "CoordinatorProxy::ReleaseImage";
  FX_DCHECK(images_.erase(image_id)) << "Expected to find image_id=" << image_id.value();
  UpdateCurrentImageCount();

  // TODO(https://fxbug.dev/449807074): this shouldn't be necessary, but due to the workaround for
  // this issue in `Layer::SendImageLayerDiffsToCoordinator()` we can sometimes resend
  // `SetLayerImage2()` with an `image_id` that has already been release.  This causes the
  // coordinator to kill Scenic's connection.  Consider removing this workaround after 449807074 is
  // fixed; we might want to leave it because it's cheap compared to the FIDL call anyway.
  for (auto& [_, layer] : layers_) {
    layer.UnsetImage(image_id);
  }

  fidl::OneWayStatus result = coordinator_->ReleaseImage(image_id.ToFidl());
  if (!result.ok()) {
    FX_LOGS(ERROR) << "Failed to call FIDL ReleaseImage method: " << result.status_string();
  }
}

void CoordinatorProxy::ImportEvent(zx::event event, const EventId& event_id) {
  CP_VERBOSE_LOG << "CoordinatorProxy::ImportEvent";
  IncrementImportEventCallsSent();
  FX_DCHECK(!events_.contains(event_id)) << "Not expecting to find event_id=" << event_id.value();

  fidl::OneWayStatus result = coordinator_->ImportEvent(std::move(event), event_id.ToFidl());
  if (!result.ok()) {
    FX_LOGS(ERROR) << "Failed to call FIDL ImportEvent method: " << result.status_string();
  }

  events_.insert(event_id);
  UpdateCurrentEventCount();
}

EventId CoordinatorProxy::ImportEvent(const zx::event& event) {
  static EventId id_generator(1);

  zx::event copied_event;
  zx_status_t status = event.duplicate(ZX_RIGHT_SAME_RIGHTS, &copied_event);
  if (status != ZX_OK) {
    FX_LOGS(ERROR) << "Failed to duplicate display controller event: "
                   << zx_status_get_string(status);
    return kInvalidEventId;
  }

  EventId id = id_generator++;
  ImportEvent(std::move(copied_event), id);
  return id;
}

void CoordinatorProxy::ReleaseEvent(const EventId& event_id) {
  CP_VERBOSE_LOG << "CoordinatorProxy::ReleaseEvent";
  FX_DCHECK(events_.erase(event_id)) << "Expected to find event_id=" << event_id.value();
  UpdateCurrentEventCount();

  fidl::OneWayStatus result = coordinator_->ReleaseEvent(event_id.ToFidl());
  if (!result.ok()) {
    FX_LOGS(ERROR) << "Failed to call FIDL ReleaseEvent method: " << result.status_string();
  }
}

LayerId CoordinatorProxy::CreateLayer() {
  CP_VERBOSE_LOG << "CoordinatorProxy::CreateLayer";
  IncrementApiCallsReceived();
  IncrementApiCallsSent();

  LayerId layer_id = next_layer_id_;
  ++next_layer_id_;

  fuchsia_hardware_display::wire::LayerId fidl_layer_id = {layer_id.value()};
  const auto create_layer_result = coordinator_.sync()->CreateLayer(fidl_layer_id);

  if (!create_layer_result.ok()) {
    FX_LOGS(ERROR) << "CreateLayer transport error: " << create_layer_result.status_string();
    return kInvalidLayerId;
  }
  if (create_layer_result->is_error()) {
    FX_LOGS(ERROR) << "CreateLayer method error: "
                   << zx_status_get_string(create_layer_result->error_value());
    return kInvalidLayerId;
  }

  FX_DCHECK(!layers_.contains(layer_id));
  layers_.try_emplace(layer_id);
  return layer_id;
}

void CoordinatorProxy::DestroyLayer(const LayerId& layer_id) {
  CP_VERBOSE_LOG << "CoordinatorProxy::DestroyLayer";
  IncrementApiCallsReceived();
  IncrementApiCallsSent();

  FX_DCHECK(layers_.contains(layer_id));
  layers_.erase(layer_id);

  // The FIDL API requires that the layer is not in use by an applied/draft state.
  FX_DCHECK(std::ranges::find(applied_display_layers_, layer_id) == applied_display_layers_.end());
  FX_DCHECK(std::ranges::find(draft_display_layers_, layer_id) == draft_display_layers_.end());

  const fidl::OneWayStatus status = coordinator_->DestroyLayer(layer_id.ToFidl());

  FX_DCHECK(status.ok()) << "Failed to call FIDL DestroyLayer method: " << status.status_string();
}

void CoordinatorProxy::SetDisplayMode(const DisplayId& display_id, const DisplayMode& mode) {
  CP_VERBOSE_LOG << "CoordinatorProxy::SetDisplayMode";
  CheckDisplayId(display_id);
  IncrementApiCallsReceived();

  draft_display_mode_ = mode;
}

void CoordinatorProxy::SetDisplayColorConversion(const DisplayId& display_id,
                                                 const std::array<float, 3>& preoffsets,
                                                 const std::array<float, 9>& coefficients,
                                                 const std::array<float, 3>& postoffsets) {
  CP_VERBOSE_LOG << "CoordinatorProxy::SetDisplayColorConversion";
  CheckDisplayId(display_id);
  IncrementApiCallsReceived();

  draft_color_conversion_preoffsets_ = preoffsets;
  draft_color_conversion_coefficients_ = coefficients;
  draft_color_conversion_postoffsets_ = postoffsets;
}

void CoordinatorProxy::SetDisplayLayers(const DisplayId& display_id,
                                        const std::span<const LayerId>& layer_ids) {
  CP_VERBOSE_LOG << "CoordinatorProxy::SetDisplayLayers(layer_count=" << layer_ids.size() << ")";
  CheckDisplayId(display_id);
  IncrementApiCallsReceived();

  draft_display_layers_.assign(layer_ids.begin(), layer_ids.end());
}

void CoordinatorProxy::SetLayerPrimaryConfig(const LayerId& layer_id,
                                             const Extent2& image_dimensions,
                                             uint32_t image_tiling_type) {
  CP_VERBOSE_LOG << "CoordinatorProxy::SetLayerPrimaryConfig(layer=" << layer_id.value() << ")";
  IncrementApiCallsReceived();

  GetLayer(layer_id).SetPrimaryConfig(image_dimensions, image_tiling_type);
}

void CoordinatorProxy::SetLayerPrimaryPosition(const LayerId& layer_id, const RotateFlip& transform,
                                               const Rectangle& image_source,
                                               const Rectangle& display_destination) {
  CP_VERBOSE_LOG << "CoordinatorProxy::SetLayerPrimaryPosition(layer=" << layer_id.value() << ")";
  IncrementApiCallsReceived();

  GetLayer(layer_id).SetPrimaryPosition(transform, image_source, display_destination);
}

void CoordinatorProxy::SetLayerPrimaryAlpha(const LayerId& layer_id, const BlendMode& blend_mode,
                                            float alpha_value) {
  CP_VERBOSE_LOG << "CoordinatorProxy::SetLayerPrimaryAlpha(layer=" << layer_id.value() << ")";
  IncrementApiCallsReceived();

  GetLayer(layer_id).SetPrimaryAlpha(blend_mode, alpha_value);
}

void CoordinatorProxy::SetLayerImage(const LayerId& layer_id, const ImageId& image_id,
                                     const EventId& wait_event_id) {
  CP_VERBOSE_LOG << "CoordinatorProxy::SetLayerImage(layer=" << layer_id.value()
                 << " image=" << image_id.value() << " wait_event=" << wait_event_id.value() << ")";
  IncrementApiCallsReceived();

  GetLayer(layer_id).SetLayerImage(image_id, wait_event_id);
}

void CoordinatorProxy::SetLayerColorConfig(const LayerId& layer_id, const WireColor& color,
                                           const Rectangle& display_destination) {
  CP_VERBOSE_LOG << "CoordinatorProxy::SetLayerColorConfig(layer=" << layer_id.value() << ")";
  IncrementApiCallsReceived();

  GetLayer(layer_id).SetColorConfig(color, display_destination);
}

zx::result<> CoordinatorProxy::ApplyConfig(const WireConfigStamp& config_stamp) {
  CP_VERBOSE_LOG << "CoordinatorProxy::ApplyConfig(stamp=" << config_stamp.value << ")";
  TRACE_DURATION("gfx", "display::CoordinatorProxy::ApplyConfig");
  IncrementApplyConfigCallsReceived();

  // Set `temp_display_equivalence_` to have the values we need.
  UpdateDisplayEquivalenceForApplyConfig();

  // Avoid thrashing the cache with small variations of failing configs.
  if (HeuristicsSayThatCheckConfigWouldFail()) {
    // Check config would fail, so we can return immediately.
    IncrementCheckConfigCallSkipped();
    ResetDraftState();
    return zx::error(ZX_ERR_BAD_STATE);
  }

  // If an equivalent equiv is already in `check_config_cache_` we can skip calling the FIDL method.
  const auto check_config_result = check_config_cache_.Get(temp_display_equivalence_);
  const bool has_cached_check_config = check_config_result.has_value();

  if (has_cached_check_config && !check_config_result.value()) {
    // Check config would fail, so we can return immediately.
    TRACE_INSTANT("gfx", "scenic_d2d_failed: cached check config failure", TRACE_SCOPE_THREAD);
    IncrementCheckConfigCallSkipped();
    ResetDraftState();
    return zx::error(ZX_ERR_BAD_STATE);
  }

  // We always need to send diffs (between the draft and applied states) to the Coordinator,
  // regardless of whether we need to do a FIDL round-trip `CheckConfig()`.
  SendDiffsToCoordinator();

  if (has_cached_check_config) {
    // Nothing to do: we know `CheckConfig()` would succeed if we called it, because if the cached
    // result was a failure we would have returned early above.
    IncrementCheckConfigCallSkipped();
  } else {
    // There was no cached result, so a round-trip FIDL call is necessary.
    // This result will be cached.
    const WireConfigResult status = FidlCheckConfig();
    if (status != WireConfigResult::kOk) {
      // Cache the failed state and cleanup before returning an error.  Because we already sent the
      // diffs to the display coordinator, we need a FIDL `DiscardConfig()` in addition to resetting
      // the draft state.
      TRACE_INSTANT("gfx", "scenic_d2d_failed: FIDL check config failure", TRACE_SCOPE_THREAD);
      CacheCheckConfigResult(false);
      ResetDraftState();
      FidlDiscardConfig();
      return zx::error(ZX_ERR_BAD_STATE);
    }

    // Cache the successful result; next time we won't need to call the FIDL `CheckConfig()`.
    CacheCheckConfigResult(true);
  }

  // We know the config is valid, so now we can call `ApplyConfig()`.
  FidlApplyConfig(config_stamp);

  // The draft config has become the applied config.
  AcceptDraftState();

  return zx::ok();
}

internal::Layer& CoordinatorProxy::GetLayer(const LayerId& layer_id) {
  auto it = layers_.find(layer_id);
  FX_DCHECK(it != layers_.end()) << "Layer " << layer_id.value()
                                 << " not found in CoordinatorProxy.";
  return it->second;
}

void CoordinatorProxy::CheckDisplayId(const DisplayId& display_id) {
  FX_DCHECK(display_id != kInvalidDisplayId);
  FX_DCHECK(display_id_ == kInvalidDisplayId || display_id_ == display_id)
      << "New display id=" << display_id.value()
      << " differs from previously-seen display id=" << display_id_.value();
  display_id_ = display_id;
}

void CoordinatorProxy::ResetDraftState() {
  TRACE_DURATION("gfx", "display::CoordinatorProxy::ResetDraftState");

  draft_display_layers_ = applied_display_layers_;
  draft_display_mode_ = applied_display_mode_;
  draft_color_conversion_preoffsets_ = applied_color_conversion_preoffsets_;
  draft_color_conversion_coefficients_ = applied_color_conversion_coefficients_;
  draft_color_conversion_postoffsets_ = applied_color_conversion_postoffsets_;

  for (auto& layer : layers_) {
    layer.second.ResetDraftState();
  }
}

void CoordinatorProxy::AcceptDraftState() {
  TRACE_DURATION("gfx", "display::CoordinatorProxy::AcceptDraftState");

  applied_display_layers_ = draft_display_layers_;
  applied_display_mode_ = draft_display_mode_;
  applied_color_conversion_preoffsets_ = draft_color_conversion_preoffsets_;
  applied_color_conversion_coefficients_ = draft_color_conversion_coefficients_;
  applied_color_conversion_postoffsets_ = draft_color_conversion_postoffsets_;

  for (auto& layer : layers_) {
    layer.second.AcceptDraftState();
  }
}

void CoordinatorProxy::SendDiffsToCoordinator() {
  TRACE_DURATION("gfx", "display::CoordinatorProxy::SendDiffsToCoordinator");

  // We send diffs for all layers, even those that aren't in the list set by `SetDisplayLayers()`.
  // This matches the semantics implemented by the display coordinator, and is sensible: doing
  // otherwise would complicate both the user's mental model and the state tracking implementation.
  for (auto& [id, layer] : layers_) {
    uint64_t api_calls_sent_for_layer = layer.SendDiffsToCoordinator(id, coordinator_);
    IncrementApiCallsSent(api_calls_sent_for_layer);
  }

  if (draft_display_layers_ != applied_display_layers_) {
    IncrementApiCallsSent();

    // Safe: see static_asserts in types::IdType<>.
    auto wire_layer_ids = fidl::VectorView<WireLayerId>::FromExternal(
        reinterpret_cast<WireLayerId*>(draft_display_layers_.data()), draft_display_layers_.size());

    const fidl::OneWayStatus status =
        coordinator_->SetDisplayLayers(display_id_.ToFidl(), wire_layer_ids);

    FX_DCHECK(status.ok()) << "Failed to call FIDL SetDisplayLayers method: "
                           << status.status_string();
  }
  if (draft_display_mode_ != applied_display_mode_) {
    IncrementApiCallsSent();

    const fidl::OneWayStatus status =
        coordinator_->SetDisplayMode(display_id_.ToFidl(), draft_display_mode_.ToWire());

    FX_DCHECK(status.ok()) << "Failed to call FIDL SetDisplayMode method: "
                           << status.status_string();
  }

  if (draft_color_conversion_preoffsets_ != applied_color_conversion_preoffsets_ ||
      draft_color_conversion_coefficients_ != applied_color_conversion_coefficients_ ||
      draft_color_conversion_postoffsets_ != applied_color_conversion_postoffsets_) {
    IncrementApiCallsSent();

    const fidl::OneWayStatus status = coordinator_->SetDisplayColorConversion(
        display_id_.ToFidl(),
        utils::ReinterpretStdArrayAsFidlArray(draft_color_conversion_preoffsets_),
        utils::ReinterpretStdArrayAsFidlArray(draft_color_conversion_coefficients_),
        utils::ReinterpretStdArrayAsFidlArray(draft_color_conversion_postoffsets_));

    FX_DCHECK(status.ok()) << "Failed to call FIDL SetDisplayColorConversion method: "
                           << status.status_string();
  }
}

void CoordinatorProxy::UpdateDisplayEquivalenceForApplyConfig() {
  TRACE_DURATION("gfx", "display::CoordinatorProxy::UpdateDisplayEquivalenceForApplyConfig");

  // This is the only method allowed to mutate `temp_display_equivalence_`.  It is const everywhere
  // else.
  internal::DisplayEquivalence& equiv =
      const_cast<internal::DisplayEquivalence&>(temp_display_equivalence_);

  equiv.layers.clear();
  for (LayerId& layer_id : draft_display_layers_) {
    equiv.layers.push_back(GetLayer(layer_id).draft_equiv());
  }
  equiv.display_mode = draft_display_mode_;
  equiv.color_conversion_preoffsets = draft_color_conversion_preoffsets_;
  equiv.color_conversion_coefficients = draft_color_conversion_coefficients_;
  equiv.color_conversion_postoffsets = draft_color_conversion_postoffsets_;
}

void CoordinatorProxy::FidlApplyConfig(const WireConfigStamp& config_stamp) {
  TRACE_DURATION("gfx", "display::CoordinatorProxy::FidlApplyConfig");
  CP_VERBOSE_LOG << "CoordinatorProxy::FidlApplyConfig(stamp=" << config_stamp.value << ")";

  IncrementApplyConfigCallsSent();

  fidl::Arena arena;
  const fidl::OneWayStatus result = coordinator_->CommitConfig(
      fuchsia_hardware_display::wire::CoordinatorCommitConfigRequest::Builder(arena)
          .stamp(config_stamp)
          .Build());

  FX_DCHECK(result.ok()) << "Failed to call FIDL ApplyConfig method: " << result.status_string();
}

void CoordinatorProxy::FidlDiscardConfig() {
  TRACE_DURATION("gfx", "display::CoordinatorProxy::FidlDiscardConfig");

  const fidl::OneWayStatus result = coordinator_->DiscardConfig();
  FX_DCHECK(result.ok()) << "Failed to call FIDL DiscardConfig method: " << result.status_string();
}

WireConfigResult CoordinatorProxy::FidlCheckConfig() {
  TRACE_DURATION("gfx", "display::CoordinatorProxy::FidlCheckConfig");

  IncrementCheckConfigCallsSent();

  const auto status = coordinator_.sync()->CheckConfig();
  FX_DCHECK(status.ok()) << "Failed to call FIDL CheckConfig method: " << status.status_string();

  return status->res;
}

void CoordinatorProxy::CacheCheckConfigResult(bool success) {
  check_config_cache_.Put(temp_display_equivalence_, success);
  inspect_check_config_cache_size_.Set(check_config_cache_.size());
}

bool CoordinatorProxy::HeuristicsSayThatCheckConfigWouldFail() {
  TRACE_DURATION("gfx", "display::CoordinatorProxy::HeuristicsSayThatCheckConfigWouldFail");

  if (!check_config_heuristics_.enable_heuristics) {
    // Heuristics are disabled.
    return false;
  }

  const Extent2& display_extent = temp_display_equivalence_.display_mode.active_area();
  for (const auto& layer : temp_display_equivalence_.layers) {
    // Image layer heuristics.
    if (const display::internal::ImageLayerEquivalence* image_layer =
            std::get_if<display::internal::ImageLayerEquivalence>(&layer.config)) {
      // Images must be displayed at the origin and cover the entire screen.
      // TODO(https://fxbug.dev/446042966): improve equivalences so that we don't need a heuristic.
      auto& display_destination = image_layer->display_destination;
      if (display_destination.x() != 0 || display_destination.y() != 0 ||
          display_destination.width() != display_extent.width() ||
          display_destination.height() != display_extent.height()) {
        TRACE_INSTANT("gfx", "scenic_d2d_failed: heuristic: not full-screen", TRACE_SCOPE_THREAD,
                      "x", TA_INT32(display_destination.x()), "y",
                      TA_INT32(display_destination.y()), "width",
                      TA_INT32(display_destination.width()), "height",
                      TA_INT32(display_destination.height()));
        return true;
      }
    }
  }
  // No heuristic complained about the configuration.
  return false;
}

}  // namespace display

namespace display::internal {

std::ostream& operator<<(std::ostream& str, const DisplayEquivalence& e) {
  str << "DisplayEquivalence";
  str << "\n\tmode = " << e.display_mode;
  str << "\n\tcolor preoffsets = ";
  for (auto offset : e.color_conversion_preoffsets) {
    str << offset << ", ";
  }
  str << "\n\tcolor coefficients = ";
  for (auto coef : e.color_conversion_coefficients) {
    str << coef << ", ";
  }
  str << "\n\tcolor postoffsets = ";
  for (auto offset : e.color_conversion_postoffsets) {
    str << offset << ", ";
  }
  str << "\n\tlayer count = " << e.layers.size() << "\n\tlayers:";
  for (size_t i = 0; i < e.layers.size(); ++i) {
    str << "\n\t\t" << i << ": " << e.layers[i];
  }
  return str;
}

std::ostream& operator<<(std::ostream& str, const LayerEquivalence& le) {
  if (const display::internal::ImageLayerEquivalence* image_le =
          std::get_if<display::internal::ImageLayerEquivalence>(&le.config)) {
    str << *image_le;
    return str;
  }
  if (const display::internal::ColorLayerEquivalence* color_le =
          std::get_if<display::internal::ColorLayerEquivalence>(&le.config)) {
    str << *color_le;
    return str;
  }
  if (const display::internal::UninitializedLayerEquivalence* uninitialized_le =
          std::get_if<display::internal::UninitializedLayerEquivalence>(&le.config)) {
    str << *uninitialized_le;
    return str;
  }
  __UNREACHABLE;
  return str;
}

std::ostream& operator<<(std::ostream& str, const ImageLayerEquivalence& le) {
  str << "ImageLayerEquiv[dst=" << le.display_destination << " src=" << le.image_source
      << " transform=" << le.image_source_transformation << " im_dims=" << le.image_dimensions
      << " tiling=" << le.image_tiling_type << " blend=" << le.blend_mode << " alpha=";
  if (le.alpha_range == ImageLayerEquivalence::AlphaRange::kAlphaOne) {
    str << "1";
  } else if (le.alpha_range == ImageLayerEquivalence::AlphaRange::kAlphaZero) {
    str << "0";
  } else {
    str << "0<a<1";
  }
  str << "]";
  return str;
}

std::ostream& operator<<(std::ostream& str, const ColorLayerEquivalence& le) {
  str << "ColorLayerEquiv[color=";
  auto& bytes = le.color.bytes;
  str << bytes[0] << "," << bytes[1] << "," << bytes[2] << "," << bytes[3] << "," << bytes[4] << ","
      << bytes[5] << "," << bytes[6] << "," << bytes[7];
  str << " format=" << static_cast<uint32_t>(le.color.format) << " dst=" << le.display_destination
      << "]";
  return str;
}

std::ostream& operator<<(std::ostream& str, const UninitializedLayerEquivalence& e) {
  str << "UninitializedLayerEquiv[]";
  return str;
}

}  // namespace display::internal

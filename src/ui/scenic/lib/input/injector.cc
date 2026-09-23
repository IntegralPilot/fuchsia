// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/input/injector.h"

#include <lib/async/cpp/time.h>
#include <lib/async/default.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>

#include "src/ui/scenic/lib/input/constants.h"
#include "src/ui/scenic/lib/utils/fidl_array_cast.h"
#include "src/ui/scenic/lib/utils/math.h"

#include <glm/glm.hpp>

namespace scenic_impl::input {

using fuchsia_ui_pointerinjector::wire::EventPhase;

namespace {

// Retain this many touch event buckets.  This ensures we see at most this
// many of them even if there isn't 10 minutes of consistent touch activity.
constexpr size_t kNumRetainedTouchEventBuckets = 10;

uint64_t GetCurrentMinute(const zx::time timestamp) { return timestamp.get() / zx::min(1).get(); }

}  // namespace

// LINT.IfChange
InjectorInspector::InjectorInspector(inspect::Node node)
    : node_(std::move(node)),
      history_stats_node_(node_.CreateLazyValues("Injection history", [this] {
        inspect::Inspector insp;
        ReportStats(insp);
        return fpromise::make_ok_promise(std::move(insp));
      })) {}

void InjectorInspector::OnPointerInjectorEvent(
    const fuchsia_ui_pointerinjector::wire::Event& event) {
  FX_DCHECK(event.has_data() && event.has_timestamp());

  if (event.data().is_viewport()) {
    // No-op.
  } else if (event.data().is_pointer_sample()) {
    last_event_timestamp_ = std::max(last_event_timestamp_, zx::time(event.timestamp()));
    UpdateHistory(last_event_timestamp_);
  } else {
    FX_LOGS(ERROR) << "pointerinjector::Event dropped from inspect metrics. Unexpected data type.";
  }
}

void InjectorInspector::UpdateHistory(const zx::time now) {
  const uint64_t current_minute = GetCurrentMinute(now);

  // Add elements to the front and pop from the back so that the newest element will be read out
  // first when we later iterate over the deque.
  if (history_.empty() || history_.front().minute_key != current_minute) {
    history_.push_front({
        .minute_key = current_minute,
    });
  }
  history_.front().num_injected_events++;

  // Pop off everything older than |kNumMinutesOfHistory|.
  while (history_.size() > kNumRetainedTouchEventBuckets &&
         (current_minute - history_.back().minute_key) >= kNumMinutesOfHistory) {
    history_.pop_back();
  }
}

void InjectorInspector::ReportStats(inspect::Inspector& inspector) const {
  inspect::Node node = inspector.GetRoot().CreateChild(
      "Last " + std::to_string(kNumMinutesOfHistory) + " minutes of injected events");

  uint64_t total = 0;
  const uint64_t current_minute = GetCurrentMinute(async::Now(async_get_default_dispatcher()));
  for (const auto& [minute, num_injected_events] : history_) {
    if (minute + kNumMinutesOfHistory <= current_minute) {
      break;
    }

    node.CreateUint("Events at minute " + std::to_string(minute), num_injected_events, &inspector);

    total += num_injected_events;
  }
  node.CreateUint("Total", total, &inspector);
  inspector.emplace(std::move(node));
}

namespace {

bool HasRequiredFields(const fuchsia_ui_pointerinjector::wire::PointerSample& pointer) {
  return pointer.has_pointer_id() && pointer.has_phase() && pointer.has_position_in_viewport();
}

bool AreValidExtents(const fidl::Array<fidl::Array<float, 2>, 2>& extents) {
  for (auto& point : extents) {
    for (float f : point) {
      if (!std::isfinite(f)) {
        return false;
      }
    }
  }

  const float min_x = extents[0][0];
  const float min_y = extents[0][1];
  const float max_x = extents[1][0];
  const float max_y = extents[1][1];
  return std::isless(min_x, max_x) && std::isless(min_y, max_y);
}

}  // namespace

Injector::Injector(std::shared_ptr<view_tree::SnapshotHolder> snapshot_holder,
                   inspect::Node inspect_node, InjectorSettings settings, Viewport viewport,
                   fidl::ServerEnd<fuchsia_ui_pointerinjector::Device> device,
                   fit::function<void()> on_channel_closed)
    : snapshot_holder_(std::move(snapshot_holder)),
      settings_(std::move(settings)),
      viewport_(std::move(viewport)),
      binding_(async_get_default_dispatcher(), std::move(device), this,
               [this](fidl::UnbindInfo info) {
                 if (info.status() != ZX_OK && info.status() != ZX_ERR_PEER_CLOSED) {
                   FX_LOGS(ERROR) << "Injector : Unbind handler called with status: "
                                  << info.status();
                 }
                 // Clean up ongoing streams before calling the supplied error handler.
                 if (!ongoing_streams_.empty()) {
                   auto snapshot = GetViewTreeSnapshot();
                   CancelOngoingStreams(*snapshot);
                 }
                 // NOTE: Triggers destruction of this object.
                 on_channel_closed_();
               }),
      on_channel_closed_(std::move(on_channel_closed)),
      inspector_(std::move(inspect_node)) {
  FX_DCHECK(snapshot_holder_);
  FX_LOGS(INFO) << "Injector : Registered new injector with "
                << " Device Id: " << settings_.device_id
                << " Device Type: " << static_cast<uint32_t>(settings_.device_type)
                << " Dispatch Policy: " << static_cast<uint32_t>(settings_.dispatch_policy)
                << " Context koid: " << settings_.context_koid
                << " and Target koid: " << settings_.target_koid;
}

void Injector::Inject(InjectRequestView request, InjectCompleter::Sync& completer) {
  TRACE_DURATION("input", "Injector::Inject");
  InjectEvents(request->events);
  if (!channel_closed_) {
    completer.Reply();
  }
}

// TODO(https://fxbug.dev/465440651): revisit if we need to add flow control here since there is
// no flow control on caller side.
void Injector::InjectEvents(InjectEventsRequestView request,
                            InjectEventsCompleter::Sync& completer) {
  InjectEvents(request->events);
  // One-way method; no reply expected.
}

void Injector::InjectEvents(fidl::VectorView<fuchsia_ui_pointerinjector::wire::Event> events) {
  TRACE_DURATION("input", "Injector::InjectEvents");

  auto snapshot = GetViewTreeSnapshot();

  if (!snapshot->IsDescendant(settings_.target_koid, settings_.context_koid)) {
    FX_LOGS(ERROR) << "Inject() called with Context (koid: " << settings_.context_koid
                   << ") and Target (koid: " << settings_.target_koid
                   << ") making an invalid hierarchy.";
    CloseChannel(ZX_ERR_BAD_STATE, *snapshot);
    return;
  }

  if (events.empty()) {
    FX_LOGS(ERROR) << "Inject() called without any events";
    CloseChannel(ZX_ERR_INVALID_ARGS, *snapshot);
    return;
  }

  for (auto& event : events) {
    TRACE_DURATION("input", "Injector::InjectEvents[event]");
    if (!event.has_timestamp() || !event.has_data()) {
      FX_LOGS(ERROR) << "Inject() called with an incomplete event";
      CloseChannel(ZX_ERR_INVALID_ARGS, *snapshot);
      return;
    }

    inspector_.OnPointerInjectorEvent(event);

    if (event.data().is_viewport()) {
      TRACE_DURATION("input", "Injector::InjectEvents[viewport]");
      const auto& new_viewport = event.data().viewport();

      {
        const zx_status_t result = IsValidViewport(new_viewport);
        if (result != ZX_OK) {
          // Errors printed inside IsValidViewport. Just close channel here.
          CloseChannel(result, *snapshot);
          return;
        }
      }

      // Update viewport from event data.
      viewport_ = {
          .extents = Extents(new_viewport.extents()),
          .context_from_viewport_transform = utils::ColumnMajorMat3ArrayToMat4(
              utils::ReinterpretFidlArrayAsStdArray(new_viewport.viewport_to_context_transform()))};
      continue;
    } else if (event.data().is_pointer_sample()) {
      TRACE_DURATION("input", "Injector::InjectEvents[pointer_sample]");
      const auto& pointer_sample = event.data().pointer_sample();

      const auto [result, stream_id] = ValidatePointerSample(pointer_sample);
      if (result != ZX_OK) {
        CloseChannel(result, *snapshot);
        return;
      }

      uint64_t trace_flow_id = event.has_trace_flow_id() ? event.trace_flow_id() : TRACE_NONCE();
      if (event.has_trace_flow_id()) {
        TRACE_FLOW_END("input", "dispatch_event_to_scenic", trace_flow_id);
      }
      TRACE_FLOW_BEGIN("input", "dispatch_event_to_client", trace_flow_id);

      ForwardEvent(event, stream_id, *snapshot, trace_flow_id);
      continue;
    } else {
      // Should be unreachable.
      FX_LOGS(WARNING) << "Unknown fuchsia::ui::pointerinjector::Data received";
    }
  }
}

std::pair<zx_status_t, StreamId> Injector::ValidatePointerSample(
    const fuchsia_ui_pointerinjector::wire::PointerSample& pointer_sample) {
  TRACE_DURATION("input", "Injector::ValidatePointerSample");
  if (!HasRequiredFields(pointer_sample)) {
    FX_LOGS(ERROR)
        << "Injected fuchsia::ui::pointerinjector::PointerSample was missing required fields";
    return {ZX_ERR_INVALID_ARGS, kInvalidStreamId};
  }

  const float x = pointer_sample.position_in_viewport()[0];
  const float y = pointer_sample.position_in_viewport()[1];
  if (!std::isfinite(x) || !std::isfinite(y)) {
    FX_LOGS(ERROR) << "fuchsia::ui::pointerinjector::PointerSample contained a NaN or inf value";
    return {ZX_ERR_INVALID_ARGS, kInvalidStreamId};
  }

  // Enforce event stream ordering rules. It keeps the event stream clean for downstream clients.
  const auto stream_id = ValidateEventStream(pointer_sample.pointer_id(), pointer_sample.phase());
  if (stream_id == kInvalidStreamId) {
    return {ZX_ERR_BAD_STATE, kInvalidStreamId};
  }

  return {ZX_OK, stream_id};
}

StreamId Injector::ValidateEventStream(uint32_t pointer_id, EventPhase phase) {
  TRACE_DURATION("input", "Injector::ValidateEventStream");
  const bool stream_is_ongoing = ongoing_streams_.contains(pointer_id);
  const bool double_add = stream_is_ongoing && phase == EventPhase::kAdd;
  const bool invalid_start = !stream_is_ongoing && phase != EventPhase::kAdd;
  if (double_add) {
    FX_LOGS(ERROR) << "Inject() called with invalid event stream: double-add, ptr-id: "
                   << pointer_id << ", stream-event-count: " << ongoing_streams_.count(pointer_id)
                   << ", phase: " << (int)phase;
    return kInvalidStreamId;
  }
  if (invalid_start) {
    FX_LOGS(ERROR) << "Inject() called with invalid event stream: invalid-start, ptr-id: "
                   << pointer_id << ", stream-event-count: " << ongoing_streams_.count(pointer_id)
                   << ", phase: " << (int)phase;
    return kInvalidStreamId;
  }

  // Update stream state.
  StreamId stream_id = kInvalidStreamId;
  if (phase == EventPhase::kAdd) {
    ongoing_streams_.emplace(pointer_id, NewStreamId());
    stream_id = ongoing_streams_.at(pointer_id);
  } else if (phase == EventPhase::kRemove || phase == EventPhase::kCancel) {
    stream_id = ongoing_streams_.at(pointer_id);
    ongoing_streams_.erase(pointer_id);
  } else {
    stream_id = ongoing_streams_.at(pointer_id);
  }

  FX_DCHECK(stream_id != kInvalidStreamId);
  return stream_id;
}

void Injector::CancelOngoingStreams(const view_tree::Snapshot& snapshot) {
  // Inject CANCEL event for each ongoing stream.
  for (const auto [pointer_id, stream_id] : ongoing_streams_) {
    CancelStream(pointer_id, stream_id, snapshot);
  }
  ongoing_streams_.clear();
}

void Injector::CloseChannel(zx_status_t epitaph, const view_tree::Snapshot& snapshot) {
  if (epitaph != ZX_OK) {
    FX_LOGS(ERROR) << "Injector : CloseChannel called with epitaph: " << epitaph;
  }
  channel_closed_ = true;
  // NOTE: the binding's `close_handler` also cancels streams.  However, receiving the `snapshot`
  // arg implies that the call stack above us has already checked out a `view_tree::SnapshotRef`, so
  // there will be a DCHECK if the binding's `close_handler` tries to check out its own ref.
  CancelOngoingStreams(snapshot);
  binding_.Close(epitaph);
}

zx_status_t Injector::IsValidViewport(const fuchsia_ui_pointerinjector::wire::Viewport& viewport) {
  TRACE_DURATION("input", "Injector::IsValidViewport");
  if (!viewport.has_extents() || !viewport.has_viewport_to_context_transform()) {
    FX_LOGS(ERROR) << "Provided fuchsia::ui::pointerinjector::Viewport had missing fields";
    return ZX_ERR_INVALID_ARGS;
  }

  if (!AreValidExtents(viewport.extents())) {
    FX_LOGS(ERROR)
        << "Provided fuchsia::ui::pointerinjector::Viewport had invalid extents. Extents min: {"
        << viewport.extents()[0][0] << ", " << viewport.extents()[0][1] << "} max: {"
        << viewport.extents()[1][0] << ", " << viewport.extents()[1][1] << "}";
    return ZX_ERR_INVALID_ARGS;
  }

  if (std::any_of(viewport.viewport_to_context_transform().begin(),
                  viewport.viewport_to_context_transform().end(),
                  [](float f) { return !std::isfinite(f); })) {
    FX_LOGS(ERROR) << "Provided fuchsia::ui::pointerinjector::Viewport "
                      "viewport_to_context_transform contained a NaN or infinity";
    return ZX_ERR_INVALID_ARGS;
  }

  // Must be invertible, i.e. determinant must be non-zero.
  const glm::mat4 viewport_to_context_transform = utils::ColumnMajorMat3ArrayToMat4(
      utils::ReinterpretFidlArrayAsStdArray(viewport.viewport_to_context_transform()));
  if (fabs(glm::determinant(viewport_to_context_transform)) <=
      std::numeric_limits<float>::epsilon()) {
    FX_LOGS(ERROR) << "Provided fuchsia::ui::pointerinjector::Viewport had a non-invertible matrix";
    return ZX_ERR_INVALID_ARGS;
  }

  return ZX_OK;
}
// LINT.ThenChange(//src/ui/scenic/lib/input/dso/injector.cc)

}  // namespace scenic_impl::input

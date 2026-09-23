// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/input/touch_system.h"

#include <fidl/fuchsia.ui.input.accessibility/cpp/fidl.h>
#include <fidl/fuchsia.ui.input/cpp/fidl.h>
#include <fidl/fuchsia.ui.pointer.augment/cpp/fidl.h>
#include <fidl/fuchsia.ui.pointer/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>
#include <zircon/status.h>

#include "src/lib/fsl/handles/object_info.h"
#include "src/ui/scenic/lib/input/constants.h"
#include "src/ui/scenic/lib/input/internal_pointer_event.h"
#include "src/ui/scenic/lib/input/touch_source.h"
#include "src/ui/scenic/lib/input/touch_source_v2.h"
#include "src/ui/scenic/lib/input/touch_source_with_local_hit.h"
#include "src/ui/scenic/lib/utils/check_is_on_thread.h"
#include "src/ui/scenic/lib/utils/helpers.h"
#include "src/ui/scenic/lib/utils/math.h"

#include <glm/glm.hpp>

namespace scenic_impl::input {

using AccessibilityPointerEvent = fuchsia_ui_input_accessibility::PointerEvent;

namespace {

// Helper function to build an AccessibilityPointerEvent when there is a
// registered accessibility listener.
AccessibilityPointerEvent BuildAccessibilityPointerEvent(const InternalTouchEvent& internal_event,
                                                         const glm::vec2& ndc_point,
                                                         const glm::vec2& local_point,
                                                         uint64_t viewref_koid) {
  AccessibilityPointerEvent event;
  event.event_time(internal_event.timestamp);
  event.device_id(internal_event.device_id);
  event.pointer_id(internal_event.pointer_id);
  event.type(fuchsia_ui_input::PointerEventType::kTouch);
  event.phase(InternalPhaseToGfxPhase(internal_event.phase));
  event.ndc_point(fuchsia_math::PointF(ndc_point.x, ndc_point.y));
  event.viewref_koid(viewref_koid);
  if (viewref_koid != ZX_KOID_INVALID) {
    event.local_point(fuchsia_math::PointF(local_point.x, local_point.y));
  }
  return event;
}

// Takes an InternalTouchEvent and returns a point in (Vulkan) Normalized Device Coordinates,
// in relation to the viewport. Intended for magnification
// TODO(https://fxbug.dev/42127641): Only here to allow the legacy a11y flow. Remove along with the
// legacy a11y code.
glm::vec2 GetViewportNDCPoint(const InternalTouchEvent& internal_event) {
  const float width = internal_event.viewport.extents.max.x - internal_event.viewport.extents.min.x;
  const float height =
      internal_event.viewport.extents.max.y - internal_event.viewport.extents.min.y;
  return {
      width > 0 ? 2.f * internal_event.position_in_viewport.x / width - 1 : 0,
      height > 0 ? 2.f * internal_event.position_in_viewport.y / height - 1 : 0,
  };
}

}  // namespace

TouchSystem::TouchSystem(async_dispatcher_t* input_dispatcher, HitTester& hit_tester,
                         inspect::Node& parent_node)
    : input_dispatcher_(input_dispatcher),
      hit_tester_(hit_tester),
      contender_inspector_(parent_node.CreateChild("GestureContenders")) {
  a11y_pointer_event_registry_.emplace(
      input_dispatcher,
      /*on_register=*/
      [this] {
        FX_CHECK(!contenders_.contains(a11y_contender_id_))
            << "on_disconnect must be called before registering a new listener";

        auto a11y_contender = std::make_unique<A11yLegacyContender>(
            /*respond*/
            [this](StreamId stream_id, GestureResponse response) {
              RecordGestureDisambiguationResponse(stream_id, a11y_contender_id_, {response});
            },
            /*deliver_to_client*/
            [this](const view_tree::Snapshot& snapshot, const InternalTouchEvent& event) {
              std::vector<fuchsia_ui_input_accessibility::PointerEvent> a11y_events;
              a11y_events.push_back(CreateAccessibilityEvent(snapshot, event));
              // Add in legacy UP and DOWN phases for ADD and REMOVE events respectively.
              const auto original_event = a11y_events.front();
              if (original_event.phase() == fuchsia_ui_input::PointerEventPhase::kAdd) {
                auto it = a11y_events.insert(a11y_events.end(), original_event);
                it->phase(fuchsia_ui_input::PointerEventPhase::kDown);
              } else if (original_event.phase() == fuchsia_ui_input::PointerEventPhase::kRemove) {
                auto it = a11y_events.insert(a11y_events.begin(), original_event);
                it->phase(fuchsia_ui_input::PointerEventPhase::kUp);
              }

              for (auto& a11y_event : a11y_events) {
                fuchsia_ui_input_accessibility::PointerEventListenerOnEventRequest request;
                request.pointer_event(std::move(a11y_event));
                auto result = accessibility_pointer_event_listener()->OnEvent(std::move(request));
                (void)result;
              }
            },
            contender_inspector_);
        a11y_pointer_event_registry_->set_on_stream_handled(
            [a11y_contender = a11y_contender.get()](
                uint32_t device_id, uint32_t pointer_id,
                fuchsia_ui_input_accessibility::EventHandling handled) {
              a11y_contender->OnStreamHandled(pointer_id, handled);
            });

        a11y_contender_ = a11y_contender.get();
        const auto [_, success] =
            contenders_.emplace(a11y_contender_id_, std::move(a11y_contender));
        FX_DCHECK(success) << "Duplicate A11yLegacyContender";
        FX_LOGS(INFO) << "A11yLegacyContender created.";
      },
      /*on_disconnect=*/
      [this] {
        FX_CHECK(contenders_.contains(a11y_contender_id_))
            << "can not disconnect before registering";
        // The listener disconnected. Release held events, delete the buffer.
        a11y_pointer_event_registry_->set_on_stream_handled(nullptr);
        EraseContender(a11y_contender_id_, ZX_KOID_INVALID);
        FX_LOGS(INFO) << "A11yLegacyContender destroyed";
      });
}

void TouchSystem::Bind(fidl::ServerEnd<fuchsia_ui_pointer_augment::LocalHit> server_end) {
  utils::CheckIsOnInputThread();
  local_hit_upgrade_registry_.AddBinding(input_dispatcher_, std::move(server_end), this,
                                         fidl::kIgnoreBindingClosure);
}

void TouchSystem::BindA11yPointerEventRegistry(
    fidl::ServerEnd<fuchsia_ui_input_accessibility::PointerEventRegistry> request) {
  utils::CheckIsOnInputThread();
  a11y_pointer_event_registry_->Bind(std::move(request));
}

zx_koid_t TouchSystem::FindViewRefKoidOfRelatedChannel(
    const fidl::ClientEnd<fuchsia_ui_pointer::TouchSource>& original) const {
  const zx_koid_t related_koid = fsl::GetRelatedKoid(original.channel().get());
  const auto it = std::find_if(
      contenders_.begin(), contenders_.end(),
      [related_koid](const auto& kv) { return kv.second->channel_koid() == related_koid; });
  return it == contenders_.end() ? ZX_KOID_INVALID : it->second->view_ref_koid_;
}

void TouchSystem::Upgrade(UpgradeRequest& request, UpgradeCompleter::Sync& completer) {
  // TODO(https://fxbug.dev/42165040): This currently requires the client to wait until the
  // TouchSource has been hooked up before making the Upgrade() call. This is not a great user
  // experience. Change this so we cache the channel if it arrives too early.
  auto original = std::move(request.original());

  auto reply_error = [&completer](fidl::ClientEnd<fuchsia_ui_pointer::TouchSource> original) {
    completer.Reply({{.augmented = {},
                      .error = std::make_unique<fuchsia_ui_pointer_augment::ErrorForLocalHit>(
                          fuchsia_ui_pointer_augment::ErrorReason::kDenied, std::move(original))}});
  };

  const zx_koid_t view_ref_koid = FindViewRefKoidOfRelatedChannel(original);
  if (view_ref_koid == ZX_KOID_INVALID) {
    reply_error(std::move(original));
    return;
  }

  // Delete the contender for the old channel.
  EraseContender(viewrefs_to_contender_ids_.at(view_ref_koid), view_ref_koid);

  // Create the new channel contender.
  const ContenderId contender_id = next_contender_id_++;
  zx::result endpoints =
      fidl::CreateEndpoints<fuchsia_ui_pointer_augment::TouchSourceWithLocalHit>();
  if (!endpoints.is_ok()) {
    FX_LOGS(ERROR) << "Failed to create endpoints for TouchSourceWithLocalHit";
    reply_error(std::move(original));
    return;
  }

  {
    const auto [_, success] = contenders_.emplace(
        contender_id,
        std::make_unique<TouchSourceWithLocalHit>(
            view_ref_koid, std::move(endpoints->server),
            /*respond*/
            [this, contender_id](StreamId stream_id,
                                 const std::vector<GestureResponse>& responses) {
              RecordGestureDisambiguationResponse(stream_id, contender_id, responses);
            },
            /*error_handler*/
            [this, contender_id, view_ref_koid] { EraseContender(contender_id, view_ref_koid); },
            /*get_local_hit*/
            [this](const view_tree::Snapshot& snapshot, const InternalTouchEvent& event) {
              // Perform a semantic hit test to find the top view a11y cares about.
              // TODO(https://fxbug.dev/42057941): If we have more than one TouchSourceWithLocalHit
              // client, this hit test will be done multiple times per injectiom redundantly. We
              // might need to improve this in the future, but as long as we're only expecting the
              // one client this is fine.
              const zx_koid_t top_koid =
                  hit_tester_.TopHitTest(snapshot, event, /*semantic_hit_test*/ true);
              glm::vec2 local_point = glm::vec2(0.f, 0.f);
              if (top_koid != ZX_KOID_INVALID) {
                const std::array<float, 9> top_view_from_viewport_transform =
                    GetDestinationFromViewportTransform(snapshot, event, top_koid);
                local_point = utils::TransformPointerCoords(
                    event.position_in_viewport,
                    utils::ColumnMajorMat3ArrayToMat4(top_view_from_viewport_transform));
              }
              return std::pair<zx_koid_t, std::array<float, 2>>{top_koid,
                                                                {local_point.x, local_point.y}};
            },
            contender_inspector_));
    FX_CHECK(success);
  }
  {
    const auto [_, success] = viewrefs_to_contender_ids_.emplace(view_ref_koid, contender_id);
    FX_CHECK(success);
  }

  // Return the new channel.
  completer.Reply({{.augmented = std::move(endpoints->client), .error = nullptr}});
}

fuchsia_ui_input_accessibility::PointerEvent TouchSystem::CreateAccessibilityEvent(
    const view_tree::Snapshot& snapshot, const InternalTouchEvent& event) {
  // Find top-hit target and send it to accessibility.
  const zx_koid_t view_ref_koid =
      hit_tester_.TopHitTest(snapshot, event, /*semantic_hit_test*/ true);

  glm::vec2 top_hit_view_local;
  if (view_ref_koid != ZX_KOID_INVALID) {
    std::optional<glm::mat4> view_from_context = snapshot.GetDestinationViewFromSourceViewTransform(
        /*source*/ event.context, /*destination*/ view_ref_koid);
    FX_DCHECK(view_from_context) << "could only happen if the view_tree_snapshot was updated "
                                    "between the event arriving and now";

    const glm::mat4 view_from_viewport =
        view_from_context.value() * event.viewport.context_from_viewport_transform;
    top_hit_view_local =
        utils::TransformPointerCoords(event.position_in_viewport, view_from_viewport);
  }
  const glm::vec2 ndc = GetViewportNDCPoint(event);

  return BuildAccessibilityPointerEvent(event, ndc, top_hit_view_local, view_ref_koid);
}

void TouchSystem::RegisterTouchSource(
    fidl::ServerEnd<fuchsia_ui_pointer::TouchSource> touch_source_server_end,
    zx_koid_t client_view_ref_koid) {
  TRACE_DURATION("input", "TouchSystem::RegisterTouchSource");
  utils::CheckIsOnInputThread();
  FX_DCHECK(client_view_ref_koid != ZX_KOID_INVALID);
  const ContenderId contender_id = next_contender_id_++;

  // Note: These closure must'nt be called in the constructor, since they depend on the
  // |contenders_| map, which isn't filled until after construction completes.
  {
    const auto [_, success] = contenders_.emplace(
        contender_id, std::make_unique<TouchSource>(
                          client_view_ref_koid, std::move(touch_source_server_end),
                          /*respond*/
                          [this, contender_id](StreamId stream_id,
                                               const std::vector<GestureResponse>& responses) {
                            RecordGestureDisambiguationResponse(stream_id, contender_id, responses);
                          },
                          /*error_handler*/
                          [this, contender_id, client_view_ref_koid] {
                            EraseContender(contender_id, client_view_ref_koid);
                          },
                          contender_inspector_));
    FX_DCHECK(success);
  }
  {
    const auto [_, success] =
        viewrefs_to_contender_ids_.emplace(client_view_ref_koid, contender_id);
    FX_DCHECK(success);
  }
}

void TouchSystem::RegisterTouchSourceV2(
    fidl::ServerEnd<fuchsia_ui_pointer::TouchSourceV2> touch_source_server_end,
    zx_koid_t client_view_ref_koid) {
  TRACE_DURATION("input", "TouchSystem::RegisterTouchSourceV2");
  utils::CheckIsOnInputThread();
  FX_DCHECK(client_view_ref_koid != ZX_KOID_INVALID);
  const ContenderId contender_id = next_contender_id_++;

  {
    const auto [it, success] = contenders_.emplace(
        contender_id,
        std::make_unique<TouchSourceV2>(
            input_dispatcher_, client_view_ref_koid, std::move(touch_source_server_end),
            /*respond*/
            [this, contender_id](StreamId stream_id,
                                 const std::vector<GestureResponse>& responses) {
              RecordGestureDisambiguationResponse(stream_id, contender_id, responses);
            },
            /*error_handler*/
            [this, contender_id, client_view_ref_koid] {
              EraseContender(contender_id, client_view_ref_koid);
            },
            contender_inspector_));
    FX_DCHECK(success);
  }
  {
    const auto [it, success] =
        viewrefs_to_contender_ids_.emplace(client_view_ref_koid, contender_id);
    FX_DCHECK(success);
  }
}

void TouchSystem::InjectTouchEventExclusive(InternalTouchEvent event, StreamId stream_id,
                                            const view_tree::Snapshot& snapshot) {
  if (!snapshot.view_tree.contains(event.target) &&
      !snapshot.unconnected_views.contains(event.target)) {
    FX_DCHECK(!contenders_.contains(static_cast<int>(event.target)));
    return;
  }
  FX_DCHECK(event.phase == Phase::kCancel || snapshot.IsDescendant(event.target, event.context))
      << "Should never allow injection of non-cancel events into broken scene graph";

  auto it = viewrefs_to_contender_ids_.find(event.target);
  if (it != viewrefs_to_contender_ids_.end()) {
    const ContenderId contender_id = it->second;
    auto contender_it = contenders_.find(contender_id);
    if (contender_it == contenders_.end()) {
      return;
    }
    GestureContender* contender = contender_it->second.get();
    // Calling EndContest() before the first event causes them to be combined in the first message
    // to the client.
    if (event.phase == Phase::kAdd) {
      contender->EndContest(stream_id, /*awarded_win=*/true);
    }

    if (!contenders_.contains(contender_id)) {
      return;
    }

    // If the target is not in the view tree then this must be a cancel event and we don't need to
    // (and can't) supply correct transforms and bounding boxes.
    if (!snapshot.view_tree.contains(event.target)) {
      FX_DCHECK(event.phase == Phase::kCancel);
      contender->UpdateStream(snapshot, stream_id, std::move(event), /*is_end_of_stream=*/true,
                              /*bounding_box=*/{});
    } else {
      contender->UpdateStream(
          snapshot, stream_id,
          EventWithReceiverFromViewportTransform<InternalTouchEvent>(snapshot, std::move(event),
                                                                     event.target),
          /*is_end_of_stream=*/event.phase == Phase::kRemove || event.phase == Phase::kCancel,
          snapshot.view_tree.at(event.target).bounding_box);
    }
  } else {
    FX_NOTREACHED();
  }
}

// The touch state machine comprises ADD/DOWN/MOVE*/UP/REMOVE. Some notes:
//  - We assume one touchscreen device, and use the device-assigned finger ID.
//  - Touch ADD associates the following ADD/DOWN/MOVE*/UP/REMOVE event sequence
//    with the set of clients available at that time. To enable gesture
//    disambiguation, we perform parallel dispatch to all clients.
//  - Touch DOWN triggers a focus change, honoring the "may receive focus" property.
//  - Touch REMOVE drops the association between event stream and client.
void TouchSystem::InjectTouchEventHitTested(InternalTouchEvent event, StreamId stream_id,
                                            const view_tree::Snapshot& snapshot) {
  if (auto winner_it = stream_winners_.find(stream_id); winner_it != stream_winners_.end()) {
    DeliverToWinner(snapshot, std::move(event), stream_id, *winner_it->second);
    return;
  }

  // New stream. Collect contenders and set up a new arena.
  if (event.phase == Phase::kAdd) {
    std::vector<ContenderId> contenders = CollectContenders(snapshot, stream_id, event);
    if (!contenders.empty()) {
      const bool is_single_contender = contenders.size() == 1;
      const ContenderId front_contender = contenders.front();
      if (is_single_contender) {
        auto contender_it = contenders_.find(front_contender);
        FX_DCHECK(contender_it != contenders_.end());
        if (contender_it != contenders_.end()) {
          GestureContender* contender = contender_it->second.get();
          contender->EndContest(stream_id, /*awarded_win=*/true);
          if (contenders_.contains(front_contender)) {
            stream_winners_[stream_id] = contender;
            DeliverToWinner(snapshot, std::move(event), stream_id, *contender);
          }
        }
        return;
      } else {
        const auto [it, success] =
            gesture_arenas_.emplace(stream_id, GestureArena{std::move(contenders)});
        FX_DCHECK(success);
      }
    }
  }

  // No arena means the contest is over and no one won.
  if (!gesture_arenas_.contains(stream_id)) {
    return;
  }

  UpdateGestureContest(snapshot, std::move(event), stream_id);
}

static bool IsRootOrDirectChildOfRoot(zx_koid_t koid, const view_tree::Snapshot& snapshot) {
  if (snapshot.root == koid) {
    return true;
  }
  if (!snapshot.view_tree.contains(koid)) {
    return false;
  }

  return snapshot.view_tree.at(koid).parent == snapshot.root;
}

std::vector<zx_koid_t> TouchSystem::GetAncestorChainTopToBottom(const view_tree::Snapshot& snapshot,
                                                                zx_koid_t bottom,
                                                                zx_koid_t top) const {
  if (bottom == top) {
    return {bottom};
  }

  // Get ancestors bottom closest to furthest.
  std::vector<zx_koid_t> ancestors = snapshot.GetAncestorsOf(bottom);
  FX_DCHECK(ancestors.empty() || std::any_of(ancestors.begin(), ancestors.end(),
                                             [top](const zx_koid_t koid) { return koid == top; }))
      << "|top| must be an ancestor of |bottom|";

  // Remove all ancestors after |top|.
  for (auto it = ancestors.begin(); it != ancestors.end(); ++it) {
    if (*it == top) {
      ancestors.erase(++it, ancestors.end());
      break;
    }
  }

  // Reverse the list and add |bottom| to the end.
  std::reverse(ancestors.begin(), ancestors.end());
  ancestors.emplace_back(bottom);
  FX_DCHECK(ancestors.front() == top);

  return ancestors;
}

std::vector<ContenderId> TouchSystem::CollectContenders(const view_tree::Snapshot& snapshot,
                                                        StreamId stream_id,
                                                        const InternalTouchEvent& event) {
  TRACE_DURATION("input", "TouchSystem::CollectContenders");
  std::vector<ContenderId> contenders;

  // Add an A11yLegacyContender if the injection context is the root of the ViewTree.
  // TODO(https://fxbug.dev/42127641): Remove when a11y is a native GD client.
  if (contenders_.contains(a11y_contender_id_) &&
      IsRootOrDirectChildOfRoot(event.context, snapshot)) {
    contenders.push_back(a11y_contender_id_);
  }

  const zx_koid_t top_koid = hit_tester_.TopHitTest(snapshot, event, /*semantic_hit_test*/ false);
  if (top_koid != ZX_KOID_INVALID) {
    // Find TouchSource contenders in priority order from furthest (valid) ancestor to top hit view.
    const std::vector<zx_koid_t> ancestors =
        GetAncestorChainTopToBottom(snapshot, top_koid, event.target);
    for (const auto koid : ancestors) {
      // If a touch contender doesn't exist it means the client didn't provide a TouchSource
      // endpoint.
      const auto it = viewrefs_to_contender_ids_.find(koid);
      if (it != viewrefs_to_contender_ids_.end()) {
        const ContenderId contender_id = it->second;
        FX_DCHECK(contenders_.contains(contender_id));
        contenders.push_back(contender_id);
      }
    }
  }

  return contenders;
}

void TouchSystem::UpdateGestureContest(const view_tree::Snapshot& snapshot,
                                       InternalTouchEvent event, StreamId stream_id) {
  TRACE_DURATION("input", "TouchSystem::UpdateGestureContest");
  const auto initial_arena_it = gesture_arenas_.find(stream_id);
  if (initial_arena_it == gesture_arenas_.end()) {
    // Contest already ended, with no winner.
    return;
  }

  const bool is_end_of_stream = event.phase == Phase::kRemove || event.phase == Phase::kCancel;
  initial_arena_it->second.UpdateStream(/*length*/ 1, is_end_of_stream);

  // Update remaining contenders.
  // Copy the vector to avoid problems if the arena is destroyed inside of UpdateStream().
  const std::vector<ContenderId> contenders = initial_arena_it->second.contenders();
  for (const auto contender_id : contenders) {
    // Don't use the arena obtained above the loop, because it may have been removed from
    // gesture_arenas_ in a previous loop iteration.
    const auto arena_it = gesture_arenas_.find(stream_id);
    if (arena_it == gesture_arenas_.end()) {
      // Break out of the loop: if we didn't find the arena in this iteration, we won't find it in
      // subsequent iterations either.
      break;
    }
    if (arena_it->second.contest_has_ended() && !arena_it->second.contains(contender_id)) {
      // Contest ended with this contender not being the winner; no need to consider it further.
      continue;
    }
    const auto it = contenders_.find(contender_id);
    if (it == contenders_.end()) {
      // This contender is no longer present, probably because the client has disconnected.
      continue;
    }

    GestureContender& contender = *it->second;
    const zx_koid_t view_ref_koid = contender.view_ref_koid_;
    if (snapshot.view_tree.contains(view_ref_koid)) {
      // Everything is fine. Send as normal.
      InternalTouchEvent event_copy = event.ShallowClone();
      if (event.wake_lease) {
        zx_status_t status =
            event.wake_lease.duplicate(ZX_RIGHT_SAME_RIGHTS, &event_copy.wake_lease);
        if (status != ZX_OK) {
          FX_PLOGS(ERROR, status) << "failed to duplicate wake lease";
        }
      }
      contender.UpdateStream(snapshot, stream_id,
                             EventWithReceiverFromViewportTransform<InternalTouchEvent>(
                                 snapshot, std::move(event_copy), view_ref_koid),
                             is_end_of_stream, snapshot.view_tree.at(view_ref_koid).bounding_box);
    } else if (contender_id == a11y_contender_id_) {
      // Send a clone of the event without transferring any possible wake lease.
      // TODO(https://fxbug.dev/42127641): A11yLegacyContender doesn't need correct transforms or
      // view bounds. Remove this branch when legacy a11y api goes away.
      contender.UpdateStream(snapshot, stream_id, event.ShallowClone(), is_end_of_stream,
                             /*bounding_box=*/{});
    } else {
      // Contender not in the view tree -> cancel the rest of the stream for that contender.
      if (!arena_it->second.contest_has_ended()) {
        // Contest ongoing -> just send a no response on behalf of |contender_id|.
        RecordGestureDisambiguationResponse(stream_id, contender_id, {GestureResponse::kNo});
        const auto current_arena_it = gesture_arenas_.find(stream_id);
        FX_DCHECK(current_arena_it == gesture_arenas_.end() ||
                  !current_arena_it->second.contains(contender_id));
      } else {
        // Contest ended -> Need to send an explicit "cancel" event to the contender.
        FX_DCHECK(arena_it->second.contenders().size() == 1 &&
                  arena_it->second.contains(contender_id));
        FX_DCHECK(event.phase != Phase::kAdd);

        // Send a clone of the event without transferring any possible wake lease.
        InternalTouchEvent event_copy = event.ShallowClone();
        event_copy.phase = Phase::kCancel;
        contender.UpdateStream(snapshot, stream_id, std::move(event_copy),
                               /*is_end_of_stream=*/true,
                               /*bounding_box=*/{});
        // The contest is definitely over, so we can manually destroy the arena here.
        gesture_arenas_.erase(stream_id);
        break;
      }
    }
  }

  DestroyArenaIfComplete(stream_id);
}

void TouchSystem::DeliverToWinner(const view_tree::Snapshot& snapshot, InternalTouchEvent event,
                                  StreamId stream_id, GestureContender& contender) {
  const bool is_end_of_stream = event.phase == Phase::kRemove || event.phase == Phase::kCancel;
  if (is_end_of_stream) {
    stream_winners_.erase(stream_id);
  }

  if (&contender == a11y_contender_) {
    // Send a clone of the event without transferring any possible wake lease.
    // The legacy accessibility pointer event listener is a global listener that does not require
    // correct coordinate transforms or view bounds (its target view ref KOID is ZX_KOID_INVALID).
    contender.UpdateStream(snapshot, stream_id, event.ShallowClone(), is_end_of_stream,
                           /*bounding_box=*/{});
    return;
  }

  const zx_koid_t view_ref_koid = contender.view_ref_koid_;
  if (!snapshot.view_tree.contains(view_ref_koid)) {
    // Contender left view tree, send Cancel.
    stream_winners_.erase(stream_id);
    InternalTouchEvent event_copy = event.ShallowClone();
    event_copy.phase = Phase::kCancel;
    contender.UpdateStream(snapshot, stream_id, std::move(event_copy),
                           /*is_end_of_stream=*/true,
                           /*bounding_box=*/{});
    return;
  }

  contender.UpdateStream(snapshot, stream_id,
                         EventWithReceiverFromViewportTransform<InternalTouchEvent>(
                             snapshot, std::move(event), view_ref_koid),
                         is_end_of_stream, snapshot.view_tree.at(view_ref_koid).bounding_box);
}

void TouchSystem::RecordGestureDisambiguationResponse(
    StreamId stream_id, ContenderId contender_id, const std::vector<GestureResponse>& responses) {
  auto arena_it = gesture_arenas_.find(stream_id);
  if (arena_it == gesture_arenas_.end() || !arena_it->second.contains(contender_id)) {
    return;
  }

  // No need to record after the contest has ended.
  if (!arena_it->second.contest_has_ended()) {
    // Update the arena.
    const ContestResults result = arena_it->second.RecordResponses(contender_id, responses);
    for (auto loser_id : result.losers) {
      // Need to check for existence, since a loser could be the result of a NO response upon
      // destruction.
      auto contender_it = contenders_.find(loser_id);
      if (contender_it != contenders_.end()) {
        contender_it->second->EndContest(stream_id, /*awarded_win*/ false);
      }
    }
    if (result.winner) {
      const ContenderId winner_id = result.winner.value();
      auto winner_it = contenders_.find(winner_id);
      if (winner_it != contenders_.end()) {
        GestureContender* winner = winner_it->second.get();
        winner->EndContest(stream_id, /*awarded_win=*/true);
        // Re-check arena and winner existence after winner->EndContest() which might re-enter.
        auto current_arena_it = gesture_arenas_.find(stream_id);
        if (current_arena_it != gesture_arenas_.end() &&
            !current_arena_it->second.stream_has_ended()) {
          if (contenders_.contains(winner_id)) {
            stream_winners_[stream_id] = winner;
          }
        }
      }
      gesture_arenas_.erase(stream_id);
      return;
    }
  }

  DestroyArenaIfComplete(stream_id);
}

void TouchSystem::DestroyArenaIfComplete(StreamId stream_id) {
  const auto arena_it = gesture_arenas_.find(stream_id);
  if (arena_it == gesture_arenas_.end()) {
    return;
  }

  const auto& arena = arena_it->second;

  // This branch will eventually be taken for every arena.
  // TODO(https://fxbug.dev/42171409): can we elaborate on why this is true?
  if (arena.contenders().empty() || (arena.contest_has_ended() && arena.stream_has_ended())) {
    gesture_arenas_.erase(stream_id);
  }
}

void TouchSystem::EraseContender(ContenderId contender_id, zx_koid_t view_ref_koid) {
  auto contender_it = contenders_.find(contender_id);
  FX_DCHECK(contender_it != contenders_.end()) << "Contender " << contender_id << " did not exist";
  if (contender_it == contenders_.end()) {
    return;
  }
  GestureContender* contender_ptr = contender_it->second.get();
  if (contender_ptr == a11y_contender_) {
    a11y_contender_ = nullptr;
  }

  // Remove from stream_winners_ if it was the winner.
  std::erase_if(stream_winners_,
                [contender_ptr](const auto& item) { return item.second == contender_ptr; });

  // TODO(https://fxbug.dev/42142976): ZX_KOID_INVALID is only passed in by legacy contenders.
  // Remove this check when they go away.
  if (view_ref_koid != ZX_KOID_INVALID) {
    const size_t success = viewrefs_to_contender_ids_.erase(view_ref_koid);
    FX_DCHECK(success) << "ViewRef " << view_ref_koid << " was not mapped to a ContenderId";
  }

  // Remove from any contests it may still be a part of.
  // Note: Need to finish walking the arena map before we start calling RecordGDResponse() since
  // it may invalidate the iterator.
  std::vector<StreamId> ongoing_streams;
  for (const auto& [stream_id, arena] : gesture_arenas_) {
    const auto contenders = arena.contenders();
    if (std::count(contenders.begin(), contenders.end(), contender_id)) {
      ongoing_streams.push_back(stream_id);
    }
  }

  contenders_.erase(contender_it);

  for (const auto stream_id : ongoing_streams) {
    RecordGestureDisambiguationResponse(stream_id, contender_id, {GestureResponse::kNo});
  }
}

}  // namespace scenic_impl::input

// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_INPUT_TOUCH_SYSTEM_H_
#define SRC_UI_SCENIC_LIB_INPUT_TOUCH_SYSTEM_H_

#include <fidl/fuchsia.ui.pointer.augment/cpp/fidl.h>
#include <fidl/fuchsia.ui.pointer/cpp/fidl.h>
#include <lib/async/dispatcher.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/sys/cpp/component_context.h>

#include <map>
#include <mutex>
#include <optional>

#include "src/lib/fxl/synchronization/thread_annotations.h"
#include "src/ui/scenic/lib/input/a11y_legacy_contender.h"
#include "src/ui/scenic/lib/input/a11y_registry.h"
#include "src/ui/scenic/lib/input/constants.h"
#include "src/ui/scenic/lib/input/gesture_arena.h"
#include "src/ui/scenic/lib/input/helper.h"
#include "src/ui/scenic/lib/input/hit_tester.h"
#include "src/ui/scenic/lib/input/touch_source_base.h"
#include "src/ui/scenic/lib/view_tree/snapshot_types.h"

namespace scenic_impl::input {

// Tracks input APIs.
//
// Thread-unsafe.  All methods are expected to be called from the same thread (the "input thread").
// The only exception is that `SnapshotHolder` can be updated from the main thread.
class TouchSystem : public fidl::Server<fuchsia_ui_pointer_augment::LocalHit> {
 public:
  explicit TouchSystem(async_dispatcher_t* input_dispatcher, HitTester& hit_tester,
                       inspect::Node& parent_node);
  ~TouchSystem() = default;

  void Bind(fidl::ServerEnd<fuchsia_ui_pointer_augment::LocalHit> server_end);
  void BindA11yPointerEventRegistry(
      fidl::ServerEnd<fuchsia_ui_input_accessibility::PointerEventRegistry> request);

  fidl::Client<fuchsia_ui_input_accessibility::PointerEventListener>&
  accessibility_pointer_event_listener() {
    return a11y_pointer_event_registry_->accessibility_pointer_event_listener();
  }

  void RegisterTouchSource(fidl::ServerEnd<fuchsia_ui_pointer::TouchSource> touch_source_server_end,
                           zx_koid_t client_view_ref_koid);

  void RegisterTouchSourceV2(
      fidl::ServerEnd<fuchsia_ui_pointer::TouchSourceV2> touch_source_server_end,
      zx_koid_t client_view_ref_koid);

  // |fidl::Server<fuchsia_ui_pointer_augment::LocalHit>|
  void Upgrade(UpgradeRequest& request, UpgradeCompleter::Sync& completer) override;

  // For tests.
  // TODO(https://fxbug.dev/42152433): Remove when integration tests are properly separated out.
  void RegisterA11yListener(
      fidl::ClientEnd<fuchsia_ui_input_accessibility::PointerEventListener> listener,
      fit::function<void(bool)> callback) {
    callback(a11y_pointer_event_registry_->RegisterListener(std::move(listener)));
  }

  // Injects a touch event directly to the View with koid |event.target|.
  void InjectTouchEventExclusive(InternalTouchEvent event, StreamId stream_id,
                                 const view_tree::Snapshot& snapshot);
  // Injects a touch event by hit testing for appropriate targets.
  void InjectTouchEventHitTested(InternalTouchEvent event, StreamId stream_id,
                                 const view_tree::Snapshot& snapshot);

 private:
  // Finds the ViewRef koid registered with the other side of the |original| channel and returns it.
  // Returns ZX_KOID_INVALID if the related channel isn't found.
  zx_koid_t FindViewRefKoidOfRelatedChannel(
      const fidl::ClientEnd<fuchsia_ui_pointer::TouchSource>& original) const;

  fuchsia_ui_input_accessibility::PointerEvent CreateAccessibilityEvent(
      const view_tree::Snapshot& snapshot, const InternalTouchEvent& event);

  // Collects all the GestureContenders for a new touch event stream.
  std::vector<ContenderId> CollectContenders(const view_tree::Snapshot& snapshot,
                                             StreamId stream_id, const InternalTouchEvent& event);

  // Updates the gesture arena and all contenders for stream |stream_id| with a new event.
  void UpdateGestureContest(const view_tree::Snapshot& snapshot, InternalTouchEvent event,
                            StreamId stream_id);

  // Delivers the event directly to the winner of the stream, bypassing the gesture arena.
  void DeliverToWinner(const view_tree::Snapshot& snapshot, InternalTouchEvent event,
                       StreamId stream_id, GestureContender& contender);

  // Records a set of responses from a gesture disambiguation contender.
  void RecordGestureDisambiguationResponse(StreamId stream_id, ContenderId contender_id,
                                           const std::vector<GestureResponse>& responses);

  // Destroy the arena if the contest is complete (i.e. no contenders left or contest over and
  // stream ended).
  void DestroyArenaIfComplete(StreamId stream_id);

  // Destroys contender specified by |contender_id| and removes it from all contests.
  void EraseContender(ContenderId contender_id, zx_koid_t view_ref_koid);

  std::vector<zx_koid_t> GetAncestorChainTopToBottom(const view_tree::Snapshot& snapshot,
                                                     zx_koid_t bottom, zx_koid_t top) const;

  async_dispatcher_t* input_dispatcher_ = nullptr;
  HitTester& hit_tester_;

  // An inspector that tracks all GestureContenders, so data can persist past contender lifetimes.
  // Must outlive all contenders.
  GestureContenderInspector contender_inspector_;

  /// FIDL server implementations.
  std::optional<A11yPointerEventRegistry> a11y_pointer_event_registry_;
  fidl::ServerBindingGroup<fuchsia_ui_pointer_augment::LocalHit> local_hit_upgrade_registry_;

  //// Gesture disambiguation state
  // Whenever a new touch event stream is started (by the injection of an ADD event) we create a
  // GestureArena to track that stream, and select a number of contenders to participate in the
  // contest. All contenders are tracked in the |contenders_| map for the duration of their
  // lifetime. The |contenders_| map is relied upon by the |gesture_arenas_| to deliver events.

  // Each gesture arena tracks one touch event stream and a set of contenders.
  std::unordered_map<StreamId, GestureArena> gesture_arenas_;

  // Map of active streams to their resolved winner.
  std::unordered_map<StreamId, GestureContender*> stream_winners_;

  // Map of all active contenders.
  std::unordered_map<ContenderId, std::unique_ptr<GestureContender>> contenders_;

  // Pointer to the legacy a11y contender, if registered.
  GestureContender* a11y_contender_ = nullptr;

  // Map of ViewRef koids to ContenderIds.
  // Does not include ContenderIds for the A11yLegacyContender, since no View is
  // uniquely associated with it.
  std::unordered_map<zx_koid_t, ContenderId> viewrefs_to_contender_ids_;

  const ContenderId a11y_contender_id_ = 1;
  ContenderId next_contender_id_ = 2;
};

}  // namespace scenic_impl::input

#endif  // SRC_UI_SCENIC_LIB_INPUT_TOUCH_SYSTEM_H_

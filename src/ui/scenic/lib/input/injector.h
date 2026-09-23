// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_INPUT_INJECTOR_H_
#define SRC_UI_SCENIC_LIB_INPUT_INJECTOR_H_

#include <fidl/fuchsia.ui.pointerinjector/cpp/wire.h>
#include <lib/async/cpp/task.h>
#include <lib/fidl/cpp/wire/server.h>
#include <lib/inspect/cpp/inspect.h>

#include <deque>
#include <unordered_map>

#include "src/lib/fxl/macros.h"
#include "src/ui/scenic/lib/input/internal_pointer_event.h"
#include "src/ui/scenic/lib/input/stream_id.h"
#include "src/ui/scenic/lib/view_tree/snapshot_holder.h"

namespace scenic_impl::input {

// Non-FIDL-type struct for keeping client defined settings.
struct InjectorSettings {
  fuchsia_ui_pointerinjector::wire::DispatchPolicy dispatch_policy =
      fuchsia_ui_pointerinjector::wire::DispatchPolicy(0u);
  uint32_t device_id = 0u;
  fuchsia_ui_pointerinjector::wire::DeviceType device_type =
      fuchsia_ui_pointerinjector::wire::DeviceType(0u);
  zx_koid_t context_koid = ZX_KOID_INVALID;
  zx_koid_t target_koid = ZX_KOID_INVALID;

  std::optional<fuchsia_input::wire::Axis> scroll_v_range;
  std::optional<fuchsia_input::wire::Axis> scroll_h_range;
  std::vector<uint8_t> button_identifiers;
};

// Utility that Injectors use to send diagnostics to Inspect.
class InjectorInspector {
 public:
  explicit InjectorInspector(inspect::Node inspect_node);

  void OnPointerInjectorEvent(const fuchsia_ui_pointerinjector::wire::Event& event);

  // How long to track injection history.
  static constexpr uint64_t kNumMinutesOfHistory = 10;

 private:
  struct InspectHistory {
    // The minute this was recorded during. Used as the key for appending new values.
    uint64_t minute_key;
    // Number of injected events during |minute_key|.
    uint64_t num_injected_events;
  };

  void UpdateHistory(zx::time now);
  void ReportStats(inspect::Inspector& inspector) const;

  inspect::Node node_;
  inspect::LazyNode history_stats_node_;

  std::deque<InspectHistory> history_;
  zx::time last_event_timestamp_{0};

  FXL_DISALLOW_COPY_AND_ASSIGN(InjectorInspector);
};

// Implementation of the |fuchsia::ui::pointerinjector::Device| interface. One instance per channel.
// LINT.IfChange
class Injector : public fidl::WireServer<fuchsia_ui_pointerinjector::Device> {
 public:
  Injector(std::shared_ptr<view_tree::SnapshotHolder> snapshot_holder, inspect::Node inspect_node,
           InjectorSettings settings, Viewport viewport,
           fidl::ServerEnd<fuchsia_ui_pointerinjector::Device> device,
           fit::function<void()> on_channel_closed);

  // Check the validity of a Viewport.
  // Returns ZX_OK if valid, otherwise logs an error message and return appropriate error code.
  static zx_status_t IsValidViewport(const fuchsia_ui_pointerinjector::wire::Viewport& viewport);

  // |fidl::WireServer<fuchsia_ui_pointerinjector::Device>|
  void Inject(InjectRequestView request, InjectCompleter::Sync& completer) override;
  void InjectEvents(InjectEventsRequestView request,
                    InjectEventsCompleter::Sync& completer) override;

  void InjectEvents(fidl::VectorView<fuchsia_ui_pointerinjector::wire::Event> events);

 protected:
  // Forwards the event to device-specific handler in InputSystem (and eventually the client).
  virtual void ForwardEvent(fuchsia_ui_pointerinjector::wire::Event& event, StreamId stream_id,
                            const view_tree::Snapshot& snapshot, uint64_t trace_flow_id) = 0;

  // Sends an appropriate Cancel event.
  virtual void CancelStream(uint32_t pointer_id, StreamId stream_id,
                            const view_tree::Snapshot& snapshot) = 0;

  const InjectorSettings& settings() const { return settings_; }
  const Viewport& viewport() const { return viewport_; }

 private:
  // Should be called only once in a single call stack. The snapshot should be
  // passed down into helper functions, rather than re-obtaining it, to ensure
  // that a consistent snapshot is being used.
  view_tree::SnapshotRef GetViewTreeSnapshot() { return snapshot_holder_->GetSnapshot(); }

  // Return value is either both valid, {ZX_OK, valid stream id} or both
  // invalid: {error, kInvalidStreamId}
  std::pair<zx_status_t, StreamId> ValidatePointerSample(
      const fuchsia_ui_pointerinjector::wire::PointerSample& pointer_sample);

  // Tracks event streams. Returns the id of the event stream if the stream is valid
  // and kInvalidStreamId otherwise.
  // Event streams are expected to start with an ADD, followed by a number of CHANGE events, and
  // ending in either a REMOVE or a CANCEL. Anything else is invalid.
  StreamId ValidateEventStream(uint32_t pointer_id,
                               fuchsia_ui_pointerinjector::wire::EventPhase phase);

  // Injects a CANCEL event for each ongoing stream and stops tracking them.
  void CancelOngoingStreams(const view_tree::Snapshot& snapshot);

  // Closes the fidl channel. This triggers the destruction of the Injector object through the
  // error handler set in InputSystem.
  // NOTE: No further method calls or member accesses should be made after CloseChannel(), since
  // they might be made on a destroyed object.
  void CloseChannel(zx_status_t epitaph, const view_tree::Snapshot& snapshot);

  const std::shared_ptr<view_tree::SnapshotHolder> snapshot_holder_;

  // Client-defined data.
  const InjectorSettings settings_;
  Viewport viewport_;

  fidl::ServerBinding<fuchsia_ui_pointerinjector::Device> binding_;
  // Set to true when CloseChannel() is called during validation failures.
  // Under C++ Wire bindings, Close() asynchronously schedules channel unbinding. Since method
  // execution continues synchronously in the current stack frame, this flag is used to suppress
  // sending a trailing method response (completer.Reply()) on a channel scheduled for closure.
  bool channel_closed_ = false;

  // Tracks stream's status (per stream id) as it moves through its state machine. Used to
  // validate each event's phase.
  // - ADD: add stream to set
  // - CHANGE: no-op
  // - REMOVE/CANCEL: remove stream from set.
  // Hence, each stream here matches ADD - CHANGE*.
  std::unordered_map<uint32_t, StreamId> ongoing_streams_;

  // Called both when an error is triggered by either the remote or the local side of the channel.
  // Triggers destruction of this object.
  const fit::function<void()> on_channel_closed_;

  InjectorInspector inspector_;
};
// LINT.ThenChange(//src/ui/scenic/lib/input/dso/injector.h)

}  // namespace scenic_impl::input

#endif  // SRC_UI_SCENIC_LIB_INPUT_INJECTOR_H_

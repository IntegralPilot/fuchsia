// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_FOCUS_FOCUS_MANAGER_H_
#define SRC_UI_SCENIC_LIB_FOCUS_FOCUS_MANAGER_H_

#include <fidl/fuchsia.ui.focus/cpp/fidl.h>
#include <fidl/fuchsia.ui.views/cpp/fidl.h>
#include <lib/async/default.h>
#include <lib/async/dispatcher.h>
#include <lib/inspect/cpp/inspect.h>

#include <memory>
#include <unordered_map>
#include <vector>

#include "src/lib/fxl/memory/weak_ptr.h"
#include "src/ui/scenic/lib/focus/view_focuser_registry.h"
#include "src/ui/scenic/lib/focus/view_ref_focused_registry.h"
#include "src/ui/scenic/lib/view_tree/snapshot_holder.h"

namespace focus {

// Provide detail on if/why focus change request was denied.
// Specific error-handling policy is responsibility of caller.
enum class FocusChangeStatus {
  kAccept = 0,
  kErrorRequesterInvalid,
  kErrorRequestInvalid,
  kErrorRequesterNotAuthorized,
  kErrorRequesterNotRequestAncestor,
  kErrorRequestCannotReceiveFocus,
  kErrorUnhandledCase,  // last
};

// Class for tracking focus state.
class FocusManager final : public fidl::Server<fuchsia_ui_focus::FocusChainListenerRegistry> {
 public:
  explicit FocusManager(async_dispatcher_t* input_dispatcher,
                        std::shared_ptr<view_tree::SnapshotHolder> snapshot_holder,
                        inspect::Node inspect_node = inspect::Node());
  FocusManager(FocusManager&& other) = delete;  // Disallow moving.

  void Bind(fidl::ServerEnd<fuchsia_ui_focus::FocusChainListenerRegistry> request);

  // Request focus transfer to the proposed ViewRef's KOID |request|, on the behalf of |requester|.
  // Return kAccept if successful.
  // - If |requester| is not authorized to focus |request|, return error.
  // - If the |request| is not in |snapshot_.view_tree|, return error.
  // - If the |request| is otherwise valid, but violates the focus transfer policy, return error.
  FocusChangeStatus RequestFocus(zx_koid_t requester, zx_koid_t request,
                                 const view_tree::Snapshot& snapshot);

  // Posts an async task to the input thread to eagerly inspect the latest view tree snapshot
  // in `snapshot_holder_`, repair any broken focus chains, and notify outstanding hanging gets.
  void OnNewViewTreeSnapshot();

  // |fidl::Server<fuchsia_ui_focus::FocusChainListenerRegistry>|
  void Register(RegisterRequest& request, RegisterCompleter::Sync& completer) override;

  const std::vector<zx_koid_t>& GetFocusChain(const view_tree::Snapshot& snapshot) {
    EnsureValidFocus(snapshot);
    return focus_chain_;
  }

  void RegisterViewRefFocused(zx_koid_t koid,
                              fidl::ServerEnd<fuchsia_ui_views::ViewRefFocused> vrf);

  void RegisterViewFocuser(zx_koid_t koid, fidl::ServerEnd<fuchsia_ui_views::Focuser> focuser);

  void RegisterFocusChainListener(
      fidl::ClientEnd<fuchsia_ui_focus::FocusChainListener> focus_chain_listener);

  // Variants that conveniently obtain a snapshot from the snapshot holder.
  FocusChangeStatus RequestFocusForTest(zx_koid_t requester, zx_koid_t request);
  void SetAutoFocusForTest(zx_koid_t requester, zx_koid_t target);
  const std::vector<zx_koid_t>& GetFocusChainForTest();

 private:
  struct FocusChainListenerEntry
      : public fidl::AsyncEventHandler<fuchsia_ui_focus::FocusChainListener> {
    FocusChainListenerEntry(uint64_t id,
                            fidl::ClientEnd<fuchsia_ui_focus::FocusChainListener> client_end,
                            async_dispatcher_t* dispatcher, fit::function<void(uint64_t)> on_error)
        : id_(id),
          on_error_(std::move(on_error)),
          client_(std::move(client_end), dispatcher, this) {}

    void on_fidl_error(fidl::UnbindInfo error) override {
      if (error.is_dispatcher_shutdown()) {
        return;
      }
      if (on_error_) {
        on_error_(id_);
      }
    }

    uint64_t id_;
    fit::function<void(uint64_t)> on_error_;
    fidl::Client<fuchsia_ui_focus::FocusChainListener> client_;
  };

  // Sets the auto focus target |requester| to |target|.
  // If |target| is ZX_KOID_INVALID the |requester| entry is removed.
  void SetAutoFocus(zx_koid_t requester, zx_koid_t target, const view_tree::Snapshot& snapshot);

  // Ensure the focus chain is valid; preserve as much of the existing focus chain as possible.
  // - If the focus chain is still valid, do nothing.
  // - Otherwise, truncate the focus chain so that every pairwise parent-child relationship is valid
  //   in the current tree.
  // - If the entire focus chain is invalid, the new focus chain will contain only the new root.
  // - If the view tree is empty, the new focus chain is empty.
  void RepairFocus(const view_tree::Snapshot& snapshot);

  // If the snapshot is new (based on its sequence number), then call `RepairFocus()`.
  void EnsureValidFocus(const view_tree::Snapshot& snapshot);

  // Transfers focus to |koid| and generates the new focus chain.
  //  - |koid| must be allowed to receive focus and must exist in the current view tree snapshot.
  //  - If the |snapshot_| is empty, then |koid| is allowed to be ZX_KOID_INVALID and will generate
  //    an empty focus_chain_.
  void SetFocus(zx_koid_t koid, const view_tree::Snapshot& snapshot);

  // Replaces the focus chain with a new one. Additionally, if the new focus chain is different from
  // the old one:
  // - send new focus chain to all FocusChainListeners.
  // - send focus gained/lost to all ViewRefFocused-type listeners.
  void SetFocusChain(std::vector<zx_koid_t> update, const view_tree::Snapshot& snapshot);

  // Dispatches the current focus chain to all registered listeners.
  void DispatchFocusChain(const view_tree::Snapshot& snapshot) const;
  // Dispatches the current focus chain to |listener|.
  void DispatchFocusChainTo(const fidl::Client<fuchsia_ui_focus::FocusChainListener>& listener,
                            const view_tree::Snapshot& snapshot) const;

  // Dispatches focus events to view clients.
  void DispatchFocusEvents(zx_koid_t old_focus, zx_koid_t new_focus);

  // Resolves auto focus when |koid| has focus and returns the resulting koid.
  zx_koid_t ResolveAutoFocus(zx_koid_t koid, const view_tree::Snapshot& snapshot) const;

  // Get the auto focus target of |koid| or, if it's unfocusable, the first focusable ancestor of
  // the auto focus target. If there is no viable auto focus target of |koid| it returns |koid|.
  zx_koid_t FindNextAutoFocusTarget(zx_koid_t koid, const view_tree::Snapshot& snapshot) const;

  static fuchsia_ui_views::ViewRef CloneViewRefOf(zx_koid_t koid,
                                                  const view_tree::Snapshot& snapshot);
  fuchsia_ui_focus::FocusChain CloneFocusChain(const view_tree::Snapshot& snapshot) const;

  async_dispatcher_t* input_dispatcher_;
  std::vector<zx_koid_t> focus_chain_;

  const std::shared_ptr<view_tree::SnapshotHolder> snapshot_holder_;
  uint64_t last_seen_sequence_number_ = 0;

  fidl::ServerBindingGroup<fuchsia_ui_focus::FocusChainListenerRegistry>
      focus_chain_listener_registry_;
  uint64_t next_focus_chain_listener_id_ = 0;
  std::unordered_map<uint64_t, std::unique_ptr<FocusChainListenerEntry>> focus_chain_listeners_;

  // Map of ViewRef koid to that View's auto focus target (if it has one).
  std::unordered_map<zx_koid_t, zx_koid_t> auto_focus_targets_;

  // Manages endpoints for fuchsia.ui.views.ViewRefFocused.
  ViewRefFocusedRegistry view_ref_focused_registry_;
  // Manages endpoints for fuchsia.ui.views.Focuser.
  ViewFocuserRegistry view_focuser_registry_;

  inspect::Node inspect_node_;
  inspect::LazyNode lazy_;

  fxl::WeakPtrFactory<FocusManager> weak_factory_{this};
};

}  // namespace focus

#endif  // SRC_UI_SCENIC_LIB_FOCUS_FOCUS_MANAGER_H_

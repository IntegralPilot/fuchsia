// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/focus/focus_manager.h"

#include <lib/async/cpp/task.h>
#include <lib/syslog/cpp/macros.h>

#include "src/ui/scenic/lib/utils/check_is_on_thread.h"
#include "src/ui/scenic/lib/utils/helpers.h"

namespace focus {

namespace {
std::string ToString(const std::vector<zx_koid_t>& chain) {
  std::string value;
  for (zx_koid_t koid : chain) {
    value += std::to_string(koid);
    value += ", ";
  }
  return value;
}
zx_koid_t FocusKoidOf(const std::vector<zx_koid_t>& chain) {
  if (chain.empty()) {
    return ZX_KOID_INVALID;
  }
  return chain.back();
}
}  // namespace

FocusManager::FocusManager(async_dispatcher_t* input_dispatcher,
                           std::shared_ptr<view_tree::SnapshotHolder> snapshot_holder,
                           inspect::Node inspect_node)
    : input_dispatcher_(input_dispatcher),
      snapshot_holder_(std::move(snapshot_holder)),
      view_ref_focused_registry_(input_dispatcher),
      view_focuser_registry_(
          /*request_focus*/
          [this](zx_koid_t requester, zx_koid_t request) {
            auto snapshot_ref = snapshot_holder_->GetSnapshot();
            return RequestFocus(requester, request, *snapshot_ref) == FocusChangeStatus::kAccept;
          },
          /*set_auto_focus*/
          [this](zx_koid_t requester, zx_koid_t request) {
            auto snapshot_ref = snapshot_holder_->GetSnapshot();
            SetAutoFocus(requester, request, *snapshot_ref);
          },
          input_dispatcher),
      inspect_node_(std::move(inspect_node)) {
  FX_DCHECK(input_dispatcher_);

  // Track the focus chain in inspect.
  lazy_ = inspect_node_.CreateLazyValues("values", [this] {
    inspect::Inspector inspector;

    auto array = inspector.GetRoot().CreateUintArray("focus_chain", focus_chain_.size());
    for (size_t i = 0; i < focus_chain_.size(); i++) {
      array.Set(i, focus_chain_[i]);
    }
    inspector.emplace(std::move(array));

    return fpromise::make_ok_promise(std::move(inspector));
  });
}

void FocusManager::Bind(fidl::ServerEnd<fuchsia_ui_focus::FocusChainListenerRegistry> request) {
  utils::CheckIsOnInputThread();
  focus_chain_listener_registry_.AddBinding(input_dispatcher_, std::move(request), this,
                                            fidl::kIgnoreBindingClosure);
}

void FocusManager::OnNewViewTreeSnapshot() {
  TRACE_DURATION("input", "FocusManager::OnNewViewTreeSnapshot");
  auto snapshot = snapshot_holder_->GetSnapshot();
  EnsureValidFocus(*snapshot);
}

FocusChangeStatus FocusManager::RequestFocus(zx_koid_t requester, zx_koid_t request,
                                             const view_tree::Snapshot& snapshot) {
  TRACE_DURATION("input", "FocusManager::RequestFocus");
  EnsureValidFocus(snapshot);

  // Invalid requester.
  if (!snapshot.view_tree.contains(requester)) {
    return FocusChangeStatus::kErrorRequesterInvalid;
  }

  // Invalid request.
  if (!snapshot.view_tree.contains(request)) {
    return FocusChangeStatus::kErrorRequestInvalid;
  }

  // Transfer policy: requester must be authorized.
  if (std::find(focus_chain_.begin(), focus_chain_.end(), requester) == focus_chain_.end()) {
    return FocusChangeStatus::kErrorRequesterNotAuthorized;
  }

  // Transfer policy: requester must be ancestor of request
  if (!snapshot.IsDescendant(/*descendant_koid*/ request, /*ancestor_koid*/ requester) &&
      request != requester) {
    return FocusChangeStatus::kErrorRequesterNotRequestAncestor;
  }

  // Transfer policy: request must be focusable
  if (!snapshot.view_tree.at(request).is_focusable) {
    return FocusChangeStatus::kErrorRequestCannotReceiveFocus;
  }

  // It's a valid request for a change to focus chain.
  SetFocus(request, snapshot);
  FX_DCHECK(focus_chain_.at(0) == snapshot.root);
  return FocusChangeStatus::kAccept;
}

FocusChangeStatus FocusManager::RequestFocusForTest(zx_koid_t requester, zx_koid_t request) {
  auto snapshot_ref = snapshot_holder_->GetSnapshot();
  return RequestFocus(requester, request, *snapshot_ref);
}

const std::vector<zx_koid_t>& FocusManager::GetFocusChainForTest() {
  auto snapshot_ref = snapshot_holder_->GetSnapshot();
  return GetFocusChain(*snapshot_ref);
}

void FocusManager::EnsureValidFocus(const view_tree::Snapshot& snapshot) {
  TRACE_DURATION("input", "FocusManager::EnsureValidFocus");
  utils::CheckIsOnInputThread();

  // This should be guaranteed by the properties of `view_tree::SnapshotRef`: nobody can hang onto
  // an older ref while a new one is checked out, because only one ref can be checked out at a time.
  FX_DCHECK(snapshot.sequence_number >= last_seen_sequence_number_);

  if (snapshot.sequence_number > last_seen_sequence_number_) {
    last_seen_sequence_number_ = snapshot.sequence_number;
    // TODO(https://fxbug.dev/42156009): This has linear cost. Look at making it cheaper.
    // ViewRefFocused clients should be registered before RepairFocus() so that they can be notified
    // about the new root getting focus.
    view_ref_focused_registry_.UpdateRegisteredViews(snapshot);
    RepairFocus(snapshot);
  }
}

void FocusManager::Register(RegisterRequest& request, RegisterCompleter::Sync& completer) {
  RegisterFocusChainListener(std::move(request.listener()));
}

void FocusManager::RegisterFocusChainListener(
    fidl::ClientEnd<fuchsia_ui_focus::FocusChainListener> focus_chain_listener) {
  TRACE_DURATION("input", "FocusManager::RegisterFocusChainListener");
  utils::CheckIsOnInputThread();

  // Retrieve snapshot and ensure the focus chain is valid for it *before* we add the new listener,
  // so that we don't dispatch the focus chain to it twice.
  auto snapshot_ref = snapshot_holder_->GetSnapshot();
  EnsureValidFocus(*snapshot_ref);

  const uint64_t id = next_focus_chain_listener_id_++;
  auto entry = std::make_unique<FocusChainListenerEntry>(
      id, std::move(focus_chain_listener), input_dispatcher_,
      [weak = weak_factory_.GetWeakPtr()](uint64_t id) {
        if (weak) {
          weak->focus_chain_listeners_.erase(id);
        }
      });
  auto* entry_ptr = entry.get();
  auto [it, success] = focus_chain_listeners_.try_emplace(id, std::move(entry));
  FX_DCHECK(success);

  // Dispatch current chain to this new listener.
  DispatchFocusChainTo(entry_ptr->client_, *snapshot_ref);
}

void FocusManager::RegisterViewRefFocused(zx_koid_t koid,
                                          fidl::ServerEnd<fuchsia_ui_views::ViewRefFocused> vrf) {
  TRACE_DURATION("gfx", "FocusManager::RegisterViewRefFocused");
  utils::CheckIsOnInputThread();
  view_ref_focused_registry_.Register(koid, std::move(vrf));
}

void FocusManager::RegisterViewFocuser(zx_koid_t koid,
                                       fidl::ServerEnd<fuchsia_ui_views::Focuser> focuser) {
  TRACE_DURATION("gfx", "FocusManager::RegisterViewFocuser");
  utils::CheckIsOnInputThread();
  view_focuser_registry_.Register(koid, std::move(focuser));
}

void FocusManager::DispatchFocusChainTo(
    const fidl::Client<fuchsia_ui_focus::FocusChainListener>& listener,
    const view_tree::Snapshot& snapshot) const {
  listener->OnFocusChange({{.focus_chain = CloneFocusChain(snapshot)}})
      .Then([](fidl::Result<fuchsia_ui_focus::FocusChainListener::OnFocusChange>& /*result*/) {
        /* No flow control yet. */
      });
}

void FocusManager::DispatchFocusChain(const view_tree::Snapshot& snapshot) const {
  for (auto& [_, entry] : focus_chain_listeners_) {
    DispatchFocusChainTo(entry->client_, snapshot);
  }
}

void FocusManager::DispatchFocusEvents(zx_koid_t old_focus, zx_koid_t new_focus) {
  if (old_focus == new_focus)
    return;

  // Send over fuchsia.ui.views.ViewRefFocused.
  view_ref_focused_registry_.UpdateFocus(old_focus, new_focus);
}

void FocusManager::SetAutoFocus(zx_koid_t requester, zx_koid_t target,
                                const view_tree::Snapshot& snapshot) {
  TRACE_DURATION("gfx", "FocusManager::SetAutoFocus");
  EnsureValidFocus(snapshot);

  if (target != ZX_KOID_INVALID) {
    auto_focus_targets_[requester] = target;
  } else {
    auto_focus_targets_.erase(requester);
  }

  // Move focus to the currently focused View to see if auto focus causes any changes.
  if (!focus_chain_.empty()) {
    SetFocus(focus_chain_.back(), snapshot);
  }
}

void FocusManager::SetAutoFocusForTest(zx_koid_t requester, zx_koid_t target) {
  auto snapshot_ref = snapshot_holder_->GetSnapshot();
  SetAutoFocus(requester, target, *snapshot_ref);
}

zx_koid_t FocusManager::FindNextAutoFocusTarget(zx_koid_t koid,
                                                const view_tree::Snapshot& snapshot) const {
  const auto it = auto_focus_targets_.find(koid);
  if (it != auto_focus_targets_.end() && snapshot.view_tree.contains(it->second)) {
    const zx_koid_t target = it->second;
    // Transfer policy: target must be a descendant of requester (koid)
    if (snapshot.IsDescendant(/*descendant_koid*/ target, /*ancestor_koid*/ koid) ||
        target == koid) {
      koid = target;
      while (koid != snapshot.root && !snapshot.view_tree.at(koid).is_focusable) {
        koid = snapshot.view_tree.at(koid).parent;
      }
    }
  }
  return koid;
}

zx_koid_t FocusManager::ResolveAutoFocus(zx_koid_t koid,
                                         const view_tree::Snapshot& snapshot) const {
  // Iterate through auto focus targets until we find a stable point (i.e. where
  // FindNextAutoFocusTarget(koid) == koid).
  zx_koid_t auto_focus_result = FindNextAutoFocusTarget(koid, snapshot);
  while (auto_focus_result != koid && auto_focus_result != snapshot.root) {
    koid = auto_focus_result;
    auto_focus_result = FindNextAutoFocusTarget(koid, snapshot);
  }
  return auto_focus_result;
}

fuchsia_ui_views::ViewRef FocusManager::CloneViewRefOf(zx_koid_t koid,
                                                       const view_tree::Snapshot& snapshot) {
  FX_DCHECK(snapshot.view_tree.contains(koid))
      << "all views in the focus chain must exist in the view tree";
  const auto& view_node = snapshot.view_tree.at(koid);
  FX_DCHECK(view_node.view_ref) << "view_ref must exist for view in focus chain";
  fuchsia_ui_views::ViewRef clone;
  clone.reference(utils::CopyZxHandle(view_node.view_ref->eventpair()));
  return clone;
}

fuchsia_ui_focus::FocusChain FocusManager::CloneFocusChain(
    const view_tree::Snapshot& snapshot) const {
  if (focus_chain_.empty()) {
    return {};
  }
  std::vector<fuchsia_ui_views::ViewRef> chain;
  chain.reserve(focus_chain_.size());
  for (const zx_koid_t koid : focus_chain_) {
    chain.push_back(CloneViewRefOf(koid, snapshot));
  }
  fuchsia_ui_focus::FocusChain full_copy;
  full_copy.focus_chain(std::move(chain));
  return full_copy;
}

void FocusManager::RepairFocus(const view_tree::Snapshot& snapshot) {
  // Old root no longer valid -> move focus to new root.
  if (focus_chain_.empty() || snapshot.root != focus_chain_.front()) {
    SetFocus(snapshot.root, snapshot);
    return;
  }

  // Even if the focus chain isn't invalid we still want to call SetFocus() on the currently focused
  // View since it may have a newly valid auto focus target.
  zx_koid_t focus_target = focus_chain_.back();

  // See if there's any place where the old focus chain breaks a parent-child relationship, and
  // truncate from there.
  // Note: Start at i = 1 so we can compare with i - 1.
  for (size_t child_index = 1; child_index < focus_chain_.size(); ++child_index) {
    const zx_koid_t child = focus_chain_.at(child_index);
    const zx_koid_t parent = focus_chain_.at(child_index - 1);
    if (!snapshot.view_tree.contains(child) || snapshot.view_tree.at(child).parent != parent) {
      focus_target = parent;
      break;
    }
  }

  // Find first focusable parent ancestor starting from |focus_target|.
  while (focus_target != snapshot.root && !snapshot.view_tree.at(focus_target).is_focusable) {
    focus_target = snapshot.view_tree.at(focus_target).parent;
  }
  SetFocus(focus_target, snapshot);
}

void FocusManager::SetFocus(zx_koid_t koid, const view_tree::Snapshot& snapshot) {
  FX_DCHECK(koid != ZX_KOID_INVALID || koid == snapshot.root);
  if (koid != ZX_KOID_INVALID) {
    FX_DCHECK(snapshot.view_tree.contains(koid));
    FX_DCHECK(snapshot.view_tree.at(koid).is_focusable);
  }

  koid = ResolveAutoFocus(koid, snapshot);

  std::vector<zx_koid_t> new_focus_chain;

  // Regenerate chain.
  while (koid != ZX_KOID_INVALID) {
    new_focus_chain.emplace_back(koid);
    koid = snapshot.view_tree.at(koid).parent;
  }
  std::reverse(new_focus_chain.begin(), new_focus_chain.end());

  SetFocusChain(std::move(new_focus_chain), snapshot);
}

void FocusManager::SetFocusChain(std::vector<zx_koid_t> update,
                                 const view_tree::Snapshot& snapshot) {
  if (update != focus_chain_) {
    FX_LOGS(DEBUG) << "Focus chain update: " << ToString(update);
    const zx_koid_t old_focus = FocusKoidOf(focus_chain_);
    const zx_koid_t new_focus = FocusKoidOf(update);

    focus_chain_ = std::move(update);

    DispatchFocusChain(snapshot);
    DispatchFocusEvents(old_focus, new_focus);
  }
}

}  // namespace focus

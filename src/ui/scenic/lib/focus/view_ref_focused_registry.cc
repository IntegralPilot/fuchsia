// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/focus/view_ref_focused_registry.h"

#include <lib/async/default.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>

#include "src/ui/scenic/lib/utils/check_is_on_thread.h"

namespace focus {

ViewRefFocusedRegistry::ViewRefFocusedRegistry(async_dispatcher_t* dispatcher)
    : dispatcher_(dispatcher ? dispatcher : async_get_default_dispatcher()) {}

void ViewRefFocusedRegistry::Register(zx_koid_t view_ref_koid,
                                      fidl::ServerEnd<fuchsia_ui_views::ViewRefFocused> endpoint) {
  utils::CheckIsOnInputThread();
  auto [_, inserted] = pending_requests_.try_emplace(view_ref_koid, std::move(endpoint));
  // This DCHECK does not assert an internally-guaranteed invariant: nothing prevents a client from
  // calling `Flatland.CreateView2` multiple times with the same control-ref/view-ref pair (in the
  // same or different Flatland sessions).  Instead, this DCHECK serves to catch accidental client
  // misuse; in release builds the new endpoint is simply dropped immediately.
  FX_DCHECK(inserted) << "endpoint emplace should always succeed";
}

void ViewRefFocusedRegistry::UpdateRegisteredViews(const view_tree::Snapshot& snapshot) {
  TRACE_DURATION("input", "ViewRefFocusedRegistry::UpdateRegisteredViews");
  utils::CheckIsOnInputThread();

  // Remove the clients which are removed from the snapshot.
  for (auto it = endpoints_.begin(); it != endpoints_.end();) {
    const zx_koid_t koid = it->first;
    if (!snapshot.view_tree.contains(koid) && !snapshot.unconnected_views.contains(koid)) {
      it = endpoints_.erase(it);
    } else {
      ++it;
    }
  }

  // Register any pending clients which are added to the snapshot.
  for (auto it = pending_requests_.begin(); it != pending_requests_.end();) {
    const zx_koid_t koid = it->first;
    if (snapshot.view_tree.contains(koid)) {
      auto [_, inserted] =
          endpoints_.emplace(koid, std::make_unique<Endpoint>(dispatcher_, std::move(it->second)));
      FX_DCHECK(inserted) << "endpoint emplace should always succeed";
      it = pending_requests_.erase(it);
    } else {
      ++it;
    }
  }
}

void ViewRefFocusedRegistry::UpdateFocus(zx_koid_t old_focus, zx_koid_t new_focus) {
  TRACE_DURATION("input", "ViewRefFocusedRegistry::UpdateFocus");
  utils::CheckIsOnInputThread();

  FX_DCHECK(old_focus != new_focus) << "invariant";
  if (auto it = endpoints_.find(old_focus); it != endpoints_.end()) {
    it->second->UpdateFocus(false);
  } else {
    FX_DLOGS(INFO) << "Client lost focus, but cannot be notified. View ref koid: " << old_focus;
  }

  if (auto it = endpoints_.find(new_focus); it != endpoints_.end()) {
    it->second->UpdateFocus(true);
  } else {
    FX_DLOGS(INFO) << "Client gained focus, but cannot be notified. View ref koid:" << new_focus;
  }
}

ViewRefFocusedRegistry::Endpoint::Endpoint(
    async_dispatcher_t* dispatcher, fidl::ServerEnd<fuchsia_ui_views::ViewRefFocused> endpoint)
    : binding_(dispatcher, std::move(endpoint), this, fidl::kIgnoreBindingClosure) {}

void ViewRefFocusedRegistry::Endpoint::Watch(WatchCompleter::Sync& completer) {
  utils::CheckIsOnInputThread();
  if (response_.has_value()) {
    // Client called Watch() while a previous Watch() was still pending. Non-compliance results in
    // channel closure according to protocol specification.
    completer.Close(ZX_ERR_BAD_STATE);
    return;
  }

  if (focused_state_.has_value()) {
    // Drain and reset.
    fuchsia_ui_views::FocusState state{{.focused = focused_state_.value()}};
    completer.Reply({std::move(state)});
    focused_state_.reset();
  } else {
    // Nothing to report yet. Stash the callback for later.
    response_ = completer.ToAsync();
  }

  FX_DCHECK(!focused_state_.has_value()) << "postcondition";
}

void ViewRefFocusedRegistry::Endpoint::UpdateFocus(bool focused) {
  if (response_.has_value()) {
    // Drain and reset.
    fuchsia_ui_views::FocusState state{{.focused = focused}};
    response_->Reply({std::move(state)});
    response_.reset();
    focused_state_.reset();
  } else {
    // Accumulate.
    focused_state_ = focused;
  }

  FX_DCHECK(!response_.has_value()) << "postcondition";
}

}  // namespace focus

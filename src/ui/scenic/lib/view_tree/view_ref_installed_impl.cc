// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/view_tree/view_ref_installed_impl.h"

#include <lib/async/default.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/rights.h>

#include "src/ui/scenic/lib/utils/check_is_on_thread.h"
#include "src/ui/scenic/lib/utils/helpers.h"

namespace view_tree {

namespace {

// Check if a ViewRef is valid and has the correct rights.
bool IsValidViewRef(const fuchsia_ui_views::wire::ViewRef& view_ref) {
  if (zx_handle_check_valid(view_ref.reference.get()) != ZX_OK) {
    FX_LOGS(INFO) << "Bad handle";
    return false;  // bad handle
  }

  zx_info_handle_basic_t info{};
  if (view_ref.reference.get_info(ZX_INFO_HANDLE_BASIC, &info, /*buffer size*/ sizeof(info),
                                  nullptr, nullptr) != ZX_OK) {
    FX_LOGS(INFO) << "No info";
    return false;  // no info
  }

  if (!(info.rights & ZX_RIGHT_WAIT)) {
    FX_LOGS(INFO) << "Bad rights";
    return false;  // unexpected rights
  }

  return true;
}

}  // namespace

ViewRefInstalledImpl::ViewRefInstalledImpl(
    std::shared_ptr<view_tree::SnapshotHolder> snapshot_holder)
    : snapshot_holder_(std::move(snapshot_holder)) {}

void ViewRefInstalledImpl::Bind(fidl::ServerEnd<fuchsia_ui_views::ViewRefInstalled> server_end) {
  utils::CheckIsOnInputThread();
  bindings_.AddBinding(async_get_default_dispatcher(), std::move(server_end), this,
                       fidl::kIgnoreBindingClosure);
}

// |fidl::WireServer<fuchsia_ui_views::ViewRefInstalled>|
void ViewRefInstalledImpl::Watch(WatchRequestView request, WatchCompleter::Sync& completer) {
  utils::CheckIsOnInputThread();
  if (!IsValidViewRef(request->view_ref)) {
    completer.ReplyError(fuchsia_ui_views::wire::ViewRefInstalledError::kInvalidViewRef);
    return;
  }

  // Check if already installed.
  const zx_koid_t view_ref_koid = utils::ExtractKoid(request->view_ref);
  if (installed_views_.contains(view_ref_koid)) {
    completer.ReplySuccess();
    return;
  }

  zx::eventpair eventpair;
  if (request->view_ref.reference.duplicate(ZX_RIGHT_SAME_RIGHTS, &eventpair) != ZX_OK) {
    completer.ReplyError(fuchsia_ui_views::wire::ViewRefInstalledError::kInvalidViewRef);
    return;
  }

  // Not invalid, not installed.
  if (!watched_views_.contains(view_ref_koid)) {
    // If it doesn't exist: add a new entry and setup the invalidation waiter.
    auto [it, success] = watched_views_.try_emplace(view_ref_koid, std::move(eventpair));
    FX_DCHECK(success);

    // When the event is invalidated, send error message and clean up.
    const zx_status_t status =
        watched_views_.at(view_ref_koid)
            .invalidation_waiter.waiter.Begin(
                async_get_default_dispatcher(),
                std::bind(&ViewRefInstalledImpl::OnViewRefInvalidated, this, view_ref_koid,
                          std::placeholders::_3, std::placeholders::_4));
    FX_DCHECK(status == ZX_OK);
  }
  // Save completer until installation or invalidation
  watched_views_.at(view_ref_koid).completers.emplace_back(completer.ToAsync());
}

void ViewRefInstalledImpl::OnNewViewTreeSnapshot() {
  utils::CheckIsOnInputThread();
  FX_DCHECK(snapshot_holder_);

  auto snapshot = snapshot_holder_->GetSnapshot();

  if (snapshot->sequence_number <= latest_sequence_number_) {
    FX_DCHECK(snapshot->sequence_number == latest_sequence_number_);
    return;
  }
  latest_sequence_number_ = snapshot->sequence_number;

  // Remove any stale views from the installed_views_ set.
  for (auto it = installed_views_.begin(); it != installed_views_.end();) {
    if (!snapshot->view_tree.contains(*it) && !snapshot->unconnected_views.contains(*it)) {
      it = installed_views_.erase(it);
    } else {
      ++it;
    }
  }

  // Update any newly installed views.
  for (const auto& [koid, _] : snapshot->view_tree) {
    const auto [__, success] = installed_views_.emplace(koid);
    if (success) {
      OnViewRefInstalled(koid);
    }
  }
}

void ViewRefInstalledImpl::OnViewRefInstalled(zx_koid_t view_ref_koid) {
  const auto it = watched_views_.find(view_ref_koid);
  if (it == watched_views_.end()) {
    return;
  }

  for (auto& completer : it->second.completers) {
    completer.ReplySuccess();
  }
  watched_views_.erase(view_ref_koid);
}

void ViewRefInstalledImpl::OnViewRefInvalidated(zx_koid_t view_ref_koid, zx_status_t status,
                                                const zx_packet_signal* signal) {
  if (status != ZX_OK) {
    FX_LOGS(WARNING)
        << "ViewRefInstalledImpl received an error status code on viewref invalidation: " << status;
  }

  // No need to check for existence. OnViewRefInvalidated is only called from invalidation_waiter.
  for (auto& completer : watched_views_.at(view_ref_koid).completers) {
    completer.ReplyError(fuchsia_ui_views::wire::ViewRefInstalledError::kInvalidViewRef);
  }
  watched_views_.erase(view_ref_koid);
}

}  // namespace view_tree

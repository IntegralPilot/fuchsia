// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/focus/view_focuser_registry.h"

#include <lib/async/default.h>
#include <lib/syslog/cpp/macros.h>

#include "src/ui/scenic/lib/utils/helpers.h"

namespace focus {

ViewFocuserRegistry::ViewFocuserRegistry(RequestFocusFunc request_focus,
                                         SetAutoFocusFunc set_auto_focus,
                                         async_dispatcher_t* dispatcher)
    : request_focus_(std::move(request_focus)),
      set_auto_focus_(std::move(set_auto_focus)),
      dispatcher_(dispatcher ? dispatcher : async_get_default_dispatcher()) {}

void ViewFocuserRegistry::Register(zx_koid_t view_ref_koid,
                                   fidl::ServerEnd<fuchsia_ui_views::Focuser> view_focuser) {
  if (endpoints_.contains(view_ref_koid)) {
    return;
  }

  endpoints_.try_emplace(
      view_ref_koid,
      std::make_unique<ViewFocuserEndpoint>(
          dispatcher_, std::move(view_focuser),
          /*on_unbound*/
          [this, view_ref_koid](fidl::UnbindInfo info) {
            if (!info.is_dispatcher_shutdown()) {
              set_auto_focus_(view_ref_koid, ZX_KOID_INVALID);
              endpoints_.erase(view_ref_koid);
            }
          },
          /*request_focus*/
          [this, requester = view_ref_koid](
              const fuchsia_ui_views::wire::ViewRef& view_ref,
              ViewFocuserEndpoint::RequestFocusCompleter::Sync& completer) {
            if (request_focus_(requester, utils::ExtractKoid(view_ref))) {
              completer.ReplySuccess();  // Request received, and honored.
              return;
            }

            completer.ReplyError(fuchsia_ui_views::Error::kDenied);  // Report a problem.
          },
          /*set_auto_focus*/
          [this, requester = view_ref_koid](zx_koid_t target_koid) {
            set_auto_focus_(requester, target_koid);
          }));
}

ViewFocuserRegistry::ViewFocuserEndpoint::ViewFocuserEndpoint(
    async_dispatcher_t* dispatcher, fidl::ServerEnd<fuchsia_ui_views::Focuser> view_focuser,
    fit::function<void(fidl::UnbindInfo)> on_unbound,
    fit::function<void(const fuchsia_ui_views::wire::ViewRef&, RequestFocusCompleter::Sync&)>
        request_focus,
    fit::function<void(zx_koid_t)> set_auto_focus)
    : request_focus_(std::move(request_focus)),
      set_auto_focus_(std::move(set_auto_focus)),
      binding_(dispatcher, std::move(view_focuser), this,
               [on_unbound = std::move(on_unbound)](fidl::UnbindInfo info) {
                 if (on_unbound) {
                   on_unbound(info);
                 }
               }) {
  FX_DCHECK(request_focus_) << "invariant";
  FX_DCHECK(set_auto_focus_) << "invariant";
}

void ViewFocuserRegistry::ViewFocuserEndpoint::RequestFocus(
    RequestFocusRequestView request, RequestFocusCompleter::Sync& completer) {
  TRACE_DURATION("input", "ViewFocuserEndpoint::RequestFocus");
  request_focus_(request->view_ref, completer);
}

void ViewFocuserRegistry::ViewFocuserEndpoint::SetAutoFocus(
    SetAutoFocusRequestView request, SetAutoFocusCompleter::Sync& completer) {
  TRACE_DURATION("input", "ViewFocuserEndpoint::SetAutoFocus");
  zx_koid_t target = ZX_KOID_INVALID;
  if (request->has_view_ref()) {
    target = utils::ExtractKoid(request->view_ref());
  }
  set_auto_focus_(target);
  completer.ReplySuccess();
}

}  // namespace focus

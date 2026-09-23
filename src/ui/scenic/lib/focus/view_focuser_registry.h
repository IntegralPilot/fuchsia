// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_FOCUS_VIEW_FOCUSER_REGISTRY_H_
#define SRC_UI_SCENIC_LIB_FOCUS_VIEW_FOCUSER_REGISTRY_H_

#include <fidl/fuchsia.ui.views/cpp/wire.h>
#include <lib/async/dispatcher.h>
#include <lib/fit/function.h>
#include <zircon/types.h>

#include <algorithm>
#include <memory>
#include <unordered_map>
#include <unordered_set>

namespace focus {

using RequestFocusFunc = fit::function<bool(/*requester*/ zx_koid_t, /*request*/ zx_koid_t)>;
using SetAutoFocusFunc = fit::function<void(/*requester*/ zx_koid_t, /*request*/ zx_koid_t)>;

// An object for managing fuchsia.ui.views.Focuser lifecycle, starting with FIDL requests and
// ending with cleanup when the client-side channel closes.
class ViewFocuserRegistry {
 public:
  explicit ViewFocuserRegistry(RequestFocusFunc request_focus, SetAutoFocusFunc set_auto_focus,
                               async_dispatcher_t* dispatcher = nullptr);

  // Because this object captures its "this" pointer in internal closures, it is unsafe to copy or
  // move it. Disable all copy and move operations.
  ViewFocuserRegistry(const ViewFocuserRegistry&) = delete;
  ViewFocuserRegistry& operator=(const ViewFocuserRegistry&) = delete;
  ViewFocuserRegistry(ViewFocuserRegistry&&) = delete;
  ViewFocuserRegistry& operator=(ViewFocuserRegistry&&) = delete;

  // Bind a FIDL request for fuchsia.ui.views.Focuser, associated with |view_ref_koid|.
  void Register(zx_koid_t view_ref_koid, fidl::ServerEnd<fuchsia_ui_views::Focuser> view_focuser);

  // For tests.
  std::unordered_set<zx_koid_t> endpoints() const {
    std::unordered_set<zx_koid_t> out;
    std::for_each(endpoints_.begin(), endpoints_.end(),
                  [&](const auto& kv) { out.insert(kv.first); });
    return out;
  }

 private:
  class ViewFocuserEndpoint : public fidl::WireServer<fuchsia_ui_views::Focuser> {
   public:
    ViewFocuserEndpoint(
        async_dispatcher_t* dispatcher, fidl::ServerEnd<fuchsia_ui_views::Focuser> view_focuser,
        fit::function<void(fidl::UnbindInfo)> on_unbound,
        fit::function<void(const fuchsia_ui_views::wire::ViewRef&, RequestFocusCompleter::Sync&)>
            request_focus,
        fit::function<void(zx_koid_t)> set_auto_focus);

    // |fidl::WireServer<fuchsia_ui_views::Focuser>|
    void RequestFocus(RequestFocusRequestView request,
                      RequestFocusCompleter::Sync& completer) override;

    // |fidl::WireServer<fuchsia_ui_views::Focuser>|
    void SetAutoFocus(SetAutoFocusRequestView request,
                      SetAutoFocusCompleter::Sync& completer) override;

   private:
    const fit::function<void(const fuchsia_ui_views::wire::ViewRef&, RequestFocusCompleter::Sync&)>
        request_focus_;
    const fit::function<void(zx_koid_t)> set_auto_focus_;
    fidl::ServerBinding<fuchsia_ui_views::Focuser> binding_;
  };

  std::unordered_map<zx_koid_t, std::unique_ptr<ViewFocuserEndpoint>> endpoints_;

  const RequestFocusFunc request_focus_;
  const SetAutoFocusFunc set_auto_focus_;
  async_dispatcher_t* dispatcher_;
};

}  // namespace focus

#endif  // SRC_UI_SCENIC_LIB_FOCUS_VIEW_FOCUSER_REGISTRY_H_

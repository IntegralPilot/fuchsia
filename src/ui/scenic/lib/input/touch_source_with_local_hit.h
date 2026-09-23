// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_INPUT_TOUCH_SOURCE_WITH_LOCAL_HIT_H_
#define SRC_UI_SCENIC_LIB_INPUT_TOUCH_SOURCE_WITH_LOCAL_HIT_H_

#include <fidl/fuchsia.ui.pointer.augment/cpp/fidl.h>
#include <fidl/fuchsia.ui.pointer/cpp/fidl.h>
#include <zircon/status.h>

#include "src/lib/fxl/macros.h"
#include "src/ui/scenic/lib/input/internal_pointer_event.h"
#include "src/ui/scenic/lib/input/touch_source_base.h"

namespace scenic_impl::input {

// Implementation of the |fidl::Server<fuchsia_ui_pointer_augment::TouchSourceWithLocalHit>|
// interface. One instance per channel.
class TouchSourceWithLocalHit
    : public TouchSourceBase,
      public fidl::Server<fuchsia_ui_pointer_augment::TouchSourceWithLocalHit> {
 public:
  // |respond| must not destroy the TouchSourceWithLocalHit object.
  TouchSourceWithLocalHit(
      zx_koid_t view_ref_koid,
      fidl::ServerEnd<fuchsia_ui_pointer_augment::TouchSourceWithLocalHit> server_end,
      fit::function<void(StreamId, const std::vector<GestureResponse>&)> respond,
      fit::function<void()> error_handler,
      fit::function<std::pair<zx_koid_t, std::array<float, 2>>(const view_tree::Snapshot&,
                                                               const InternalTouchEvent&)>
          get_local_hit,
      GestureContenderInspector& inspector);

  ~TouchSourceWithLocalHit() override = default;

  // |fidl::Server<fuchsia_ui_pointer_augment::TouchSourceWithLocalHit>|
  void Watch(WatchRequest& request, WatchCompleter::Sync& completer) override;

  // |fidl::Server<fuchsia_ui_pointer_augment::TouchSourceWithLocalHit>|
  void UpdateResponse(UpdateResponseRequest& request,
                      UpdateResponseCompleter::Sync& completer) override {
    TouchSourceBase::UpdateResponseBase(
        request.interaction(), std::move(request.response()),
        [completer = completer.ToAsync()]() mutable { completer.Reply(); });
  }

 protected:
  // |TouchSourceBase|
  void CloseChannel(zx_status_t epitaph) override;
  void Augment(const view_tree::Snapshot& snapshot, AugmentedTouchEvent&,
               const InternalTouchEvent&) override;

 private:
  fidl::ServerBinding<fuchsia_ui_pointer_augment::TouchSourceWithLocalHit> binding_;
  const fit::function<std::pair<zx_koid_t, std::array<float, 2>>(const view_tree::Snapshot&,
                                                                 const InternalTouchEvent&)>
      get_local_hit_;
};

}  // namespace scenic_impl::input

#endif  // SRC_UI_SCENIC_LIB_INPUT_TOUCH_SOURCE_WITH_LOCAL_HIT_H_

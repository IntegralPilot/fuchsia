// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/input/touch_source_with_local_hit.h"

#include <lib/async/default.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include "src/lib/fsl/handles/object_info.h"

namespace scenic_impl::input {

TouchSourceWithLocalHit::TouchSourceWithLocalHit(
    zx_koid_t view_ref_koid,
    fidl::ServerEnd<fuchsia_ui_pointer_augment::TouchSourceWithLocalHit> server_end,
    fit::function<void(StreamId, const std::vector<GestureResponse>&)> respond,
    fit::function<void()> error_handler,
    fit::function<std::pair<zx_koid_t, std::array<float, 2>>(const view_tree::Snapshot&,
                                                             const InternalTouchEvent&)>
        get_local_hit,
    GestureContenderInspector& inspector)
    : TouchSourceBase(fsl::GetKoid(server_end.channel().get()), view_ref_koid, std::move(respond),
                      inspector),
      binding_(async_get_default_dispatcher(), std::move(server_end), this,
               [error_handler = std::move(error_handler)](fidl::UnbindInfo info) {
                 if (info.status() != ZX_OK && info.status() != ZX_ERR_PEER_CLOSED) {
                   FX_LOGS(ERROR) << "TouchSourceWithLocalHit fidl channel closed: "
                                  << info.FormatDescription();
                 } else {
                   FX_LOGS(INFO) << "TouchSourceWithLocalHit fidl channel closed: "
                                 << info.FormatDescription();
                 }
                 error_handler();
               }),
      get_local_hit_(std::move(get_local_hit)) {}

void TouchSourceWithLocalHit::Watch(WatchRequest& request, WatchCompleter::Sync& completer) {
  TouchSourceBase::WatchBase(
      std::move(request.responses()),
      [completer = completer.ToAsync()](std::vector<AugmentedTouchEvent> events) mutable {
        std::vector<fuchsia_ui_pointer_augment::TouchEventWithLocalHit> out_events;
        out_events.reserve(events.size());
        for (auto& event : events) {
          if (!event.local_hit.has_value()) {
            if (!event.touch_event.interaction_result().has_value()) {
              FX_LOGS(WARNING) << "Local hit not set!";  // "impossible" but still happens
            }
            event.local_hit = {.local_viewref_koid = ZX_KOID_INVALID, .local_point = {0.f, 0.f}};
          }

          out_events.emplace_back(std::move(event.touch_event), event.local_hit->local_viewref_koid,
                                  event.local_hit->local_point);
        }
        completer.Reply({{.events = std::move(out_events)}});
      });
}

void TouchSourceWithLocalHit::CloseChannel(zx_status_t epitaph) {
  FX_LOGS(WARNING) << "Closing TouchSourceWithLocalHit due to " << zx_status_get_string(epitaph);
  binding_.Close(epitaph);
}

void TouchSourceWithLocalHit::Augment(const view_tree::Snapshot& snapshot,
                                      AugmentedTouchEvent& out_event,
                                      const InternalTouchEvent& in_event) {
  const auto [view_ref_koid, local_point] = get_local_hit_(snapshot, in_event);
  out_event.local_hit = {
      .local_viewref_koid = view_ref_koid,
      .local_point = local_point,
  };
}

}  // namespace scenic_impl::input

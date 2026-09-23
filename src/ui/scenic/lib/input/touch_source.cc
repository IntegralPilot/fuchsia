// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/input/touch_source.h"

#include <lib/async/default.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/status.h>

#include "src/lib/fsl/handles/object_info.h"

namespace scenic_impl::input {

TouchSource::TouchSource(zx_koid_t view_ref_koid,
                         fidl::ServerEnd<fuchsia_ui_pointer::TouchSource> touch_source,
                         fit::function<void(StreamId, const std::vector<GestureResponse>&)> respond,
                         fit::function<void()> error_handler, GestureContenderInspector& inspector)
    : TouchSourceBase(fsl::GetKoid(touch_source.channel().get()), view_ref_koid, std::move(respond),
                      inspector),
      binding_(async_get_default_dispatcher(), std::move(touch_source), this,
               [error_handler = std::move(error_handler)](fidl::UnbindInfo info) {
                 if (info.status() != ZX_OK && info.status() != ZX_ERR_PEER_CLOSED) {
                   FX_LOGS(ERROR) << "TouchSource fidl channel closed: "
                                  << info.FormatDescription();
                 } else {
                   FX_LOGS(INFO) << "TouchSource fidl channel closed: " << info.FormatDescription();
                 }
                 // NOTE: Triggers destruction of this object.
                 error_handler();
               }) {}

void TouchSource::CloseChannel(zx_status_t epitaph) {
  FX_LOGS(WARNING) << "Closing TouchSource due to " << zx_status_get_string(epitaph);
  binding_.Close(epitaph);
}

}  // namespace scenic_impl::input

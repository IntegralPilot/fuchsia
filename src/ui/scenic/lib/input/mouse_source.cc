// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/input/mouse_source.h"

#include <lib/async/default.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/status.h>

#include "src/lib/fsl/handles/object_info.h"

namespace scenic_impl::input {

MouseSource::MouseSource(fidl::ServerEnd<fuchsia_ui_pointer::MouseSource> mouse_source,
                         fit::function<void()> error_handler, async_dispatcher_t* dispatcher)
    : MouseSourceBase(fsl::GetKoid(mouse_source.channel().get()),
                      [this](zx_status_t epitaph) { CloseChannel(epitaph); }),
      binding_(dispatcher, std::move(mouse_source), this,
               [error_handler = std::move(error_handler)](fidl::UnbindInfo info) {
                 if (info.status() != ZX_OK && info.status() != ZX_ERR_PEER_CLOSED) {
                   FX_LOGS(ERROR) << "MouseSource fidl channel closed: "
                                  << info.FormatDescription();
                 } else {
                   FX_LOGS(INFO) << "MouseSource fidl channel closed: " << info.FormatDescription();
                 }
                 error_handler();
               }) {}

void MouseSource::CloseChannel(zx_status_t epitaph) {
  FX_LOGS(WARNING) << "Closing MouseSource due to " << zx_status_get_string(epitaph);
  binding_.Close(epitaph);
}

}  // namespace scenic_impl::input

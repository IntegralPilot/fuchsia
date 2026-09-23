// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ld/fuchsia-debugdata.h>
#include <lib/zx/channel.h>
#include <lib/zx/vmo.h>
#include <zircon/compiler.h>
#include <zircon/sanitizer.h>

#include "zircon_impl.h"

// Publish VMO and return back event pair handle which controls the lifetime of
// the VMO.
__EXPORT zx_handle_t __sanitizer_publish_data(const char* sink_name, zx_handle_t vmo) {
  ld::Debugdata debugdata{sink_name, zx::vmo{vmo}};

  zx::unowned_channel svc_client_end{__zircon_namespace_svc};
  if (!svc_client_end->is_valid()) {
    return ZX_HANDLE_INVALID;
  }

  zx::result token = std::move(debugdata).Publish(svc_client_end->borrow());
  if (token.is_error()) [[unlikely]] {
    constexpr std::string_view kError = "cannot send fuchsia.debugdata.Publisher/Publish message";
    __sanitizer_log_write(kError.data(), kError.size());
    return ZX_HANDLE_INVALID;
  }

  return token->release();
}

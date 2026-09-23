// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/types/view_ref.h"

#include <lib/trace/event.h>

#include "src/lib/fsl/handles/object_info.h"

namespace types {

inline zx_koid_t ExtractViewRefKoid(const zx::eventpair& ep) {
  // We cannot depend on the `utils` library, so we can't use `utils::ExtractKoid()`, but we still
  // want this to show up in traces, so we replicate the function.
  TRACE_DURATION("gfx", "ExtractViewRefKoid");
  return fsl::GetKoid(ep.get());
}

ViewRef::ViewRef(fuchsia_ui_views::ViewRef ref)
    : ref_(std::move(ref)), koid_(ExtractViewRefKoid(ref_.reference())) {}

std::ostream& operator<<(std::ostream& str, const types::ViewRef& vr) {
  const auto& koid = vr.koid();
  if (koid == ZX_KOID_INVALID) {
    str << "ViewRef(INVALID)";
  } else {
    str << "ViewRef(" << koid << ")";
  }
  return str;
}

}  // namespace types

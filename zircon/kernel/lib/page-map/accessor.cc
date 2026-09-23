// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "lib/page-map/accessor.h"

#include <lib/page-map/entry.h>

namespace page_map::internal {

// This is a free function rather than a method on Accessor to avoid requiring that TUs #including
// accessor.h must also #include all of entry.h's.  We cannot simply define this as a static
// Accessor method here because Accessor is a template.
void ReleaseEntry(Entry* entry) { entry->Release(); }

}  // namespace page_map::internal

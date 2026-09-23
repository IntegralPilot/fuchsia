// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_INPUT_CONSTANTS_H_
#define SRC_UI_SCENIC_LIB_INPUT_CONSTANTS_H_

#include <lib/fit/function.h>
#include <zircon/types.h>

namespace view_tree {
class Snapshot;
}

namespace scenic_impl::input {

// RequestFocusFunc should attempt to move focus to the passed in zx_koid_t `request`,
// initiated by `requester`.
// If the passed in `request` koid is ZX_KOID_INVALID, then focus should be moved to
// the `requester` instead. If there is no root, then the call should silently fail.
using RequestFocusFunc =
    fit::function<void(zx_koid_t requester, zx_koid_t request, const view_tree::Snapshot&)>;

}  // namespace scenic_impl::input

#endif  // SRC_UI_SCENIC_LIB_INPUT_CONSTANTS_H_

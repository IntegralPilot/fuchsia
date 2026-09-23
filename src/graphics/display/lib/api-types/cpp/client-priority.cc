// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/api-types/cpp/client-priority.h"

#include <concepts>
#include <type_traits>

namespace display {

static_assert(std::is_standard_layout_v<ClientPriority>);
static_assert(std::is_trivially_assignable_v<ClientPriority, ClientPriority>);
static_assert(std::is_trivially_copyable_v<ClientPriority>);
static_assert(std::is_trivially_copy_constructible_v<ClientPriority>);
static_assert(std::is_trivially_destructible_v<ClientPriority>);
static_assert(std::is_trivially_move_assignable_v<ClientPriority>);
static_assert(std::is_trivially_move_constructible_v<ClientPriority>);

static_assert(std::totally_ordered<ClientPriority>);

}  // namespace display

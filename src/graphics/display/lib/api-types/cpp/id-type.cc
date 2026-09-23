// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/api-types/cpp/id-type.h"

#include <fidl/fuchsia.hardware.display.types/cpp/wire.h>

#include <concepts>
#include <type_traits>

namespace display::internal {

namespace {

using TestIdTypeTraits =
    DefaultIdTypeTraits<uint64_t, fuchsia_hardware_display_types::wire::DisplayId>;
using TestIdType = IdType<TestIdTypeTraits>;

static_assert(std::is_standard_layout_v<TestIdType>);
static_assert(std::is_trivially_assignable_v<TestIdType, TestIdType>);
static_assert(std::is_trivially_copyable_v<TestIdType>);
static_assert(std::is_trivially_copy_constructible_v<TestIdType>);
static_assert(std::is_trivially_destructible_v<TestIdType>);
static_assert(std::is_trivially_move_assignable_v<TestIdType>);
static_assert(std::is_trivially_move_constructible_v<TestIdType>);

static_assert(std::totally_ordered<TestIdType>);

}  // namespace

}  // namespace display::internal

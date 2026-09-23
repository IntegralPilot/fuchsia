// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <memory>
#include <type_traits>

void SendableCompileTimeTests() {
  // Test that |std::is_invocable| can figure out move-only type requirements.
  // |async_patterns::BindForSending| relies on this kind of check so we should
  // make sure it works. See `sendable.h` for details.
  auto move_only_sink = [](std::unique_ptr<int>) {};
  static_assert(std::is_invocable_v<decltype(move_only_sink), std::unique_ptr<int>>);
  static_assert(!std::is_invocable_v<decltype(move_only_sink), std::unique_ptr<int>&>);
}

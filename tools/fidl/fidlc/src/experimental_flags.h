// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef TOOLS_FIDL_FIDLC_SRC_EXPERIMENTAL_FLAGS_H_
#define TOOLS_FIDL_FIDLC_SRC_EXPERIMENTAL_FLAGS_H_

#include <map>
#include <optional>
#include <string_view>

namespace fidlc {

// Flags for experimental compiler features, enabled with --experimental.
// Flags are temporary, so each one should have a bug tracking its removal.
enum class ExperimentalFlag : uint8_t {
  // Enable the experimental RFC-0052 newtype implementation.
  // TODO(https://fxbug.dev/42158155): Finish newtypes.
  kAllowNewTypes = 1 << 0,

  // Write <name>.index.json alongside the <name>.json IR.
  // TODO(https://fxbug.dev/42055022): Stabilize Kythe index JSON output.
  kOutputIndexJson = 1 << 1,

  // Enable the experimental Zircon C types for the Zither project.
  // TODO(https://fxbug.dev/42061412): Stabilize experimental zx C types.
  kZxCTypes = 1 << 2,

  // Allow any types in error syntax, not just (u)int32 or enums thereof.
  // TODO(https://fxbug.dev/42052574): Currently for FDomain. Remove when allowed everywhere.
  kAllowArbitraryErrorTypes = 1 << 3,

  // Allow use of the @no_resource attribute.
  kNoResourceAttribute = 1 << 4,
};

// The mapping from names to experimental flags.
extern const std::map<std::string_view, ExperimentalFlag> kAllExperimentalFlags;

// A set of experimental flags.
class ExperimentalFlagSet {
 public:
  void Enable(ExperimentalFlag flag) { flags_ |= static_cast<Underlying>(flag); }
  bool IsEnabled(ExperimentalFlag flag) const { return flags_ & static_cast<Underlying>(flag); }

 private:
  using Underlying = std::underlying_type_t<ExperimentalFlag>;
  Underlying flags_ = 0;
};

}  // namespace fidlc

#endif  // TOOLS_FIDL_FIDLC_SRC_EXPERIMENTAL_FLAGS_H_

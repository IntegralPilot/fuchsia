// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/devicetree/devicetree.h>
#include <lib/devicetree/matcher.h>
#include <stdio.h>
#include <zircon/compiler.h>

#include <string_view>

#include "lib/devicetree/internal/matcher.h"

namespace devicetree::internal {
namespace {
constexpr std::string_view kAliasNode = "/aliases";
}

ScanState AliasMatcher::OnNode(const NodePath& path, const PropertyDecoder& decoder) {
  switch (path.CompareWith(kAliasNode)) {
    case NodePath::Comparison::kParent:
    case NodePath::Comparison::kIndirectAncestor:
      return ScanState::kActive;
    case NodePath::Comparison::kEqual:
      return ScanState::kDone;
    case NodePath::Comparison::kMismatch:
    case NodePath::Comparison::kChild:
    case NodePath::Comparison::kIndirectDescendent:
      return ScanState::kDoneWithSubtree;
  }
  __UNREACHABLE;
}

ScanState AliasMatcher::OnSubtree(const NodePath& path) { return ScanState::kActive; }

ScanState AliasMatcher::OnScan() { return ScanState::kDone; }

void AliasMatcher::OnError(std::string_view err) {
  printf("%.*s\n", static_cast<int>(err.length()), err.data());
}

}  // namespace devicetree::internal

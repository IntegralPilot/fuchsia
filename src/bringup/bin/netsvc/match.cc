// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/bringup/bin/netsvc/match.h"

#include <lib/stdcompat/string_view.h>

bool EndsWithWildcardMatch(std::string_view target, std::string_view pattern) {
  size_t wildcard = pattern.find('*');

  if (wildcard == std::string_view::npos) {
    return cpp20::ends_with(target, pattern);
  }

  // Take the portion before the wildcard as the required substring to search for.
  std::string_view substr = pattern.substr(0, wildcard);
  // Update pattern for the portion to be searched in the recursive calls.
  pattern.remove_prefix(wildcard + 1);
  if (substr.empty()) {
    // No pattern that needs matching, so skip this wildcard. This happens if two wildcards are
    // placed in a row.
    return EndsWithWildcardMatch(target, pattern);
  }

  // Iterate over all the substring matches, and recursively check the remaining pattern from that
  // point.
  size_t search_pos = 0;
  size_t match_pos;
  while ((match_pos = target.find(substr, search_pos)) != std::string_view::npos) {
    std::string_view remain_target = target.substr(match_pos + substr.length());
    if (EndsWithWildcardMatch(remain_target, pattern)) {
      return true;
    }
    // Set the next search position to just after the match we just tried.
    search_pos = match_pos + 1;
  }

  return false;
}

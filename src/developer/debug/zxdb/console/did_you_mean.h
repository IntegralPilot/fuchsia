// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_CONSOLE_DID_YOU_MEAN_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_CONSOLE_DID_YOU_MEAN_H_

#include <stdio.h>

#include <cstddef>
#include <limits>
#include <map>
#include <optional>
#include <string>

namespace zxdb {

// Returns the edit distance between the two strings.
size_t EditDistance(const std::string& lhs, const std::string& rhs);

// Returns a suggestion for what the user might have meant by candidate given the available choices.
//
// The suggestion will be at most max_edit_distance away from the candidate.
template <typename T, typename U>
inline std::optional<std::string> DidYouMean(const std::string& candidate,
                                             const std::map<std::string, T> noun_choices,
                                             const std::map<std::string, U> verb_choices,
                                             size_t max_edit_distance) {
  std::optional<std::string> best_choice;
  size_t best_distance = std::numeric_limits<size_t>::max();

  auto evaluate = [&](const std::string& choice) {
    size_t distance = EditDistance(candidate, choice);
    if (distance < best_distance) {
      best_choice = choice;
      best_distance = distance;
    }
  };

  for (const auto& entry : noun_choices) {
    evaluate(entry.first);
  }

  for (const auto& entry : verb_choices) {
    evaluate(entry.first);
  }

  if (best_distance <= max_edit_distance) {
    return best_choice;
  }

  return std::nullopt;
}

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_CONSOLE_DID_YOU_MEAN_H_

// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/console/did_you_mean.h"

#include <vector>

namespace zxdb {

// Returns the edit distance between the two strings.
size_t EditDistance(const std::string& lhs, const std::string& rhs) {
  // Create a table to store the edit distances.
  std::vector<std::vector<size_t>> edit_distance(lhs.size() + 1,
                                                 std::vector<size_t>(rhs.size() + 1));

  // Initialize the first row and column.
  for (size_t i = 0; i <= lhs.size(); ++i) {
    edit_distance[i][0] = i;
  }
  for (size_t j = 0; j <= rhs.size(); ++j) {
    edit_distance[0][j] = j;
  }

  // Calculate the edit distances.
  for (size_t i = 1; i <= lhs.size(); ++i) {
    for (size_t j = 1; j <= rhs.size(); ++j) {
      if (lhs[i - 1] == rhs[j - 1]) {
        edit_distance[i][j] = edit_distance[i - 1][j - 1];
      } else {
        edit_distance[i][j] = std::min({edit_distance[i - 1][j], edit_distance[i][j - 1],
                                        edit_distance[i - 1][j - 1]}) +
                              1;
      }
    }
  }

  // Return the edit distance.
  return edit_distance[lhs.size()][rhs.size()];
}

}  // namespace zxdb

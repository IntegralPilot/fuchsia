// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_COMMON_FUZZY_MATCHER_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_COMMON_FUZZY_MATCHER_H_

#include <sstream>
#include <string>
#include <vector>

namespace zxdb {

// A helper class to match output sequentially as |MatchesLine| is called repeatedly.
class FuzzyMatcher {
 public:
  explicit FuzzyMatcher(const std::string& content) : content_(content) {}

  // The content should contain a line that matches the given substrings.
  // |allow_out_of_order| indicates that |pattern| may appear anywhere in |content_|.
  // Note that this method will consume the content, so a subsequent call may return differently.
  bool MatchesLine(const std::vector<std::string_view>& substrs, bool allow_out_of_order);

  // This variant takes a pattern string, which contains "??" that can match everything.
  // |allow_out_of_order| indicates that |pattern| may appear anywhere in |content_|.
  bool MatchesLine(std::string_view pattern, bool allow_out_of_order);

 private:
  std::stringstream content_;
};

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_COMMON_FUZZY_MATCHER_H_

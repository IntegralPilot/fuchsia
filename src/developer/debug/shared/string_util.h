// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_SHARED_STRING_UTIL_H_
#define SRC_DEVELOPER_DEBUG_SHARED_STRING_UTIL_H_

#include <string_view>

namespace debug {
// Returns true if the first argument begins in exactly the second.
bool StringStartsWith(std::string_view str, std::string_view begins_with);

// Returns true if the first argument ends in exactly the second.
bool StringEndsWith(std::string_view str, std::string_view ends_with);

bool StringContains(std::string_view haystack, std::string_view needle);
}  // namespace debug

#endif  // SRC_DEVELOPER_DEBUG_SHARED_STRING_UTIL_H_

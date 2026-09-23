// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/shared/string_util.h"

#include "lib/stdcompat/string_view.h"

namespace debug {

bool StringStartsWith(std::string_view str, std::string_view begins_with) {
  return str.starts_with(begins_with);
}

bool StringEndsWith(std::string_view str, std::string_view ends_with) {
  return str.ends_with(ends_with);
}

bool StringContains(std::string_view haystack, std::string_view needle) {
  return cpp23::contains(haystack, needle);
}

}  // namespace debug

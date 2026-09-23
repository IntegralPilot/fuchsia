// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_UTILS_REGEXP_H_
#define SRC_DEVELOPER_FORENSICS_UTILS_REGEXP_H_

#include <re2/re2.h>

namespace forensics {

constexpr re2::LazyRE2 MakeLazyRE2(const char* const str) {
  return re2::LazyRE2{str, re2::RE2::CannedOptions::DefaultOptions, {}, nullptr, {}};
}

}  // namespace forensics

#endif  // SRC_DEVELOPER_FORENSICS_UTILS_REGEXP_H_

// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_BRINGUP_BIN_NETSVC_MATCH_H_
#define SRC_BRINGUP_BIN_NETSVC_MATCH_H_

#include <string_view>

// Returns if the pattern matches the end of target, where the pattern matches a * to any length of
// characters.
bool EndsWithWildcardMatch(std::string_view target, std::string_view pattern);

#endif  // SRC_BRINGUP_BIN_NETSVC_MATCH_H_

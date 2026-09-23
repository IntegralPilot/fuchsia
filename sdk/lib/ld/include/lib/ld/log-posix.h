// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_LOG_POSIX_H_
#define LIB_LD_LOG_POSIX_H_

#include <string_view>

namespace ld {

// This is a callable object that gets output to stderr.
// It should be called with a single full log message ending in a newline.
class Log {
 public:
  // This is a recommended maximum buffer size to use for log messages.
  // A message longer than this can be split across multiple lines.
  static constexpr size_t kBufferSize = 128;

  // Returns the number of chars written, or -1.
  int operator()(std::string_view str) const;

  // Returns true if there is any point in calling the output function.
  // Formatting output can be skipped if it won't go anywhere, but that's
  // presumed never to be the case for the POSIX stderr version.
  constexpr explicit operator bool() const { return true; }
};

}  // namespace ld

#endif  // LIB_LD_LOG_POSIX_H_

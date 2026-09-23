// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/vfs/cpp/debug.h"

#ifdef FS_TRACE_DEBUG_ENABLED

std::ostream& operator<<(std::ostream& os, const fs::DeprecatedOptions& options) {
  return os << "DeprecatedOptions{ flags: " << fidl::ostream::Formatted(options.flags)
            << ", rights: " << fidl::ostream::Formatted(options.rights) << "}";
}

std::ostream& operator<<(std::ostream& os, fs::CreationType type) {
  switch (type) {
    case fs::CreationType::kDirectory:
      return os << "directory";
    case fs::CreationType::kFile:
      return os << "file";
  }
}

#endif

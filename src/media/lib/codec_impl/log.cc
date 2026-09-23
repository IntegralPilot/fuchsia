// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/lib/codec_impl/include/lib/media/codec_impl/log.h"

#include <string_view>

namespace codec_impl {
namespace internal {

const char* BaseName(const char* path) {
  std::string_view null_terminated_path(path);
  size_t pos = null_terminated_path.find_last_of('/') + 1;
  return (pos < null_terminated_path.size()) ? null_terminated_path.substr(pos).data()
                                             : null_terminated_path.data();
}

}  // namespace internal
}  // namespace codec_impl

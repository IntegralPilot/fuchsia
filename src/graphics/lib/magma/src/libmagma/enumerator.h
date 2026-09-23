// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_LIB_MAGMA_SRC_LIBMAGMA_ENUMERATOR_H_
#define SRC_GRAPHICS_LIB_MAGMA_SRC_LIBMAGMA_ENUMERATOR_H_

#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/fit/result.h>
#include <lib/magma/magma_common_defs.h>

#include <cstdint>
#include <string>

namespace magma {
class Enumerator {
 public:
  explicit Enumerator(const char* device_namespace, zx::channel dir_channel)
      : device_namespace_(device_namespace),
        client_(fidl::ClientEnd<fuchsia_io::Directory>(std::move(dir_channel))) {}

  // Returns the number of device paths written.
  fit::result<magma_status_t, uint32_t> Enumerate(uint32_t device_path_count,
                                                  uint32_t device_path_size,
                                                  char* device_paths_out);

 private:
  std::string device_namespace_;
  fidl::WireSyncClient<fuchsia_io::Directory> client_;
};
}  // namespace magma

#endif  // SRC_GRAPHICS_LIB_MAGMA_SRC_LIBMAGMA_ENUMERATOR_H_

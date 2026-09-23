// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_FDIO_UNISTD_H_
#define LIB_FDIO_UNISTD_H_

#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/zx/result.h>

#include <fbl/ref_ptr.h>

struct fdio;

namespace fdio_internal {

struct OpenAtOptions {
  // Allow this open request to resolve to a directory. If the operation might open a non-directory,
  // generates a ZX_ERR_NOT_FILE error.
  bool allow_directory;

  // Permit absolute path as input. If the provided path is absolute, instead
  // generates a ZX_ERR_INVALID_ARGS error.
  bool allow_absolute_path;
};

// Translates deprecated `fuchsia.io/OpenFlags` to an equivalent set of `fuchsia.io/Flags`.
fuchsia_io::wire::Flags TranslateDeprecatedFlags(fuchsia_io::wire::OpenFlags deprecated_flags);

// Implements openat logic.
zx::result<fbl::RefPtr<fdio>> OpenAt(int dirfd, const char* path, fuchsia_io::Flags flags,
                                     OpenAtOptions options);

}  // namespace fdio_internal

void fdio_chdir(fbl::RefPtr<fdio> io, const char* path);

#endif  // LIB_FDIO_UNISTD_H_

// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_COMPONENT_INCOMING_CPP_DIRECTORY_H_
#define LIB_COMPONENT_INCOMING_CPP_DIRECTORY_H_

#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/component/incoming/cpp/constants.h>
#include <lib/component/incoming/cpp/internal.h>
#include <lib/zx/result.h>

#include <string_view>

namespace component {

// Opens the directory specified by `path` with given `flags` in the component's incoming namespace.
// `path` must be absolute, containing a leading "/". If `flags` is omitted, defaults to read-only.
//
// # Errors
//
//   * `ZX_ERR_BAD_PATH`: `path` is too long.
//   * `ZX_ERR_NOT_FOUND`: `path` was not found in the incoming namespace.
//   * `ZX_ERR_INVALID_ARGS`: `flags` includes a non-directory protocol.
inline zx::result<fidl::ClientEnd<fuchsia_io::Directory>> OpenDirectory(
    std::string_view path, fuchsia_io::wire::Flags flags = kServiceRootFlags) {
  return internal::OpenDirectory(path, flags);
}

// Opens the directory specified by `path` relative to `dir` with given `flags`. `path` must be
// relative, and point to a valid entry under `dir`. If `flags` is omitted, defaults to read-only.
//
// The operation completes asynchronously, which means a `zx::ok()` result does *not* ensure `path`
// actually exists. Errors are communicated via an epitaph on the returned channel.
//
// # Errors
//
//   * `ZX_ERR_INVALID_ARGS`: `path` is too long or `flags` includes a non-directory protocol.
inline zx::result<fidl::ClientEnd<fuchsia_io::Directory>> OpenDirectoryAt(
    fidl::UnownedClientEnd<fuchsia_io::Directory> dir, std::string_view path,
    fuchsia_io::wire::Flags flags = kServiceRootFlags) {
  return internal::OpenDirectoryAt(dir, path, flags);
}

}  // namespace component

#endif  // LIB_COMPONENT_INCOMING_CPP_DIRECTORY_H_

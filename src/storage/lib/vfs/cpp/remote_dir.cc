// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/vfs/cpp/remote_dir.h"

#include <fidl/fuchsia.io/cpp/common_types.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/string_view.h>
#include <zircon/assert.h>
#include <zircon/availability.h>

#include <utility>

#include "src/storage/lib/vfs/cpp/debug.h"

namespace fio = fuchsia_io;

namespace fs {

RemoteDir::RemoteDir(fidl::ClientEnd<fio::Directory> remote_dir_client)
    : remote_client_(std::move(remote_dir_client)) {
  ZX_DEBUG_ASSERT(remote_client_);
}

RemoteDir::~RemoteDir() = default;

fio::NodeProtocolKinds RemoteDir::GetProtocols() const {
  return fio::NodeProtocolKinds::kDirectory;
}

bool RemoteDir::IsRemote() const { return true; }

#if FUCHSIA_API_LEVEL_LESS_THAN(32) || FUCHSIA_API_LEVEL_AT_LEAST(PLATFORM)
void RemoteDir::DeprecatedOpenRemote(fio::OpenFlags flags, fio::ModeType mode,
                                     fidl::StringView path,
                                     fidl::ServerEnd<fio::Node> object) const {
  // We consume |object| when making the wire call to the remote end, so on failure there isn't
  // anywhere for us to propagate the error.
  [[maybe_unused]] auto status =
      fidl::WireCall(remote_client_)->DeprecatedOpen(flags, mode, path, std::move(object));
  FS_PRETTY_TRACE_DEBUG("RemoteDir::DeprecatedOpenRemote: path='", path, "', flags=", flags,
                        ", response=", status.FormatDescription());
}
#endif

#if FUCHSIA_API_LEVEL_AT_LEAST(31)
void RemoteDir::OpenRemote(fuchsia_io::wire::OpenableOpenRequest request) const {
#else
void RemoteDir::OpenRemote(fuchsia_io::wire::DirectoryOpenRequest request) const {
#endif
  // We consume the |request| channel when making the wire call to the remote end, so on failure
  // there isn't anywhere for us to propagate the error.
  [[maybe_unused]] auto status =
      fidl::WireCall(remote_client_)
          ->Open(request.path, request.flags, request.options, std::move(request.object));
  FS_PRETTY_TRACE_DEBUG("RemoteDir::OpenRemote: path='", request.path, "', flags=", request.flags,
                        "', options=", request.options, ", response=", status.FormatDescription());
}

}  // namespace fs

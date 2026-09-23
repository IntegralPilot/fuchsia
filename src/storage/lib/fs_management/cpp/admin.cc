// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/fs_management/cpp/admin.h"

#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/fdio/directory.h>
#include <lib/zx/channel.h>
#include <zircon/processargs.h>
#include <zircon/syscalls.h>

#include <array>
#include <vector>

#include <fbl/vector.h>

namespace fs_management {

__EXPORT
zx::result<fidl::ClientEnd<fuchsia_io::Directory>> FsRootHandle(
    fidl::UnownedClientEnd<fuchsia_io::Directory> export_root, fuchsia_io::wire::Flags flags) {
  auto [client, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
  const fidl::Status result =
      fidl::WireCall(export_root)->Open("root", flags, {}, server.TakeChannel());
  return zx::make_result(result.status(), std::move(client));
}

}  // namespace fs_management

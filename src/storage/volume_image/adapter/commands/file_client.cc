// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/volume_image/adapter/commands/file_client.h"

#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/fdio/directory.h>

zx::result<fidl::ClientEnd<fuchsia_io::File>> OpenFile(const char* path) {
  auto [client, server] = fidl::Endpoints<fuchsia_io::File>::Create();
  return zx::make_result(fdio_open3(path,
                                    static_cast<uint64_t>(fuchsia_io::wire::kPermReadable |
                                                          fuchsia_io::wire::Flags::kProtocolFile),
                                    server.TakeChannel().release()),
                         std::move(client));
}

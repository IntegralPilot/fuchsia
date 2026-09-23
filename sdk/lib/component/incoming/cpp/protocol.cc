// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/directory.h>

namespace component {

zx::result<fidl::ClientEnd<fuchsia_io::Directory>> OpenServiceRoot(std::string_view path) {
  auto [client, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
  return zx::make_result(fdio_open3(std::string(path).c_str(), uint64_t{kServiceRootFlags},
                                    server.TakeChannel().release()),
                         std::move(client));
}

}  // namespace component

// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.io/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/svc/outgoing.h>
#include <lib/zx/channel.h>

#include "src/storage/lib/vfs/cpp/remote_dir.h"
#include "src/storage/memfs/memfs.h"
#include "src/storage/memfs/vnode_dir.h"

namespace fio = fuchsia_io;

int main(int argc, char* argv[]) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);

  zx::result result = memfs::Memfs::Create(loop.dispatcher(), "<tmp>");
  if (result.is_error()) {
    fprintf(stderr, "Failed to create memfs: %s\n", result.status_string());
    return -1;
  }
  auto [memfs, root] = std::move(result).value();

  svc::Outgoing outgoing(loop.dispatcher());

  std::pair<fbl::String, fio::Flags> dirs[] = {
      {"read_only", fio::kPermReadable},
      {"read_write", fio::kPermReadable | fio::kPermWritable},
      {"read_exec", fio::kPermReadable | fio::kPermExecutable},
      {"read_only_after_scoped", fio::kPermReadable | fio::kPermWritable},
  };

  for (const auto& [name, flags] : dirs) {
    auto [client, server] = fidl::Endpoints<fio::Directory>::Create();
    zx_status_t status = memfs->Serve(root, server.TakeChannel(), flags);
    if (status != ZX_OK) {
      fprintf(stderr, "Failed to serve '%s': %s\n", name.c_str(), zx_status_get_string(status));
      return -1;
    }
    fbl::RefPtr remote = fbl::MakeRefCounted<fs::RemoteDir>(std::move(client));
    status = outgoing.root_dir()->AddEntry(name, std::move(remote));
    if (status != ZX_OK) {
      fprintf(stderr, "Failed to add '%s' to outgoing: %s\n", name.c_str(),
              zx_status_get_string(status));
      return -1;
    }
  }

  if (zx_status_t status = outgoing.ServeFromStartupInfo(); status != ZX_OK) {
    fprintf(stderr, "Failed to serve outgoing dir: %s\n", zx_status_get_string(status));
    return -1;
  }

  if (zx_status_t status = loop.Run(); status != ZX_OK) {
    fprintf(stderr, "Failed to run loop: %s\n", zx_status_get_string(status));
    return -1;
  }

  return 0;
}

// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/io.h>
#include <lib/vfs/cpp/pseudo_dir.h>
#include <lib/vfs/cpp/remote_dir.h>
#include <zircon/processargs.h>

// Serve /pkg as /pkg in the outgoing directory.
int main(int argc, const char* const* argv) {
  constexpr auto kServeFlags = fuchsia_io::wire::kPermReadable | fuchsia_io::wire::kPermExecutable;
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  auto [client_end, server_end] = fidl::Endpoints<fuchsia_io::Directory>::Create();
  zx_status_t status =
      fdio_open3("/pkg", static_cast<uint64_t>(kServeFlags), server_end.TakeChannel().release());
  if (status != ZX_OK) {
    fprintf(stderr, "Failed to open /pkg");
    return -1;
  }

  vfs::PseudoDir root_dir;
  root_dir.AddEntry("pkg", std::make_unique<vfs::RemoteDir>(std::move(client_end)));

  fidl::ServerEnd<fuchsia_io::Directory> outgoing_dir{
      zx::channel(zx_take_startup_handle(PA_DIRECTORY_REQUEST))};
  status = root_dir.Serve(kServeFlags, std::move(outgoing_dir));

  if (status != ZX_OK) {
    fprintf(stderr, "Failed to serve outgoing.");
    return -1;
  }

  loop.Run();
  return 0;
}

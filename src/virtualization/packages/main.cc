// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/io.h>
#include <lib/svc/outgoing.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>

#include "src/storage/lib/vfs/cpp/remote_dir.h"

// Serve /pkg as /pkg in the outgoing directory to provide access to configuration data
// i.e. /pkg/data/guest.cfg
int main(int argc, char** argv) {
  fuchsia_logging::LogSettingsBuilder builder;
  builder.WithTags({"guest_package"}).BuildAndInitialize();
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);

  auto endpoints = fidl::CreateEndpoints<fuchsia_io::Directory>();
  if (endpoints.is_error()) {
    FX_PLOGS(ERROR, endpoints.error_value()) << "Coudn't create endpoints";
    return -1;
  }
  zx_status_t status;
  status = fdio_open3("/pkg", static_cast<uint64_t>(fuchsia_io::wire::kPermReadable),
                      endpoints->server.TakeChannel().release());
  if (status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Failed to open /pkg";
    return -1;
  }

  // sys::OutgoingDirectory doesn't support executable rights, so use svc::Outgoing.
  svc::Outgoing outgoing(loop.dispatcher());

  auto pkg_outgoing_dir = fbl::MakeRefCounted<fs::RemoteDir>(std::move(endpoints->client));
  outgoing.root_dir()->AddEntry("pkg", pkg_outgoing_dir);

  status = outgoing.ServeFromStartupInfo();

  if (status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Failed to serve /pkg to outgoing";
    return -1;
  }
  return loop.Run();
}

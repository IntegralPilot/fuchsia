// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/svc/outgoing.h>

#include <iostream>

#include "minimal_driver_runtime.h"
#include "tun_ctl.h"

int main(int argc, const char** argv) {
  // In order to create the dispatchers needed by the network device implementation a minimal driver
  // runtime implementation is needed.
  zx::result runtime = network::tun::MinimalDriverRuntime::Create();
  if (runtime.is_error()) {
    std::cerr << "Failed to create driver runtime: " << runtime.status_string() << std::endl;
    return -1;
  }

  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  async_dispatcher_t* dispatcher = loop.dispatcher();
  zx::result result = network::tun::TunCtl::Create(dispatcher);
  if (result.is_error()) {
    std::cerr << "error: TunCtl::Create: " << result.status_string() << std::endl;
    return -1;
  }
  std::unique_ptr<network::tun::TunCtl> ctl(std::move(result.value()));
  svc::Outgoing outgoing(dispatcher);

  zx_status_t status;
  status = outgoing.ServeFromStartupInfo();
  if (status != ZX_OK) {
    std::cerr << "error: ServeFromStartupInfo: " << zx_status_get_string(status) << std::endl;
    return -1;
  }

  status = outgoing.svc_dir()->AddEntry(
      fidl::DiscoverableProtocolName<fuchsia_net_tun::Control>,
      fbl::MakeRefCounted<fs::Service>(
          [ctl = ctl.get()](fidl::ServerEnd<fuchsia_net_tun::Control> request) {
            ctl->Connect(std::move(request));
            return ZX_OK;
          }));
  if (status != ZX_OK) {
    std::cerr << "error: AddEntry: " << zx_status_get_string(status) << std::endl;
    return -1;
  }

  loop.Run();
  return 0;
}

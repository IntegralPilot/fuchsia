// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "vendor_helper_generic.h"

#include <lib/async-loop/cpp/loop.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/syslog/cpp/macros.h>

int main() {
  async::Loop async_loop(&kAsyncLoopConfigNeverAttachToThread);

  component::OutgoingDirectory outgoing = component::OutgoingDirectory(async_loop.dispatcher());

  zx::result result = outgoing.ServeFromStartupInfo();
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to serve outgoing directory: " << result.status_string();
    return -1;
  }

  VendorHelperServerGeneric server;
  fidl::ServerBindingGroup<fuchsia_gpu_magma_test::VendorHelper> bindings;

  result =
      outgoing.AddUnmanagedProtocol<fuchsia_gpu_magma_test::VendorHelper>(bindings.CreateHandler(
          &server, async_loop.dispatcher(), std::mem_fn(&VendorHelperServerGeneric::OnClosed)));
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to add VendorHelper protocol: " << result.status_string();
    return -1;
  }

  async_loop.Run();

  return 0;
}

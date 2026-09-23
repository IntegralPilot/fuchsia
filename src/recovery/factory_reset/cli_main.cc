// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.recovery/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>

// A simple command-line tool for initiating factory reset.
int main(int argc, const char** argv) {
  zx::result client_end = component::Connect<fuchsia_recovery::FactoryReset>();
  if (client_end.is_error()) {
    fprintf(stderr, "failed to connect to fuchsia.recovery/FactoryReset: %s\n",
            client_end.status_string());
    return -1;
  }
  const fidl::WireResult result = fidl::WireCall(client_end.value())->Reset();
  if (!result.ok()) {
    if (result.is_peer_closed()) {
      fprintf(stderr,
              "If you're running this from the serial console, "
              "that's unsupported -- try again from fx shell.\n");
    } else {
      fprintf(stderr, "fuchsia.recovery/FactoryReset.Reset call failed: %s\n",
              result.status_string());
    }
    return -1;
  }
  const fidl::WireResponse response = result.value();
  if (zx_status_t status = response.status; status != ZX_OK) {
    fprintf(stderr, "fuchsia.recovery/FactoryReset.Reset returned error: %s\n",
            zx_status_get_string(status));
    return -1;
  }
  return 0;
}

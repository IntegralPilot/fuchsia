// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/inspect/component/cpp/component.h>
#include <lib/syslog/cpp/macros.h>

#include "src/ui/scenic/lib/image-compression/image_compression.h"

int main(int argc, const char** argv) {
  // Create the main async event loop.
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);

  component::OutgoingDirectory outgoing(loop.dispatcher());

  // Initialize inspect
  inspect::ComponentInspector inspector(loop.dispatcher(), inspect::PublishOptions{});
  inspector.Health().StartingUp();

  image_compression::ImageCompression impl;

  ZX_ASSERT(outgoing
                .AddUnmanagedProtocol<fuchsia_ui_compression_internal::ImageCompressor>(
                    impl.GetHandler(loop.dispatcher()))
                .is_ok());

  zx::result result = outgoing.ServeFromStartupInfo();
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to serve outgoing directory: " << result.status_string();
    return -1;
  }

  inspector.Health().Ok();
  FX_LOGS(DEBUG) << "Initialized.";

  // Run the loop until it is shutdown.
  loop.Run();
  return 0;
}

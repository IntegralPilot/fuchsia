// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.blobfs.internal/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace-provider/provider.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <cstdlib>
#include <memory>

#include "src/storage/blobfs/compression/decompressor_sandbox/decompressor_impl.h"

int main(int argc, const char** argv) {
  async::Loop trace_loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  zx_status_t status = trace_loop.StartThread();
  if (status != ZX_OK) {
    return EXIT_FAILURE;
  }
  trace::TraceProviderWithFdio trace_provider(trace_loop.dispatcher());

  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  component::OutgoingDirectory outgoing(loop.dispatcher());

  if (zx::result result = outgoing.ServeFromStartupInfo(); result.is_error()) {
    FX_PLOGS(ERROR, result.status_value()) << "Failed to serve outgoing directory";
    return EXIT_FAILURE;
  }

  if (zx::result result = outgoing.AddProtocol<fuchsia_blobfs_internal::DecompressorCreator>(
          std::make_unique<blobfs::DecompressorImpl>());
      result.is_error()) {
    FX_PLOGS(ERROR, result.status_value()) << "Failed to server DecompressorImpl";
    return EXIT_FAILURE;
  }

  return loop.Run();
}

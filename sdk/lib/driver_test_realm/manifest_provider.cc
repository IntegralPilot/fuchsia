// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.driver.test/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/default.h>
#include <lib/component/incoming/cpp/directory.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>

namespace {

class ManifestProvider final : public fidl::WireServer<fuchsia_driver_test::ManifestProvider> {
 public:
  void GetManifest(GetManifestRequestView request, GetManifestCompleter::Sync& completer) override {
    zx::result dir = component::OpenDirectory("/pkg/meta/");
    if (dir.is_error()) {
      FX_LOGS(ERROR) << "failed to open /pkg/meta/: " << dir.error_value();
      completer.ReplyError(dir.error_value());
      return;
    }

    std::string path;
    if (request->has_using_subpackage() && request->using_subpackage()) {
      path = "driver_test_realm_base_subpackage.cm";
    } else {
      path = "driver_test_realm_base_no_subpackage.cm";
    }

    auto [client, server] = fidl::Endpoints<fuchsia_io::File>::Create();
    auto open_result =
        fidl::Call(dir.value())
            ->Open({{
                .path = path,
                .flags = fuchsia_io::Flags::kPermReadBytes | fuchsia_io::Flags::kProtocolFile,
                .options = {},
                .object = server.TakeChannel(),
            }});
    if (open_result.is_error()) {
      FX_LOGS(ERROR) << "failed to open the base manifest: "
                     << open_result.error_value().FormatDescription();
      completer.ReplyError(open_result.error_value().status());
      return;
    }

    auto memory_result = fidl::Call(client)->GetBackingMemory(
        {{fuchsia_io::VmoFlags::kRead | fuchsia_io::VmoFlags::kSharedBuffer}});
    if (memory_result.is_error()) {
      FX_LOGS(ERROR) << "failed to GetBackingMemory of the base manifest: "
                     << memory_result.error_value().FormatDescription();
      completer.ReplyError(ZX_ERR_INTERNAL);
      return;
    }

    zx::stream out;
    zx_status_t stream_result =
        zx::stream::create(ZX_STREAM_MODE_READ, memory_result->vmo(), 0, &out);
    if (stream_result != ZX_OK) {
      FX_LOGS(ERROR) << "Failed to create stream from base manifest vmo";
      completer.ReplyError(ZX_ERR_INTERNAL);
      return;
    }
    completer.ReplySuccess(std::move(out));
  }
};

}  // namespace

int main(int argc, const char** argv) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  fuchsia_logging::LogSettingsBuilder builder;
  builder.WithDispatcher(loop.dispatcher()).BuildAndInitialize();

  component::OutgoingDirectory outgoing(loop.dispatcher());
  {
    zx::result result = outgoing.AddProtocol<fuchsia_driver_test::ManifestProvider>(
        std::make_unique<ManifestProvider>());
    ZX_ASSERT(result.is_ok());
  }

  {
    zx::result result = outgoing.ServeFromStartupInfo();
    ZX_ASSERT(result.is_ok());
  }

  loop.Run();
  return 0;
}

// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.audio.device/cpp/markers.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/syslog/cpp/macros.h>

#include <memory>

#include "src/media/audio/services/common/fidl_thread.h"
#include "src/media/audio/services/device_registry/testing/stub_adr/stub_control_creator_server.h"
#include "src/media/audio/services/device_registry/testing/stub_adr/stub_provider_server.h"
#include "src/media/audio/services/device_registry/testing/stub_adr/stub_registry_server.h"

namespace media_audio {
namespace {

zx_status_t RegisterAndServeOutgoing(component::OutgoingDirectory& outgoing,
                                     const std::shared_ptr<FidlThread>& thread) {
  auto status = outgoing.AddUnmanagedProtocol<fuchsia_audio_device::Provider>(
      [t = thread](fidl::ServerEnd<fuchsia_audio_device::Provider> server_end) mutable {
        FX_LOGS(INFO) << "Incoming connection for fuchsia.audio.device.Provider";

        // The underlying BaseFidlServer::Create ensures that this server stays alive until the FIDL
        // connection drops. Thus, the shared_ptr is unused and can expire at the end of this scope.
        [[maybe_unused]] auto provider = StubProviderServer::Create(t, std::move(server_end));
      });

  if (status.is_error()) {
    FX_LOGS(ERROR) << "Failed to add Provider protocol: " << status.error_value() << " ("
                   << status.status_string() << ")";
    return status.status_value();
  }

  status = outgoing.AddUnmanagedProtocol<fuchsia_audio_device::Registry>(
      [t = thread](fidl::ServerEnd<fuchsia_audio_device::Registry> server_end) mutable {
        FX_LOGS(INFO) << "Incoming connection for fuchsia.audio.device.Registry";

        // The underlying BaseFidlServer::Create ensures that this server stays alive until the FIDL
        // connection drops. Thus, the shared_ptr is unused and can expire at the end of this scope.
        [[maybe_unused]] auto registry = StubRegistryServer::Create(t, std::move(server_end));
      });
  if (status.is_error()) {
    FX_LOGS(ERROR) << "Failed to add Registry protocol: " << status.error_value() << " ("
                   << status.status_string() << ")";
    return status.status_value();
  }

  status = outgoing.AddUnmanagedProtocol<fuchsia_audio_device::ControlCreator>(
      [t = thread](fidl::ServerEnd<fuchsia_audio_device::ControlCreator> server_end) mutable {
        FX_LOGS(INFO) << "Incoming connection for fuchsia.audio.device.ControlCreator";

        // The underlying BaseFidlServer::Create ensures that this server stays alive until the FIDL
        // connection drops. Thus, the shared_ptr is unused and can expire at the end of this scope.
        [[maybe_unused]] auto control_creator =
            StubControlCreatorServer::Create(t, std::move(server_end));
      });
  if (status.is_error()) {
    FX_LOGS(ERROR) << "Failed to add ControlCreator protocol: " << status.error_value() << " ("
                   << status.status_string() << ")";
    return status.status_value();
  }

  // Set up an outgoing directory with the startup handle (provided by the system to components so
  // they can serve out FIDL protocols etc).
  return outgoing.ServeFromStartupInfo().status_value();
}

}  // namespace
}  // namespace media_audio

int main(int argc, const char** argv) {
  FX_LOGS(INFO) << "StubAdrServer is starting up";

  // Create a loop, and use it to create our OutgoingDirectory...
  auto loop = std::make_shared<async::Loop>(&kAsyncLoopConfigAttachToCurrentThread);
  auto thread =
      media_audio::FidlThread::CreateFromCurrentThread("StubAdrServerMain", loop->dispatcher());
  component::OutgoingDirectory outgoing{thread->dispatcher()};

  // ...then register the FIDL services and serve them out, so clients can call them...
  if (media_audio::RegisterAndServeOutgoing(outgoing, thread) != ZX_OK) {
    FX_LOGS(ERROR) << "RegisterAndServeOutgoing failed to serve outgoing directory";
    return -1;
  }

  // ...then run our loop here in main().
  loop->Run();

  FX_LOGS(INFO) << "Exiting StubAdrServer main()";
  return 0;
}

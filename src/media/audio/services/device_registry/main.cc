// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/scheduler/role.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace-provider/provider.h>

#include <memory>
#include <string>

#include "src/media/audio/services/common/fidl_thread.h"
#include "src/media/audio/services/device_registry/audio_device_registry.h"
#include "src/media/audio/services/device_registry/audio_device_registry_config.h"
#include "src/media/audio/services/device_registry/inspector.h"
#include "src/media/audio/services/device_registry/logging.h"
#include "src/media/audio/services/device_registry/strings.h"

using media_audio::Inspector;

int main(int argc, const char** argv) {
  fuchsia_logging::LogSettingsBuilder builder;
  builder.WithTags({media_audio::kAdrLoggingTag}).BuildAndInitialize();
  ADR_LOG(media_audio::kLogMain) << "AudioDeviceRegistry is starting up";

  auto config = audio_device_registry_config::Config::TakeFromStartupHandle();
  bool ignore_devices = config.ignore_devices();

  // Create a loop, and use it to create our AudioDeviceRegistry singleton...
  auto loop = std::make_shared<async::Loop>(&kAsyncLoopConfigAttachToCurrentThread);

  // Set a Scheduler Profile for our main thread, if we will operate normally (i.e. with devices).
  if (ignore_devices) {
    ADR_LOG(media_audio::kLogRoleManager)
        << "'ignore_devices' is set: will not apply Scheduler Profile";
  } else if (auto role_status =
                 fuchsia_scheduler::SetRoleForThisThread(media_audio::kAdrSchedulerRole);
             role_status != ZX_OK) {
    // Failing to apply a Scheduler Profile is not fatal (e.g., it may happen in tests), but warn
    // because performance may suffer.
    FX_LOGS(WARNING) << "Failed to apply Scheduler Profile: " << role_status;
  } else {
    ADR_LOG(media_audio::kLogRoleManager) << "********** Applied Scheduler Profile *******";
  }

  auto adr_thread = media_audio::FidlThread::CreateFromCurrentThread(media_audio::kAdrThreadName,
                                                                     loop->dispatcher());
  auto adr_service = std::make_shared<media_audio::AudioDeviceRegistry>(adr_thread);

  trace::TraceProviderWithFdio trace_provider(loop->dispatcher(), media_audio::kAdrTraceProvider);

  // ...then create the connection to Inspect, so we can chronicle the subsequent actions...
  Inspector::Initialize(loop->dispatcher());

  // ...then start the device detection process (which continues after this call returns)...
  if (auto status = adr_service->StartDeviceDetection(ignore_devices); status != ZX_OK) {
    auto str = std::string("StartDeviceDetection failed to start devfs device detection: ") +
               std::to_string(status);
    FX_LOGS(ERROR) << str;
    Inspector::Singleton()->RecordUnhealthy(str);
    return -1;
  }

  // ...then register the FIDL services and serve them out, so clients can call them...
  if (auto status = adr_service->RegisterAndServeOutgoing(); status != ZX_OK) {
    auto str = std::string("RegisterAndServeOutgoing failed to serve outgoing directory: ") +
               std::to_string(status);
    FX_LOGS(ERROR) << str;
    Inspector::Singleton()->RecordUnhealthy(str);
    return -2;
  }

  // ...then chronicle that adr_service has completed its "starting up" steps...
  Inspector::Singleton()->RecordHealthOk();

  // ...then run our loop here in main(), so AudioDeviceRegistry doesn't have to deal with it.
  loop->Run();

  ADR_LOG(media_audio::kLogMain) << "Exiting AudioDeviceRegistry main()";
  return 0;
}

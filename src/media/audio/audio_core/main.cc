// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace-provider/provider.h>

#include "src/lib/fxl/command_line.h"
#include "src/media/audio/audio_core/audio_core_impl.h"
#include "src/media/audio/audio_core/base_capturer.h"
#include "src/media/audio/audio_core/clock.h"
#include "src/media/audio/audio_core/plug_detector.h"
#include "src/media/audio/audio_core/process_config_loader.h"
#include "src/media/audio/audio_core/profile_acquirer.h"
#include "src/media/audio/audio_core/profile_provider.h"
#include "src/media/audio/audio_core/reporter.h"
#include "src/media/audio/audio_core/thermal_watcher.h"
#include "src/media/audio/audio_core/threading_model.h"
#include "src/media/audio/audio_core/ultrasound_factory.h"

namespace media::audio {

constexpr char kProcessConfigPath[] = "/config/data/audio_core_config.json";

static int StartAudioCore(const fxl::CommandLine& cl) {
  fuchsia_logging::LogSettingsBuilder builder;
  builder.WithMinLogSeverity(FUCHSIA_LOG_INFO).WithTags({"audio_core"}).BuildAndInitialize();

  FX_LOGS(INFO) << "AudioCore starting up";

  // Page in and pin our executable.
  auto result = AcquireMemoryRole(zx::vmar::root_self(), "fuchsia.media.audio.core");
  if (result.is_error()) {
    // Failing to apply a Memory Profile is not fatal (e.g. it may happen in tests),
    // but we warn because performance may suffer.
    FX_PLOGS(WARNING, result.status_value())
        << "Unable to set memory role for the audio_core process";
  }

  auto process_config = ProcessConfigLoader::LoadProcessConfig(kProcessConfigPath);
  if (process_config.is_error()) {
    FX_LOGS(WARNING) << "Failed to load " << kProcessConfigPath << ": " << process_config.error()
                     << ". Falling back to default configuration.";
    process_config = fpromise::ok(ProcessConfig::Builder()
                                      .SetDefaultVolumeCurve(VolumeCurve::DefaultForMinGain(
                                          VolumeCurve::kDefaultGainForMinVolume))
                                      .Build());
  }
  FX_CHECK(process_config);
  auto threading_model = ThreadingModel::CreateWithMixStrategy(
      MixStrategy::kThreadPerMix, process_config.value().mix_profile_config());
  trace::TraceProviderWithFdio trace_provider(threading_model->FidlDomain().dispatcher());

  auto component_context = sys::ComponentContext::CreateAndServeOutgoingDirectory();
  auto enable_cobalt = !cl.HasOption("disable-cobalt");
  Reporter::InitializeSingleton(*component_context, threading_model->FidlDomain().dispatcher(),
                                threading_model->IoDomain().dispatcher(), enable_cobalt);

  auto context = Context::Create(std::move(threading_model), std::move(component_context),
                                 PlugDetector::Create(), process_config.take_value(),
                                 std::make_shared<AudioCoreClockFactory>());
  context->PublishOutgoingServices();

  auto thermal_watcher = ThermalWatcher::CreateAndWatch(*context);
  auto ultrasound_factory = UltrasoundFactory::CreateAndServe(context.get());

  ProfileProvider profile_provider(context->component_context(),
                                   context->process_config().mix_profile_config());
  context->component_context().outgoing()->AddPublicService(
      profile_provider.GetFidlRequestHandler());

  context->threading_model().RunAndJoinAllThreads();

  // In normal operation these threads run perpetually, so exiting represents an error condition.
  return -1;
}

}  // namespace media::audio

int main(int argc, const char** argv) {
  return media::audio::StartAudioCore(fxl::CommandLineFromArgcArgv(argc, argv));
}

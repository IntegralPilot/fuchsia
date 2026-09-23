// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.feedback.internal/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fidl/cpp/binding.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace-provider/provider.h>
#include <lib/zx/time.h>
#include <zircon/processargs.h>

#include <optional>

#include "src/developer/forensics/feedback/config.h"
#include "src/developer/forensics/feedback/constants.h"
#include "src/developer/forensics/feedback/redactor_factory.h"
#include "src/developer/forensics/feedback_data/constants.h"
#include "src/developer/forensics/feedback_data/system_log_recorder/controller.h"
#include "src/developer/forensics/feedback_data/system_log_recorder/encoding/production_encoding.h"
#include "src/developer/forensics/feedback_data/system_log_recorder/shutdown_message.h"
#include "src/developer/forensics/feedback_data/system_log_recorder/system_log_recorder.h"
#include "src/lib/timekeeper/system_clock.h"

namespace forensics {
namespace feedback_data {
namespace system_log_recorder {

constexpr zx::duration kWritePeriod = zx::sec(1);

int main() {
  async::Loop main_loop(&kAsyncLoopConfigAttachToCurrentThread);
  fuchsia_logging::LogSettingsBuilder builder;
  builder.WithTags({"forensics", "feedback"}).BuildAndInitialize();

  // We receive a channel that we interpret as a fuchsia.process.lifecycle.Lifecycle
  // connection.
  zx::channel lifecycle_channel(zx_take_startup_handle(PA_HND(PA_USER0, 0)));
  if (!lifecycle_channel.is_valid()) {
    FX_LOGS(FATAL) << "Received invalid lifecycle channel";
    return EXIT_FAILURE;
  }

  const std::optional<feedback::FeedbackConfig> feedback_config = feedback::GetFeedbackConfig();
  if (!feedback_config.has_value()) {
    FX_LOGS(FATAL) << "Failed to read feedback config";
    return EXIT_FAILURE;
  }

  async::Loop write_loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  trace::TraceProviderWithFdio trace_provider(main_loop.dispatcher(), "system_log_recorder");

  if (const zx_status_t status = write_loop.StartThread("writer-thread"); status != ZX_OK) {
    FX_PLOGS(FATAL, status) << "Failed to start writer thread";
    return EXIT_FAILURE;
  }

  std::unique_ptr<sys::ComponentContext> context =
      sys::ComponentContext::CreateAndServeOutgoingDirectory();

  SystemLogRecorder recorder(
      main_loop.dispatcher(), write_loop.dispatcher(), context->svc(),
      SystemLogRecorder::WriteParameters{
          .period = kWritePeriod,
          .max_write_size = kMaxWriteSize,
          .logs_dir = feedback::kCurrentLogsDir,
          .max_num_files = feedback::kPersistedLogsNumFiles,
          .total_log_size = feedback::kPersistedLogsTotalSize,
          .metadata_path = feedback::kCurrentDiskBackedLogsMetadataPath,
          .fallback_buffer_size = feedback_data::kFallbackLogBufferSize,
      },
      // Don't set up Inspect because all messages in the previous boot log
      // are in the current boot log and counted in Inspect.
      feedback::RedactorFromConfig(nullptr /*no inspect*/, feedback_config->build_type_config),
      std::make_unique<ProductionEncoder>(), std::make_unique<ProductionDecoder>());

  context->outgoing()->AddPublicService(
      [&recorder](zx::channel channel, async_dispatcher_t* dispatcher) {
        fidl::BindServer(
            dispatcher,
            fidl::ServerEnd<fuchsia_feedback_internal::SystemLogRecorder>(std::move(channel)),
            &recorder);
      },
      fuchsia_feedback_internal::SystemLogRecorder::kDiscoverableName);

  // Set up the controller to shut down or flush the buffers of the system log recorder when it gets
  // the signal to do so.
  Controller controller;
  ::fidl::Binding<fuchsia::process::lifecycle::Lifecycle> lifecycle_binding(
      &controller, std::move(lifecycle_channel), main_loop.dispatcher());

  controller.SetStop([&] {
    recorder.Flush(ShutdownMessage(timekeeper::SystemClock().BootNow()),
                   [&] { lifecycle_binding.Close(ZX_OK); });
    // Don't stop the loop so incoming logs can be persisted while waiting to terminate
    // components.
  });

  recorder.Start();

  main_loop.Run();

  FX_LOGS(INFO) << "Shutting down the system log recorder";

  return EXIT_SUCCESS;
}

}  // namespace system_log_recorder
}  // namespace feedback_data
}  // namespace forensics

// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/last_reboot/reporter.h"

#include <lib/async/cpp/task.h>
#include <lib/fpromise/bridge.h>
#include <lib/fpromise/result.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/time.h>
#include <zircon/types.h>

#include <string>
#include <vector>

#include "src/developer/forensics/feedback/config.h"
#include "src/developer/forensics/utils/errors.h"
#include "src/lib/files/file.h"
#include "src/lib/fsl/vmo/strings.h"

namespace forensics {
namespace last_reboot {
namespace {

constexpr char kHasReportedOnPath[] = "/tmp/has_reported_on_reboot_log.txt";

}  // namespace

Reporter::Reporter(async_dispatcher_t* dispatcher, cobalt::Logger* cobalt, RedactorBase* redactor,
                   fuchsia::feedback::CrashReporter* crash_reporter)
    : dispatcher_(dispatcher),
      executor_(dispatcher),
      cobalt_(cobalt),
      redactor_(redactor),
      crash_reporter_(crash_reporter) {}

void Reporter::ReportOn(const feedback::RebootLog& reboot_log, zx::duration crash_reporting_delay,
                        feedback::SpontaneousRebootReason spontaneous_reboot_reason) {
  if (files::IsFile(kHasReportedOnPath)) {
    FX_LOGS(INFO)
        << "Reboot log has already been reported on in another instance of this component "
           "for this boot cycle";
    return;
  }

  if (!files::WriteFile(kHasReportedOnPath, /*data=*/"", /*size=*/0)) {
    FX_LOGS(ERROR) << "Failed to record reboot log as reported on";
  }

  const zx::duration uptime = reboot_log.GetFinalShutdownInfo().Uptime().value_or(zx::usec(0));
  cobalt_->LogDuration(reboot_log.GetFinalShutdownInfo().ToCobaltLastRebootReason(), uptime);

  if (!reboot_log.GetFinalShutdownInfo().IsCrash()) {
    return;
  }

  executor_.schedule_task(
      FileCrashReport(reboot_log, crash_reporting_delay, spontaneous_reboot_reason));
}

namespace {

fuchsia::feedback::CrashReport CreateCrashReport(
    const feedback::RebootLog& reboot_log, RedactorBase* redactor,
    feedback::SpontaneousRebootReason spontaneous_reboot_reason) {
  // Build the crash report.
  fuchsia::feedback::CrashReport report;
  const feedback::FinalShutdownInfo& final_shutdown_info = reboot_log.GetFinalShutdownInfo();

  report.set_program_name(final_shutdown_info.ToCrashProgramName())
      .set_crash_signature(final_shutdown_info.ToCrashSignature(spontaneous_reboot_reason))
      .set_is_fatal(true);
  if (final_shutdown_info.Uptime().has_value()) {
    report.set_program_uptime(final_shutdown_info.Uptime()->get());
  }

  std::string reboot_log_str = reboot_log.RebootLogStr();

  // Build the crash report attachments.
  fsl::SizedVmo vmo;
  if (fsl::VmoFromString(redactor->Redact(reboot_log_str), &vmo)) {
    std::vector<fuchsia::feedback::Attachment> attachments(1);
    attachments.back().key = "reboot_crash_log";
    attachments.back().value = std::move(vmo).ToTransport();
    report.set_attachments(std::move(attachments));
  }

  return report;
}

}  // namespace

::fpromise::promise<void> Reporter::FileCrashReport(
    const feedback::RebootLog& reboot_log, const zx::duration delay,
    feedback::SpontaneousRebootReason spontaneous_reboot_reason) {
  ::fuchsia::feedback::CrashReport report =
      CreateCrashReport(reboot_log, redactor_, spontaneous_reboot_reason);

  ::fpromise::bridge<void, Error> bridge;

  auto completer =
      std::make_shared<::fpromise::completer<void, Error>>(std::move(bridge.completer));

  delayed_crash_reporting_.Reset([this, report = std::move(report), completer]() mutable {
    crash_reporter_->FileReport(
        std::move(report), [completer](fuchsia::feedback::CrashReporter_FileReport_Result result) {
          if (!*completer) {
            return;
          }

          if (result.is_err()) {
            completer->complete_error(Error::kBadValue);
          } else {
            completer->complete_ok();
          }
        });
  });

  if (const zx_status_t status = async::PostDelayedTask(
          dispatcher_, [cb = delayed_crash_reporting_.callback()] { cb(); }, delay);
      status != ZX_OK) {
    completer->complete_error(Error::kAsyncTaskPostFailure);
  }

  return bridge.consumer.promise_or(::fpromise::error(Error::kLogicError))
      .then([this](const ::fpromise::result<void, Error>& result) {
        delayed_crash_reporting_.Cancel();
        if (result.is_error()) {
          FX_LOGS(WARNING) << "Failed to file a crash report: " << ToString(result.error());
        }

        return ::fpromise::ok();
      });
}

}  // namespace last_reboot
}  // namespace forensics

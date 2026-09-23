// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <errno.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fd.h>
#include <lib/fdio/fdio.h>
#include <lib/fidl/txn_header.h>
#include <lib/zx/channel.h>
#include <stdint.h>
#include <stdio.h>
#include <zircon/status.h>

#include <string_view>
#include <utility>

#include <runtests-utils/log-exporter.h>

namespace runtests {

LogExporter::LogExporter(FILE* output_file, async_dispatcher_t* dispatcher,
                         fidl::ServerEnd<fuchsia_logger::LogListenerSafe> channel,
                         ErrorHandler error_handler, FileErrorHandler file_error_handler)
    : file_error_handler_(std::move(file_error_handler)),
      output_file_(output_file),
      binding_(dispatcher, std::move(channel), this,
               [error_handler = std::move(error_handler)](fidl::UnbindInfo info) {
                 if (error_handler != nullptr) {
                   error_handler(info.status());
                 }
               }) {}

LogExporter::~LogExporter() {
  if (output_file_ != nullptr) {
    fclose(output_file_);
  }
}

// Returns only seconds part
uint64_t GetSeconds(zx::time_boot timestamp) { return timestamp.get() / 1000000000UL; }

// Returns only nano seconds part
uint64_t GetNanoSeconds(zx::time_boot timestamp) { return (timestamp.get() / 1000UL) % 1000000UL; }

#define RETURN_IF_ERROR(expr) \
  do {                        \
    ssize_t n = (expr);       \
    if (n < 0) {              \
      return n;               \
    }                         \
  } while (false)

int LogExporter::WriteSeverity(int32_t severity) {
  switch (severity) {
    case static_cast<int32_t>(fuchsia_logger::wire::LogLevelFilter::kInfo):
      return fputs(" INFO", output_file_);
    case static_cast<int32_t>(fuchsia_logger::wire::LogLevelFilter::kWarn):
      return fputs(" WARNING", output_file_);
    case static_cast<int32_t>(fuchsia_logger::wire::LogLevelFilter::kError):
      return fputs(" ERROR", output_file_);
    case static_cast<int32_t>(fuchsia_logger::wire::LogLevelFilter::kFatal):
      return fputs(" FATAL", output_file_);
    default:
      // all other cases severity would be a negative number so print it as
      // VLOG(n) where severity=-n
      return fprintf(output_file_, " VLOG(%d)", -severity);
  }
}

ssize_t LogExporter::LogMessage(fuchsia_logger::wire::LogMessage message) {
  RETURN_IF_ERROR(fprintf(output_file_, "[%05ld.%06ld][%lu][%lu]", GetSeconds(message.time),
                          GetNanoSeconds(message.time), message.pid, message.tid));
  RETURN_IF_ERROR(fputs("[", output_file_));
  for (size_t i = 0; i < message.tags.size(); ++i) {
    if (i != 0) {
      RETURN_IF_ERROR(fputs(", ", output_file_));
    }
    const fidl::StringView& tag = message.tags[i];
    RETURN_IF_ERROR(fwrite(tag.data(), 1, tag.size(), output_file_));
  }
  RETURN_IF_ERROR(fputs("]", output_file_));

  RETURN_IF_ERROR(WriteSeverity(message.severity));

  RETURN_IF_ERROR(fputs(": ", output_file_));
  RETURN_IF_ERROR(fwrite(message.msg.data(), 1, message.msg.size(), output_file_));
  RETURN_IF_ERROR(fputs("\n", output_file_));
  if (message.dropped_logs > 0) {
    bool log = true;
    bool found = false;
    for (DroppedLogs& dl : dropped_logs_) {
      if (dl.pid == message.pid) {
        found = true;
        // only update our vector when we get new dropped_logs value.
        if (dl.dropped_logs < message.dropped_logs) {
          dl.dropped_logs = message.dropped_logs;
        } else {
          log = false;
        }
        break;
      }
    }
    if (!found) {
      dropped_logs_.push_back(DroppedLogs{message.pid, message.dropped_logs});
    }
    if (log) {
      RETURN_IF_ERROR(fprintf(output_file_, "[%05ld.%06ld][%lu][%lu]", GetSeconds(message.time),
                              GetNanoSeconds(message.time), message.pid, message.tid));
      RETURN_IF_ERROR(fputs("[", output_file_));
      for (size_t i = 0; i < message.tags.size(); ++i) {
        if (i != 0) {
          RETURN_IF_ERROR(fputs(", ", output_file_));
        }
        const fidl::StringView& tag = message.tags[i];
        RETURN_IF_ERROR(fwrite(tag.data(), 1, tag.size(), output_file_));
      }
      RETURN_IF_ERROR(fputs("]", output_file_));
      RETURN_IF_ERROR(
          fprintf(output_file_, " WARNING: Dropped logs count:%d\n", message.dropped_logs));
    }
  }
  return 0;
}

void LogExporter::Log(LogRequestView request, LogCompleter::Sync& completer) {
  if (LogMessage(request->log) < 0) {
    NotifyFileError(strerror(errno));
  };
  completer.Reply();
}

void LogExporter::LogMany(LogManyRequestView request, LogManyCompleter::Sync& completer) {
  for (const auto& msg : request->log) {
    if (LogMessage(msg) < 0) {
      NotifyFileError(strerror(errno));
    }
  }
  completer.Reply();
}

void LogExporter::Done(DoneCompleter::Sync& completer) {}

void LogExporter::NotifyFileError(const char* error) {
  fclose(output_file_);
  output_file_ = nullptr;
  if (file_error_handler_) {
    file_error_handler_(error);
  }
}

std::unique_ptr<LogExporter> LaunchLogExporter(async_dispatcher_t* dispatcher,
                                               const std::string_view syslog_path,
                                               ExporterLaunchError* error) {
  *error = ExporterLaunchError::NO_ERROR;
  fbl::String syslog_path_str = fbl::String(syslog_path.data());
  FILE* syslog_file = fopen(syslog_path_str.c_str(), "w");
  if (syslog_file == nullptr) {
    fprintf(stderr, "Error: Could not open syslog file '%s': %s\n", syslog_path_str.c_str(),
            strerror(errno));
    *error = ExporterLaunchError::OPEN_FILE;
    return nullptr;
  }

  zx::result syslog = component::Connect<fuchsia_logger::Log>();
  if (syslog.is_error()) {
    fprintf(stderr, "LaunchLogExporter: cannot connect to logger service: %d (%s).\n",
            syslog.error_value(), syslog.status_string());
    *error = ExporterLaunchError::CONNECT_TO_LOGGER_SERVICE;
    return nullptr;
  }

  // Create a log exporter channel and pass it to logger service.
  zx::result endpoints = fidl::CreateEndpoints<fuchsia_logger::LogListenerSafe>();
  if (endpoints.is_error()) {
    fprintf(stderr, "LaunchLogExporter: cannot create channel for listener: %d (%s).\n",
            endpoints.error_value(), endpoints.status_string());
    *error = ExporterLaunchError::CREATE_CHANNEL;
    return nullptr;
  }
  auto& [client, server] = endpoints.value();

  const fidl::OneWayStatus status =
      fidl::WireCall(syslog.value())->ListenSafe(std::move(client), {});
  if (!status.ok()) {
    fprintf(stderr, "LaunchLogExporter: cannot pass listener to logger service: %d (%s).\n",
            status.status(), status.status_string());
    *error = ExporterLaunchError::FIDL_ERROR;
    return nullptr;
  }

  // Connect log exporter channel to object and start message loop on it.
  return std::make_unique<LogExporter>(
      syslog_file, dispatcher, std::move(server),
      [](zx_status_t status) {
        if (status != ZX_ERR_CANCELED) {
          fprintf(stderr, "log exporter: Failed: %d (%s).\n", status, zx_status_get_string(status));
        }
      },
      [](const char* error) {
        fprintf(stderr, "log exporter: Error writing to file: %s.\n", error);
      });
}

}  // namespace runtests

// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/crash_reports/crash_server.h"

#include <fuchsia/net/http/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/time.h>

#include <memory>
#include <string_view>
#include <variant>

#include <src/lib/fostr/fidl/fuchsia/net/http/formatting.h>

#include "src/developer/forensics/crash_reports/sized_data_reader.h"
#include "src/developer/forensics/feedback/annotations/annotation_manager.h"
#include "src/developer/forensics/feedback/annotations/constants.h"
#include "src/developer/forensics/feedback/annotations/types.h"
#include "src/developer/forensics/utils/cobalt/metrics.h"
#include "src/developer/forensics/utils/time.h"
#include "src/lib/fsl/socket/blocking_drain.h"
#include "src/lib/fsl/vmo/sized_vmo.h"
#include "src/lib/fsl/vmo/vector.h"
#include "src/lib/fxl/strings/substitute.h"
#include "third_party/crashpad/src/util/net/http_body.h"
#include "third_party/crashpad/src/util/net/http_headers.h"
#include "third_party/crashpad/src/util/net/http_multipart_builder.h"
#include "third_party/crashpad/src/util/net/url.h"

namespace forensics {
namespace crash_reports {
namespace {

cobalt::ReportUploadSize ToCobaltReportUploadSize(const uint64_t size_bytes) {
  const uint64_t kib = size_bytes / 1024;
  if (kib < 250) {
    return cobalt::ReportUploadSize::kLessThan250;
  }
  if (kib < 500) {
    return cobalt::ReportUploadSize::kLessThan500;
  }
  if (kib < 750) {
    return cobalt::ReportUploadSize::kLessThan750;
  }
  if (kib < 1000) {
    return cobalt::ReportUploadSize::kLessThan1000;
  }
  if (kib < 1250) {
    return cobalt::ReportUploadSize::kLessThan1250;
  }
  if (kib < 1500) {
    return cobalt::ReportUploadSize::kLessThan1500;
  }
  if (kib < 1750) {
    return cobalt::ReportUploadSize::kLessThan1750;
  }
  if (kib < 2000) {
    return cobalt::ReportUploadSize::kLessThan2000;
  }
  if (kib < 2250) {
    return cobalt::ReportUploadSize::kLessThan2250;
  }
  if (kib < 2500) {
    return cobalt::ReportUploadSize::kLessThan2500;
  }
  if (kib < 2750) {
    return cobalt::ReportUploadSize::kLessThan2750;
  }
  if (kib < 3000) {
    return cobalt::ReportUploadSize::kLessThan3000;
  }
  if (kib < 3250) {
    return cobalt::ReportUploadSize::kLessThan3250;
  }
  if (kib < 3500) {
    return cobalt::ReportUploadSize::kLessThan3500;
  }
  if (kib < 3750) {
    return cobalt::ReportUploadSize::kLessThan3750;
  }
  if (kib < 4000) {
    return cobalt::ReportUploadSize::kLessThan4000;
  }
  if (kib < 4250) {
    return cobalt::ReportUploadSize::kLessThan4250;
  }
  if (kib < 4500) {
    return cobalt::ReportUploadSize::kLessThan4500;
  }
  if (kib < 4750) {
    return cobalt::ReportUploadSize::kLessThan4750;
  }
  if (kib < 5000) {
    return cobalt::ReportUploadSize::kLessThan5000;
  }

  return cobalt::ReportUploadSize::kGreaterThan5000;
}

cobalt::ReportUploadStatus ToCobaltReportUploadStatus(const CrashServer::UploadStatus status) {
  switch (status) {
    case CrashServer::UploadStatus::kSuccess:
      return cobalt::ReportUploadStatus::kSuccess;
    case CrashServer::UploadStatus::kFailure:
      return cobalt::ReportUploadStatus::kFailure;
    case CrashServer::UploadStatus::kThrottled:
      return cobalt::ReportUploadStatus::kThrottled;
    case CrashServer::UploadStatus::kTimedOut:
      return cobalt::ReportUploadStatus::kTimedOut;
  }
}

// Builds a fuchsia::net::http::Request.
std::optional<fuchsia::net::http::Request> BuildRequest(const std::string_view method,
                                                        const std::string_view url,
                                                        const zx::duration timeout,
                                                        const crashpad::HTTPHeaders& headers,
                                                        crashpad::HTTPBodyStream& body) {
  using fuchsia::net::http::Body;
  using fuchsia::net::http::Header;
  using fuchsia::net::http::Request;

  std::vector<Header> http_headers;
  for (const auto& [name, value] : headers) {
    http_headers.push_back(Header{
        .name = std::vector<uint8_t>(name.begin(), name.end()),
        .value = std::vector<uint8_t>(value.begin(), value.end()),
    });
  }

  // Create the request body as a single VMO.
  // TODO(https://fxbug.dev/42137232): Consider using a zx::socket to transmit the HTTP request body
  // to the server piecewise.
  std::vector<uint8_t> body_vec;

  // Reserve 256 kb for the request body.
  body_vec.reserve(256 * 1024);
  while (true) {
    // Copy the body in 32 kb chunks.
    std::array<uint8_t, 32 * 1024> buf;
    const crashpad::FileOperationResult result = body.GetBytesBuffer(buf.data(), buf.max_size());

    FX_CHECK(result >= 0);
    if (result == 0) {
      break;
    }

    body_vec.insert(body_vec.end(), buf.data(), buf.data() + result);
  }

  fsl::SizedVmo body_vmo;
  if (!fsl::VmoFromVector(body_vec, &body_vmo)) {
    return std::nullopt;
  }

  Request request;
  request.set_method(std::string(method))
      .set_url(std::string(url))
      .set_deadline(zx::deadline_after(timeout).get())
      .set_headers(std::move(http_headers))
      .set_body(Body::WithBuffer(std::move(body_vmo).ToTransport()));

  return request;
}

}  // namespace

CrashServer::CrashServer(async_dispatcher_t* dispatcher,
                         std::shared_ptr<sys::ServiceDirectory> services, const std::string& url,
                         LogTags* tags, feedback::AnnotationManager* annotation_manager,
                         timekeeper::Clock* clock)
    : dispatcher_(dispatcher),
      services_(services),
      url_(url),
      tags_(tags),
      annotation_manager_(annotation_manager),
      clock_(clock) {
  services_->Connect(loader_.NewRequest(dispatcher_));
  loader_.set_error_handler([](const zx_status_t status) {
    FX_PLOGS(WARNING, status) << "Lost connection to fuchsia.net.http.Loader";
  });
}

void CrashServer::MakeRequest(const Report& report, const Snapshot& snapshot,
                              cobalt::Logger& cobalt,
                              ::fit::function<void(UploadStatus, std::string)> callback) {
  // Make sure a call to fuchsia.net.http.Loader/Fetch isn't outstanding.
  FX_CHECK(!pending_request_);

  std::vector<SizedDataReader> attachment_readers;
  attachment_readers.reserve(report.Attachments().size() + 2u /*minidump and snapshot*/);

  std::map<std::string, crashpad::FileReaderInterface*> file_readers;

  for (const auto& [k, v] : report.Attachments()) {
    if (k.empty()) {
      continue;
    }
    attachment_readers.emplace_back(v);
    file_readers.emplace(k, &attachment_readers.back());
  }

  if (report.Minidump().has_value()) {
    attachment_readers.emplace_back(report.Minidump().value());
    file_readers.emplace("uploadFileMinidump", &attachment_readers.back());
  }

  // Append the product and version parameters to the URL.
  const std::map<std::string, std::string> annotations =
      PrepareAnnotations(report, snapshot, annotation_manager_, clock_->BootNow());
  FX_CHECK(annotations.contains("product"));
  FX_CHECK(annotations.contains("version"));
  std::string url = fxl::Substitute("$0?product=$1&version=$2", url_,
                                    crashpad::URLEncode(annotations.at("product")),
                                    crashpad::URLEncode(annotations.at("version")));
  if (annotations.contains("guid")) {
    url += fxl::Substitute("&guid=$0", crashpad::URLEncode(annotations.at("guid")));
  }

  // We have to build the MIME multipart message ourselves as all the public Crashpad helpers are
  // asynchronous and we won't be able to know the upload status nor the server report ID.
  crashpad::HTTPMultipartBuilder http_multipart_builder;
  http_multipart_builder.SetGzipEnabled(true);

  for (const auto& [key, value] : annotations) {
    http_multipart_builder.SetFormData(key, value);
  }

  // Add the snapshot archive (only relevant for ManagedSnapshots).
  if (std::holds_alternative<ManagedSnapshot>(snapshot)) {
    const auto& s = std::get<ManagedSnapshot>(snapshot);
    if (const std::shared_ptr<const ManagedSnapshot::Archive> archive = s.LockArchive(); archive) {
      attachment_readers.emplace_back(archive->value);
      file_readers.emplace(archive->key, &attachment_readers.back());
    }
  }

  for (const auto& [filename, content] : file_readers) {
    http_multipart_builder.SetFileAttachment(filename, filename, content,
                                             "application/octet-stream");
  }
  crashpad::HTTPHeaders headers;
  http_multipart_builder.PopulateContentHeaders(&headers);

  std::optional<::fuchsia::net::http::Request> request =
      BuildRequest("POST", url, zx::min(3), headers, *http_multipart_builder.GetBodyStream());

  auto complete = [&cobalt, callback = std::move(callback)](
                      CrashServer::UploadStatus status, uint64_t upload_size,
                      std::string server_report_id, zx::duration duration) mutable {
    cobalt.LogEvent(cobalt::Event(cobalt::EventType::kInteger,
                                  cobalt_registry::kReportUploadDurationMetricId,
                                  {static_cast<uint32_t>(ToCobaltReportUploadStatus(status)),
                                   static_cast<uint32_t>(ToCobaltReportUploadSize(upload_size))},
                                  duration.to_secs()));

    callback(status, std::move(server_report_id));
  };

  if (!request.has_value()) {
    complete(CrashServer::UploadStatus::kFailure, /*upload_size=*/0,
             /*server_report_id=*/"", zx::sec(0));
    return;
  }

  uint64_t upload_size = 0;
  if (request->has_body() && request->body().is_buffer()) {
    upload_size = request->body().buffer().size;
  }

  if (!loader_) {
    services_->Connect(loader_.NewRequest(dispatcher_));
  }

  const zx::time_boot start_time = clock_->BootNow();
  const std::string tags = tags_->Get(report.Id());

  loader_->Fetch(std::move(request.value()), [this, tags, upload_size, start_time,
                                              complete = std::move(complete)](
                                                 fuchsia::net::http::Response response) mutable {
    pending_request_ = false;

    auto complete_error = [&](CrashServer::UploadStatus status) {
      complete(status, upload_size, /*server_report_id=*/"", clock_->BootNow() - start_time);
    };

    auto complete_ok = [&](std::string server_report_id) {
      complete(CrashServer::UploadStatus::kSuccess, upload_size, std::move(server_report_id),
               clock_->BootNow() - start_time);
    };

    if (response.has_error()) {
      FX_LOGST(WARNING, tags.c_str()) << "Experienced network error: " << response.error();
      if (response.error() == fuchsia::net::http::Error::DEADLINE_EXCEEDED) {
        complete_error(CrashServer::UploadStatus::kTimedOut);
      } else {
        complete_error(CrashServer::UploadStatus::kFailure);
      }
      return;
    }

    std::string response_body;
    if (response.has_body()) {
      if (!fsl::BlockingDrainFrom(std::move(*response.mutable_body()),
                                  [&response_body](const void* data, uint32_t len) {
                                    const char* begin = static_cast<const char*>(data);
                                    response_body.insert(response_body.end(), begin, begin + len);
                                    return len;
                                  })) {
        FX_LOGST(WARNING, tags.c_str()) << "Failed to read http body";
        response_body.clear();
      }
    } else {
      FX_LOGST(WARNING, tags.c_str()) << "Http response is missing body";
    }

    if (!response.has_status_code()) {
      FX_LOGST(ERROR, tags.c_str()) << "No status code received: " << response_body;
      complete_error(CrashServer::UploadStatus::kFailure);
      return;
    }

    if (response.status_code() == 429) {
      FX_LOGST(WARNING, tags.c_str()) << "Upload throttled by server: " << response_body;
      complete_error(CrashServer::UploadStatus::kThrottled);
      return;
    }

    if (response.status_code() < 200 || response.status_code() >= 204) {
      FX_LOGST(WARNING, tags.c_str()) << "Failed to upload report, received HTTP status code "
                                      << response.status_code() << ": " << response_body;
      complete_error(CrashServer::UploadStatus::kFailure);
      return;
    }

    if (response_body.empty()) {
      complete_error(CrashServer::UploadStatus::kFailure);
    } else {
      complete_ok(std::move(response_body));
    }
  });

  pending_request_ = true;
}

std::map<std::string, std::string> CrashServer::PrepareAnnotations(
    const Report& report, const Snapshot& snapshot,
    const feedback::AnnotationManager* annotation_manager, const zx::time_boot uptime) {
  // Start with annotations from |report| and only add "presence" annotations.
  //
  // If |snapshot| is a MissingSnapshot, they contain potentially new information about why the
  // underlying data was dropped by the SnapshotManager.
  AnnotationMap annotations = report.Annotations();

  if (std::holds_alternative<MissingSnapshot>(snapshot)) {
    const auto& s = std::get<MissingSnapshot>(snapshot);
    annotations.Set(s.PresenceAnnotations());
  }

  // The crash server is responsible for adding the following annotations because adding the
  // annotations to the crash report earlier in the crash reporting flow could result in the values
  // being incorrect if the upload doesn't succeed until a later time.
  //
  // The report upload uptime should be a boot time because it's potentially used to determine the
  // UTC time if the UTC time wasn't available when the report was generated.
  std::optional<std::string> formatted_uptime = FormatDuration(zx::duration(uptime.get()));
  annotations.Set(feedback::kDebugReportUploadUptime, formatted_uptime.has_value()
                                                          ? ErrorOrString(*formatted_uptime)
                                                          : ErrorOrString(Error::kBadValue));

  if (const feedback::Annotations immediate_annotations =
          annotation_manager->ImmediatelyAvailable();
      immediate_annotations.contains(feedback::kSystemBootIdCurrentKey)) {
    annotations.Set(feedback::kDebugReportUploadBootId,
                    immediate_annotations.at(feedback::kSystemBootIdCurrentKey));
  }

  return annotations.Raw();
}

}  // namespace crash_reports
}  // namespace forensics

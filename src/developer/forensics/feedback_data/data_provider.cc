// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback_data/data_provider.h"

#include <fuchsia/feedback/cpp/fidl.h>
#include <lib/fpromise/promise.h>
#include <lib/fpromise/result.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/time.h>
#include <zircon/errors.h>
#include <zircon/status.h>
#include <zircon/types.h>
#include <zircon/utc.h>

#include <map>
#include <memory>
#include <utility>
#include <vector>

#include "src/developer/forensics/feedback/annotations/annotation_manager.h"
#include "src/developer/forensics/feedback/annotations/encode.h"
#include "src/developer/forensics/feedback/annotations/types.h"
#include "src/developer/forensics/feedback/attachments/types.h"
#include "src/developer/forensics/feedback_data/constants.h"
#include "src/developer/forensics/utils/archive.h"
#include "src/developer/forensics/utils/cobalt/metrics.h"
#include "src/developer/forensics/utils/utc_clock_ready_watcher.h"
#include "src/lib/fsl/vmo/sized_vmo.h"
#include "src/lib/uuid/uuid.h"

namespace forensics {
namespace feedback_data {
namespace {

using fuchsia::feedback::Snapshot;

// Timeout for a single asynchronous piece of data, e.g., syslog collection, if the client didn't
// specify one.
//
// 30s seems reasonable to collect everything.
const zx::duration kDefaultDataTimeout = zx::sec(30);

}  // namespace

DataProvider::DataProvider(async_dispatcher_t* dispatcher,
                           std::shared_ptr<sys::ServiceDirectory> services,
                           timekeeper::Clock* clock, RedactorBase* redactor,
                           const bool is_first_instance,
                           const std::set<std::string>& default_snapshot_annotations,
                           const feedback::AttachmentKeys& attachment_allowlist,
                           cobalt::Logger* cobalt, feedback::AnnotationManager* annotation_manager,
                           feedback::AttachmentManager* attachment_manager,
                           InspectDataBudget* inspect_data_budget)
    : dispatcher_(dispatcher),
      services_(std::move(services)),
      utc_clock_ready_watcher_(dispatcher_, zx::unowned_clock(zx_utc_reference_get())),
      metadata_(dispatcher_, clock, &utc_clock_ready_watcher_, redactor, is_first_instance,
                default_snapshot_annotations, attachment_allowlist),
      cobalt_(cobalt),
      annotation_manager_(annotation_manager),
      annotation_metrics_(cobalt_),
      attachment_manager_(attachment_manager),
      attachment_metrics_(cobalt_),
      executor_(dispatcher_),
      inspect_data_budget_(inspect_data_budget) {}

::fpromise::promise<feedback::Annotations> DataProvider::GetAnnotations(
    const zx::duration timeout) {
  return annotation_manager_->GetAll(timeout).and_then([this](feedback::Annotations& annotations) {
    annotation_metrics_.LogMetrics(annotations);
    return ::fpromise::ok(std::move(annotations));
  });
}

::fpromise::promise<feedback::Attachments> DataProvider::GetAttachments(
    const zx::duration timeout) {
  return attachment_manager_->GetAttachments(timeout).and_then(
      [this](feedback::Attachments& attachments) {
        attachment_metrics_.LogMetrics(attachments);
        return ::fpromise::ok(std::move(attachments));
      });
}

void DataProvider::GetAnnotations(fuchsia::feedback::GetAnnotationsParameters params,
                                  GetAnnotationsCallback callback) {
  const zx::duration timeout = params.has_collection_timeout_per_annotation()
                                   ? zx::duration(params.collection_timeout_per_annotation())
                                   : kDefaultDataTimeout;

  // TODO(https://fxbug.dev/42153749): Track how long GetAnnotations took via Cobalt.
  executor_.schedule_task(GetAnnotations(timeout).and_then(
      [callback = std::move(callback)](feedback::Annotations& annotations) {
        callback(feedback::Encode<fuchsia::feedback::Annotations>(annotations));
      }));
}

void DataProvider::GetSnapshot(fuchsia::feedback::GetSnapshotParameters params,
                               GetSnapshotCallback callback) {
  const zx::duration timeout = (params.has_collection_timeout_per_data())
                                   ? zx::duration(params.collection_timeout_per_data())
                                   : kDefaultDataTimeout;

  fidl::ServerEnd<fuchsia_io::File> file_server;
  if (params.has_response_channel()) {
    file_server = fidl::ServerEnd<fuchsia_io::File>(std::move(*params.mutable_response_channel()));
  }

  FX_LOGS(INFO) << "Generating snapshot";
  GetSnapshotInternal(
      timeout, uuid::Generate(),
      [this, callback = std::move(callback), file_server = std::move(file_server)](
          const feedback::Annotations& annotations, fsl::SizedVmo archive) mutable {
        FX_LOGS(INFO) << "Generated snapshot";
        Snapshot snapshot;

        // Add the annotations to the FIDL object and as file in the snapshot itself.
        if (auto fidl = feedback::Encode<fuchsia::feedback::Annotations>(annotations);
            fidl.has_annotations2()) {
          snapshot.set_annotations2(fidl.annotations2());
        }

        if (archive.vmo().is_valid()) {
          if (file_server.is_valid()) {
            ServeArchive(std::move(archive), std::move(file_server));
          } else {
            snapshot.set_archive(
                {.key = kSnapshotFilename, .value = std::move(archive).ToTransport()});
          }
        }

        callback(std::move(snapshot));
      });
}

void DataProvider::GetSnapshotInternal(zx::duration timeout, const std::string& uuid,
                                       GetSnapshotInternalCallback callback) {
  GetSnapshotInternal(timeout, uuid,
                      [callback = std::move(callback)](const feedback::Annotations& annotations,
                                                       fsl::SizedVmo archive) mutable {
                        callback(annotations, {.key = kSnapshotFilename,
                                               .value = std::move(archive).ToTransport()});
                      });
}

void DataProvider::GetSnapshotInternal(
    zx::duration timeout, const std::string& uuid,
    fit::callback<void(feedback::Annotations, fsl::SizedVmo)> callback) {
  const uint64_t timer_id = cobalt_->StartTimer();

  auto join = ::fpromise::join_promises(GetAnnotations(timeout), GetAttachments(timeout));
  using result_t = decltype(join)::value_type;

  auto promise = join.and_then(
      [this, uuid, timer_id, callback = std::move(callback)](result_t& results) mutable {
        FX_CHECK(std::get<0>(results).is_ok()) << "Impossible annotation collection failure";
        FX_CHECK(std::get<1>(results).is_ok()) << "Impossible attachment collection failure";

        const feedback::Annotations& annotations = std::get<0>(results).value();
        const feedback::Attachments& attachments = std::get<1>(results).value();
        std::map<std::string, std::string> snapshot_files;

        // Add the annotations to |snapshot_files|
        std::string file = feedback::Encode<std::string>(annotations);
        snapshot_files[kAttachmentAnnotations] = std::move(file);

        // Add the attachments to |snapshot_files|
        for (const auto& [key, value] : attachments) {
          if (value.HasValue()) {
            snapshot_files[key] = value.Value();
          }
        }

        snapshot_files[kAttachmentMetadata] = metadata_.MakeMetadata(
            annotations, attachments, uuid, annotation_manager_->IsMissingNonPlatformAnnotations());

        fsl::SizedVmo archive;

        // We bundle the attachments into a single archive.
        if (std::map<std::string, ArchiveFileStats> file_size_stats;
            Archive(snapshot_files, &archive, &file_size_stats)) {
          inspect_data_budget_->UpdateBudget(file_size_stats);
          cobalt_->LogCount(SnapshotVersion::kCobalt, archive.size());
          cobalt_->LogElapsedTime(cobalt::SnapshotGenerationFlow::kSuccess, timer_id);
        } else {
          cobalt_->LogElapsedTime(cobalt::SnapshotGenerationFlow::kFailure, timer_id);
          archive.vmo().reset();
        }
        callback(annotations, std::move(archive));
        return ::fpromise::ok();
      });

  executor_.schedule_task(std::move(promise));
}

bool DataProvider::ServeArchive(fsl::SizedVmo archive,
                                fidl::ServerEnd<fuchsia_io::File> file_server) {
  const size_t archive_index = next_served_archive_index_++;
  auto served_archive = std::make_unique<ServedArchive>(std::move(archive));

  if (!served_archive->Serve(std::move(file_server), dispatcher_, [this, archive_index]() mutable {
        served_archives_.erase(archive_index);
      })) {
    return false;
  }

  served_archives_.emplace(archive_index, std::move(served_archive));
  return true;
}

ServedArchive::ServedArchive(fsl::SizedVmo archive)
    : file_(std::move(archive.vmo()), archive.size()) {}

bool ServedArchive::Serve(fidl::ServerEnd<fuchsia_io::File> file_server,
                          async_dispatcher_t* dispatcher, std::function<void()> completed) {
  channel_closed_observer_ =
      std::make_unique<async::WaitOnce>(file_server.channel().get(), ZX_CHANNEL_PEER_CLOSED);
  if (const zx_status_t status =
          file_.Serve(fuchsia_io::wire::kPermReadable, std::move(file_server));
      status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Cannot serve archive";
    return false;
  }

  if (const zx_status_t status = channel_closed_observer_->Begin(
          dispatcher, [completed = std::move(completed)](...) { completed(); });
      status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Cannot attach observer to server end";
    return false;
  }

  return true;
}

}  // namespace feedback_data
}  // namespace forensics

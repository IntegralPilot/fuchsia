// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/crash_reports/queue.h"

#include <lib/async/cpp/task.h>
#include <lib/fit/defer.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/time.h>
#include <zircon/errors.h>

#include <string>
#include <utility>

#include "src/developer/forensics/crash_reports/annotation_map.h"
#include "src/developer/forensics/crash_reports/filing_result.h"
#include "src/developer/forensics/crash_reports/info/queue_info.h"
#include "src/developer/forensics/crash_reports/item_location.h"
#include "src/developer/forensics/crash_reports/reporting_policy_watcher.h"
#include "src/developer/forensics/crash_reports/snapshot.h"
#include "src/developer/forensics/feedback/annotations/constants.h"
#include "src/lib/fxl/strings/join_strings.h"
#include "src/lib/fxl/strings/string_printf.h"

namespace forensics {
namespace crash_reports {

Queue::Queue(async_dispatcher_t* dispatcher, std::shared_ptr<sys::ServiceDirectory> services,
             std::shared_ptr<InfoContext> info_context, LogTags* tags, ReportStore* report_store,
             CrashServer* crash_server)
    : dispatcher_(dispatcher),
      services_(services),
      info_context_(info_context),
      tags_(tags),
      report_store_(report_store),
      crash_server_(crash_server),
      metrics_(std::move(info_context)) {
  FX_CHECK(dispatcher_);
  FX_CHECK(crash_server_);

  InitFromStore();
}

void Queue::InitFromStore() {
  // Note: The upload attempt data is lost when the component stops and all reports start with
  // upload attempts of 0.
  for (const ReportId& report_id : report_store_->GetReports()) {
    const std::string uuid = report_store_->GetSnapshotUuid(report_id);

    // It could technically be an hourly snapshot, but either a) the snapshot has not been persisted
    // so it is okay to have another one here, or b) the snapshot was persisted, indicating that
    // we're likely on a device with enough storage space such that taking another hourly snapshot
    // wouldn't be detrimental.
    blocked_reports_.emplace_back(report_id, uuid, false /*not a known hourly report*/);
    AddReportUsingSnapshot(uuid, report_id);
  }

  std::sort(blocked_reports_.begin(), blocked_reports_.end(),
            [](const PendingReport& lhs, const PendingReport& rhs) {
              return lhs.report_id <= rhs.report_id;
            });

  if (!blocked_reports_.empty()) {
    FX_LOGS(INFO) << "Initializing queue with reports: " << ReportIdsStr(blocked_reports_);
  }

  // Clean up any stranded snapshots. While it shouldn't happen, a stranded snapshot here would be:
  // * A snapshot in /cache with all associated reports in memory or /tmp that didn't survive a
  //   device reboot or Feedback restart
  // * A snapshot in /tmp with all associated reports in memory that didn't survive a Feedback
  //   restart
  for (const std::string& uuid : report_store_->GetSnapshotStore()->GetSnapshotUuids()) {
    if (DeleteSnapshotIfNoClients(uuid)) {
      FX_LOGS(ERROR) << "Found stranded snapshot with uuid '" << uuid << "'";
    }
  }
}

uint64_t Queue::Size() const {
  return ready_reports_.size() + blocked_reports_.size() + (bool)active_report_;
}

bool Queue::IsEmpty() const { return Size() == 0; }

ReportId Queue::LatestReport() const {
  const ReportId active_report_id = (active_report_) ? active_report_->report_id : 0u;
  return std::max({
      active_report_id,
      ready_reports_.back().report_id,
      blocked_reports_.back().report_id,
  });
}

bool Queue::Contains(const ReportId report_id) const {
  return (std::find_if(ready_reports_.cbegin(), ready_reports_.cend(),
                       [=](const PendingReport& r) { return r.report_id == report_id; }) !=
          ready_reports_.cend()) ||
         (std::find_if(blocked_reports_.cbegin(), blocked_reports_.cend(),
                       [=](const PendingReport& r) { return r.report_id == report_id; }) !=
          blocked_reports_.cend()) ||
         (active_report_ && active_report_->report_id == report_id);
}

void Queue::PruneHourlyReports() {
  std::vector<ReportId> hourly_reports;
  auto add_if_hourly = [&hourly_reports](const PendingReport& report) {
    if (report.is_hourly_report) {
      hourly_reports.push_back(report.report_id);
    }
  };

  if (active_report_) {
    add_if_hourly(*active_report_);
  }
  for (const PendingReport& report : ready_reports_) {
    add_if_hourly(report);
  }
  for (const PendingReport& report : blocked_reports_) {
    add_if_hourly(report);
  }

  if (hourly_reports.size() <= 2) {
    return;
  }

  std::sort(hourly_reports.begin(), hourly_reports.end());
  const std::vector<ReportId> to_delete(hourly_reports.begin() + 1, hourly_reports.end() - 1);

  auto erase_from = [this](std::deque<PendingReport>& reports, const ReportId id) {
    auto it = std::find_if(reports.begin(), reports.end(),
                           [id](const PendingReport& report) { return report.report_id == id; });
    if (it != reports.end()) {
      Retire(std::move(*it), RetireReason::kDelete);
      reports.erase(it);
    }
  };

  for (const ReportId id : to_delete) {
    if (active_report_ && active_report_->report_id == id) {
      active_report_->delete_post_upload = PendingReport::DeletionReason::kPruned;
    }

    erase_from(ready_reports_, id);
    erase_from(blocked_reports_, id);
  }
}

bool Queue::IsPeriodicUploadScheduled() const {
  return unblock_all_every_fifteen_minutes_task_.is_pending();
}

void Queue::StopUploading() {
  stop_uploading_ = true;

  // Re-add all active reports so they're put in the store (if need be) and not uploaded
  // immediately.
  for (PendingReport& report : ready_reports_) {
    const bool add_to_store = report.HasReport();
    Add(std::move(report), /*consider_eager_upload=*/false, add_to_store);
  }
  ready_reports_.clear();

  for (PendingReport& report : blocked_reports_) {
    Retire(std::move(report), RetireReason::kArchive);
  }
  blocked_reports_.clear();

  unblock_all_every_fifteen_minutes_task_.Cancel();
}

bool Queue::Add(Report report, FilingResultFn callback) {
  // Remove clients with special case snapshots. These clients will be present in
  // |snapshot_clients_|, but will be listed under their intended snapshot uuid rather than under
  // the special case snapshot uuid.
  if (!snapshot_clients_.contains(report.SnapshotUuid())) {
    FX_CHECK(IsSpecialCaseSnapshot(report.SnapshotUuid()));
    for (auto& [uuid, reports] : snapshot_clients_) {
      reports.erase(report.Id());
      DeleteSnapshotIfNoClients(uuid);
    }
  }

  const bool result = Add(PendingReport(std::move(report), std::move(callback)),
                          /*consider_eager_upload=*/true,
                          /*add_to_store=*/true);

  // Hourly reports are pruned after `Add` to ensure that only 2 hourly reports are kept in the
  // queue.
  PruneHourlyReports();
  return result;
}

bool Queue::Add(PendingReport pending_report, const bool consider_eager_upload,
                const bool add_to_store) {
  if (reporting_policy_ == ReportingPolicy::kDoNotFileAndDelete) {
    Retire(std::move(pending_report), RetireReason::kDelete,
           FilingResult::kReportNotFiledUserOptedOut);
    return true;
  }

  if (consider_eager_upload && reporting_policy_ == ReportingPolicy::kUpload && !stop_uploading_) {
    ready_reports_.push_back(std::move(pending_report));
    Upload(/*set_network_reachable_on_success=*/true);
    return true;
  }

  if (add_to_store && pending_report.HasReport()) {
    const std::optional<ItemLocation> report_location = AddToStore(pending_report.TakeReport());
    if (!report_location.has_value()) {
      Retire(std::move(pending_report), RetireReason::kDelete, FilingResult::kPersistenceError);
      return false;
    }

    switch (*report_location) {
      case ItemLocation::kCache:
        pending_report.SendFilingResult(FilingResult::kReportOnDisk);
        break;
      case ItemLocation::kTmp:
      case ItemLocation::kMemory:
        pending_report.SendFilingResult(FilingResult::kReportInMemory);
        break;
    }
  }

  if (reporting_policy_ == ReportingPolicy::kArchive) {
    Retire(std::move(pending_report), RetireReason::kArchive);
    return true;
  }

  if (!stop_uploading_) {
    blocked_reports_.push_back(std::move(pending_report));
  }
  return true;
}

std::optional<ItemLocation> Queue::AddToStore(Report report) {
  std::vector<ReportId> garbage_collected_reports;
  const std::optional<ItemLocation> location =
      report_store_->Add(std::move(report), &garbage_collected_reports);

  // Retire each pending report that is garbage collected by the store.
  for (PendingReport& pending_report : ready_reports_) {
    if (std::find(garbage_collected_reports.cbegin(), garbage_collected_reports.cend(),
                  pending_report.report_id) != garbage_collected_reports.end()) {
      Retire(std::move(pending_report), RetireReason::kGarbageCollected);
    }
  }
  for (PendingReport& pending_report : blocked_reports_) {
    if (std::find(garbage_collected_reports.cbegin(), garbage_collected_reports.cend(),
                  pending_report.report_id) != garbage_collected_reports.end()) {
      Retire(std::move(pending_report), RetireReason::kGarbageCollected);
    }
  }

  // Erase all pending reports that were garbage collected.
  ready_reports_.erase(std::remove_if(ready_reports_.begin(), ready_reports_.end(),
                                      [&](const PendingReport& pending_report) {
                                        return std::find(garbage_collected_reports.cbegin(),
                                                         garbage_collected_reports.cend(),
                                                         pending_report.report_id) !=
                                               garbage_collected_reports.end();
                                      }),
                       ready_reports_.end());
  blocked_reports_.erase(std::remove_if(blocked_reports_.begin(), blocked_reports_.end(),
                                        [&](const PendingReport& pending_report) {
                                          return std::find(garbage_collected_reports.cbegin(),
                                                           garbage_collected_reports.cend(),
                                                           pending_report.report_id) !=
                                                 garbage_collected_reports.end();
                                        }),
                         blocked_reports_.end());

  return location;
}

void Queue::Upload(const bool set_network_reachable_on_success) {
  // Don't upload if the queue isn't allow to upload.
  if (stop_uploading_ || reporting_policy_ != ReportingPolicy::kUpload) {
    return;
  }

  // Don't upload if there aren't any reports to uploade or a report is already being uploaded.
  if (ready_reports_.empty() || active_report_ || crash_server_->HasPendingRequest()) {
    return;
  }

  active_report_ = std::move(ready_reports_.front());
  ready_reports_.pop_front();

  bool add_to_store = active_report_->HasReport();
  if (!active_report_->HasReport()) {
    std::optional<Report> report = report_store_->Get(active_report_->report_id);

    if (!report.has_value()) {
      Retire(std::move(*active_report_), RetireReason::kGarbageCollected);
      active_report_ = std::nullopt;
      Upload(/*set_network_reachable_on_success=*/false);
      return;
    }

    active_report_->SetReport(std::move(*report));
  }

  // The upload will fail if the annotations are empty.
  if (active_report_->report->Annotations().Raw().empty()) {
    FX_LOGST(INFO, tags_->Get(active_report_->report_id))
        << "Dropping report with empty annotations";
    Retire(std::move(*active_report_), RetireReason::kGarbageCollected);
    active_report_ = std::nullopt;
    Upload(/*set_network_reachable_on_success=*/false);
    return;
  }

  metrics_.IncrementUploadAttempts(active_report_->report_id);

  // Don't overwrite annotations about why the snapshot is missing if the report already contains
  // that information.
  Snapshot snapshot = report_store_->GetSnapshotStore()->GetSnapshot(active_report_->snapshot_uuid);

  if (const AnnotationMap& annotations = active_report_->report->Annotations();
      std::holds_alternative<MissingSnapshot>(snapshot) &&
      annotations.Contains(feedback::kDebugSnapshotErrorKey) &&
      annotations.Contains(feedback::kDebugSnapshotPresentKey)) {
    MissingSnapshot& s = std::get<MissingSnapshot>(snapshot);

    s.PresenceAnnotations().erase(feedback::kDebugSnapshotErrorKey);
    s.PresenceAnnotations().erase(feedback::kDebugSnapshotPresentKey);
  }

  crash_server_->MakeRequest(
      *active_report_->report, snapshot, info_context_->Cobalt(),
      [this, add_to_store, set_network_reachable_on_success](CrashServer::UploadStatus status,
                                                             std::string server_report_id) mutable {
        switch (status) {
          case CrashServer::UploadStatus::kSuccess: {
            Retire(std::move(*active_report_), RetireReason::kUpload, FilingResult::kReportUploaded,
                   server_report_id);
            if (set_network_reachable_on_success) {
              SetNetworkIsReachable(true);
            }
            break;
          }
          case CrashServer::UploadStatus::kThrottled:
            Retire(std::move(*active_report_), RetireReason::kThrottled,
                   FilingResult::kServerError);
            break;
          case CrashServer::UploadStatus::kTimedOut:
            Retire(std::move(*active_report_), RetireReason::kTimedOut, FilingResult::kServerError);
            break;
          case CrashServer::UploadStatus::kFailure:
            if (active_report_->delete_post_upload ==
                PendingReport::DeletionReason::kUserOptedOut) {
              Retire(std::move(*active_report_), RetireReason::kDelete,
                     FilingResult::kReportNotFiledUserOptedOut);
            } else if (active_report_->delete_post_upload ==
                       PendingReport::DeletionReason::kPruned) {
              Retire(std::move(*active_report_), RetireReason::kDelete);
            } else {
              // If the report isn't deleted and should be added to the store post-upload, its
              // content should still be present, e.g., DeleteAll didn't delete it.
              if (add_to_store) {
                FX_CHECK(active_report_->HasReport());
              }
              Add(std::move(*active_report_), /*consider_eager_upload=*/false, add_to_store);
            }
            break;
        }
        active_report_ = std::nullopt;
        Upload(/*set_network_reachable_on_success=*/false);
      });

  // Clear the report from memory if it won't be added to the store. Don't yet clear
  // |active_report_| so we know to not yet attempt another upload and can properly retire
  // |active_report_| once we get a response from the server.
  if (!add_to_store) {
    active_report_->TakeReport();
  }
}

void Queue::Retire(PendingReport pending_report, const Queue::RetireReason reason,
                   const std::optional<FilingResult> filing_result,
                   const std::optional<std::string>& server_report_id) {
  const char* tags = tags_->Get(pending_report.report_id);
  switch (reason) {
    case RetireReason::kArchive:
      FX_LOGST(INFO, tags) << "Archived local report. Located under /tmp/reports or /cache/reports";
      metrics_.Retire(pending_report, reason);
      // Don't clean up resources if the report is being archived.
      return;
    case RetireReason::kUpload:
      FX_CHECK(server_report_id.has_value());
      FX_LOGST(INFO, tags) << "Successfully uploaded report at https://crash.corp.google.com/"
                           << *server_report_id;
      break;
    case RetireReason::kThrottled:
      FX_LOGST(INFO, tags) << "Upload throttled by server";
      break;
    case RetireReason::kTimedOut:
      FX_LOGST(INFO, tags) << "Upload timed out, not re-trying";
      break;
    case RetireReason::kDelete:
      FX_LOGST(INFO, tags) << "Deleted local report";
      break;
    case RetireReason::kGarbageCollected:
      FX_LOGST(INFO, tags) << "Garbage collected local report";
      break;
  }

  // SendFilingResult is immediately called when reports are put into persistence.
  if (filing_result.has_value() && pending_report.callback != nullptr) {
    pending_report.SendFilingResult(*filing_result, server_report_id);
  }

  metrics_.Retire(pending_report, reason, server_report_id);
  tags_->Unregister(pending_report.report_id);
  report_store_->Remove(pending_report.report_id);

  // Remove clients that don't have special case snapshots.
  snapshot_clients_[pending_report.snapshot_uuid].erase(pending_report.report_id);
  if (snapshot_clients_[pending_report.snapshot_uuid].empty()) {
    snapshot_clients_.erase(pending_report.snapshot_uuid);
  }

  DeleteSnapshotIfNoClients(pending_report.snapshot_uuid);
  PreventStrandedSnapshot(pending_report.snapshot_uuid);
}

void Queue::AddReportUsingSnapshot(const std::string& uuid, ReportId report) {
  snapshot_clients_[uuid].insert(report);
}

bool Queue::DeleteSnapshotIfNoClients(const std::string& uuid) {
  if (NumReportsUsingSnapshot(uuid) == 0) {
    report_store_->GetSnapshotStore()->DeleteSnapshot(uuid);
    return true;
  }

  return false;
}

size_t Queue::NumReportsUsingSnapshot(const std::string& uuid) {
  return snapshot_clients_.contains(uuid) ? snapshot_clients_.at(uuid).size() : 0;
}

void Queue::PreventStrandedSnapshot(const std::string& uuid) {
  const std::optional<ItemLocation> snapshot_location =
      report_store_->GetSnapshotStore()->SnapshotLocation(uuid);

  if (!snapshot_location.has_value() || *snapshot_location != ItemLocation::kCache ||
      SuggestedSnapshotLocation(uuid) != ItemLocation::kTmp) {
    return;
  }

  // The snapshot is in /cache, but the suggested location is /tmp. This means we are at risk of a
  // stranded snapshot after a device reboot.
  report_store_->GetSnapshotStore()->MoveToTmp(uuid);
  if (report_store_->GetSnapshotStore()->SnapshotExists(uuid)) {
    return;
  }

  // Failed to move to /tmp - update reports still associated with this snapshot as to why the
  // snapshot won't be attached.
  for (const ReportId report_id : snapshot_clients_.at(uuid)) {
    FX_CHECK(report_store_->Contains(report_id))
        << "|snapshot_clients_| not in sync with |report_store_|";

    report_store_->AddAnnotation(report_id, feedback::kDebugSnapshotErrorKey, "failed move to tmp");
    report_store_->AddAnnotation(report_id, feedback::kDebugSnapshotPresentKey, "false");
  }
}

ItemLocation Queue::SuggestedSnapshotLocation(const std::string& uuid) {
  const std::vector<ReportId> cache_reports = report_store_->GetCacheReports();

  // Check if any reports in /cache are associated with the snapshot for |uuid|.
  if (std::any_of(cache_reports.begin(), cache_reports.end(), [this, uuid](ReportId id) {
        return report_store_->GetSnapshotUuid(id) == uuid;
      })) {
    return ItemLocation::kCache;
  }

  // No reports in /cache are associated with |uuid|, so there's no reason to store the snapshot for
  // |uuid| in /cache.
  return ItemLocation::kTmp;
}

void Queue::BlockAll() {
  // Move all ready reports to blocked and add all reports to the store that haven't been
  // added yet.
  for (PendingReport& pending_report : ready_reports_) {
    const bool add_to_store = pending_report.HasReport();
    Add(std::move(pending_report), /*consider_eager_upload=*/false, add_to_store);
  }
  ready_reports_.clear();
}

void Queue::UnblockAll() {
  if (stop_uploading_ || reporting_policy_ != ReportingPolicy::kUpload) {
    return;
  }

  ready_reports_.insert(ready_reports_.end(), std::make_move_iterator(blocked_reports_.begin()),
                        std::make_move_iterator(blocked_reports_.end()));
  blocked_reports_.clear();
  Upload(/*set_network_reachable_on_success=*/false);
}

void Queue::DeleteAll() {
  FX_LOGS(INFO) << fxl::StringPrintf("Deleting all %zu pending reports and associated snapshots",
                                     Size());

  for (PendingReport& pending_report : ready_reports_) {
    Retire(std::move(pending_report), RetireReason::kDelete,
           FilingResult::kReportNotFiledUserOptedOut);
  }
  ready_reports_.clear();

  for (PendingReport& pending_report : blocked_reports_) {
    // |blocked_reports_| are in persistence; FilingResultCallback was already called.
    Retire(std::move(pending_report), RetireReason::kDelete);
  }
  blocked_reports_.clear();

  // Delete the report being uploaded, but don't retire it; the PendingReport is needed
  // post-upload and will be retired once it is used.
  if (active_report_) {
    if (active_report_->HasReport()) {
      // The report may have been already deleted from memory after initiating upload.
      active_report_->TakeReport();
    }
    active_report_->delete_post_upload = PendingReport::DeletionReason::kUserOptedOut;
  }

  report_store_->RemoveAll();
  report_store_->GetSnapshotStore()->DeleteAll();
}

// The queue is inheritly conservative with uploading crash reports meaning that a report that is
// forbidden from being uploaded will never be uploaded while crash reports that are permitted to
// be uploaded may later be considered to be forbidden. This is due to the fact that when uploads
// are disabled all reports are immediately archived after having been added to the queue, thus we
// never have to worry that a report that shouldn't be uploaded ends up being uploaded when the
// reporting policy changes.
void Queue::WatchReportingPolicy(ReportingPolicyWatcher* watcher) {
  auto OnReportingPolicyChange = [this](const ReportingPolicy policy) {
    reporting_policy_ = policy;
    switch (reporting_policy_) {
      case ReportingPolicy::kDoNotFileAndDelete:
        unblock_all_every_fifteen_minutes_task_.Cancel();
        DeleteAll();
        break;
      case ReportingPolicy::kUpload:
        UnblockAllEveryFifteenMinutes();
        break;
      case ReportingPolicy::kArchive:
        // The reporting policy shouldn't change to Archive outside of tests. Moreover, once the
        // reporting policy is Archive it should never change to another value mid-boot.
        StopUploading();
        break;
      case ReportingPolicy::kUndecided:
        BlockAll();
        unblock_all_every_fifteen_minutes_task_.Cancel();
        break;
    }
  };

  OnReportingPolicyChange(watcher->CurrentPolicy());
  watcher->OnPolicyChange([=](const ReportingPolicy policy) { OnReportingPolicyChange(policy); });
}

void Queue::SetNetworkIsReachable(const bool is_reachable) {
  if (!stop_uploading_ && is_reachable && reporting_policy_ == ReportingPolicy::kUpload) {
    if (!blocked_reports_.empty()) {
      FX_LOGS(INFO) << "Uploading " << blocked_reports_.size()
                    << " reports on network reachable: " << ReportIdsStr(blocked_reports_);
      UnblockAll();
    }
  }
}

void Queue::UnblockAllEveryFifteenMinutes() {
  if (stop_uploading_) {
    return;
  }

  if (!blocked_reports_.empty()) {
    FX_LOGS(INFO) << "Uploading " << blocked_reports_.size()
                  << " reports on as a part of the 15-minute periodic upload: "
                  << ReportIdsStr(blocked_reports_);
    UnblockAll();
  }

  if (const zx_status_t status =
          unblock_all_every_fifteen_minutes_task_.PostDelayed(dispatcher_, zx::min(15));
      status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Error posting periodic upload task to async loop. Won't retry.";
  }
}

void Queue::PendingReport::SendFilingResult(FilingResult result,
                                            const std::optional<std::string>& report_id) {
  report_id.has_value() ? callback(result, *report_id) : callback(result, "");
}

Queue::PendingReport::PendingReport(Report report, FilingResultFn callback)
    : report_id(report.Id()),
      snapshot_uuid(report.SnapshotUuid()),
      is_hourly_report(report.IsHourlyReport()),
      report(std::move(report)),
      callback(std::move(callback)),
      delete_post_upload(DeletionReason::kNone) {}

Queue::PendingReport::PendingReport(const ReportId report_id, const std::string snapshot_uuid,
                                    const bool is_hourly_report)
    : report_id(report_id),
      snapshot_uuid(std::move(snapshot_uuid)),
      is_hourly_report(is_hourly_report),
      report(std::nullopt),
      callback([](const FilingResult& result, const std::string& report_id) {}),
      delete_post_upload(DeletionReason::kNone) {}

void Queue::PendingReport::SetReport(Report r) { report = std::move(r); }

Report Queue::PendingReport::TakeReport() {
  FX_CHECK(report);
  Report r = std::move(*report);
  report = std::nullopt;
  return r;
}

bool Queue::PendingReport::HasReport() const { return (bool)report; }

Queue::UploadMetrics::UploadMetrics(std::shared_ptr<InfoContext> info_context)
    : info_(std::move(info_context)) {}

void Queue::UploadMetrics::IncrementUploadAttempts(const ReportId report_id) {
  upload_attempts_[report_id]++;
  info_.RecordUploadAttemptNumber(upload_attempts_[report_id]);
}

void Queue::UploadMetrics::Retire(const PendingReport& pending_report,
                                  const RetireReason retire_reason,
                                  const std::optional<std::string>& server_report_id) {
  switch (retire_reason) {
    case RetireReason::kUpload:
      FX_CHECK(server_report_id.has_value());
      info_.MarkReportAsUploaded(*server_report_id, upload_attempts_[pending_report.report_id]);
      break;
    case RetireReason::kDelete:
      info_.MarkReportAsDeleted(upload_attempts_[pending_report.report_id]);
      break;
    case RetireReason::kThrottled:
      info_.MarkReportAsThrottledByServer(upload_attempts_[pending_report.report_id]);
      break;
    case RetireReason::kTimedOut:
      info_.MarkReportAsTimedOut(upload_attempts_[pending_report.report_id]);
      break;
    case RetireReason::kArchive:
      info_.MarkReportAsArchived();
      break;
    case RetireReason::kGarbageCollected:
      info_.MarkReportAsGarbageCollected(upload_attempts_[pending_report.report_id]);
      break;
  }
  upload_attempts_.erase(pending_report.report_id);
}

std::string Queue::ReportIdsStr(const std::deque<PendingReport>& reports) const {
  std::vector<std::string> report_id_strs;
  for (const PendingReport& pending_report : reports) {
    report_id_strs.push_back(std::to_string(pending_report.report_id));
  }

  return "[" + fxl::JoinStrings(report_id_strs, ", ") + "]";
}

}  // namespace crash_reports
}  // namespace forensics

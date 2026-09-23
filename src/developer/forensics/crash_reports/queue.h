// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_CRASH_REPORTS_QUEUE_H_
#define SRC_DEVELOPER_FORENSICS_CRASH_REPORTS_QUEUE_H_

#include <lib/async/cpp/task.h>
#include <lib/async/dispatcher.h>

#include <map>
#include <string>
#include <unordered_map>
#include <vector>

#include "src/developer/forensics/crash_reports/crash_server.h"
#include "src/developer/forensics/crash_reports/filing_result.h"
#include "src/developer/forensics/crash_reports/info/info_context.h"
#include "src/developer/forensics/crash_reports/info/queue_info.h"
#include "src/developer/forensics/crash_reports/log_tags.h"
#include "src/developer/forensics/crash_reports/report.h"
#include "src/developer/forensics/crash_reports/report_id.h"
#include "src/developer/forensics/crash_reports/report_store.h"
#include "src/developer/forensics/crash_reports/reporting_policy_watcher.h"
#include "src/lib/fxl/macros.h"

namespace forensics {
namespace crash_reports {

// Queues pending reports and processes them according to the reporting policy.
class Queue {
 public:
  Queue(async_dispatcher_t* dispatcher, std::shared_ptr<sys::ServiceDirectory> services,
        std::shared_ptr<InfoContext> info_context, LogTags* tags, ReportStore* report_store,
        CrashServer* crash_server);

  // Watcher functions that allow the queue to react to external events, such as
  //  1) the reporting policy changing or
  //  2) the network status changing.
  void WatchReportingPolicy(ReportingPolicyWatcher* watcher);
  void SetNetworkIsReachable(bool is_reachable);

  bool Add(Report report, FilingResultFn callback);

  // Identifies |report| as a crash report that used the snapshot referred to by |uuid|.
  //
  // Note: this is needed because the Queue manages the lifetime of snapshots. Reports are added
  // asynchronously and it may be possible for the Queue to think all reports using a snapshot are
  // retired depending on how Add and Upload are ordered.
  void AddReportUsingSnapshot(const std::string& uuid, ReportId report);

  uint64_t Size() const;
  bool IsEmpty() const;
  ReportId LatestReport() const;
  bool Contains(ReportId report_id) const;
  bool IsPeriodicUploadScheduled() const;

  // Forces the queue to automatically put all reports in the store and stop all uploads.
  void StopUploading();

 private:
  // Internal representation of a report including metadata about the report and an optional
  // in-memory version of the report.
  //
  // Note: |report| will be set iff it is actively being uploaded or hasn't been added to the store.
  struct PendingReport {
    explicit PendingReport(Report report, FilingResultFn callback);
    PendingReport(ReportId report_id, std::string snapshot_uuid, bool is_hourly_report);

    PendingReport(const PendingReport&) = delete;
    PendingReport& operator=(const PendingReport&) = delete;
    PendingReport(PendingReport&&) = default;
    PendingReport& operator=(PendingReport&&) = default;

    // Utility method for interacting with |report|.
    void SetReport(Report report);
    Report TakeReport();
    bool HasReport() const;

    // Executes |callback| using the parameters specified.
    void SendFilingResult(FilingResult filing_result,
                          const std::optional<std::string>& report_id = std::nullopt);

    ReportId report_id;
    std::string snapshot_uuid;
    bool is_hourly_report;
    std::optional<Report> report;
    FilingResultFn callback;

    enum class DeletionReason : std::uint8_t {
      kNone,
      kUserOptedOut,
      kPruned,
    };

    // Set to a value other than kNone iff the report is the active report and needs to be deleted
    // once it becomes blocked.
    DeletionReason delete_post_upload;
  };

  // Why a report is being retired.
  enum class RetireReason { kUpload, kDelete, kThrottled, kTimedOut, kArchive, kGarbageCollected };

  // Instantiates the queue from state in the store.
  void InitFromStore();

  // Internal Add() method with information on whether the report can be uploaded immediately and
  // put in the store. |consider_eager_upload| and |add_to_store| will be false if the report
  // already has an upload attempt or put in the store, respectively.
  bool Add(PendingReport pending_report, bool consider_eager_upload, bool add_to_store);

  std::optional<ItemLocation> AddToStore(Report report);

  // Stops using |pending_report| for the provided reason and cleans up its resources.
  void Retire(PendingReport pending_report, RetireReason reason,
              std::optional<FilingResult> filing_result = std::nullopt,
              const std::optional<std::string>& server_report_id = std::nullopt);

  // Attempts to upload all reports in |ready_reports_|. Reports are retired if they're uploaded or
  // throttled and re-added to the queue if the upload fails.
  //
  // If |set_network_reachable_on_success| is true, a successful upload will unblock all reports.
  // Non-eager uploads should NOT set |set_network_reachable_on_success| to true because it could
  // result in the same report failing to upload many times if the network reachability is
  // transient. For example, imagine the queue has 20 blocked reports. If 1 upload succeeds, but 19
  // fail, all 19 would retry. If, on the next try, just 1 succeeds again, then all 18 remaining
  // would retry, etc.
  void Upload(bool set_network_reachable_on_success);

  // Make all reports blocked.
  void BlockAll();

  // Makes all reports ready and call Upload.
  void UnblockAll();

  void DeleteAll();
  void UnblockAllEveryFifteenMinutes();

  // Deletes the snapshot referred to by |uuid| if there are no reports associated with the snapshot
  // in |snapshot_clients_|. Returns true if the snapshot was deleted.
  bool DeleteSnapshotIfNoClients(const std::string& uuid);

  // Removes all hourly reports from the queue except for the oldest and newest.
  void PruneHourlyReports();

  // Returns the number of crash reports that use the snapshot referred to by |uuid|.
  //
  // Note: it's technically possible for this value to be too large if a report hasn't been added,
  // but its association to a snapshot has been recorded with AddSnapshotClient.
  size_t NumReportsUsingSnapshot(const std::string& uuid);

  // Attempts to remove the risk of the snapshot for |uuid| becoming a stranded snapshot. A
  // stranded snapshot is a snapshot on disk that does not have any associated crash reports on the
  // device.
  void PreventStrandedSnapshot(const std::string& uuid);

  // Suggests where the snapshot for |uuid| should be stored based on the locations of crash reports
  // associated with |uuid|.
  ItemLocation SuggestedSnapshotLocation(const std::string& uuid);

  std::string ReportIdsStr(const std::deque<PendingReport>& reports) const;

  // Utility class for recording metrics about reports.
  class UploadMetrics {
   public:
    explicit UploadMetrics(std::shared_ptr<InfoContext> info_context);
    void IncrementUploadAttempts(ReportId report_id);

    // Record |report_id| as being retired and erase any state associated with it.
    void Retire(const PendingReport& pending_report, RetireReason retire_reason,
                const std::optional<std::string>& server_report_id = std::nullopt);

   private:
    QueueInfo info_;
    std::map<ReportId, size_t> upload_attempts_;
  };

  async_dispatcher_t* dispatcher_;
  const std::shared_ptr<sys::ServiceDirectory> services_;
  std::shared_ptr<InfoContext> info_context_;
  LogTags* tags_;
  ReportStore* report_store_;
  CrashServer* crash_server_;
  UploadMetrics metrics_;

  async::TaskClosureMethod<Queue, &Queue::UnblockAllEveryFifteenMinutes>
      unblock_all_every_fifteen_minutes_task_{this};

  ReportingPolicy reporting_policy_{ReportingPolicy::kUndecided};
  bool stop_uploading_{false};

  // A report is either:
  //  1) Active (actively being uploaded).
  //  2) Ready (can become the active report).
  //  3) Blocked (not ready or active and won't become so unless a stimulus triggers it, e.g., the
  //  network becoming reachable).
  std::optional<PendingReport> active_report_;
  std::deque<PendingReport> ready_reports_;
  std::deque<PendingReport> blocked_reports_;

  // Which snapshot is associated with what reports.
  std::map<std::string, std::set<ReportId>> snapshot_clients_;

  FXL_DISALLOW_COPY_AND_ASSIGN(Queue);
};

}  // namespace crash_reports
}  // namespace forensics

#endif  // SRC_DEVELOPER_FORENSICS_CRASH_REPORTS_QUEUE_H_

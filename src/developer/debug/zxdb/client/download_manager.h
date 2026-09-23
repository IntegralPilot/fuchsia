// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_DOWNLOAD_MANAGER_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_DOWNLOAD_MANAGER_H_

#include <map>
#include <memory>
#include <set>

#include "src/developer/debug/zxdb/client/symbol_server.h"
#include "src/developer/debug/zxdb/client/system_observer.h"
#include "src/developer/debug/zxdb/symbols/debug_symbol_file_type.h"
#include "src/lib/fxl/memory/weak_ptr.h"

namespace zxdb {

class Download;
class System;

// This class manages all downloads that are requested when they are not found locally.
class DownloadManager : public SystemObserver {
 public:
  explicit DownloadManager(System* system);
  ~DownloadManager();

  // Requests to download |build_id| from any remote source, the first returning a valid symbol file
  // with a matching build id will be loaded. |file_type| determines whether the file should be a
  // binary with symbols or a separate file with a ".debug" suffix with just debug info and no
  // binary information.
  void RequestDownload(const std::string& build_id, DebugSymbolFileType file_type);

  bool HasDownload(const std::string& build_id) const;

  // SystemObserver implementation.
  void DidCreateSymbolServer(SymbolServer* server) override;
  void OnSymbolServerStatusChanged(SymbolServer* server) override;

  // Get a test download object.
  Download* InjectDownloadForTesting(const std::string& build_id);

  // Abandons a download injected with |InjectDownloadForTesting|.
  void AbandonTestingDownload(const std::string& build_id);

  bool DownloadsInProgress() const { return !downloads_.empty(); }

 private:
  using DownloadIdentifier = std::pair<std::string, DebugSymbolFileType>;

  // Get a download object corresponding to |build_id| and |file_type|. If no matching object is
  // found, a new one is allocated and will request a download against all configured and ready
  // symbol servers.
  Download* GetDownload(std::string build_id, DebugSymbolFileType file_type);

  // Creates a Download object with the given |build_id| and |file_type|, note the caller is
  // responsible for adding the resulting pointer to |downloads_|.
  std::unique_ptr<Download> MakeDownload(const std::string& build_id,
                                         DebugSymbolFileType file_type);

  // A download succeeded,
  void OnDownloadSucceeded(const std::string& path, const std::string& build_id,
                           DebugSymbolFileType file_type);

  // Adds |server| to the download object associated with any modules that are missing
  // symbols in the current process.
  void OnSymbolServerBecomesReady(SymbolServer* server);

  // Called every time a new download starts.
  void DownloadStarted(const DownloadIdentifier& dl_id, Download* download);

  // Called every time a download ends.
  void DownloadFinished();

  size_t download_count_ = 0;

  size_t download_success_count_ = 0;

  size_t download_fail_count_ = 0;

  size_t servers_ready_ = 0;

  // These servers have failed to authenticate and will never become ready.
  size_t servers_failed_auth_ = 0;

  // These servers accountered repeated errors and will never becomre ready again.
  size_t servers_unreachable_ = 0;

  // Downloads currently in progress or pending server availability. This holds an owning pointer to
  // the Download object so it can be persisted while we wait for servers to become ready.
  std::map<DownloadIdentifier, std::unique_ptr<Download>> downloads_;

  System* system_;  // owns |this|.

  fxl::WeakPtrFactory<DownloadManager> weak_factory_;
};

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_DOWNLOAD_MANAGER_H_

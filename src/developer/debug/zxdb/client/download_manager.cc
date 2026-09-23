// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/client/download_manager.h"

#include <lib/syslog/cpp/macros.h>

#include <map>
#include <memory>

#include "src/developer/debug/shared/logging/logging.h"
#include "src/developer/debug/shared/message_loop.h"
#include "src/developer/debug/zxdb/client/download_observer.h"
#include "src/developer/debug/zxdb/client/process.h"
#include "src/developer/debug/zxdb/client/session.h"
#include "src/developer/debug/zxdb/client/symbol_server.h"
#include "src/developer/debug/zxdb/client/system.h"
#include "src/developer/debug/zxdb/common/err.h"
#include "src/developer/debug/zxdb/symbols/debug_symbol_file_type.h"
#include "src/developer/debug/zxdb/symbols/loaded_module_symbols.h"
#include "src/developer/debug/zxdb/symbols/process_symbols.h"
#include "src/lib/fxl/strings/string_printf.h"

namespace zxdb {

namespace {
std::string DebugSymbolFileTypeToString(DebugSymbolFileType file_type) {
  switch (file_type) {
    case DebugSymbolFileType::kDebugInfo:
      return "debuginfo";
    case DebugSymbolFileType::kBinary:
      return "binary";
  }
}
}  // namespace

// When we want to download symbols for a build ID, we create a Download object. This object is
// owned by the DownloadManager. We then fire off requests to all symbol servers we know about
// asking to download the symbols. These requests are completed in sequence to all of the servers
// the DownloadManager knows about, until one reports that the download succeeded. If all the
// servers are tried and none of them are informed the request was successful, DownloadManager will
// drop the object. The destructor of the Download object calls a callback that handles notifying
// the rest of the system of those results.
//
// If we receive more notifications that other servers are ready to receive download requests, they
// are queued and will be tried as a fallback until the download succeeds or we run out of servers.
class Download {
 public:
  Download(const std::string& build_id, DebugSymbolFileType file_type,
           SymbolServer::FetchCallback result_cb)
      : build_id_(build_id),
        file_type_(file_type),
        result_cb_(std::move(result_cb)),
        weak_factory_(this) {
    DEBUG_LOG(DownloadManager) << "Starting download for " << build_id_;
  }

  ~Download() { Finish(); }

  bool active() const { return !!result_cb_ && download_in_progress_; }

  // Add a symbol server to this download.
  void AddServer(SymbolServer* server);

 private:
  // Notify this download object that we have gotten the symbols if we're going to get them.
  void Finish();

  // Notify this Download object that a transaction failed.
  void Error(const Err& err);

  size_t failed_server_count_ = 0;
  std::string build_id_;
  DebugSymbolFileType file_type_;
  // Collection for errors encountered during download.
  std::map<ErrType, std::vector<std::string>> errors_;
  std::string path_;
  SymbolServer::FetchCallback result_cb_;
  std::vector<fit::callback<void()>> server_fetches_;
  bool download_in_progress_ = false;
  fxl::WeakPtrFactory<Download> weak_factory_;
};

void Download::Finish() {
  if (!result_cb_) {
    return;
  }

  Err final_err;
  if (errors_.size() == 1) {
    final_err = Err(errors_.begin()->first);
  } else if (!errors_.empty()) {
    final_err =
        Err(fxl::StringPrintf("Multiple errors encountered while downloading %s for build_id %s",
                              DebugSymbolFileTypeToString(file_type_).c_str(), build_id_.c_str()));
  }

  // Specialize the error message if the build id was not found on any servers.
  if (final_err.type() == ErrType::kNotFound) {
    final_err = Err("%s for build_id %s not found on %zu servers\n",
                    DebugSymbolFileTypeToString(file_type_).c_str(), build_id_.c_str(),
                    failed_server_count_);
  }

  if (final_err.has_error()) {
    LOGS(Info) << "Download for " << build_id_ << " failed(" << ErrTypeToString(final_err.type())
               << "): " << final_err.msg();
  } else {
    DEBUG_LOG(DownloadManager) << "Download for " << build_id_ << " succeeded.";
  }

  if (debug::MessageLoop::Current()) {
    debug::MessageLoop::Current()->PostTask(
        FROM_HERE, [result_cb = std::move(result_cb_), final_err,
                    path = std::move(path_)]() mutable { result_cb(final_err, path); });
  }

  result_cb_ = nullptr;
}

void Download::AddServer(SymbolServer* server) {
  if (!result_cb_) {
    return;
  }
  DEBUG_LOG(DownloadManager) << "Adding server " << server->name() << " to download for "
                             << build_id_;

  SymbolServer::FetchCallback cb = [weak_this = weak_factory_.GetWeakPtr(), server](
                                       const Err& err, const std::string& path) {
    if (!weak_this)
      return;

    weak_this->download_in_progress_ = false;

    if (path.empty()) {
      if (err.msg().empty()) {
        DEBUG_LOG(DownloadManager) << "Error downloading " << weak_this->build_id_ << " from "
                                   << server->name() << ": " << ErrTypeToString(err.type());
      } else {
        DEBUG_LOG(DownloadManager)
            << "Error downloading " << weak_this->build_id_ << " from " << server->name() << ": "
            << ErrTypeToString(err.type()) << ", " << err.msg();
      }
      weak_this->Error(err);
    } else {
      weak_this->errors_.clear();
      weak_this->path_ = path;
      weak_this->Finish();
    }
  };

  if (!download_in_progress_) {
    download_in_progress_ = true;
    server->Fetch(build_id_, file_type_, std::move(cb));
  } else {
    // It's safe to capture |this| here since |server_fetches_| is owned by
    // the Download.
    server_fetches_.push_back([server, this, cb = std::move(cb)]() mutable {
      server->Fetch(build_id_, file_type_, std::move(cb));
    });
  }
}

void Download::Error(const Err& err) {
  if (!result_cb_)
    return;

  failed_server_count_++;

  errors_[err.type()].push_back(err.msg());

  if (!download_in_progress_ && !server_fetches_.empty()) {
    auto fetch = std::move(server_fetches_.back());
    server_fetches_.pop_back();

    download_in_progress_ = true;
    fetch();
    return;
  }

  // Tried all available servers, report completion.
  Finish();
}

// ============================ DownloadManager ===============================

DownloadManager::DownloadManager(System* system) : system_(system), weak_factory_(this) {
  system_->AddObserver(this);
}

DownloadManager::~DownloadManager() { system_->RemoveObserver(this); }

void DownloadManager::RequestDownload(const std::string& build_id, DebugSymbolFileType file_type) {
  GetDownload(build_id, file_type);
}

void DownloadManager::DidCreateSymbolServer(SymbolServer* server) {
  OnSymbolServerStatusChanged(server);
}

void DownloadManager::OnSymbolServerStatusChanged(SymbolServer* server) {
  switch (server->state()) {
    case SymbolServer::State::kReady:
      OnSymbolServerBecomesReady(server);
      break;
    case SymbolServer::State::kAuth:
      servers_failed_auth_++;
      break;
    case SymbolServer::State::kUnreachable:
      servers_unreachable_++;
      break;
    default:
      break;
  }
}

void DownloadManager::OnSymbolServerBecomesReady(SymbolServer* server) {
  servers_ready_++;
  Download* download = nullptr;

  for (const auto& target : system_->GetTargets()) {
    auto process = target->GetProcess();
    if (!process)
      continue;

    for (const auto& mod : process->GetSymbols()->GetStatus()) {
      if (!mod.symbols || !mod.symbols->module_symbols()) {
        download = GetDownload(mod.build_id, DebugSymbolFileType::kDebugInfo);
      } else if (!mod.symbols->module_symbols()->HasBinary()) {
        download = GetDownload(mod.build_id, DebugSymbolFileType::kBinary);
      }

      if (download) {
        download->AddServer(server);
      }
    }
  }
}

Download* DownloadManager::InjectDownloadForTesting(const std::string& build_id) {
  return GetDownload(build_id, DebugSymbolFileType::kDebugInfo);
}

void DownloadManager::AbandonTestingDownload(const std::string& build_id) {
  if (const auto& it = downloads_.find({build_id, DebugSymbolFileType::kDebugInfo});
      it != downloads_.end()) {
    it->second.reset();
  }
}

std::unique_ptr<Download> DownloadManager::MakeDownload(const std::string& build_id,
                                                        DebugSymbolFileType file_type) {
  // |this| owns the Download object, so it's safe to capture in these callbacks.
  return std::make_unique<Download>(
      build_id, file_type, [build_id, file_type, this](const Err& err, const std::string& path) {
        if (err.ok() && !path.empty()) {
          OnDownloadSucceeded(path, build_id, file_type);
        } else if (servers_ready_ + servers_failed_auth_ < system_->GetSymbolServers().size()) {
          // Erase the download and return early if not all servers have reported ready or failed to
          // authenticate. We don't want to report an error yet. Once all the servers have reported,
          // the download will try again and report the error if none succeed.
          downloads_.erase({build_id, file_type});
          return;
        } else {
          download_fail_count_++;
          system_->NotifyFailedToFindDebugSymbols(err, build_id, file_type);
        }

        downloads_.erase({build_id, file_type});

        DownloadFinished();
      });
}

Download* DownloadManager::GetDownload(std::string build_id, DebugSymbolFileType file_type) {
  DownloadIdentifier download_id{build_id, file_type};
  Download* download = nullptr;

  if (const auto& it = downloads_.find(download_id); it != downloads_.end()) {
    return it->second.get();
  }

  // Avoid starting a download if all symbol servers are in a state where they will
  // never become ready. Otherwise, the download could never complete.
  if (servers_failed_auth_ + servers_unreachable_ < system_->GetSymbolServers().size()) {
    auto unique_download = MakeDownload(build_id, file_type);
    download = unique_download.get();
    downloads_[download_id] = std::move(unique_download);

    // Add all configured servers to the new download object.
    DownloadStarted(download_id, download);
  } else {
    DEBUG_LOG(DownloadManager) << "No symbol servers for download for " << build_id;
  }

  return download;
}

void DownloadManager::OnDownloadSucceeded(const std::string& path, const std::string& build_id,
                                          DebugSymbolFileType file_type) {
  download_success_count_++;

  // Adds the file manually since the build_id could already be marked as missing in the
  // build_id_index.
  system_->GetSymbols()->build_id_index().AddOneFile(path);

  for (const auto target : system_->GetTargets()) {
    if (auto process = target->GetProcess()) {
      // Don't need local symbol lookup when retrying a download, so can pass an empty
      // module name.
      process->GetSymbols()->RetryLoadBuildID(std::string(), build_id, file_type);
    }
  }
}

void DownloadManager::DownloadStarted(const DownloadIdentifier& dl_id, Download* download) {
  FX_DCHECK(system_);

  for (auto& server : system_->GetSymbolServers()) {
    if (server->state() == SymbolServer::State::kReady) {
      download->AddServer(server);
    } else {
      DEBUG_LOG(DownloadManager) << "Symbol server " << server->name() << " is not ready.";
    }
  }

  if (download_count_ == 0) {
    for (auto& observer : system_->session()->download_observers()) {
      observer.OnDownloadsStarted();
    }
  }

  download_count_++;
}

void DownloadManager::DownloadFinished() {
  download_count_--;

  FX_DCHECK(system_);
  if (download_count_ == 0) {
    for (auto& observer : system_->session()->download_observers()) {
      observer.OnDownloadsStopped(download_success_count_, download_fail_count_);
    }

    download_success_count_ = download_fail_count_ = 0;
  }
}

bool DownloadManager::HasDownload(const std::string& build_id) const {
  auto download = downloads_.find({build_id, DebugSymbolFileType::kDebugInfo});

  if (download == downloads_.end()) {
    download = downloads_.find({build_id, DebugSymbolFileType::kBinary});
  }

  if (download == downloads_.end()) {
    return false;
  }

  auto ptr = download->second.get();
  return ptr;
}
}  // namespace zxdb

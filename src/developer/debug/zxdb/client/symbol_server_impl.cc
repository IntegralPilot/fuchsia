// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/client/symbol_server_impl.h"

#include <lib/syslog/cpp/macros.h>

#include <cstdio>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <memory>

#include "lib/fit/function.h"
#include "rapidjson/document.h"
#include "rapidjson/filereadstream.h"
#include "rapidjson/filewritestream.h"
#include "rapidjson/istreamwrapper.h"
#include "rapidjson/writer.h"
#include "src/developer/debug/shared/message_loop.h"
#include "src/developer/debug/shared/string_util.h"
#include "src/developer/debug/zxdb/client/session.h"
#include "src/developer/debug/zxdb/client/setting_schema_definition.h"
#include "src/developer/debug/zxdb/common/err_or.h"
#include "src/developer/debug/zxdb/common/string_util.h"
#include "src/lib/fxl/strings/string_printf.h"

namespace zxdb {

namespace {

constexpr const char* kTokenServer = "https://www.googleapis.com/oauth2/v4/token";
constexpr const char* kCloudServer = "https://storage.googleapis.com/";

bool DocIsAuthInfo(const rapidjson::Document& document) {
  return !document.HasParseError() && document.IsObject() && document.HasMember("access_token");
}

std::string ToDebugInfoDFilePath(const std::string& name, DebugSymbolFileType file_type) {
  std::string ret = "buildid/" + name;

  if (file_type == DebugSymbolFileType::kDebugInfo) {
    ret += "/debuginfo";
  } else if (file_type == DebugSymbolFileType::kBinary) {
    ret += "/executable";
  }

  return ret;
}

std::string ToDebugFileName(const std::string& name, DebugSymbolFileType file_type) {
  if (file_type == DebugSymbolFileType::kDebugInfo) {
    return name + ".debug";
  }

  return name;
}

}  // namespace

SymbolServerImpl::SymbolServerImpl(Session* session, const std::string& url,
                                   bool require_authentication)
    : SymbolServer(session, url), base_url_(url), weak_factory_(this) {
  // gs:// is a shortcut for fuchsia's symbol server buckets.
  if (debug::StringStartsWith(url, "gs://")) {
    base_url_ = kCloudServer + url.substr(5);
  }

  if (base_url_.back() != '/') {
    base_url_ += "/";
  }

  if (require_authentication) {
    DoInit();
  } else {
    ChangeState(SymbolServer::State::kReady);
  }
}

Err SymbolServerImpl::HandleRequestResult(Curl::Error result, uint64_t response_code,
                                          size_t previous_ready_count) {
  if (!result && response_code == 200) {
    return Err();
  }

  if (state() != SymbolServer::State::kReady || previous_ready_count != ready_count_) {
    return Err(fxl::StringPrintf("%s: Internal error. (%d)", base_url_.c_str(),
                                 static_cast<int>(state())));
  }

  Err out_err;
  if (result) {
    out_err = Err("Could not contact server: " + result.ToString());
    // Fall through to retry.
  } else if (response_code == 401) {
    return Err(base_url_ + ": Authentication expired.");
  } else if (response_code == 404) {
    return Err(ErrType::kNotFound, base_url_);
  } else {
    out_err = Err("Unexpected response: " + std::to_string(response_code) + "  " + base_url_);
    // Fall through to retry.
  }

  error_log_.push_back(out_err.msg());
  IncrementRetries();

  return out_err;
}

void SymbolServerImpl::DoAuthenticate(const std::map<std::string, std::string>& post_data,
                                      fit::callback<void(const Err&)> cb) {
  ChangeState(SymbolServer::State::kBusy);

  auto curl = fxl::MakeRefCounted<Curl>();

  // When running on GCE, metadata server is used to get the access token and post_data is unused.
  // https://cloud.google.com/compute/docs/access/create-enable-service-accounts-for-instances#applications
  if (auto metadata_server = std::getenv("GCE_METADATA_HOST")) {
    curl->SetURL("http://" + std::string(metadata_server) +
                 "/computeMetadata/v1/instance/service-accounts/default/token");
    curl->headers().push_back("Metadata-Flavor: Google");
  } else {
    curl->SetURL(kTokenServer);
    curl->set_post_data(post_data);
  }

  auto response = std::make_shared<std::string>();
  curl->set_data_callback([response](const std::string& data) {
    response->append(data);
    return data.size();
  });

  curl->Perform([weak_this = weak_factory_.GetWeakPtr(), cb = std::move(cb), response](
                    Curl*, Curl::Error result) mutable {
    if (weak_this)
      weak_this->OnAuthenticationResponse(std::move(result), std::move(cb), *response);
  });
}

void SymbolServerImpl::OnAuthenticationResponse(Curl::Error result,
                                                fit::callback<void(const Err&)> cb,
                                                const std::string& response) {
  if (result) {
    std::string error = "Could not contact authentication server: ";
    error += result.ToString();

    error_log_.push_back(error);
    ChangeState(SymbolServer::State::kAuth);
    cb(Err(error));
    return;
  }

  rapidjson::Document document;
  document.Parse(response);

  if (!DocIsAuthInfo(document)) {
    error_log_.push_back("Authentication failed");
    ChangeState(SymbolServer::State::kAuth);
    cb(Err("Authentication failed"));
    return;
  }

  set_access_token(document["access_token"].GetString());

  bool new_refresh = false;
  if (document.HasMember("refresh_token")) {
    new_refresh = true;
    set_refresh_token(document["refresh_token"].GetString());
  }

  ChangeState(SymbolServer::State::kReady);
  cb(Err());

  if (new_refresh) {
    if (FILE* fp = GetGoogleApiAuthCache("wb")) {
      fwrite(refresh_token().data(), 1, refresh_token().size(), fp);
      fclose(fp);
    }
  }
}

fxl::RefPtr<Curl> SymbolServerImpl::PrepareCurl(const std::string& build_id,
                                                DebugSymbolFileType file_type) {
  if (state() != SymbolServer::State::kReady) {
    return nullptr;
  }

  std::string full_url = base_url_;
  if (debug::StringEndsWith(base_url_, "debug/")) {
    // Still support the the gcs_flat endpoint for fetching symbols.
    full_url += ToDebugFileName(build_id, file_type);
  } else {
    full_url += ToDebugInfoDFilePath(build_id, file_type);
  }

  auto curl = fxl::MakeRefCounted<Curl>();
  FX_DCHECK(curl);

  curl->SetURL(full_url);

  // When require_authentication is true.
  if (!access_token().empty()) {
    curl->headers().push_back("Authorization: Bearer " + access_token());
  }

  return curl;
}

void SymbolServerImpl::Fetch(const std::string& build_id, DebugSymbolFileType file_type,
                             SymbolServer::FetchCallback cb) {
  auto curl = PrepareCurl(build_id, file_type);

  if (!curl) {
    debug::MessageLoop::Current()->PostTask(
        FROM_HERE, [cb = std::move(cb)]() mutable { cb(Err("Server not ready."), std::string()); });
    return;
  }

  FetchWithCurl(build_id, file_type, curl, std::move(cb));
}

void SymbolServerImpl::FetchWithCurl(const std::string& build_id, DebugSymbolFileType file_type,
                                     fxl::RefPtr<Curl> curl, FetchCallback cb) {
  auto cache_path = session()->system().settings().GetString(ClientSettings::System::kSymbolCache);
  std::string path;

  if (!cache_path.empty()) {
    std::error_code ec;
    auto path_obj = std::filesystem::path(cache_path);

    if (std::filesystem::is_directory(path_obj, ec)) {
      // Download to a temporary file, so if we get cancelled (or we get sent a 404 page instead of
      // the real symbols) we don't pollute the build ID folder.
      std::string name = ToDebugFileName(build_id, file_type) + ".part";

      path = path_obj / name;
    }
  }

  // Compute the destination file from the build ID.
  if (build_id.size() <= 2) {
    debug::MessageLoop::Current()->PostTask(FROM_HERE, [build_id, cb = std::move(cb)]() mutable {
      cb(Err("Invalid build ID \"" + build_id + "\" for symbol fetch."), "");
    });
    return;
  }
  auto target_path = std::filesystem::path(cache_path) / build_id.substr(0, 2);
  auto target_name = ToDebugFileName(build_id.substr(2), file_type);

  FILE* file = nullptr;

  // We don't have a folder specified where downloaded symbols go. We'll just drop it in tmp and at
  // least you'll be able to use them for this session.
  if (path.empty()) {
    path = "/tmp/zxdb_downloaded_symbolsXXXXXX\0";
    int fd = mkstemp(path.data());
    path.pop_back();

    if (fd >= 0) {
      // Ownership of the fd is absorbed by fdopen.
      file = fdopen(fd, "wb");
    }
  } else {
    file = std::fopen(path.c_str(), "wb");
  }

  if (!file) {
    debug::MessageLoop::Current()->PostTask(FROM_HERE, [cb = std::move(cb)]() mutable {
      cb(Err("Error opening temporary file."), "");
    });
    return;
  }

  auto cleanup = [file, path, cache_path, target_path,
                  target_name](const Err& in_err) -> ErrOr<std::string> {
    fclose(file);

    std::error_code ec;
    if (in_err.has_error()) {
      // Need to cleanup the file in the error case.
      std::filesystem::remove(path, ec);
      return in_err;  // Just forward the same error to the caller.
    }

    if (cache_path.empty()) {
      return Err("No symbol cache specified.");
    }

    std::filesystem::create_directory(target_path, ec);
    if (std::filesystem::is_directory(target_path, ec)) {
      std::filesystem::rename(path, target_path / target_name, ec);
      return std::string(target_path / target_name);
    } else {
      return Err("Could not move file in to cache.");
    }
  };

  curl->set_data_callback(
      [file](const std::string& data) { return std::fwrite(data.data(), 1, data.size(), file); });

  size_t previous_ready_count = ready_count_;

  curl->Perform([weak_this = weak_factory_.GetWeakPtr(), cleanup, cb = std::move(cb),
                 previous_ready_count](Curl* curl, Curl::Error result) mutable {
    if (!weak_this)
      return;
    Err request_err =
        weak_this->HandleRequestResult(result, curl->ResponseCode(), previous_ready_count);

    ErrOr<std::string> path_or = cleanup(request_err);
    cb(path_or.err_or_empty(), path_or.value_or_empty());
  });
}

}  // namespace zxdb

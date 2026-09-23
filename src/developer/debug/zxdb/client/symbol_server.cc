// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/client/symbol_server.h"

#include <filesystem>
#include <fstream>

#include <rapidjson/document.h>
#include <rapidjson/istreamwrapper.h>

#include "src/developer/debug/shared/logging/logging.h"
#include "src/developer/debug/shared/string_util.h"
#include "src/developer/debug/zxdb/client/symbol_server_impl.h"

namespace zxdb {
namespace {

constexpr size_t kMaxRetries = 5;
constexpr char kClientId[] =
    "446450136466-2hr92jrq8e6i4tnsa56b52vacp7t3936"
    ".apps.googleusercontent.com";
constexpr char kClientSecret[] = "uBfbay2KCy9t4QveJ-dOqHtp";

std::optional<std::filesystem::path> GetGcloudCredentialsFile() {
  std::string config_dir;
  if (auto cloudsdk_config = std::getenv("CLOUDSDK_CONFIG")) {
    config_dir = cloudsdk_config;
  } else if (auto home = std::getenv("HOME")) {
    config_dir = std::string(home) + "/.config/gcloud";
  }

  if (!std::filesystem::is_directory(config_dir)) {
    return std::nullopt;
  }

  std::filesystem::path credentials = config_dir + "/application_default_credentials.json";
  if (!std::filesystem::exists(credentials) || !std::filesystem::is_regular_file(credentials)) {
    return std::nullopt;
  }

  return credentials;
}

}  // namespace

void SymbolServer::IncrementRetries() {
  if (++retries_ == kMaxRetries) {
    ChangeState(SymbolServer::State::kUnreachable);
  }
}

void SymbolServer::ChangeState(SymbolServer::State state) {
  if (state_ == state) {
    return;
  }

  state_ = state;

  if (state_ == SymbolServer::State::kReady) {
    retries_ = 0;
    error_log_.clear();
    ready_count_++;
  }

  if (state_change_callback_)
    state_change_callback_(this, state_);
}

std::unique_ptr<SymbolServer> SymbolServer::FromURL(Session* session, const std::string& url,
                                                    bool require_authentication) {
  return std::make_unique<SymbolServerImpl>(session, url, require_authentication);
}

void SymbolServer::DoInit() {
  if (state() != SymbolServer::State::kAuth && state() != SymbolServer::State::kInitializing) {
    return;
  }

  if (std::getenv("GCE_METADATA_HOST") || LoadCachedAuth() || LoadGCloudAuth()) {
    ChangeState(SymbolServer::State::kBusy);
    AuthRefresh();
  } else {
    ChangeState(SymbolServer::State::kAuth);
    if (!GetGcloudCredentialsFile()) {
      LOGS(Warn)
          << name_ << " failed to authenticate: "
          << "Gcloud credentials file not found. You may need to install the gcloud command line"
             " tool: https://cloud.google.com/sdk/docs/install and run"
             " `gcloud auth application-default login`.";
    }
  }
}

void SymbolServer::AuthRefresh() {
  std::map<std::string, std::string> post_data;
  post_data["refresh_token"] = refresh_token_;
  post_data["client_id"] = client_id_;
  post_data["client_secret"] = client_secret_;
  post_data["grant_type"] = "refresh_token";

  DoAuthenticate(post_data, [this](const Err& err) {
    if (err.has_error()) {
      LOGS(Warn) << "Server " << name_ << " failed authentication: " << err.msg() << ".";
      if (client_secret_.empty() && !GetGcloudCredentialsFile()) {
        LOGS(Warn) << "Could not load gcloud auth token. You may need to run "
                      "`gcloud auth application-default login`";
      } else if (auto credentials_file = GetGcloudCredentialsFile()) {
        LOGS(Warn) << "gcloud application-default credentials exist in "
                   << *GetGcloudCredentialsFile()
                   << ". Check file permissions and your account permissions.";
      }
    }
  });
}

FILE* SymbolServer::GetGoogleApiAuthCache(const char* mode) {
  if (auto home = std::getenv("HOME")) {
    std::filesystem::path path =
        std::filesystem::path(home) / ".fuchsia" / "debug" / "googleapi_auth";

    if (!std::filesystem::exists(path)) {
      return nullptr;
    }

    return fopen(path.c_str(), mode);
  }

  return nullptr;
}

bool SymbolServer::LoadCachedAuth() {
  FILE* fp = GetGoogleApiAuthCache("rb");

  if (!fp) {
    return false;
  }

  std::vector<char> buf(65536);
  buf.resize(fread(buf.data(), 1, buf.size(), fp));
  bool success = feof(fp);
  fclose(fp);

  if (!success) {
    return false;
  }

  client_id_ = kClientId;
  client_secret_ = kClientSecret;
  refresh_token_ = std::string(buf.data(), buf.data() + buf.size());

  return true;
}

bool SymbolServer::LoadGCloudAuth() {
  std::optional<std::string> gcloud_credentials = GetGcloudCredentialsFile();
  if (!gcloud_credentials) {
    return false;
  }

  std::ifstream credential_file(*gcloud_credentials);
  if (!credential_file) {
    return false;
  }

  rapidjson::IStreamWrapper input_stream(credential_file);
  rapidjson::Document credentials;
  credentials.ParseStream(input_stream);

  if (credentials.HasParseError() || !credentials.IsObject() ||
      !credentials.HasMember("client_id") || !credentials.HasMember("client_secret") ||
      !credentials.HasMember("refresh_token")) {
    return false;
  }

  client_id_ = credentials["client_id"].GetString();
  client_secret_ = credentials["client_secret"].GetString();
  refresh_token_ = credentials["refresh_token"].GetString();

  return true;
}

std::string ServerStateToString(const SymbolServer::State& state) {
  switch (state) {
    case SymbolServer::State::kInitializing:
      return "initializing";
    case SymbolServer::State::kAuth:
      return "authenticating";
    case SymbolServer::State::kBusy:
      return "busy";
    case SymbolServer::State::kReady:
      return "ready";
    case SymbolServer::State::kUnreachable:
      return "unreachable";
    default:
      return "unknown";
  }
}

}  // namespace zxdb

// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_SYMBOL_SERVER_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_SYMBOL_SERVER_H_

#include <functional>
#include <map>
#include <memory>
#include <string>
#include <vector>

#include "lib/fit/function.h"
#include "src/developer/debug/zxdb/client/client_object.h"
#include "src/developer/debug/zxdb/common/err.h"
#include "src/developer/debug/zxdb/symbols/debug_symbol_file_type.h"

namespace zxdb {

class SymbolServer : public ClientObject {
 public:
  // Callback used to receive the results of trying to fetch symbols. The string given is the path
  // where the symbols were downloaded. If the string is empty the symbols were unavailable. The
  // error is only set in the event of a connection error. If the symbols are simply unavailable the
  // error will not be set.
  using FetchCallback = fit::callback<void(const Err&, const std::string&)>;

  enum class State {
    kInitializing,  // The server just gets created. It will become kBusy or kReady shortly.
    kAuth,          // The user's credentials have failed authentication.
    kBusy,          // The server is doing authentication.
    kReady,         // The authentication is done and the server is ready to use.
    kUnreachable,   // Too many failed downloads and the server is unusable.
  };

  static std::unique_ptr<SymbolServer> FromURL(Session* session, const std::string& url,
                                               bool require_authentication);

  const std::string& name() const { return name_; }

  const std::vector<std::string>& error_log() const { return error_log_; }

  State state() const { return state_; }
  void set_state_change_callback(fit::callback<void(SymbolServer*, State)> cb) {
    state_change_callback_ = std::move(cb);
  }

  // Attempt to download symbols for the given build ID.
  virtual void Fetch(const std::string& build_id, DebugSymbolFileType file_type,
                     FetchCallback cb) = 0;

  const std::string& access_token() const { return access_token_; }
  void set_access_token(const std::string& access_token) { access_token_ = access_token; }
  const std::string& refresh_token() const { return refresh_token_; }
  void set_refresh_token(const std::string& refresh_token) { refresh_token_ = refresh_token; }

 protected:
  explicit SymbolServer(Session* session, const std::string& name)
      : ClientObject(session), name_(name) {}
  void ChangeState(State state);
  void IncrementRetries();

  // Initialize the class. We want the constructor to do this, but the test mock might need to be
  // manipulated first, so we break this out into a separate function.
  void DoInit();

  // TODO(https://fxbug.dev/42140066): There are currently no use cases where different buckets have
  // different authentication mechanisms (everything is either public, or uses gcloud
  // application-default credentials). This function, and the helpers below, could be shared in the
  // base class (maybe an object?) to authenticate new servers as they are initialized, rather than
  // delegating to the implementations.
  virtual void DoAuthenticate(const std::map<std::string, std::string>& data,
                              fit::callback<void(const Err&)> cb) = 0;

  FILE* GetGoogleApiAuthCache(const char* mode);

  // Load authentication from debugger's config file. Returns whether the loading
  // succeeds.
  bool LoadCachedAuth();

  // Load authentication from gcloud config file. Returns whether the loading succeeds.
  bool LoadGCloudAuth();

  // Use the refresh token to get a new access token.
  void AuthRefresh();

  std::vector<std::string> error_log_;
  size_t retries_ = 0;

  // Incremented each time the state becomes ready.
  size_t ready_count_ = 0;

 private:
  State state_ = State::kInitializing;

  // URL as originally used to construct the class. This is mostly to be used to identify the server
  // in the UI. The actual URL may be processed to handle custom protocol identifiers etc.
  std::string name_;

  std::string access_token_;
  std::string refresh_token_;

  // client_id_ and client_secret_ might be different from kClientId and kClientSecret
  // if we're using gcloud's credential.
  std::string client_id_;
  std::string client_secret_;

  fit::callback<void(SymbolServer*, State)> state_change_callback_;
};

std::string ServerStateToString(const SymbolServer::State& state);

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_SYMBOL_SERVER_H_

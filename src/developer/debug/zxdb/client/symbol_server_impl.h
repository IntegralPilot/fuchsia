// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_SYMBOL_SERVER_IMPL_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_SYMBOL_SERVER_IMPL_H_

#include <map>

#include "lib/fit/function.h"
#include "src/developer/debug/zxdb/client/symbol_server.h"
#include "src/developer/debug/zxdb/common/curl.h"
#include "src/lib/fxl/memory/weak_ptr.h"

namespace zxdb {

class SymbolServerImpl : public SymbolServer {
 public:
  // Construct a new symbol server. Expects a url with one of these formats:
  //   1. gs://bucket/[namespace]
  //   2. https://debuginfod.server.com
  // Format (1) may be appended with "/debug" to hit the gcs_flat endpoint. If there is no format
  // following the GCS bucket namespace, the "/buildid" debuginfod endpoint will be assumed.
  SymbolServerImpl(Session* session, const std::string& url, bool require_authentication);

  // SymbolServer implementation.
  void DoAuthenticate(const std::map<std::string, std::string>& data,
                      fit::callback<void(const Err&)> cb) override;

  void Fetch(const std::string& build_id, DebugSymbolFileType file_type,
             SymbolServer::FetchCallback cb) override;

 private:
  // General dispatch from the result of a Curl transaction. Handles the error cases and converts
  // to a zxdb Err.
  Err HandleRequestResult(Curl::Error result, uint64_t response_code, size_t previous_ready_count);

  fxl::RefPtr<Curl> PrepareCurl(const std::string& build_id, DebugSymbolFileType file_type);
  void FetchWithCurl(const std::string& build_id, DebugSymbolFileType file_type,
                     fxl::RefPtr<Curl> curl, FetchCallback cb);

  void OnAuthenticationResponse(Curl::Error result, fit::callback<void(const Err&)> cb,
                                const std::string& response);

  // This is the base URL to the server, including a trailing '/'. Depending on the server we're
  // talking to, some processing may be applied to the URL given to the constructor to hit the
  // correct endpoint.
  std::string base_url_;

  fxl::WeakPtrFactory<SymbolServerImpl> weak_factory_;
};

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_SYMBOL_SERVER_IMPL_H_

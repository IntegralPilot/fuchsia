// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_TESTING_STUBS_LOADER_H_
#define SRC_DEVELOPER_FORENSICS_TESTING_STUBS_LOADER_H_

#include <fidl/fuchsia.net.http/cpp/fidl.h>
#include <fidl/fuchsia.net.http/cpp/test_base.h>

#include <optional>
#include <string>
#include <vector>

#include "src/developer/forensics/testing/stubs/fidl_server.h"

namespace forensics {
namespace stubs {

struct LoaderResponse {
  static LoaderResponse WithError(fuchsia_net_http::Error error);
  static LoaderResponse WithError(uint32_t status_code);
  static LoaderResponse WithBody(uint32_t status_code, const std::string& body);

  std::optional<fuchsia_net_http::Error> error;
  std::optional<uint32_t> status_code;
  std::optional<std::string> body;
};

using LoaderBase = SingleBindingFidlServer<fuchsia_net_http::Loader>;

class Loader : public LoaderBase {
 public:
  Loader(async_dispatcher_t* dispatcher, std::vector<LoaderResponse> responses)
      : responses_(std::move(responses)), next_response_(responses_.begin()) {}
  ~Loader() override;

  void Fetch(FetchRequest& request, FetchCompleter::Sync& completer) override;
  std::string LastRequestUrl() const;

 private:
  std::vector<LoaderResponse> responses_;
  std::vector<LoaderResponse>::const_iterator next_response_;
  std::string url_;
};

}  // namespace stubs
}  // namespace forensics

#endif  // SRC_DEVELOPER_FORENSICS_TESTING_STUBS_LOADER_H_

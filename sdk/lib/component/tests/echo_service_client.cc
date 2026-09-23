// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fidl.service.test/cpp/fidl.h>
#include <lib/component/incoming/cpp/service.h>
#include <lib/syslog/cpp/macros.h>

int main(int argc, const char** argv) {
  FX_LOGS(INFO) << "Starting EchoService client";

  auto result = component::OpenService<fidl_service_test::EchoService>();
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to connect to EchoService: " << result.status_string();
    return result.status_value();
  }

  auto foo = result->connect_foo();
  if (foo.is_error()) {
    FX_LOGS(ERROR) << "Didn't received expected response when connecting to `foo`: "
                   << foo.status_string();
    return foo.status_value();
  }

  fidl::SyncClient<fidl_service_test::Echo> client(std::move(foo.value()));
  auto response = client->EchoString(fidl::Request<fidl_service_test::Echo::EchoString>("Hello!"));
  if (response.is_error()) {
    FX_LOGS(ERROR) << "Didn't receive expected response: "
                   << response.error_value().status_string();
    return response.error_value().status();
  }

  return 0;
}

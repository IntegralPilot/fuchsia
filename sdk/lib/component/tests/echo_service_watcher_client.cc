// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fidl.service.test/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/incoming/cpp/service.h>
#include <lib/component/incoming/cpp/service_member_watcher.h>
#include <lib/syslog/cpp/macros.h>

using Echo = fidl_service_test::Echo;
using EchoService = fidl_service_test::EchoService;
constexpr const char kTestString[] = "Hogwash";

zx::result<> CheckInstance(zx::result<fidl::ClientEnd<Echo>> result,
                           const std::string& prefix = "") {
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to find instance " << result.status_string();
    return result.take_error();
  }
  fidl::WireResult<Echo::EchoString> echo_result =
      fidl::WireCall(result.value())->EchoString(fidl::StringView(kTestString));
  if (!echo_result.ok()) {
    FX_LOGS(ERROR) << "Failed to call EchoString " << echo_result.status_string();
    return zx::error(echo_result.status());
  }
  auto response = echo_result.Unwrap();
  std::string actual(response->response.data(), response->response.size());
  // make sure the suffix is correct:
  if (!actual.ends_with(kTestString)) {
    FX_LOGS(ERROR) << "EchoString returned " << actual
                   << " but was expected to end with: " << kTestString;
    return zx::error(ZX_ERR_INTERNAL);
  }
  // if we gave a prefix, check that:
  if (!prefix.empty() && !actual.starts_with(prefix)) {
    FX_LOGS(ERROR) << "EchoString returned " << actual
                   << " but was expected to begin with: " << prefix;
    return zx::error(ZX_ERR_INTERNAL);
  }
  return zx::ok();
}

zx_status_t GetInstances(component::SyncServiceMemberWatcher<EchoService::Foo>& watcher) {
  {
    zx::result result = CheckInstance(watcher.GetNextInstance(true));
    if (result.is_error()) {
      FX_LOGS(ERROR) << "First instance failed: " << result.status_string();
      return result.status_value();
    }
  }
  {  // Check the second instance
    zx::result result = CheckInstance(watcher.GetNextInstance(true));
    if (result.is_error()) {
      FX_LOGS(ERROR) << "Second instance failed: " << result.status_string();
      return result.status_value();
    }
  }
  {  // The third check should give us a stop indicator:
    zx::result result = watcher.GetNextInstance(true);
    if (result.status_value() != ZX_ERR_STOP) {
      FX_LOGS(ERROR) << "Expected ZX_ERR_STOP but got: " << result.status_string();
      return ZX_ERR_INTERNAL;
    }
  }
  return ZX_OK;
}

zx_status_t TestSyncServiceMemberWatcher() {
  component::SyncServiceMemberWatcher<EchoService::Foo> watcher;
  return GetInstances(watcher);
}

zx_status_t TestSyncServiceMemberWatcherSetServiceRoot() {
  zx::result<fidl::ClientEnd<fuchsia_io::Directory>> svc_dir = component::OpenServiceRoot();
  ZX_ASSERT(svc_dir.is_ok());
  component::SyncServiceMemberWatcher<EchoService::Foo> watcher(svc_dir->borrow());
  return GetInstances(watcher);
}

int main(int argc, const char** argv) {
  FX_LOGS(INFO) << "Starting EchoService watcher client";

  zx_status_t s1 = TestSyncServiceMemberWatcher();
  zx_status_t s2 = TestSyncServiceMemberWatcherSetServiceRoot();
  return s1 || s2;
}

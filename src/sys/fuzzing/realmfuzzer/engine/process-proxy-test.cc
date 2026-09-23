// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sys/fuzzing/realmfuzzer/engine/process-proxy-test.h"

#include <zircon/status.h>

#include "src/sys/fuzzing/common/async-eventpair.h"
#include "src/sys/fuzzing/realmfuzzer/target/process.h"

namespace fuzzing {

void ProcessProxyTest::SetUp() {
  AsyncTest::SetUp();
  pool_ = std::make_shared<ModulePool>();
}

std::unique_ptr<ProcessProxy> ProcessProxyTest::CreateAndConnectProxy(zx::process process) {
  return CreateAndConnectProxy(std::move(process), MakeOptions());
}

std::unique_ptr<ProcessProxy> ProcessProxyTest::CreateAndConnectProxy(zx::process process,
                                                                      const OptionsPtr& options) {
  AsyncEventPair eventpair(executor());
  return CreateAndConnectProxy(std::move(process), options, eventpair.Create());
}

std::unique_ptr<ProcessProxy> ProcessProxyTest::CreateAndConnectProxy(zx::process process,
                                                                      zx::eventpair eventpair) {
  return CreateAndConnectProxy(std::move(process), MakeOptions(), std::move(eventpair));
}

std::unique_ptr<ProcessProxy> ProcessProxyTest::CreateAndConnectProxy(zx::process process,
                                                                      const OptionsPtr& options,
                                                                      zx::eventpair eventpair) {
  auto process_proxy = std::make_unique<ProcessProxy>(executor(), pool_);
  process_proxy->Configure(options);
  zx_info_handle_basic_t info;
  auto status = process.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  FX_CHECK(status == ZX_OK) << zx_status_get_string(status);
  InstrumentedProcess instrumented = {
      .eventpair = std::move(eventpair),
      .process = std::move(process),
  };
  EXPECT_EQ(process_proxy->Connect(info.koid, instrumented), ZX_OK);
  return process_proxy;
}

}  // namespace fuzzing

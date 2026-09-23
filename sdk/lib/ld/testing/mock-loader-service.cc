// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "lib/ld/testing/mock-loader-service.h"

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/dispatcher.h>
#include <lib/elfldltl/testing/get-test-data.h>
#include <lib/fit/defer.h>
#include <zircon/dlfcn.h>

#include <filesystem>

#include <gmock/gmock.h>

namespace ld::testing {
namespace {

using ::testing::Mock;
using ::testing::Return;

}  // namespace

// A mock server implementation serving the fuchsia.ldsvc.Loader protocol. When
// it receives a request, it will invoke the associated MOCK_METHOD. If the
// test caller did not call Expect* for the request before it is made, then
// the MockServer will fail the test.
class MockLoaderService::MockServer : public fidl::WireServer<fuchsia_ldsvc::Loader> {
 public:
  MockServer() = default;

  ~MockServer() {
    if (backing_loop_) {
      backing_loop_->Shutdown();
    }
  }

  void Init(fidl::ServerEnd<fuchsia_ldsvc::Loader> server) {
    backing_loop_.emplace(&kAsyncLoopConfigNoAttachToCurrentThread);
    zx_status_t status = backing_loop_->StartThread("MockLoaderService");
    ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);
    fidl::BindServer(backing_loop_->dispatcher(), std::move(server), this);
  }

  // Note the mocked methods take std::string, though the Expect* wrappers take
  // std::string_view.  The std::string_view passed to the wrappers is not
  // guaranteed to point to memory that's valid after the call, so they copy
  // into a fresh std::string for EXPECT_CALL; those stay live in the mock.

  MOCK_METHOD(zx::result<zx::vmo>, MockLoadObject, (std::string));

  MOCK_METHOD(zx::result<>, MockConfig, (std::string));

 private:
  // The fidl::WireServer<fuchsia_ldsvc::Loader> implementation, each FIDL
  // method will call into its associated gMock method when invoked.

  // Done is not used in ld tests.
  void Done(DoneCompleter::Sync& completer) override { ADD_FAILURE() << "unexpected Done call"; }

  void LoadObject(LoadObjectRequestView request, LoadObjectCompleter::Sync& completer) override {
    auto result = MockLoadObject(std::string(request->object_name.get()));
    completer.Reply(result.status_value(), std::move(result).value_or(zx::vmo()));
  }

  void Config(ConfigRequestView request, ConfigCompleter::Sync& completer) override {
    auto result = MockConfig(std::string(request->config.get()));
    completer.Reply(result.status_value());
  }

  // Clone is not used in ld tests.
  void Clone(CloneRequestView request, CloneCompleter::Sync& completer) override {
    ADD_FAILURE() << "unexpected Clone call";
  }

  std::optional<async::Loop> backing_loop_;
};

MockLoaderService::MockLoaderService() = default;

MockLoaderService::~MockLoaderService() = default;

void MockLoaderService::Init() {
  mock_server_ = std::make_unique<::testing::StrictMock<MockServer>>();
  auto endpoints = fidl::Endpoints<fuchsia_ldsvc::Loader>::Create();
  ASSERT_NO_FATAL_FAILURE(mock_server_->Init(std::move(endpoints.server)));
  mock_client_ = std::move(endpoints.client);

  // Provide protocol-valid default responses for unexpected calls.  These will
  // make the code under test reach its normal error paths instead of doing
  // something that could never really happen (looking at a default-constructed
  // zx::result).  With StrictMock, these don't prevent reporting unexpected
  // calls as test failures.

  ON_CALL(*mock_server_, MockConfig)
      // An unexpected Config just succeeds.
      .WillByDefault(Return(zx::ok()));

  // An unexpected LoadObject fails as if for a missing file.  This cannot
  // just use ::testing::Return() since that needs to copy its value.
  // Even though this error value could be copied, zx::result<zx::vmo> is a
  // move-only type and so can't be stored by ::testing::Return().
  constexpr auto load_object_not_found = []() -> zx::result<zx::vmo> {
    return zx::error{ZX_ERR_NOT_FOUND};
  };
  ON_CALL(*mock_server_, MockLoadObject)
      .WillByDefault(::testing::InvokeWithoutArgs(load_object_not_found));
}

void MockLoaderService::ExpectLoadObject(std::string_view name,
                                         zx::result<zx::vmo> expected_result) {
  EXPECT_CALL(*mock_server_, MockLoadObject(std::string{name}))
      .InSequence(sequence_)
      .WillOnce(Return(std::move(expected_result)));
}

void MockLoaderService::ExpectConfig(std::string_view name, zx::result<> expected_result) {
  EXPECT_CALL(*mock_server_, MockConfig(std::string{name}))
      .InSequence(sequence_)
      .WillOnce(Return(expected_result));
}

void MockLoaderServiceForTest::Needed(std::initializer_list<std::string_view> names) {
  for (std::string_view name : names) {
    ExpectDependency(name);
  }
}

void MockLoaderServiceForTest::Needed(
    std::initializer_list<std::pair<std::string_view, bool>> name_found_pairs) {
  for (auto [name, found] : name_found_pairs) {
    if (found) {
      ExpectDependency(name);
    } else {
      ExpectMissing(name);
    }
  }
}

void MockLoaderServiceForTest::ExpectLoadObject(std::string_view name,
                                                zx::result<zx::vmo> expected_result) {
  ASSERT_NO_FATAL_FAILURE(ReadyMock());
  mock_loader_->ExpectLoadObject(name, std::move(expected_result));
}

void MockLoaderServiceForTest::ExpectLoadObject(std::string_view name, zx::vmo vmo) {
  ASSERT_TRUE(vmo);
  ExpectLoadObject(name, zx::ok(std::move(vmo)));
}

void MockLoaderServiceForTest::ExpectDependency(std::string_view name) {
  ExpectLoadObject(name, GetVmo(name));
}

void MockLoaderServiceForTest::ExpectRootModule(std::string_view name) {
  ExpectLoadObject(name, GetVmo(name));
}

void MockLoaderServiceForTest::ExpectMissing(std::string_view name) {
  ExpectLoadObject(name, zx::error{ZX_ERR_NOT_FOUND});
}

void MockLoaderServiceForTest::ExpectConfig(std::string_view config) {
  ASSERT_NO_FATAL_FAILURE(ReadyMock());
  mock_loader_->ExpectConfig(config, zx::ok());
}

fidl::ClientEnd<fuchsia_ldsvc::Loader>& MockLoaderServiceForTest::client() {
  ReadyMock();
  return mock_loader_->client();
}

zx::channel MockLoaderServiceForTest::TakeLdsvc() {
  zx::channel ldsvc;
  if (mock_loader_) {
    ldsvc = mock_loader_->client().TakeChannel();
  }
  return ldsvc;
}

zx::unowned_channel MockLoaderServiceForTest::BorrowLdsvc() {
  zx::unowned_channel ldsvc;
  if (mock_loader_) {
    ldsvc = mock_loader_->client().channel().borrow();
  }
  return ldsvc;
}

void MockLoaderServiceForTest::CallWithLdsvcInstalled(fit::function<void()> func) {
  // Initialize the mock loader for tests that have not set expectations on it.
  ASSERT_NO_FATAL_FAILURE(ReadyMock());
  // Install the mock loader as the system loader.
  auto mock_ldsvc = BorrowLdsvc();
  ASSERT_TRUE(mock_ldsvc->is_valid());

  // Restore the loader to the previous system loader at the close of function scope.
  auto restore_system_ldsvc =
      fit::defer([system_ldsvc = zx::channel{dl_set_loader_service(mock_ldsvc->get())}]() mutable {
        dl_set_loader_service(system_ldsvc.release());
      });

  // Now that the mock loader service is installed, call the function.
  func();
}

void MockLoaderServiceForTest::VerifyAndClearExpectations() {
  Mock::VerifyAndClearExpectations(&mock_loader_);
}

zx::vmo MockLoaderServiceForTest::GetVmo(std::string_view name) {
  const std::string path = path_prefix_ / name;
  return elfldltl::testing::GetTestLibVmo(path);
}

void MockLoaderServiceForTest::ReadyMock() {
  if (!mock_loader_) {
    mock_loader_ = std::make_unique<MockLoaderService>();
    ASSERT_NO_FATAL_FAILURE(mock_loader_->Init());
  }
}

zx::result<zx::vmo> MockLoaderServiceForTest::LoadObject(std::string_view name) {
  ReadyMock();
  if (!mock_loader_->mock_server_) {
    return zx::error{ZX_ERR_INTERNAL};
  }
  auto result = mock_loader_->mock_server_->MockLoadObject(std::string(name));
  EXPECT_TRUE(result.is_ok() || result.is_error());
  return result;
}

}  // namespace ld::testing

// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_TESTING_MOCK_LOADER_SERVICE_H_
#define LIB_LD_TESTING_MOCK_LOADER_SERVICE_H_

#include <fidl/fuchsia.ldsvc/cpp/wire.h>
#include <lib/zx/result.h>

#include <filesystem>
#include <string_view>

#include <gmock/gmock.h>

namespace ld::testing {

// MockLoaderService is a mock interface for testing that specific requests
// are made over the fuchsia.ldsvc.Loader protocol.  It is controlled by the
// MockLoaderServiceForTest class (see below).
//
// This class initializes a mock server that serves the fuchsia.ldsvc.Loader
// protocol and provides a reference to a FIDL client to make requests with.
// `Expect*` handlers are provided for each protocol request so that the test
// caller may add an expectation that the method is called and define what the
// mock server should return in its response. For example:
//
// ```
// MockLoaderService mock_loader_service;
// ASSERT_NO_FATAL_FAILURE(mock_loader_service.Init());
// mock_loader_service.ExpectLoadObject("foo.so", zx::ok(zx::vmo()));
// ...
// ```
//
// The private `MockServer` is a StrictMock that will enforce that the test
// calls the `Expect*` handler for every request the mock server receives.
//
// If there are multiple Expect* handles set for the MockLoaderService, the
// test will verify the requests are made in the order of the Expect* calls.
class MockLoaderService {
 public:
  MockLoaderService();

  MockLoaderService(const MockLoaderService&) = delete;

  MockLoaderService(MockLoaderService&&) = delete;

  ~MockLoaderService();

  // This must be called before other methods.
  // It should be used inside ASSERT_NO_FATAL_FAILURE(...).
  void Init();

  // Returns true if Init() has been called and succeeded.
  bool Ready() const { return static_cast<bool>(mock_server_); }

  // Tell the mock server to expect a LoadObject request for a VMO with `name`
  // and to return the `expected_result`.
  void ExpectLoadObject(std::string_view name, zx::result<zx::vmo> expected_result);

  // Tell the mock server to expect a Config request with `name` and to return
  // `expected_result`.
  void ExpectConfig(std::string_view name, zx::result<> expected_result);

  fidl::ClientEnd<fuchsia_ldsvc::Loader>& client() { return mock_client_; }

 private:
  class MockServer;
  friend class MockLoaderServiceForTest;

  std::unique_ptr<::testing::StrictMock<MockServer>> mock_server_;
  fidl::ClientEnd<fuchsia_ldsvc::Loader> mock_client_;
  // Passing this same object in `.InSequence(sequence_)` on every
  // `EXPECT_CALL(...)` enforces that the fuchsia.ldsvc.Loader requests are
  // made in the order that Expect* functions are called, without interfering
  // with other mock objects as ::testing::InSequence would.
  ::testing::Sequence sequence_;
};

// MockLoaderForTest is used by tests to manage an instance of the
// MockLoaderService. This class provides the public test API to prime the mock
// loader with FIDL request expectations and the responses the mock loader
// should return when receiving the request.
//
// MockLoaderForTest lazily initializes its mock_loader_ member, so tests must
// call Init() before interacting with the mock loader service:
//
// ```
// MockLoaderServiceForTest mock_;
// ASSERT_NO_FATAL_FAILURE(mock_.Init());
// mock_.ExpectLoadObject("foo.so", zx::ok(zx::vmo()));
// ...
// ```
class MockLoaderServiceForTest {
 public:
  MockLoaderServiceForTest() = default;
  MockLoaderServiceForTest(const MockLoaderServiceForTest&) = delete;
  MockLoaderServiceForTest(MockLoaderServiceForTest&&) = delete;

  // Set the prefix used on names to look up using the ambient loader service.
  template <typename T>
  void set_path_prefix(T&& path) {
    path_prefix_ = std::forward<T>(path);
  }

  template <typename T>
  void set_path_prefix(std::optional<T> path) {
    if (path) {
      path_prefix_ = *std::move(path);
    } else {
      path_prefix_ = std::filesystem::path{};
    };
  }

  // Do std::filesystem::path::append to that prefix unless source is empty.
  void path_prefix_append(const auto& source) {
    if (!source.empty()) {
      path_prefix_ /= source;
    }
  }

  const std::filesystem::path& path_prefix() const { return path_prefix_; }

  // Fetch a VMO from the ambient loader service using the prefix set by
  // set_path_prefix().
  zx::vmo GetVmo(std::string_view name);

  // Prime the mock loader with the VMOs for the list of dependency names, and
  // add the expectation on the mock loader that it will receive a LoadObject
  // request for each of these dependencies in the order that they are listed.
  void Needed(std::initializer_list<std::string_view> names);

  // Similar to above, except that a boolean `found` is paired with the
  // dependency name to potentially prime the mock loader to return a 'not
  // found' error when it receives the LoadObject request for that dependency.
  void Needed(std::initializer_list<std::pair<std::string_view, bool>> name_found_pairs);

  // A generic interface to prime the mock loader with the `expected_result`
  // for when it receives a LoadObject request for the given `name`.
  void ExpectLoadObject(std::string_view name, zx::result<zx::vmo> expected_result);

  // This is an overload that will check the validity of the VMO before priming
  // the mock loader with a zx::ok result.
  void ExpectLoadObject(std::string_view name, zx::vmo vmo);

  // Prime the mock loader with a dependency VMO and add the expectation on the
  // mock loader that it will receive a LoadObject request for that dependency.
  void ExpectDependency(std::string_view name);

  // Prime the mock loader with a root module VMO and add the expectation on
  // the mock loader that it will receive a LoadObject request for that root
  // module.
  void ExpectRootModule(std::string_view name);

  // Prime the mock loader with a "not found" error for `name` and add the
  // expectation that it will receive a LoadObject request for a VMO with the
  // given `name`.
  void ExpectMissing(std::string_view name);

  // Prime the mock loader with config and add the expectation on the mock
  // loader that it will receive a Config request.
  void ExpectConfig(std::string_view config);

  // Return a reference to the client end to the MockLoader's FIDL server. This
  // will start the server if it hasn't been initialized yet.
  fidl::ClientEnd<fuchsia_ldsvc::Loader>& client();

  // Take ownership of the client end to the MockLoader's FIDL server.
  zx::channel TakeLdsvc();

  // Borrow the client end to the MockLoader's FIDL server.
  zx::unowned_channel BorrowLdsvc();

  // Call `func` with the mock loader installed as the system loader so it will
  // handle fuchsia.ldsvc.Loader requests made during the duration of `func()`.
  void CallWithLdsvcInstalled(fit::function<void()> func);

  // Check that all the registered expectations on the mock_loader_ have been
  // satisfied and then clear its expectations list.
  void VerifyAndClearExpectations();

  // Short-circuit the FIDL protocol and just do what the protocol call would.
  zx::result<zx::vmo> LoadObject(std::string_view name);

 private:
  void ReadyMock();

  std::unique_ptr<MockLoaderService> mock_loader_;
  std::filesystem::path path_prefix_;
};

}  // namespace ld::testing

#endif  // LIB_LD_TESTING_MOCK_LOADER_SERVICE_H_

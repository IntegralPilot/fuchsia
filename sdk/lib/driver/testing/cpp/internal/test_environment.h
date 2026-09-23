// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_TESTING_CPP_INTERNAL_TEST_ENVIRONMENT_H_
#define LIB_DRIVER_TESTING_CPP_INTERNAL_TEST_ENVIRONMENT_H_

#include <fidl/fuchsia.io/cpp/wire.h>
#include <fidl/fuchsia.logger/cpp/fidl.h>
#include <lib/driver/outgoing/cpp/outgoing_directory.h>
#include <lib/fdf/dispatcher.h>

namespace fdf_testing::internal {

// The |TestEnvironment| manages the mocked test environment that the driver being tested uses.
// It provides the server backing the driver's incoming namespace. This incoming namespace can
// be customized by the user through the |incoming_directory| method. The |TestEnvironment|
// uses a fdf::OutgoingDirectory which can support driver transport. Therefore it must be used
// with an fdf_dispatcher.
//
// # Thread safety
//
// This class is thread-unsafe. Instances must be managed and used from a synchronized dispatcher.
// See
// https://fuchsia.dev/fuchsia-src/development/languages/c-cpp/thread-safe-async#synchronized-dispatcher
//
// If the dispatcher used for it is the foreground dispatcher, the TestEnvironment does not need to
// be wrapped in a DispatcherBound.
//
// If the dispatcher is a background dispatcher, the suggestion is to wrap this inside of an
// |async_patterns::TestDispatcherBound|.
class TestEnvironment final {
 public:
  explicit TestEnvironment(fdf_dispatcher_t* dispatcher = nullptr);

  // Get the fdf::OutgoingDirectory that backs the driver's incoming namespace.
  fdf::OutgoingDirectory& incoming_directory() {
    std::lock_guard guard(checker_);
    return incoming_directory_server_;
  }

  const fdf::OutgoingDirectory& incoming_directory() const {
    std::lock_guard guard(checker_);
    return incoming_directory_server_;
  }

  // Adds LogSink to the incoming directory. If required, this *must* be called before calling
  // `Initialize`.
  template <typename Callback>
  zx::result<> AddLogSink(Callback callback) {
    if (logsink_added_) {
      return zx::error(ZX_ERR_BAD_STATE);
    }
    logsink_added_ = true;
    return incoming_directory_server_.component().AddUnmanagedProtocol<fuchsia_logger::LogSink>(
        [callback = std::move(callback)](fidl::ServerEnd<fuchsia_logger::LogSink> server_end) {
          callback(std::move(server_end));
        });
  }

  zx::result<> Initialize(fidl::ServerEnd<fuchsia_io::Directory> incoming_directory_server_end);

 private:
  static const char kTestEnvironmentThreadSafetyDescription[];
  fdf_dispatcher_t* dispatcher_;
  fdf::OutgoingDirectory incoming_directory_server_;
  bool logsink_added_ = false;
  async::synchronization_checker checker_;
};

}  // namespace fdf_testing::internal

#endif  // LIB_DRIVER_TESTING_CPP_INTERNAL_TEST_ENVIRONMENT_H_

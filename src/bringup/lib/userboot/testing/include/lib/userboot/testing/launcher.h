// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_BRINGUP_LIB_USERBOOT_TESTING_INCLUDE_LIB_USERBOOT_TESTING_LAUNCHER_H_
#define SRC_BRINGUP_LIB_USERBOOT_TESTING_INCLUDE_LIB_USERBOOT_TESTING_LAUNCHER_H_

#include <lib/zx/handle.h>
#include <lib/zx/job.h>
#include <lib/zx/process.h>
#include <lib/zx/result.h>

#include <concepts>
#include <vector>

#include <fbl/unique_fd.h>

// All the functions here return zx::result types.  But they also produce
// detailed gtest EXPECT_* failures for any errors.

namespace userboot::testing {

// TestJob holds a child of zx::job::default_job(), killed on destruction.
class TestJob {
 public:
  TestJob() = default;
  TestJob(TestJob&&) = default;
  TestJob& operator=(TestJob&&) = default;
  ~TestJob();

  // Create the job.  Raises gtest assertions for failure.
  void Init();

  // Return true if Init() has been called and it succeeded.
  explicit operator bool() const { return job_.is_valid(); }

  // Duplicate the job to pass to the launcher.  For errors, returns the
  // invalid handle after logging gtest expectation failures.
  zx::job Get();

 private:
  zx::job job_;
};
static_assert(std::default_initializable<TestJob>);
static_assert(std::movable<TestJob>);

class Launcher {
 public:
  /// Acquire the launcher service.  This can be done just once and then reused
  /// across all tests.
  static zx::result<Launcher> Create();

  explicit operator bool() const { return channel_.is_valid(); }

  /// Launch a test process as if it were userboot, inside the test job.  (It
  /// fails quickly if the job is invalid, so TestJob::Get() can be passed in
  /// unchecked.)  This sends an initial message with essential handles about
  /// the process itself, which is handled by the custom startup code in the
  /// userboot library.  The log fd is a pipe whose underlying zx::socket is
  /// sent in place of the zx::debuglog sent in the kernel's initial message
  /// (the libc startup code handles either one).  The handles are whatever it
  /// expects from the kernel in the second message, which the userboot program
  /// will read via TakeBootstrapChannel() from <lib/userboot/startup.h>.
  ///
  /// The handles in each message are always shuffled pseudo-randomly to test
  /// that the userboot program is not sensitive to their order, as there is no
  /// guarantee what order the kernel will use.  The shuffle is based on gtest
  /// testing::UnitTest::random_seed() so it can be recreated via the logging
  /// and machinery that gtest provides for its random permutation testing.
  zx::result<zx::process> Launch(zx::job job, zx::vmo executable, fbl::unique_fd log,
                                 std::vector<zx::handle> handles);

 private:
  // This holds the FIDL endpoint, but without including all the FIDL headers.
  zx::channel channel_;
};
static_assert(std::default_initializable<Launcher>);
static_assert(std::movable<Launcher>);

/// Get an executable VMO for the test program using fdio.  Returns the new,
/// owned VMO handle, or an error; but also produces gtest EXPECT_* failures
/// for any errors.
zx::result<zx::vmo> GetExecutable(const char* filename);

/// Wait for the process to terminate, and return its exit code.
zx::result<int64_t> WaitForTermination(zx::unowned_process process);

}  // namespace userboot::testing

#endif  // SRC_BRINGUP_LIB_USERBOOT_TESTING_INCLUDE_LIB_USERBOOT_TESTING_LAUNCHER_H_

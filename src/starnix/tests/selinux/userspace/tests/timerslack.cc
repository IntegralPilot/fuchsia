// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <sys/eventfd.h>
#include <sys/mman.h>
#include <sys/xattr.h>
#include <unistd.h>

#include <string>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

#include "src/lib/files/file.h"
#include "src/starnix/tests/selinux/userspace/util.h"

extern std::string DoPrePolicyLoadWork() { return "timerslack_policy"; }

namespace {

class ScopedTargetProcess {
 public:
  ScopedTargetProcess(std::string label = "") {
    exit_event_ = fbl::unique_fd(eventfd(0, 0));
    fbl::unique_fd transitioned_event(eventfd(0, 0));
    child_ = fork();
    if (!child_) {
      bool success = true;
      if (!label.empty()) {
        success = files::WriteFile("/proc/thread-self/attr/current", label);
      }
      uint64_t event_buf = 1;
      write(transitioned_event.get(), &event_buf, sizeof(event_buf));
      read(exit_event_.get(), &event_buf, sizeof(event_buf));
      _exit(!success);
    }
    // Block until the subprocess transitions to the expected label.
    char buf[8];
    read(transitioned_event.get(), buf, sizeof(buf));
  }

  ~ScopedTargetProcess() {
    uint64_t to_write = 1;
    write(exit_event_.get(), &to_write, sizeof(to_write));
    int wstatus;
    waitpid(child_, &wstatus, 0);
    if (!WIFEXITED(wstatus) || WEXITSTATUS(wstatus) != 0) {
      ADD_FAILURE() << "target process exited with a failure";
    }
  }

  pid_t pid() { return child_; }

 private:
  pid_t child_;
  fbl::unique_fd exit_event_;
};

TEST(TimerslackNsTest, ReadWriteSelfNoPermRequired) {
  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:timerslack_no_perms_t:s0", [&] {
    std::string contents;
    ASSERT_TRUE(files::ReadFileToString("/proc/self/timerslack_ns", &contents));
    ASSERT_TRUE(files::WriteFile("/proc/self/timerslack_ns", "0"));
  }));
}

struct TestCase {
  const char* label;
  bool read_succeeds;
  bool write_succeeds;
};

class TimerslackNsTestWithParam : public ::testing::TestWithParam<TestCase> {};

TEST_P(TimerslackNsTestWithParam, Read) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ScopedTargetProcess target_process("test_u:test_r:timerslack_target_t:s0");

  ASSERT_TRUE(RunSubprocessAs(std::string("test_u:test_r:") + GetParam().label + ":s0", [&] {
    std::string path =
        std::string("/proc/") + std::to_string(target_process.pid()) + "/timerslack_ns";
    std::string contents;
    ASSERT_EQ(files::ReadFileToString(path, &contents), GetParam().read_succeeds);
  }));
}

TEST_P(TimerslackNsTestWithParam, Write) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ScopedTargetProcess target_process("test_u:test_r:timerslack_target_t:s0");

  ASSERT_TRUE(RunSubprocessAs(std::string("test_u:test_r:") + GetParam().label + ":s0", [&] {
    std::string path =
        std::string("/proc/") + std::to_string(target_process.pid()) + "/timerslack_ns";
    ASSERT_EQ(files::WriteFile(path, "0"), GetParam().write_succeeds);
  }));
}

constexpr TestCase kTests[] = {
    {.label = "timerslack_no_perms_t", .read_succeeds = false, .write_succeeds = false},
    {.label = "timerslack_getsched_nice_perms_t", .read_succeeds = true, .write_succeeds = false},
    {.label = "timerslack_setsched_nice_perms_t", .read_succeeds = false, .write_succeeds = true},
    {.label = "timerslack_getsched_perm_t", .read_succeeds = false, .write_succeeds = false},
    {.label = "timerslack_setsched_perm_t", .read_succeeds = false, .write_succeeds = false},
    {.label = "timerslack_nice_perm_t", .read_succeeds = false, .write_succeeds = false},
};

INSTANTIATE_TEST_SUITE_P(TimerslackNsPermsTest, TimerslackNsTestWithParam,
                         ::testing::ValuesIn(kTests));

}  // namespace

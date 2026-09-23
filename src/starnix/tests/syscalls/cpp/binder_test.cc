// Copyright 2025 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <lib/fit/function.h>
#include <stdint.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/mount.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/sysmacros.h>
#include <unistd.h>

#include <algorithm>
#include <atomic>
#include <format>
#include <thread>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>
#include <linux/android/binder.h>
#include <linux/android/binderfs.h>
#include <linux/netlink.h>

#include "src/starnix/tests/syscalls/cpp/binder/common.h"
#include "src/starnix/tests/syscalls/cpp/binder/manager_provider_client_test.h"
#include "src/starnix/tests/syscalls/cpp/binder_helper.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace {

bool skip_binder_tests = false;

class BinderTest : public ::testing::Test {
 public:
  static void SetUpTestSuite() {
    // The unshare() call will isolate the mount namespaces for the running
    // test process. This allows the Linux-based tests to execute syscalls with
    // root permissions, without fear of messing the environment up. While the
    // Starnix tests don't strictly need to unshare, it's beneficial to run the
    // same test binaries on Linux and on Starnix so we can be sure the semantics
    // match. As a side effect, this means that the mounted directories will not
    // be viewable in traditional ways, e.g. ffx component explore.
    // TODO(https://fxbug.dev/317285180) don't skip on baseline
    int rv = unshare(CLONE_NEWNS);
    if (rv == -1 && errno == EPERM) {
      // GTest does not support GTEST_SKIP() from a suite setup, so record that we want to skip
      // every test here and skip in SetUp().
      skip_binder_tests = true;
      return;
    }
    ASSERT_EQ(rv, 0) << "unshare(CLONE_NEWNS) failed: " << strerror(errno) << "(" << errno << ")";
  }

  void SetUp() override {
    if (skip_binder_tests) {
      GTEST_SKIP() << "Permission denied for unshare(CLONE_NEWNS), skipping suite.";
    }

    ASSERT_FALSE(temp_dir_.path().empty());

    // Listen to uevents during mount.
    uevent_fd = fbl::unique_fd(socket(AF_NETLINK, SOCK_RAW, NETLINK_KOBJECT_UEVENT));
    ASSERT_TRUE(uevent_fd) << strerror(errno);

    int buf_sz = 16 * 1024 * 1024;
    ASSERT_THAT(setsockopt(uevent_fd.get(), SOL_SOCKET, SO_RCVBUF, &buf_sz, sizeof(buf_sz)),
                SyscallSucceeds());

    struct sockaddr_nl addr = {
        .nl_family = AF_NETLINK,
        .nl_pid = 0,
        .nl_groups = 0xffffffff,
    };
    ASSERT_THAT(bind(uevent_fd.get(), (struct sockaddr*)&addr, sizeof(addr)), SyscallSucceeds());

    // Mount sysfs to check for device presence.
    ASSERT_THAT(mkdir(TestPath("sys").c_str(), 0o700), SyscallSucceeds());
    ASSERT_THAT(mount("sysfs", TestPath("sys").c_str(), "sysfs", 0, nullptr), SyscallSucceeds());

    ASSERT_THAT(mkdir(TestPath("binderfs").c_str(), 0o700), SyscallSucceeds());
    if (mount("binder", TestPath("binderfs").c_str(), "binder", 0, nullptr) < 0) {
      ASSERT_EQ(errno, ENODEV);
      GTEST_SKIP() << "binderfs is not available, skipping test.";
    }
  }

  std::string TestPath(const char* path) const { return temp_dir_.path() + "/" + path; }

  ::testing::AssertionResult NoUeventReceived() {
    char buffer[4096];
    ssize_t bytes = recv(uevent_fd.get(), buffer, sizeof(buffer), MSG_DONTWAIT);
    if (bytes != -1) {
      return ::testing::AssertionFailure() << "Received uevent";
    } else if (errno != EAGAIN && errno != EWOULDBLOCK) {
      return ::testing::AssertionFailure() << "Recv failed: " << strerror(errno);
    } else {
      return ::testing::AssertionSuccess();
    }
  }

 private:
  test_helper::ScopedTempDir temp_dir_;
  fbl::unique_fd uevent_fd;
};

TEST_F(BinderTest, NoUeventOnMount) { ASSERT_TRUE(NoUeventReceived()); }

TEST_F(BinderTest, SetContextMgrWithNull) {
  fbl::unique_fd binder =
      fbl::unique_fd(open(TestPath("binderfs/binder").c_str(), O_RDWR | O_CLOEXEC));
  ASSERT_TRUE(binder) << strerror(errno);

  EXPECT_THAT(ioctl(binder.get(), BINDER_SET_CONTEXT_MGR, 0), SyscallSucceeds());
}

TEST_F(BinderTest, InvalidCommandError) {
  fbl::unique_fd binder =
      fbl::unique_fd(open(TestPath("binderfs/binder").c_str(), O_RDWR | O_CLOEXEC));
  ASSERT_TRUE(binder) << strerror(errno);

  uint32_t writebuf[1];
  writebuf[0] = -1;
  struct binder_write_read bwr = {};
  bwr.write_buffer = (binder_uintptr_t)writebuf;
  bwr.write_size = sizeof(uint32_t);
  bwr.write_consumed = 0;

  ASSERT_THAT(ioctl(binder.get(), BINDER_WRITE_READ, &bwr), SyscallFailsWithErrno(EINVAL));

  // The failing command is not consumed.
  EXPECT_EQ(bwr.write_consumed, size_t(0));
}

TEST_F(BinderTest, ValidThenInvalidCommand) {
  fbl::unique_fd binder =
      fbl::unique_fd(open(TestPath("binderfs/binder").c_str(), O_RDWR | O_CLOEXEC));
  ASSERT_TRUE(binder) << strerror(errno);

  uint32_t writebuf[2];
  writebuf[0] = BC_ENTER_LOOPER;
  writebuf[1] = -1;
  struct binder_write_read bwr = {};
  bwr.write_buffer = (binder_uintptr_t)writebuf;
  bwr.write_size = 2 * sizeof(uint32_t);
  bwr.write_consumed = 0;

  ASSERT_THAT(ioctl(binder.get(), BINDER_WRITE_READ, &bwr), SyscallFailsWithErrno(EINVAL));

  // The first command is consumed.
  EXPECT_EQ(bwr.write_consumed, sizeof(uint32_t));
}

TEST_F(BinderTest, IgnoreAlreadyConsumed) {
  fbl::unique_fd binder =
      fbl::unique_fd(open(TestPath("binderfs/binder").c_str(), O_RDWR | O_CLOEXEC));
  ASSERT_TRUE(binder) << strerror(errno);

  uint32_t writebuf[2];
  writebuf[0] = -1;
  writebuf[1] = BC_ENTER_LOOPER;
  struct binder_write_read bwr = {};
  bwr.write_buffer = (binder_uintptr_t)writebuf;
  bwr.write_size = 2 * sizeof(uint32_t);
  bwr.write_consumed = sizeof(uint32_t);

  ASSERT_THAT(ioctl(binder.get(), BINDER_WRITE_READ, &bwr), SyscallSucceeds());

  EXPECT_EQ(bwr.write_consumed, 2 * sizeof(uint32_t));
  EXPECT_EQ(bwr.write_size, 2 * sizeof(uint32_t));

  // We can reuse the structure for the next write, no command will be executed.
  ASSERT_THAT(ioctl(binder.get(), BINDER_WRITE_READ, &bwr), SyscallSucceeds());
}

TEST_F(BinderTest, BinderControl) {
  fbl::unique_fd binder_control =
      fbl::unique_fd(open(TestPath("binderfs/binder-control").c_str(), O_RDWR | O_CLOEXEC));
  ASSERT_TRUE(binder_control) << strerror(errno);

  struct binderfs_device new_device = {};
  std::string kBinderName = "binder-test";
  std::ranges::copy(kBinderName, new_device.name);
  EXPECT_THAT(ioctl(binder_control.get(), BINDER_CTL_ADD, &new_device), SyscallSucceeds());

  // The ioctl set the correct major and minor numbers.
  struct stat sb = {};
  EXPECT_THAT(stat(TestPath("binderfs/binder-test").c_str(), &sb), SyscallSucceeds());
  EXPECT_EQ(sb.st_mode, S_IFCHR | 0o600u);
  EXPECT_EQ(major(sb.st_rdev), new_device.major);
  EXPECT_EQ(minor(sb.st_rdev), new_device.minor);

  // The result is a usable binder device.
  fbl::unique_fd binder =
      fbl::unique_fd(open(TestPath("binderfs/binder-test").c_str(), O_RDWR | O_CLOEXEC));
  ASSERT_TRUE(binder) << strerror(errno);
  struct binder_version version = {};
  EXPECT_THAT(ioctl(binder.get(), BINDER_VERSION, &version), SyscallSucceeds());

  EXPECT_TRUE(NoUeventReceived());
  // sysfs has entries for usual devices (1:3 is /dev/null)
  EXPECT_THAT(access(TestPath("sys/dev/char/1:3/").c_str(), F_OK), SyscallSucceeds());
  // It doesn't have an entry for our new binder device.
  EXPECT_THAT(
      access(std::format("{}/{}:{}", TestPath("sys/dev/char"), new_device.major, new_device.minor)
                 .c_str(),
             F_OK),
      SyscallFailsWithErrno(ENOENT));
  // It doesn't have an entry for binder-control.
  EXPECT_THAT(fstat(binder_control.get(), &sb), SyscallSucceeds());
  EXPECT_THAT(
      access(std::format("{}/{}:{}", TestPath("sys/dev/char"), major(sb.st_rdev), minor(sb.st_rdev))
                 .c_str(),
             F_OK),
      SyscallFailsWithErrno(ENOENT));
}

TEST_F(BinderTest, BinderControlExists) {
  fbl::unique_fd binder_control =
      fbl::unique_fd(open(TestPath("binderfs/binder-control").c_str(), O_RDWR | O_CLOEXEC));
  ASSERT_TRUE(binder_control) << strerror(errno);

  struct binderfs_device new_device = {};
  std::string kBinderName = "binder";
  std::ranges::copy(kBinderName, new_device.name);
  EXPECT_THAT(ioctl(binder_control.get(), BINDER_CTL_ADD, &new_device),
              SyscallFailsWithErrno(EEXIST));
}

TEST_F(BinderTest, BinderControlInvalidPathDot) {
  fbl::unique_fd binder_control =
      fbl::unique_fd(open(TestPath("binderfs/binder-control").c_str(), O_RDWR | O_CLOEXEC));
  ASSERT_TRUE(binder_control) << strerror(errno);

  struct binderfs_device new_device = {};
  std::string kBinderName = ".";
  std::ranges::copy(kBinderName, new_device.name);
  EXPECT_THAT(ioctl(binder_control.get(), BINDER_CTL_ADD, &new_device),
              SyscallFailsWithErrno(EACCES));
}

TEST_F(BinderTest, BinderControlInvalidPathDotDot) {
  fbl::unique_fd binder_control =
      fbl::unique_fd(open(TestPath("binderfs/binder-control").c_str(), O_RDWR | O_CLOEXEC));
  ASSERT_TRUE(binder_control) << strerror(errno);

  struct binderfs_device new_device = {};
  std::string kBinderName = "..";
  std::ranges::copy(kBinderName, new_device.name);
  EXPECT_THAT(ioctl(binder_control.get(), BINDER_CTL_ADD, &new_device),
              SyscallFailsWithErrno(EACCES));
}

TEST_F(BinderTest, BinderControlInvalidPathEmpty) {
  fbl::unique_fd binder_control =
      fbl::unique_fd(open(TestPath("binderfs/binder-control").c_str(), O_RDWR | O_CLOEXEC));
  ASSERT_TRUE(binder_control) << strerror(errno);

  struct binderfs_device new_device = {};
  std::string kBinderName;
  std::ranges::copy(kBinderName, new_device.name);
  EXPECT_THAT(ioctl(binder_control.get(), BINDER_CTL_ADD, &new_device),
              SyscallFailsWithErrno(EACCES));
}

TEST_F(BinderTest, BinderControlInvalidPathReserved) {
  fbl::unique_fd binder_control =
      fbl::unique_fd(open(TestPath("binderfs/binder-control").c_str(), O_RDWR | O_CLOEXEC));
  ASSERT_TRUE(binder_control) << strerror(errno);

  struct binderfs_device new_device = {};
  std::string kBinderName = "binder-control";
  std::ranges::copy(kBinderName, new_device.name);
  EXPECT_THAT(ioctl(binder_control.get(), BINDER_CTL_ADD, &new_device),
              SyscallFailsWithErrno(EEXIST));
}

TEST_F(BinderTest, BinderControlInvalidPathSlash) {
  fbl::unique_fd binder_control =
      fbl::unique_fd(open(TestPath("binderfs/binder-control").c_str(), O_RDWR | O_CLOEXEC));
  ASSERT_TRUE(binder_control) << strerror(errno);

  struct binderfs_device new_device = {};
  std::string kBinderName = "my/binder";
  std::ranges::copy(kBinderName, new_device.name);
  EXPECT_THAT(ioctl(binder_control.get(), BINDER_CTL_ADD, &new_device),
              SyscallFailsWithErrno(EACCES));
}

// This test verifies that sending a file descriptor to a process that is in the middle of exiting
// does not crash the kernel or return a protocol error. Instead, it should fail gracefully,
// eventually returning BR_DEAD_REPLY or BR_FAILED_REPLY.
TEST_F(BinderTest, SendFdToExitingProcess) {
  using namespace starnix_binder;

  auto manager_ready = test_helper::MakeRendezvous();
  pid_t manager_pid = 0;

  // Spawn the manager using the framework helper.
  auto manager = ManagerProcess(
      TestPath("binderfs"),
      [&](test_helper::ForkHelper& fork_helper, fit::closure manager_behavior) {
        manager_pid = fork_helper.RunInForkedProcess(std::move(manager_behavior));
        return manager_pid;
      },
      std::move(manager_ready.poker));

  manager_ready.holder.hold();

  auto binder_and_map = OpenBinderAndMap(TestPath("binderfs"));
  ASSERT_TRUE(binder_and_map.fd_);
  ASSERT_THAT(binder_and_map.mapping_, SyscallResultIsOk());
  const auto& binder = binder_and_map.fd_;

  // Prepare transaction with FD.
  fbl::unique_fd fd_to_send(SAFE_SYSCALL(open("/dev/null", O_RDONLY)));
  FdTransaction fd_transaction(0, kServiceSendFd, fd_to_send.get());
  const auto& write_buffer = fd_transaction.write_buffer;

  // Spawn a background thread that continuously sends the transaction with the FD to the manager.
  // The transaction is two-way and the manager does not reply, so the thread will block in ioctl()
  // until the manager process is killed.
  auto send_started = test_helper::MakeRendezvous();
  std::atomic<int> ioctl_errno(0);
  std::atomic<bool> got_error(false);
  std::atomic<bool> got_dead_or_failed_reply(false);
  std::thread send_thread([&, send_started = std::move(send_started.poker)]() mutable {
    bool starting = true;
    while (true) {
      struct binder_write_read bwr = {};
      bwr.write_buffer = (binder_uintptr_t)&write_buffer;
      bwr.write_size = sizeof(write_buffer);
      bwr.write_consumed = 0;

      uint32_t read_buf[32] = {};
      bwr.read_buffer = (binder_uintptr_t)read_buf;
      bwr.read_size = sizeof(read_buf);
      bwr.read_consumed = 0;

      // Wait to poke send_started until we are about to enter ioctl().
      if (starting) {
        starting = false;
        send_started.poke();
      }
      if (ioctl(binder.get(), BINDER_WRITE_READ, &bwr) < 0) {
        ioctl_errno = errno;
        break;
      }

      ParsedMessage pm = ParseMessage((binder_uintptr_t)read_buf, bwr.read_consumed);
      for (auto cmd : pm.returns_) {
        if (cmd == BR_ERROR) {
          got_error = true;
          return;
        }
        if (cmd == BR_DEAD_REPLY || cmd == BR_FAILED_REPLY) {
          got_dead_or_failed_reply = true;
          return;
        }
      }
    }
  });

  send_started.holder.hold();

  // Kill the manager and wait for the send thread to finish.
  manager.call();
  send_thread.join();

  // We expect the transactions to fail with BR_DEAD_REPLY or BR_FAILED_REPLY after the child died.
  EXPECT_FALSE(got_error.load());
  EXPECT_TRUE(got_dead_or_failed_reply.load());
  EXPECT_THAT(ioctl_errno.load(), SyscallSucceedsWithValue(0));
}

// Linux's binder driver rejects mmap with PROT_WRITE and clears VM_MAYWRITE to
// prevent mprotect from adding write permission. Verify Starnix matches this
// behavior.
TEST_F(BinderTest, MmapRejectsProtWrite) {
  using namespace starnix_binder;
  fbl::unique_fd binder =
      fbl::unique_fd(open(TestPath("binderfs/binder").c_str(), O_RDWR | O_CLOEXEC));
  ASSERT_TRUE(binder) << strerror(errno);

  auto mapping = test_helper::ScopedMMap::MMap(nullptr, kBinderMMapSize, PROT_READ | PROT_WRITE,
                                               MAP_PRIVATE, binder.get(), 0);
  ASSERT_TRUE(mapping.is_error()) << "mmap with PROT_WRITE should fail with EPERM";
  EXPECT_EQ(mapping.error_value(), EPERM) << strerror(mapping.error_value());
}

TEST_F(BinderTest, MprotectCannotAddWriteToBinder) {
  using namespace starnix_binder;
  fbl::unique_fd binder =
      fbl::unique_fd(open(TestPath("binderfs/binder").c_str(), O_RDWR | O_CLOEXEC));
  ASSERT_TRUE(binder) << strerror(errno);

  auto mapping = test_helper::ScopedMMap::MMap(nullptr, kBinderMMapSize, PROT_READ, MAP_PRIVATE,
                                               binder.get(), 0);
  ASSERT_TRUE(mapping.is_ok()) << mapping.error_value();

  EXPECT_THAT(mprotect(mapping->mapping(), kBinderMMapSize, PROT_READ | PROT_WRITE),
              SyscallFailsWithErrno(EACCES))
      << "mprotect should not be able to add PROT_WRITE to a binder mapping";
}

}  // namespace

namespace starnix_binder {

class WithoutSEStarnix : public WithOrWithoutSEStarnix {
 public:
  pid_t SpawnManager(test_helper::ForkHelper& fork_helper, fit::closure manager_behavior) override {
    return fork_helper.RunInForkedProcess(std::move(manager_behavior));
  }
  pid_t SpawnProvider(test_helper::ForkHelper& fork_helper,
                      fit::closure provider_behavior) override {
    return fork_helper.RunInForkedProcess(std::move(provider_behavior));
  }
  pid_t SpawnClient(test_helper::ForkHelper& fork_helper, fit::closure client_behavior) override {
    return fork_helper.RunInForkedProcess(std::move(client_behavior));
  }
  void ValidateClientSecctxSeenByProvider(std::string_view secctx) override {
    // Nothing to validate here in the Not-SE flavor.
  }
  bool SkipEntirely() override { return !test_helper::IsStarnix(); }
};

INSTANTIATE_TYPED_TEST_SUITE_P(BinderWithoutSEStarnix, ManagerProviderClientTest, WithoutSEStarnix);

}  // namespace starnix_binder

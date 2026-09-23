// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <lib/fit/defer.h>
#include <lib/fit/function.h>
#include <lib/fit/result.h>
#include <sys/socket.h>
#include <unistd.h>

#include <cerrno>
#include <cstring>
#include <fstream>
#include <string_view>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>
#include <linux/audit.h>
#include <linux/netlink.h>

#include "src/lib/files/file.h"
#include "src/lib/files/path.h"
#include "src/starnix/tests/selinux/userspace/audit_utils.h"
#include "src/starnix/tests/selinux/userspace/util.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

extern std::string DoPrePolicyLoadWork() { return "netlink_audit_policy"; }

namespace {

constexpr int NETLINK_BUF_SIZE = 4096;

TEST(NetlinkAuditTest, RegisterAuditDaemon) {
  fbl::unique_fd fd = OpenNetlinkAuditSocket();
  ASSERT_TRUE(fd.is_valid());
  ASSERT_TRUE(RegisterAsAuditDaemon(fd.get()).is_ok());
  EXPECT_TRUE(UnregisterAuditDaemon(fd.get()).is_ok());
}

TEST(NetlinkAuditTest, DeregisterInexistentAuditDaemon) {
  fbl::unique_fd fd = OpenNetlinkAuditSocket();
  ASSERT_TRUE(fd.is_valid());
  ASSERT_TRUE(UnregisterAuditDaemon(fd.get()).is_ok());
}

TEST(NetlinkAuditTest, RegisterAuditDaemonWithOtherPid) {
  fbl::unique_fd fd = OpenNetlinkAuditSocket();
  ASSERT_TRUE(fd.is_valid());
  ASSERT_THAT(RegisterAsAuditDaemon(fd.get(), getpid() + 1), SyscallResultIsErrno(EINVAL));
}

TEST(NetlinkAuditTest, RegisterAuditDaemonTwice) {
  fbl::unique_fd fd1 = OpenNetlinkAuditSocket();
  fbl::unique_fd fd2 = OpenNetlinkAuditSocket();
  ASSERT_TRUE(fd1.is_valid());
  ASSERT_TRUE(fd2.is_valid());

  ASSERT_TRUE(RegisterAsAuditDaemon(fd1.get()).is_ok());
  ASSERT_THAT(RegisterAsAuditDaemon(fd2.get()), SyscallResultIsErrno(EEXIST));

  EXPECT_TRUE(UnregisterAuditDaemon(fd1.get()).is_ok());
}

TEST(NetlinkAuditTest, MessageReceivedOnlyOnDaemonSocket) {
  fbl::unique_fd fd1 = OpenNetlinkAuditSocket();
  fbl::unique_fd fd2 = OpenNetlinkAuditSocket();
  ASSERT_TRUE(fd1.is_valid());
  ASSERT_TRUE(fd2.is_valid());

  // Send a message on fd1 before registering it as the daemon.
  const std::string message = "test_message_case_1";
  ASSERT_TRUE(SendUserAuditMessage(fd1.get(), 101, message).is_ok());
  ASSERT_TRUE(RegisterAsAuditDaemon(fd1.get()).is_ok());

  char buf[NETLINK_BUF_SIZE]{};
  EXPECT_THAT(ReceiveNetlinkMessage(fd2.get(), buf, sizeof(buf), false),
              SyscallResultIsErrno(EAGAIN));

  // The message should be on fd1.
  struct nlmsghdr* nlh;
  do {
    auto recv_res = ReceiveNetlinkMessage(fd1.get(), buf, sizeof(buf));
    ASSERT_TRUE(recv_res.is_ok());
    ASSERT_GE(recv_res.value(), NLMSG_HDRLEN);
    nlh = reinterpret_cast<struct nlmsghdr*>(buf);
  } while (nlh->nlmsg_type != AUDIT_USER_AVC);
  char* data = static_cast<char*>(NLMSG_DATA(nlh));
  EXPECT_NE(strstr(data, message.c_str()), nullptr);

  EXPECT_TRUE(UnregisterAuditDaemon(fd1.get()).is_ok());
}

TEST(NetlinkAuditTest, ForkedChildReadsParentsQueuedMessage) {
  fbl::unique_fd fd = OpenNetlinkAuditSocket();
  auto fork_helper = test_helper::ForkHelper();
  ASSERT_TRUE(fd.is_valid());

  const std::string message = "message_before_fork";
  ASSERT_TRUE(SendUserAuditMessage(fd.get(), 201, message).is_ok());
  ASSERT_TRUE(RegisterAsAuditDaemon(fd.get()).is_ok());

  pid_t child_pid = fork_helper.RunInForkedProcess([&] {
    // Child process: try to read the message from the fd.
    char buf[NETLINK_BUF_SIZE]{};
    struct nlmsghdr* nlh;
    do {
      auto recv_res = ReceiveNetlinkMessage(fd.get(), buf, sizeof(buf));
      ASSERT_TRUE(recv_res.is_ok());
      ASSERT_GE(recv_res.value(), NLMSG_HDRLEN);
      nlh = reinterpret_cast<struct nlmsghdr*>(buf);
    } while (nlh->nlmsg_type != AUDIT_USER_AVC);

    char* data = static_cast<char*>(NLMSG_DATA(nlh));
    EXPECT_NE(strstr(data, message.c_str()), nullptr);
  });
  ASSERT_GE(child_pid, 0) << "fork failed: " << strerror(errno);

  // Parent process: wait for child and check its exit code.
  ASSERT_TRUE(fork_helper.WaitForChildren());
  EXPECT_TRUE(UnregisterAuditDaemon(fd.get()).is_ok());
}

TEST(NetlinkAuditTest, ChildDaemonReadsParentsMessageFromNewSocket) {
  fbl::unique_fd fd_parent_initial = OpenNetlinkAuditSocket();
  auto fork_helper = test_helper::ForkHelper();
  const std::string message = "message_from_parent_new_socket";
  ASSERT_TRUE(fd_parent_initial.is_valid());

  pid_t child_pid = fork_helper.RunInForkedProcess([&] {
    // Register as the audit daemon on the inherited socket.
    ASSERT_TRUE(RegisterAsAuditDaemon(fd_parent_initial.get()).is_ok());
    // Child process: try to read the message from the fd.
    char buf[NETLINK_BUF_SIZE]{};
    struct nlmsghdr* nlh;
    do {
      auto recv_res = ReceiveNetlinkMessage(fd_parent_initial.get(), buf, sizeof(buf));
      ASSERT_TRUE(recv_res.is_ok());
      ASSERT_GE(recv_res.value(), NLMSG_HDRLEN);
      nlh = reinterpret_cast<struct nlmsghdr*>(buf);
    } while (nlh->nlmsg_type != AUDIT_USER_AVC);

    char* data = static_cast<char*>(NLMSG_DATA(nlh));
    EXPECT_NE(strstr(data, message.c_str()), nullptr);
    EXPECT_TRUE(UnregisterAuditDaemon(fd_parent_initial.get()).is_ok());
  });
  ASSERT_GE(child_pid, 0) << "fork failed: " << strerror(errno);

  // Close the original socket. The child now has the only reference.
  fd_parent_initial.reset();

  // Open a new socket and send the message.
  fbl::unique_fd fd_parent_new = OpenNetlinkAuditSocket();
  ASSERT_TRUE(fd_parent_new.is_valid());
  ASSERT_TRUE(SendUserAuditMessage(fd_parent_new.get(), 301, message).is_ok());

  ASSERT_TRUE(fork_helper.WaitForChildren());
}

TEST(NetlinkAuditTest, ForkedChildUnregistersParentOnOneSocket) {
  fbl::unique_fd fd = OpenNetlinkAuditSocket();
  auto fork_helper = test_helper::ForkHelper();
  ASSERT_TRUE(fd.is_valid());

  ASSERT_TRUE(RegisterAsAuditDaemon(fd.get()).is_ok());

  pid_t child_pid = fork_helper.RunInForkedProcess([&] {
    // Child process: try to read the message from the fd.
    ASSERT_FALSE(UnregisterAuditDaemon(fd.get()).is_ok());
  });
  ASSERT_GE(child_pid, 0) << "fork failed: " << strerror(errno);

  // Parent process: wait for child and check its exit code.
  ASSERT_TRUE(fork_helper.WaitForChildren());
  EXPECT_TRUE(UnregisterAuditDaemon(fd.get()).is_ok());
}

TEST(NetlinkAuditTest, SocketCreateAllowed) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:audit_netlink_create_test_t:s0", [&] {
    EXPECT_THAT(socket(AF_NETLINK, SOCK_RAW, NETLINK_AUDIT), SyscallSucceeds())
        << "Netlink socket open should succeed";
  }));
}

TEST(NetlinkAuditTest, SocketCreateDenied) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:audit_netlink_restricted_test_t:s0", [&] {
    EXPECT_THAT(socket(AF_NETLINK, SOCK_RAW, NETLINK_AUDIT), SyscallFailsWithErrno(EACCES))
        << "Netlink socket open should fail";
  }));
}

struct SendTestParam {
  std::string_view label;
  bool should_wait_ack;
  int expected_errno;
};

class NetlinkAuditSendTest : public ::testing::TestWithParam<SendTestParam> {};

const char kCreateOnlyLabel[] = "test_u:test_r:audit_netlink_create_test_t:s0";
const char kReadLabel[] = "test_u:test_r:audit_netlink_read_test_t:s0";
const char kWriteLabel[] = "test_u:test_r:audit_netlink_write_test_t:s0";
const char kWriteRelayLabel[] = "test_u:test_r:audit_netlink_write_relay_test_t:s0";
const char kAllowWithWriteCapLabel[] = "test_u:test_r:audit_netlink_allow_with_write_cap_test_t:s0";
const char kAllowWithoutWriteCapLabel[] =
    "test_u:test_r:audit_netlink_allow_without_write_cap_test_t:s0";

TEST_P(NetlinkAuditSendTest, Send) {
  auto send_test = GetParam();
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs(send_test.label, [&] {
    fbl::unique_fd fd = OpenNetlinkAuditSocket();
    ASSERT_TRUE(fd.is_valid());

    auto result = SendUserAuditMessage(
        fd.get(), 1, std::string(send_test.label) + ": GENERIC_MESSAGE", send_test.should_wait_ack);
    // If the errno is expected to be 0, check for success.
    if (send_test.expected_errno == 0) {
      ASSERT_THAT(result, SyscallResultIsOk()) << "Netlink send should succeed";
    } else {
      ASSERT_THAT(result, SyscallResultIsErrno(send_test.expected_errno));
    }
  }));
}

INSTANTIATE_TEST_SUITE_P(NetlinkAuditWriteTestSuite, NetlinkAuditSendTest,
                         ::testing::Values(SendTestParam{kCreateOnlyLabel, false, EACCES},
                                           SendTestParam{kReadLabel, false, EACCES},
                                           SendTestParam{kWriteLabel, false, EACCES},
                                           SendTestParam{kWriteRelayLabel, false, 0},
                                           SendTestParam{kAllowWithoutWriteCapLabel, true, EPERM},
                                           SendTestParam{kAllowWithWriteCapLabel, true, 0}));

}  // namespace

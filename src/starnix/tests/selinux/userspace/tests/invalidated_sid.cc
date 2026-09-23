// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

#include <cstddef>
#include <cstdint>
#include <cstring>
#include <string>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

#include "src/lib/fxl/strings/string_printf.h"
#include "src/starnix/tests/selinux/userspace/util.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace {

constexpr char kInitialDomainContext[] = "test_u:test_r:test_invalidated_domain_t:s0";
constexpr char kUnlabeledContext[] = "unlabeled_u:unlabeled_r:unlabeled_t:s0";

enum class HandshakeMessage : uint8_t {
  kConnected = 1,
  kReloaded = 2,
  kVerified = 3,
};

fit::result<int, std::string> ReadProcAttr(pid_t pid, std::string_view attr_name) {
  auto attr = ReadFile(fxl::StringPrintf("/proc/%d/attr/%s", pid, std::string(attr_name).c_str()));
  if (attr.is_error()) {
    return attr;
  }
  return RemoveTrailingNul(attr.value());
}

}  // namespace

constexpr char kAbstractSocketName[] = "\0invalidated_sid_test_socket";
constexpr socklen_t kAbstractSocketAddressLength =
    offsetof(struct sockaddr_un, sun_path) + sizeof(kAbstractSocketName) - 1;

// The abstract UNIX domain stream socket serves a dual purpose in this test:
// 1. It acts as the IPC coordination channel to sequence events between the
//    parent and child processes (connection, policy reload, and verification).
// 2. The connected endpoint (`accepted_socket_`) is retained across the policy
//    reload so that `SO_PEERSEC` queries on it can be verified before and after.
class InvalidatedSidTestSuite : public ::testing::Test {
 public:
  static void SetUpTestSuite() {
    sockaddr_un addr{.sun_family = AF_UNIX};
    memcpy(addr.sun_path, kAbstractSocketName, sizeof(kAbstractSocketName));

    // 1. Create listening abstract UNIX domain stream socket.
    int listen_raw_fd;
    ASSERT_THAT(listen_raw_fd = socket(AF_UNIX, SOCK_STREAM, 0), SyscallSucceeds());
    fbl::unique_fd listen_fd(listen_raw_fd);

    ASSERT_THAT(bind(listen_fd.get(), reinterpret_cast<const sockaddr*>(&addr),
                     kAbstractSocketAddressLength),
                SyscallSucceeds());
    ASSERT_THAT(listen(listen_fd.get(), 1), SyscallSucceeds());

    // 2. Fork child process under the initial domain.
    fork_helper_.OnlyWaitForForkedChildren();
    child_pid_ = RunInForkedProcessWithLabel(fork_helper_, kInitialDomainContext, [addr]() {
      // Step A: Create socket and connect to parent.
      int client_raw_fd;
      EXPECT_THAT(client_raw_fd = socket(AF_UNIX, SOCK_STREAM, 0), SyscallSucceeds());
      fbl::unique_fd client_socket(client_raw_fd);

      EXPECT_THAT(connect(client_socket.get(), reinterpret_cast<const sockaddr*>(&addr),
                          kAbstractSocketAddressLength),
                  SyscallSucceeds());

      // Step B: Signal parent that connection is established.
      HandshakeMessage msg = HandshakeMessage::kConnected;
      EXPECT_THAT(write(client_socket.get(), &msg, sizeof(msg)),
                  SyscallSucceedsWithValue(sizeof(msg)));

      // Step C: Wait for parent to reload policy.
      EXPECT_THAT(read(client_socket.get(), &msg, sizeof(msg)),
                  SyscallSucceedsWithValue(sizeof(msg)));
      EXPECT_EQ(msg, HandshakeMessage::kReloaded);

      // Step D: Verify our own task context fell back to unlabeled.
      EXPECT_THAT(ReadTaskAttr("current"), SyscallResultIsOk(kUnlabeledContext));

      // Step E: Signal parent that verification succeeded.
      msg = HandshakeMessage::kVerified;
      EXPECT_THAT(write(client_socket.get(), &msg, sizeof(msg)),
                  SyscallSucceedsWithValue(sizeof(msg)));

      // Step F: Wait for parent to shut down the socket to release us.
      char buf[1];
      EXPECT_THAT(read(client_socket.get(), buf, sizeof(buf)), SyscallSucceedsWithValue(0));
    });

    // 3. Accept connection from child and verify initial peer security context.
    int accepted_raw_fd;
    ASSERT_THAT(accepted_raw_fd = accept(listen_fd.get(), nullptr, nullptr), SyscallSucceeds());
    accepted_socket_.reset(accepted_raw_fd);

    HandshakeMessage msg;
    ASSERT_THAT(read(accepted_socket_.get(), &msg, sizeof(msg)),
                SyscallSucceedsWithValue(sizeof(msg)));
    ASSERT_EQ(msg, HandshakeMessage::kConnected);
    EXPECT_THAT(GetPeerSec(accepted_socket_.get()), SyscallResultIsOk(kInitialDomainContext));
    EXPECT_THAT(ReadProcAttr(child_pid_, "current"), SyscallResultIsOk(kInitialDomainContext));

    // 4. Reload policy (which omits test_invalidated_domain_t).
    auto policy_bytes = ReadFile("data/policies/invalidated_sid_reloaded_policy");
    ASSERT_THAT(policy_bytes, SyscallResultIsOk());
    EXPECT_THAT(WriteExistingFile("/sys/fs/selinux/load", policy_bytes.value()),
                SyscallResultIsOk());

    // 5. Signal child that policy has been reloaded and wait for child verification.
    msg = HandshakeMessage::kReloaded;
    ASSERT_THAT(write(accepted_socket_.get(), &msg, sizeof(msg)),
                SyscallSucceedsWithValue(sizeof(msg)));

    ASSERT_THAT(read(accepted_socket_.get(), &msg, sizeof(msg)),
                SyscallSucceedsWithValue(sizeof(msg)));
    ASSERT_EQ(msg, HandshakeMessage::kVerified);
  }

  static void TearDownTestSuite() {
    accepted_socket_.reset();
    EXPECT_TRUE(fork_helper_.WaitForChildren());
  }

 protected:
  static pid_t child_pid() { return child_pid_; }
  static int accepted_socket() { return accepted_socket_.get(); }

 private:
  static test_helper::ForkHelper fork_helper_;
  static pid_t child_pid_;
  static fbl::unique_fd accepted_socket_;
};

test_helper::ForkHelper InvalidatedSidTestSuite::fork_helper_;
pid_t InvalidatedSidTestSuite::child_pid_ = 0;
fbl::unique_fd InvalidatedSidTestSuite::accepted_socket_;

extern std::string DoPrePolicyLoadWork() { return "invalidated_sid_initial_policy"; }

TEST_F(InvalidatedSidTestSuite, TaskContextFallsBackToUnlabeled) {
  EXPECT_THAT(ReadProcAttr(child_pid(), "current"), SyscallResultIsOk(kUnlabeledContext));
}

TEST_F(InvalidatedSidTestSuite, SocketPeerContextFallsBackToUnlabeled) {
  EXPECT_THAT(GetPeerSec(accepted_socket()), SyscallResultIsOk(kUnlabeledContext));
}

// Copyright 2025 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <lib/fit/defer.h>
#include <lib/fit/function.h>
#include <lib/stdcompat/string_view.h>
#include <sys/ioctl.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <unistd.h>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

#include "src/starnix/tests/selinux/userspace/util.h"
#include "src/starnix/tests/syscalls/cpp/binder/common.h"
#include "src/starnix/tests/syscalls/cpp/binder/manager_provider_client_test.h"
#include "src/starnix/tests/syscalls/cpp/binder_helper.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

extern std::string DoPrePolicyLoadWork() { return "binder_policy"; }

namespace starnix_binder {

namespace {

/* The smallest possible value for which this test passes; otherwise arbitrary. */
constexpr size_t kReadBufferSize = 80;

// Makes the current process register itself as a Binder context manager,
// then reads in an infinite loop any messages and transactions that arrive.
// Incoming transactions are given empty replies but if future tests need more
// sophisticated behavior that will be fine too.
void ContextManagerLoop(std::string_view dir, fit::closure ready) {
  auto fd_and_mapping = OpenBinderAndMap(dir);

  // Register itself as the context manager
  ASSERT_THAT(ioctl(fd_and_mapping.fd_.get(), BINDER_SET_CONTEXT_MGR, 0), SyscallSucceeds());

  // Enter looper
  {
    EnterLooperWriteBuffer enter_looper_write_buffer = {
        .command = BC_ENTER_LOOPER,
    };
    struct binder_write_read enter_looper_write_read = {
        .write_size = sizeof(enter_looper_write_buffer),
        .write_consumed = 0,
        .write_buffer = (binder_uintptr_t)&enter_looper_write_buffer,
    };
    ASSERT_THAT(ioctl(fd_and_mapping.fd_.get(), BINDER_WRITE_READ, &enter_looper_write_read),
                SyscallSucceeds());
  }

  ready();

  while (true) {
    std::array<uint8_t, kReadBufferSize> read_buffer = {};
    struct binder_write_read write_read = {
        .read_size = sizeof(read_buffer),
        .read_consumed = 0,
        .read_buffer = (binder_uintptr_t)read_buffer.data(),
    };
    ASSERT_THAT(ioctl(fd_and_mapping.fd_.get(), BINDER_WRITE_READ, &write_read), SyscallSucceeds());

    binder_uintptr_t cursor = (binder_uintptr_t)read_buffer.data();
    binder_uintptr_t limit = cursor + write_read.read_consumed;
    while (cursor < limit) {
      binder_driver_return_protocol returned = *(binder_driver_return_protocol*)(cursor);
      cursor += sizeof(binder_driver_return_protocol);

      switch (returned) {
        case BR_NOOP:
        case BR_TRANSACTION_COMPLETE:
          break;
        case BR_INCREFS:
        case BR_ACQUIRE:
        case BR_RELEASE:
        case BR_DECREFS:
          cursor += sizeof(struct binder_ptr_cookie);
          break;
        case BR_TRANSACTION: {
          struct binder_transaction_data& transaction_data =
              *(struct binder_transaction_data*)cursor;

          ReplyWriteBuffer reply_write_buffer = {.command = BC_REPLY,
                                                 .data = {
                                                     .target = {.ptr = 0},
                                                     .cookie = transaction_data.cookie,
                                                     .code = transaction_data.code,
                                                 }};
          struct binder_write_read reply_write_read = {
              .write_size = sizeof(reply_write_buffer),
              .write_consumed = 0,
              .write_buffer = (binder_uintptr_t)&reply_write_buffer,
          };

          ASSERT_THAT(ioctl(fd_and_mapping.fd_.get(), BINDER_WRITE_READ, &reply_write_read),
                      SyscallSucceeds());

          cursor += sizeof(transaction_data);
          break;
        }
        case BR_REPLY:
        case BR_DEAD_BINDER:
        case BR_FAILED_REPLY:
        case BR_DEAD_REPLY:
        case BR_ERROR:
        default:
          FAIL() << "Unexpected \"returned\" value " << returned << "!";
      }
    }
  }
}

// Starts a context manager process.
// Returns a value that on destruction kills the process.
auto ScopedContextManagerProcess(std::string_view dir, fit::closure ready) {
  auto fork_helper = std::make_unique<test_helper::ForkHelper>();
  pid_t pid = RunInForkedProcessWithLabel(
      *fork_helper, "test_u:test_r:binder_context_manager_test_t:s0",
      [dir, ready = std::move(ready)]() mutable { ContextManagerLoop(dir, std::move(ready)); });
  auto cleanup = fit::defer([pid, fork_helper = std::move(fork_helper)]() {
    ASSERT_THAT(kill(pid, SIGKILL), SyscallSucceeds());
    fork_helper->ExpectSignal(SIGKILL);
    fork_helper->OnlyWaitForForkedChildren();
    ASSERT_TRUE(fork_helper->WaitForChildren());
  });
  return cleanup;
}

class BinderTest : public ::testing::Test {
 protected:
  void SetUp() override {
    ASSERT_THAT(mount(nullptr, temp_dir_.path().c_str(), "binder", 0, nullptr), SyscallSucceeds());
  }

  void TearDown() override {
    // TODO: https://fxbug.dev/443944960 - `ASSERT_THAT` this `umount` `SyscallSucceeds()`.
    umount(temp_dir_.path().c_str());
  }

  test_helper::ScopedTempDir temp_dir_;
};

// Test opening binder from the default domain.
TEST_F(BinderTest, OpenBinderNoTestDomain) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  fbl::unique_fd binder = OpenBinder(temp_dir_.path());
  EXPECT_TRUE(binder) << strerror(errno);
}

// Test opening binder from the test domain.
TEST_F(BinderTest, OpenBinderWithTestDomain) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:binder_open_test_t:s0", [&] {
    fbl::unique_fd binder = OpenBinder(temp_dir_.path());
    EXPECT_TRUE(binder) << strerror(errno);
  }));
}

class ContextManagerPermission : public BinderTest,
                                 public testing::WithParamInterface<std::string_view> {};

// Test becoming the service manager with and without the `set_context_mgr` permission.
TEST_P(ContextManagerPermission, BecomeServiceManager) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  const auto label = ContextManagerPermission::GetParam();
  bool expect_success = !cpp23::contains(label, "_no_");
  ASSERT_TRUE(RunSubprocessAs(label, [&] {
    fbl::unique_fd binder = OpenBinder(temp_dir_.path());
    EXPECT_TRUE(binder) << strerror(errno);
    if (expect_success) {
      EXPECT_THAT(ioctl(binder.get(), BINDER_SET_CONTEXT_MGR, 0), SyscallSucceeds());
    } else {
      EXPECT_THAT(ioctl(binder.get(), BINDER_SET_CONTEXT_MGR, 0), SyscallFailsWithErrno(EACCES));
    }
  }));
}

const auto kContextManagerPermissionValues =
    ::testing::Values("test_u:test_r:binder_context_manager_test_t:s0",
                      "test_u:test_r:binder_no_context_manager_test_t:s0");
INSTANTIATE_TEST_SUITE_P(BinderTest, ContextManagerPermission, kContextManagerPermissionValues);

class CallPermission : public BinderTest, public testing::WithParamInterface<std::string_view> {};

// Test doing a binder call with and without the `call` permission.
TEST_P(CallPermission, DoCall) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  std::string_view label = CallPermission::GetParam();
  bool expect_success = !cpp23::contains(label, "_no_");

  test_helper::Rendezvous context_manager_ready = test_helper::MakeRendezvous();
  auto context_manager = ScopedContextManagerProcess(
      temp_dir_.path(),
      [ready = std::move(context_manager_ready.poker)]() mutable { ready.poke(); });
  context_manager_ready.holder.hold();

  ASSERT_TRUE(RunSubprocessAs(label, [&] {
    fbl::unique_fd binder = OpenBinder(temp_dir_.path());
    ASSERT_TRUE(binder) << strerror(errno);

    auto mapping = test_helper::ScopedMMap::MMap(nullptr, kBinderMMapSize, PROT_READ, MAP_PRIVATE,
                                                 binder.get(), 0);
    ASSERT_TRUE(mapping.is_ok()) << mapping.error_value();

    TransactionWriteBuffer transaction_payload = {
        .command = BC_TRANSACTION,
        .data =
            {
                .target =
                    {
                        .handle = kServiceManagerHandle,
                    },
            },
    };

    struct binder_write_read payload = {};
    payload.write_buffer = (binder_uintptr_t)&transaction_payload;
    payload.write_size = sizeof(TransactionWriteBuffer);

    std::array<uint8_t, kReadBufferSize> read_buffer = {};
    payload.read_size = sizeof(read_buffer);
    payload.read_buffer = (binder_uintptr_t)read_buffer.data();
    payload.read_consumed = 0;

    ASSERT_THAT(ioctl(binder.get(), BINDER_WRITE_READ, &payload), SyscallSucceeds());
    ParsedMessage message =
        ParseMessage((binder_uintptr_t)read_buffer.data(), payload.read_consumed);
    if (expect_success) {
      ASSERT_THAT(message.returns_, ::testing::ElementsAre(BR_NOOP, BR_TRANSACTION_COMPLETE));
    } else {
      ASSERT_THAT(message.returns_, ::testing::ElementsAre(BR_NOOP, BR_FAILED_REPLY));
    }
  }));
}

const auto kCallPermissionValues = ::testing::Values(
    "test_u:test_r:binder_ioctl_call_test_t:s0", "test_u:test_r:binder_ioctl_no_call_test_t:s0");
INSTANTIATE_TEST_SUITE_P(BinderTest, CallPermission, kCallPermissionValues);

class Impersonation
    : public BinderTest,
      public testing::WithParamInterface<std::tuple<std::string_view, std::string_view>> {};

TEST_P(Impersonation, ImpersonateViaTransition) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  auto& [pretransition_label, posttransition_label] = Impersonation::GetParam();
  bool expect_success = cpp23::contains(pretransition_label, "_allow_") &&
                        cpp23::contains(posttransition_label, "_allow_");

  test_helper::Rendezvous context_manager_ready = test_helper::MakeRendezvous();

  std::unique_ptr<test_helper::ForkHelper> context_manager_fork_helper =
      std::make_unique<test_helper::ForkHelper>();
  context_manager_fork_helper->OnlyWaitForForkedChildren();
  pid_t context_manager_pid = RunInForkedProcessWithLabel(
      *context_manager_fork_helper, "test_u:test_r:binder_context_manager_t:s0",
      [&, ready = std::move(context_manager_ready.poker)]() mutable {
        ContextManagerLoop(temp_dir_.path(),
                           [ready = std::move(ready)]() mutable { ready.poke(); });
      });
  auto context_manager =
      fit::defer([context_manager_pid, fork_helper = std::move(context_manager_fork_helper)]() {
        ASSERT_THAT(kill(context_manager_pid, SIGKILL), SyscallSucceeds());
        fork_helper->ExpectSignal(SIGKILL);
        ASSERT_TRUE(fork_helper->WaitForChildren());
      });

  context_manager_ready.holder.hold();

  std::unique_ptr<test_helper::ForkHelper> transactor_fork_helper =
      std::make_unique<test_helper::ForkHelper>();

  RunInForkedProcessWithLabel(*transactor_fork_helper, pretransition_label, [&]() {
    auto fd_and_mapping = OpenBinderAndMap(temp_dir_.path());

    auto transition_result = WriteTaskAttr("current", posttransition_label);
    ASSERT_TRUE(transition_result.is_ok()) << "Failed to transition to \"" << posttransition_label
                                           << "\" with error " << transition_result.error_value();

    std::string_view hello = "Hello!";
    TransactionWriteBuffer hello_write_buffer = {
        .command = BC_TRANSACTION,
        .data =
            {
                .target =
                    {
                        .handle = kServiceManagerHandle,
                    },
                .cookie = 0,
                .code = 0,
                .flags = 0,
                .data_size = hello.size(),
                .offsets_size = 0,
                .data =
                    {
                        .ptr =
                            {
                                .buffer = (binder_uintptr_t)hello.data(),
                                .offsets = (binder_uintptr_t) nullptr,
                            },
                    },
            },
    };
    struct binder_write_read hello_write_read = {
        .write_size = sizeof(hello_write_buffer),
        .write_consumed = 0,
        .write_buffer = (binder_uintptr_t)&hello_write_buffer,
    };

    ASSERT_THAT(ioctl(fd_and_mapping.fd_.get(), BINDER_WRITE_READ, &hello_write_read),
                SyscallSucceeds());

    bool br_reply_observed = false;
    bool br_transaction_complete_observed = false;
    bool br_failed_reply_observed = false;
    auto transaction_succeeded = [&] {
      return br_reply_observed &&
             // TODO: https://fxbug.dev/443721582 - why doesn't this process observe a
             // BR_TRANSACTION_COMPLETE when run with Starnix? It seems to see one when run with
             // Linux?
             (br_transaction_complete_observed || test_helper::IsStarnix());
    };
    auto transaction_failed = [&] { return br_failed_reply_observed; };

    while (!transaction_failed() && !transaction_succeeded()) {
      std::array<uint8_t, kReadBufferSize> read_buffer = {};
      struct binder_write_read drain_write_read = {
          .read_size = sizeof(read_buffer),
          .read_consumed = 0,
          .read_buffer = (binder_uintptr_t)read_buffer.data(),
      };
      ASSERT_THAT(ioctl(fd_and_mapping.fd_.get(), BINDER_WRITE_READ, &drain_write_read),
                  SyscallSucceeds());

      binder_uintptr_t cursor = (binder_uintptr_t)read_buffer.data();
      binder_uintptr_t limit = cursor + drain_write_read.read_consumed;
      while (cursor < limit) {
        binder_driver_return_protocol returned = *(binder_driver_return_protocol*)(cursor);
        cursor += sizeof(binder_driver_return_protocol);

        switch (returned) {
          case BR_NOOP:
            break;
          case BR_TRANSACTION_COMPLETE:
            br_transaction_complete_observed = true;
            break;
          case BR_INCREFS:
          case BR_ACQUIRE:
          case BR_RELEASE:
          case BR_DECREFS:
            cursor += sizeof(struct binder_ptr_cookie);
            break;
          case BR_TRANSACTION:
            cursor += sizeof(struct binder_transaction_data);
            break;
          case BR_REPLY:
            br_reply_observed = true;
            cursor += sizeof(struct binder_transaction_data);
            break;
          case BR_FAILED_REPLY:
            br_failed_reply_observed = true;
            break;
          case BR_DEAD_BINDER:
          case BR_DEAD_REPLY:
          case BR_ERROR:
          default:
            FAIL() << "Unexpected \"returned\" value " << returned << "!";
        }
      }
    }
    ASSERT_TRUE(expect_success ? br_reply_observed : br_failed_reply_observed);
  });
  transactor_fork_helper->OnlyWaitForForkedChildren();
}

// TODO: https://fxbug.dev/443684136 - add verification that for the "_deny_" test executions, the
// "denied" lines emitted to the audit log correspond across SELinux and SEStarnix.
const auto kImpersonatePermissionValues = ::testing::Combine(
    ::testing::Values("test_u:test_r:binder_impersonation_allow_call_pretransition_t:s0",
                      "test_u:test_r:binder_impersonation_deny_call_pretransition_t:s0"),
    ::testing::Values("test_u:test_r:binder_impersonation_allow_impersonate_posttransition_t:s0",
                      "test_u:test_r:binder_impersonation_deny_impersonate_posttransition_t:s0"));
INSTANTIATE_TEST_SUITE_P(BinderTest, Impersonation, kImpersonatePermissionValues);

// Verifies that `security_binder_transaction` evaluates permissions against the target's
// cached credentials recorded when `/dev/binder` was opened.
// Even if the target process transitions to a different security domain after opening
// `/dev/binder`, subsequent transactions to it continue to be authorized against the target's
// cached open-time domain.
TEST_F(BinderTest, TargetUsesCachedSid) {
  auto enforce = ScopedEnforcement::SetEnforcing();

  // The target process initially opens `/dev/binder` as `binder_context_manager_t`, sets itself as
  // context manager, and then transitions to `binder_service_provider_t`.
  // The caller runs as `binder_target_domain_transition_client_t`.
  // The policy allows the caller to call `binder_context_manager_t`, but NOT
  // `binder_service_provider_t`.
  const char* kTargetInitialLabel = "test_u:test_r:binder_context_manager_t:s0";
  const char* kTargetPostTransitionLabel = "test_u:test_r:binder_service_provider_t:s0";
  const char* kClientLabel = "test_u:test_r:binder_target_domain_transition_client_t:s0";

  test_helper::Rendezvous ready = test_helper::MakeRendezvous();
  test_helper::Rendezvous done = test_helper::MakeRendezvous();
  test_helper::ForkHelper target_fork_helper;

  RunInForkedProcessWithLabel(
      target_fork_helper, kTargetInitialLabel,
      [&, ready_poker = std::move(ready.poker), done_holder = std::move(done.holder)]() mutable {
        auto fd_and_mapping = OpenBinderAndMap(temp_dir_.path());
        ASSERT_THAT(ioctl(fd_and_mapping.fd_.get(), BINDER_SET_CONTEXT_MGR, 0), SyscallSucceeds());
        ASSERT_TRUE(WriteTaskAttr("current", kTargetPostTransitionLabel).is_ok());

        ready_poker.poke();
        done_holder.hold();
      });

  ready.holder.hold();
  auto cleanup = fit::defer([&] { done.poker.poke(); });

  ASSERT_TRUE(RunSubprocessAs(kClientLabel, [&] {
    auto fd_and_mapping = OpenBinderAndMap(temp_dir_.path());

    TransactionWriteBuffer transaction_payload = {
        .command = BC_TRANSACTION,
        .data =
            {
                .target =
                    {
                        .handle = kServiceManagerHandle,
                    },
                .flags = TF_ONE_WAY,
            },
    };

    struct binder_write_read payload = {};
    payload.write_buffer = (binder_uintptr_t)&transaction_payload;
    payload.write_size = sizeof(TransactionWriteBuffer);

    std::array<uint8_t, kReadBufferSize> read_buffer = {};
    payload.read_size = sizeof(read_buffer);
    payload.read_buffer = (binder_uintptr_t)read_buffer.data();

    ASSERT_THAT(ioctl(fd_and_mapping.fd_.get(), BINDER_WRITE_READ, &payload), SyscallSucceeds());
    ParsedMessage message =
        ParseMessage((binder_uintptr_t)read_buffer.data(), payload.read_consumed);

    // The transaction succeeds because authorization uses the target's cached open-time
    // domain (`binder_context_manager_t`), which the client has permission to call.
    EXPECT_THAT(message.returns_, ::testing::Contains(BR_TRANSACTION_COMPLETE));
  }));
}

}  // namespace

class WithSEStarnix : public WithOrWithoutSEStarnix {
 public:
  pid_t SpawnManager(test_helper::ForkHelper& fork_helper, fit::closure manager_behavior) override {
    return RunInForkedProcessWithLabel(fork_helper, "test_u:test_r:binder_context_manager_t:s0",
                                       std::move(manager_behavior));
  }
  pid_t SpawnProvider(test_helper::ForkHelper& fork_helper,
                      fit::closure provider_behavior) override {
    return RunInForkedProcessWithLabel(fork_helper, "test_u:test_r:binder_service_provider_t:s0",
                                       std::move(provider_behavior));
  }
  pid_t SpawnClient(test_helper::ForkHelper& fork_helper, fit::closure client_behavior) override {
    return RunInForkedProcessWithLabel(fork_helper, "test_u:test_r:binder_service_client_t:s0",
                                       std::move(client_behavior));
  }
  void ValidateClientSecctxSeenByProvider(std::string_view secctx) override {
    // TODO(nathaniel): SELinux Test Suite uses functions from selinux/context.h for this
    // comparison; do we want to be that orthodox?
    ASSERT_STREQ(std::string(secctx).data(), "test_u:test_r:binder_service_client_t:s0");
  }
  bool SkipEntirely() override { return false; }

 private:
  ScopedEnforcement enforcing_ = ScopedEnforcement::SetEnforcing();
};

INSTANTIATE_TYPED_TEST_SUITE_P(BinderTest, ManagerProviderClientTest, WithSEStarnix);

}  // namespace starnix_binder

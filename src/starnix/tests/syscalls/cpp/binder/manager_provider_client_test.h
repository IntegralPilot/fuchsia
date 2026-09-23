// Copyright 2025 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STARNIX_TESTS_SYSCALLS_CPP_BINDER_MANAGER_PROVIDER_CLIENT_TEST_H_
#define SRC_STARNIX_TESTS_SYSCALLS_CPP_BINDER_MANAGER_PROVIDER_CLIENT_TEST_H_

#include <sys/mount.h>

#include <utility>

#include <gtest/gtest.h>

#include "src/starnix/tests/syscalls/cpp/binder/client.h"
#include "src/starnix/tests/syscalls/cpp/binder/manager.h"
#include "src/starnix/tests/syscalls/cpp/binder/provider.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace starnix_binder {

// Helper interface for running this suite's tests either with SEStarnix or
// without (so, as just ordinary Starnix).
class WithOrWithoutSEStarnix {
  // Run the given behavior as a separate process with the manager role.
  virtual pid_t SpawnManager(test_helper::ForkHelper& fork_helper,
                             fit::closure manager_behavior) = 0;

  // Run the given behavior as a separate process with the provider role.
  virtual pid_t SpawnProvider(test_helper::ForkHelper& fork_helper,
                              fit::closure provider_behavior) = 0;

  // Run the given behavior as a separate process with the client role.
  virtual pid_t SpawnClient(test_helper::ForkHelper& fork_helper, fit::closure client_behavior) = 0;

  // Validate on the provider the security information passed regarding the client.
  virtual void ValidateClientSecctxSeenByProvider(std::string_view secctx) = 0;

  // TODO: https://fxbug.dev/317285180 - Drop this; we don't want it.
  virtual bool SkipEntirely() = 0;
};

// A test that runs with three intercommunicating subprocesses: a context manager, a service
// provider, and a client.
template <typename T>
class ManagerProviderClientTest : public testing::Test {
 protected:
  void SameOperationsAsSELinuxTestSuiteBinderTestSubTestOne();
};

TYPED_TEST_SUITE_P(ManagerProviderClientTest);

TYPED_TEST_P(ManagerProviderClientTest, SameOperationsAsSELinuxTestSuiteBinderTestSubTestOne) {
  TypeParam with_or_without_se = {};

  if (with_or_without_se.SkipEntirely()) {
    GTEST_SKIP() << "needs to run where Binder is available";
  }

  test_helper::ScopedTempDir temp_dir_ = {};
  ASSERT_THAT(mount(nullptr, temp_dir_.path().c_str(), "binder", 0, nullptr), SyscallSucceeds());

  {
    test_helper::Rendezvous manager_ready = test_helper::MakeRendezvous();
    auto manager = ManagerProcess(
        temp_dir_.path(),
        [&with_or_without_se](test_helper::ForkHelper& fork_helper, fit::closure manager_behavior) {
          return with_or_without_se.SpawnManager(fork_helper, std::move(manager_behavior));
        },
        std::move(manager_ready.poker));

    manager_ready.holder.hold();

    test_helper::Rendezvous provider_ready = test_helper::MakeRendezvous();
    auto provider = ProviderProcess(
        temp_dir_.path(),
        [&with_or_without_se](test_helper::ForkHelper& fork_helper,
                              fit::closure provider_behavior) {
          return with_or_without_se.SpawnProvider(fork_helper, std::move(provider_behavior));
        },
        std::move(provider_ready.poker),
        [&with_or_without_se](std::string_view secctx) {
          with_or_without_se.ValidateClientSecctxSeenByProvider(secctx);
        });

    provider_ready.holder.hold();

    test_helper::Rendezvous client_completed = test_helper::MakeRendezvous();
    auto client = ClientProcess(
        temp_dir_.path(),
        [&with_or_without_se](test_helper::ForkHelper& fork_helper, fit::closure client_behavior) {
          with_or_without_se.SpawnClient(fork_helper, std::move(client_behavior));
        },
        std::move(client_completed.poker));

    client_completed.holder.hold();
  }

  // TODO: https://fxbug.dev/443944960 - `ASSERT_THAT` this `umount` `SyscallSucceeds()`.
  umount(temp_dir_.path().c_str());
}

REGISTER_TYPED_TEST_SUITE_P(ManagerProviderClientTest,
                            SameOperationsAsSELinuxTestSuiteBinderTestSubTestOne);

}  // namespace starnix_binder

#endif  // SRC_STARNIX_TESTS_SYSCALLS_CPP_BINDER_MANAGER_PROVIDER_CLIENT_TEST_H_

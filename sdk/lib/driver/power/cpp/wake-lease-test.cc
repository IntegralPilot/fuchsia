// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async/cpp/task.h>
#include <lib/async/default.h>
#include <lib/driver/power/cpp/wake-lease.h>
#include <lib/fit/function.h>
#include <lib/fpromise/single_threaded_executor.h>
#include <lib/inspect/cpp/inspector.h>
#include <lib/inspect/cpp/reader.h>
#include <lib/inspect/cpp/vmo/types.h>
#include <lib/zx/clock.h>
#include <lib/zx/eventpair.h>
#include <lib/zx/time.h>
#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/syscalls/object.h>
#include <zircon/time.h>

#include <cstddef>

#include <fbl/ref_ptr.h>
#include <gtest/gtest.h>
#include <src/lib/testing/loop_fixture/real_loop_fixture.h>
#include <src/storage/lib/vfs/cpp/service.h>
#include <src/storage/lib/vfs/cpp/synchronous_vfs.h>

#include "testing-common.h"

namespace power_lib_test {

class WakeLeaseTest : public gtest::RealLoopFixture {};

namespace {

// Prepares the resources needed to run the fake SAG server.
void PrepFakeSag(
    fbl::RefPtr<fs::Service>& sag,
    std::shared_ptr<fidl::ServerBindingGroup<fuchsia_power_system::ActivityGovernor>>& bindings,
    async::Loop& loop, std::shared_ptr<SystemActivityGovernor>& sag_server) {
  zx::event exec_opportunistic, wake_assertive;
  zx::event::create(0, &exec_opportunistic);
  zx::event::create(0, &wake_assertive);
  sag_server =
      std::make_shared<SystemActivityGovernor>(std::move(wake_assertive), loop.dispatcher());

  bindings = std::make_shared<fidl::ServerBindingGroup<fuchsia_power_system::ActivityGovernor>>();
  sag = fbl::MakeRefCounted<fs::Service>(
      [&](fidl::ServerEnd<fuchsia_power_system::ActivityGovernor> chan) {
        bindings->AddBinding(loop.dispatcher(), std::move(chan), sag_server.get(),
                             fidl::kIgnoreBindingClosure);
        return ZX_OK;
      });
}

// Waits for the ManualWakeLease to observe a suspension signal, then runs
// `do_after_suspend`.
void DoOperationAfterSuspend(
    const std::shared_ptr<fdf_power::ManualWakeLease>& op, async::Loop& loop,
    const std::function<void(const std::shared_ptr<fdf_power::ManualWakeLease>&, async::Loop&)>&
        do_after_suspend) {
  if (!op->IsSuspended()) {
    async::PostDelayedTask(
        loop.dispatcher(),
        [op, &loop, do_after_suspend]() { DoOperationAfterSuspend(op, loop, do_after_suspend); },
        zx::msec(100));
    return;
  };

  do_after_suspend(op, loop);
}

// Call |op->Start()| once |op| reports it considers the system resumed.
// After that, instruct |sag| to suspend. Concurrently call
// |DoOperationAfterSuspend|, passing |do_after_suspended| which means that
// |do_after_suspended| runs once |op| reports it considers the system
// suspended.
void StartOperationWhenResumedThenSuspend(
    const std::shared_ptr<fdf_power::ManualWakeLease>& op, async::Loop& client_loop,
    const std::shared_ptr<SystemActivityGovernor>& sag, async::Loop& server_loop,
    const std::function<void(std::shared_ptr<fdf_power::ManualWakeLease>, async::Loop&)>&
        do_after_suspended) {
  if (op->IsSuspended()) {
    async::PostDelayedTask(
        client_loop.dispatcher(),
        [op, &client_loop, sag, &server_loop, do_after_suspended]() {
          StartOperationWhenResumedThenSuspend(op, client_loop, sag, server_loop,
                                               do_after_suspended);
        },
        zx::msec(100));
    return;
  }

  EXPECT_TRUE(op->Start());
  EXPECT_TRUE(op->GetWakeLeaseCopy().is_error());
  async::PostTask(server_loop.dispatcher(), [sag]() { sag->SendBeforeSuspend(); });
  DoOperationAfterSuspend(op, client_loop, do_after_suspended);
}

void CheckLeaseAcquired(const std::shared_ptr<fdf_power::ManualWakeLease>& op, async::Loop& loop) {
  EXPECT_TRUE(op->End().is_ok());
  loop.Quit();
}

struct ExpectedInspectValues {
  uint64_t requests_over_threshold;
  uint64_t acquisitions_over_threshold;
  uint64_t total_acquisitions;
};

void CheckLeaseInspectData(const std::shared_ptr<inspect::Inspector>& inspector,
                           ExpectedInspectValues values) {
  fpromise::result<inspect::Hierarchy> hierarchy =
      fpromise::run_single_threaded(inspect::ReadFromInspector(*inspector));
  EXPECT_TRUE(hierarchy.is_ok());
  EXPECT_EQ(hierarchy.value()
                .node()
                .get_property<inspect::UintPropertyValue>("Lease acquisitions exceeding threshold")
                ->value(),
            values.acquisitions_over_threshold);
  EXPECT_EQ(hierarchy.value()
                .node()
                .get_property<inspect::UintPropertyValue>("Leases requested longer than threshold")
                ->value(),
            values.requests_over_threshold);
  EXPECT_EQ(hierarchy.value()
                .node()
                .get_property<inspect::UintPropertyValue>("Total Lease Acquisitions")
                ->value(),
            values.total_acquisitions);
}

// Run a `TimeoutWakeLease` test with a fake SAG where the fake SAG and the
// client run on their own threads. This creates a `TimeoutWakeLease` which is
// connected to the fake SAG. Once the fake SAG and lease are created,
// `test_operations` and `sag_operations` functions are called concurrently
// on the appropriate dispatcher. `test_operations` has access to the lease,
// fake_sag and their respective dispatchers so that it can run the test logic.
//
// It is expected that `test_operations` will quit the `client_loop` before
// it completes because the test infrastructure code waits on this loop before
// terminating.
template <typename WakeLease>
void DoWakeLeaseTest(
    const std::function<
        void(std::shared_ptr<WakeLease>, async::Loop&, std::shared_ptr<inspect::Inspector>,
             std::shared_ptr<SystemActivityGovernor>&, async::Loop&)>& test_operations,
    const std::function<void(std::shared_ptr<SystemActivityGovernor>)>& sag_operations,
    uint32_t long_duration_threshold_ms = 100) {
  async::Loop server_loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  server_loop.StartThread("server-loop");

  async::Loop client_loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  client_loop.StartThread("client-loop");

  // The server needs to outlive the client, so create references that exist
  // until after the client's work concludes. Later we'll make sure the shared
  // pointers are destroyed on teh server thread.
  std::shared_ptr<SystemActivityGovernor> sag_server;
  std::shared_ptr<fidl::ServerBindingGroup<fuchsia_power_system::ActivityGovernor>> bindings;
  fbl::RefPtr<fs::Service> sag;

  // Create a channel connected to client and server.
  fidl::Endpoints<fuchsia_power_system::ActivityGovernor> sag_endpoints =
      fidl::Endpoints<fuchsia_power_system::ActivityGovernor>::Create();

  std::shared_ptr<inspect::Inspector> inspector = std::make_shared<inspect::Inspector>();
  async::PostTask(server_loop.dispatcher(), [&client_loop, &server_loop, &sag_server, &bindings,
                                             &sag, &sag_endpoints, test_operations, sag_operations,
                                             &inspector, long_duration_threshold_ms]() mutable {
    // First create SAG and related entities.
    PrepFakeSag(sag, bindings, server_loop, sag_server);

    sag->ConnectService(sag_endpoints.server.TakeChannel());

    // Extract the channel from the client end, because passing a ClientEnd to
    // another thread causes problems with thread unsafe FIDL bindings.
    zx::channel client = sag_endpoints.client.TakeChannel();

    // Now that we've initialized the server, initialize the client side.
    async::PostTask(client_loop.dispatcher(), [&client_loop, &server_loop,
                                               client = std::move(client), &sag_server,
                                               test_operations, sag_operations, &inspector,
                                               long_duration_threshold_ms]() mutable {
      // Create the wake lease on the client's thread so the client
      // because the FIDL bindings are not threadsafe.
      std::shared_ptr<WakeLease> op = std::make_shared<WakeLease>(
          client_loop.dispatcher(), "test-operation",
          fidl::ClientEnd<fuchsia_power_system::ActivityGovernor>(std::move(client)),
          &inspector->GetRoot(), false, long_duration_threshold_ms);

      // Run whatever the test wants us to run on the server's thread.
      async::PostTask(server_loop.dispatcher(),
                      [&sag_server, sag_operations]() { sag_operations(sag_server); });

      // Run the function provided by the test code.
      async::PostTask(client_loop.dispatcher(),
                      [op, &server_loop, &client_loop, inspector, test_operations, &sag_server]() {
                        test_operations(op, client_loop, inspector, sag_server, server_loop);
                      });
    });
  });

  // The client will quit its loop after doing its work, so wait for it.
  client_loop.JoinThreads();

  // On the server thread, destroy the server objects.
  async::PostTask(server_loop.dispatcher(), [&sag_server, &bindings, &sag, &server_loop]() {
    // Destroy all the shared objects on the thread where they were created.
    sag.reset();
    bindings.reset();
    sag_server.reset();
    server_loop.Quit();
  });

  // Shut down the server thread.
  server_loop.JoinThreads();
}

}  // namespace

// Create an ManualWakeLease and allow it to observe a resume signal. Then
// check that no actual lease is taken.
TEST_F(WakeLeaseTest, TestManualWakeLeaseWhenResumed) {
  const std::function<void(const std::shared_ptr<fdf_power::ManualWakeLease>, async::Loop&,
                           const std::shared_ptr<inspect::Inspector>,
                           const std::shared_ptr<SystemActivityGovernor>&, async::Loop&)>
      test_func =
          [&test_func](const std::shared_ptr<fdf_power::ManualWakeLease> op, async::Loop& loop,
                       const std::shared_ptr<inspect::Inspector>& inspector,
                       const std::shared_ptr<SystemActivityGovernor>& sag, async::Loop& sag_loop) {
            if (op->IsSuspended()) {
              async::PostDelayedTask(
                  loop.dispatcher(),
                  [op, &loop, sag, inspector, &sag_loop, &test_func]() {
                    test_func(op, loop, inspector, sag, sag_loop);
                  },
                  zx::msec(100));
              return;
            }

            EXPECT_FALSE(op->GetWakeLeaseCopy().is_ok());
            loop.Quit();
          };

  DoWakeLeaseTest<fdf_power::ManualWakeLease>(
      test_func,
      [](const std::shared_ptr<SystemActivityGovernor>& sag) { sag->SendAfterResume(); });
}

// After the ManualWakeLease is created, have it observe a resume and then
// verify it works as expected when the operation starts and ends.
TEST_F(WakeLeaseTest, TestManualWakeLeaseStartAndEndAfterResumeIsObserved) {
  const std::function<void(const std::shared_ptr<fdf_power::ManualWakeLease>, async::Loop&,
                           const std::shared_ptr<inspect::Inspector>,
                           const std::shared_ptr<SystemActivityGovernor>&, async::Loop&)>
      test_func =
          [&test_func](const std::shared_ptr<fdf_power::ManualWakeLease> op, async::Loop& loop,
                       const std::shared_ptr<inspect::Inspector>& inspector,
                       const std::shared_ptr<SystemActivityGovernor>& sag, async::Loop& sag_loop) {
            // Wait for us to be in a resumed state so the atomic op obesrved the
            // system state change
            if (op->IsSuspended()) {
              async::PostDelayedTask(
                  loop.dispatcher(),
                  [op, &loop, inspector, sag, &sag_loop, &test_func]() {
                    test_func(op, loop, inspector, sag, sag_loop);
                  },
                  zx::msec(100));
              return;
            }

            // Since the system is resumed we expect no lease to be taken
            EXPECT_TRUE(op->Start());
            EXPECT_TRUE(op->GetWakeLeaseCopy().is_error());
            // Since the system was resumed teh whole time, there should be no
            // lease to return when the operation ends.
            EXPECT_TRUE(op->End().is_error());
            loop.Quit();
          };

  DoWakeLeaseTest<fdf_power::ManualWakeLease>(
      test_func,
      [](const std::shared_ptr<SystemActivityGovernor>& sag) { sag->SendAfterResume(); });
}

// Test ManualWakeLease when it starts while the system is suspended. Also
// check that duplicate `Start` calls result in taking only one lease.
TEST_F(WakeLeaseTest, TestManualWakeLeaseWhenSuspended) {
  std::function<void(const std::shared_ptr<fdf_power::ManualWakeLease>, async::Loop&,
                     const std::shared_ptr<inspect::Inspector>,
                     const std::shared_ptr<SystemActivityGovernor>&, async::Loop&)>
      test_func = [](const std::shared_ptr<fdf_power::ManualWakeLease> op, async::Loop& loop,
                     const std::shared_ptr<inspect::Inspector>& inspector,
                     const std::shared_ptr<SystemActivityGovernor>& sag, async::Loop& sag_loop) {
        EXPECT_TRUE(op->IsSuspended());
        EXPECT_TRUE(op->Start());
        EXPECT_TRUE(op->GetWakeLeaseCopy()->is_valid());
        EXPECT_TRUE(op->Start());
        EXPECT_TRUE(op->GetWakeLeaseCopy()->is_valid());
        loop.Quit();
      };
  DoWakeLeaseTest<fdf_power::ManualWakeLease>(
      test_func, [](const std::shared_ptr<SystemActivityGovernor>& sag) {});
}

// Checks that when we are suspended we can start an ManualWakeLease and then
// end it without error.
TEST_F(WakeLeaseTest, TestManualWakeLeaseStartAndEndWhileSuspended) {
  std::function<void(const std::shared_ptr<fdf_power::ManualWakeLease>, async::Loop&,
                     const std::shared_ptr<inspect::Inspector>,
                     const std::shared_ptr<SystemActivityGovernor>&, async::Loop&)>
      test_func = [](const std::shared_ptr<fdf_power::ManualWakeLease> op, async::Loop& loop,
                     const std::shared_ptr<inspect::Inspector>& inspector,
                     const std::shared_ptr<SystemActivityGovernor>& sag, async::Loop& sag_loop) {
        EXPECT_TRUE(op->IsSuspended());
        EXPECT_TRUE(op->Start());
        EXPECT_TRUE(op->End().is_ok());
        loop.Quit();
      };

  DoWakeLeaseTest<fdf_power::ManualWakeLease>(
      test_func, [](const std::shared_ptr<SystemActivityGovernor>& sag) {});
}

// Test that inspect data for long durations changes as expected. This runs through various
// lease starts and ends when the lease object things the system is suspended and resumed and for
// durations that are above or below the duration threshold.
TEST_F(WakeLeaseTest, TestInspectBasicData) {
  uint32_t long_sleep_us = 200 * 1000;
  uint32_t short_sleep_us = long_sleep_us / 20;
  uint32_t long_duration_ms = (long_sleep_us / 2) / 1000;

  std::function<void(const std::shared_ptr<fdf_power::ManualWakeLease>,
                     std::shared_ptr<inspect::Inspector>, async::Loop&,
                     const std::shared_ptr<SystemActivityGovernor>&)>
      acquired_tester = [&acquired_tester, long_sleep_us, short_sleep_us](
                            const std::shared_ptr<fdf_power::ManualWakeLease>& lease,
                            const std::shared_ptr<inspect::Inspector>& inspect, async::Loop& loop,
                            const std::shared_ptr<SystemActivityGovernor>& sag) {
        if (!lease->IsSuspended()) {
          async::PostDelayedTask(
              loop.dispatcher(),
              [&acquired_tester, lease, inspect, &loop, sag]() {
                acquired_tester(lease, inspect, loop, sag);
              },
              zx::msec(10));
          return;
        }

        CheckLeaseInspectData(inspect, {.requests_over_threshold = 2,
                                        .acquisitions_over_threshold = 0,
                                        .total_acquisitions = 0});

        lease->Start();
        usleep(long_sleep_us);
        zx::result<zx::eventpair> ep = lease->End();

        CheckLeaseInspectData(inspect, {
                                           .requests_over_threshold = 3,
                                           .acquisitions_over_threshold = 1,
                                           .total_acquisitions = 1,
                                       });

        lease->Start();
        usleep(short_sleep_us);
        ep = lease->End();

        CheckLeaseInspectData(inspect, {
                                           .requests_over_threshold = 3,
                                           .acquisitions_over_threshold = 1,
                                           .total_acquisitions = 2,
                                       });

        lease->Start();
        usleep(long_sleep_us);
        ep = lease->End();
        CheckLeaseInspectData(inspect, {
                                           .requests_over_threshold = 4,
                                           .acquisitions_over_threshold = 2,
                                           .total_acquisitions = 3,
                                       });

        loop.Quit();
      };

  std::function<void(const std::shared_ptr<fdf_power::ManualWakeLease>,
                     std::shared_ptr<inspect::Inspector>, async::Loop&,
                     const std::shared_ptr<SystemActivityGovernor>&, async::Loop&)>
      request_tester = [&request_tester, &acquired_tester, long_sleep_us](
                           const std::shared_ptr<fdf_power::ManualWakeLease>& lease,
                           const std::shared_ptr<inspect::Inspector>& inspect, async::Loop& loop,
                           const std::shared_ptr<SystemActivityGovernor>& sag,
                           async::Loop& sag_loop) {
        if (lease->IsSuspended()) {
          async::PostDelayedTask(
              loop.dispatcher(),
              [&request_tester, lease, inspect, &loop, sag, &sag_loop]() {
                request_tester(lease, inspect, loop, sag, sag_loop);
              },
              zx::msec(50));
          return;
        }
        CheckLeaseInspectData(inspect, {
                                           .requests_over_threshold = 0,
                                           .acquisitions_over_threshold = 0,
                                           .total_acquisitions = 0,
                                       });

        lease->Start();
        usleep(long_sleep_us);
        zx::result<zx::eventpair> ep = lease->End();

        CheckLeaseInspectData(inspect, {
                                           .requests_over_threshold = 1,
                                           .acquisitions_over_threshold = 0,
                                           .total_acquisitions = 0,
                                       });

        lease->Start();
        usleep(long_sleep_us);
        ep = lease->End();
        CheckLeaseInspectData(inspect, {
                                           .requests_over_threshold = 2,
                                           .acquisitions_over_threshold = 0,
                                           .total_acquisitions = 0,
                                       });

        async::PostTask(sag_loop.dispatcher(), [sag]() { sag->SendBeforeSuspend(); });

        acquired_tester(lease, inspect, loop, sag);
      };

  std::function<void(const std::shared_ptr<fdf_power::ManualWakeLease>, async::Loop&,
                     const std::shared_ptr<inspect::Inspector>,
                     const std::shared_ptr<SystemActivityGovernor>&, async::Loop&)>
      test_fn = [&request_tester](
                    const std::shared_ptr<fdf_power::ManualWakeLease>& lease,
                    async::Loop& lease_loop, const std::shared_ptr<inspect::Inspector>& inspector,
                    const std::shared_ptr<SystemActivityGovernor>& sag, async::Loop& sag_loop) {
        request_tester(lease, inspector, lease_loop, sag, sag_loop);
      };

  DoWakeLeaseTest<fdf_power::ManualWakeLease>(
      test_fn, [](const std::shared_ptr<SystemActivityGovernor>& sag) { sag->SendAfterResume(); },
      long_duration_ms);
}

// Tests that inspect data updates as expected when a lease is started while suspended, held
// longer than the threshold, and then dropped after resume.
TEST_F(WakeLeaseTest, TestLongInspectAcrossSuspend) {
  uint32_t sleep_us = 200 * 1000;
  uint32_t long_duration_ms = (sleep_us / 2) / 1000;

  std::function<void(const std::shared_ptr<fdf_power::ManualWakeLease>, async::Loop&,
                     std::shared_ptr<inspect::Inspector>,
                     const std::shared_ptr<SystemActivityGovernor>&, async::Loop&)>
      post_resume =
          [sleep_us, &post_resume](
              const std::shared_ptr<fdf_power::ManualWakeLease>& lease, async::Loop& client_loop,
              const std::shared_ptr<inspect::Inspector>& inspect,
              const std::shared_ptr<SystemActivityGovernor>& sag, async::Loop& server_loop) {
            if (lease->IsSuspended()) {
              async::PostDelayedTask(
                  client_loop.dispatcher(),
                  [post_resume, lease, &client_loop, inspect, sag, &server_loop]() {
                    post_resume(lease, client_loop, inspect, sag, server_loop);
                  },
                  zx::msec(10));
              return;
            }

            usleep(sleep_us);
            zx::result<zx::eventpair> ep = lease->End();

            CheckLeaseInspectData(inspect, {
                                               .requests_over_threshold = 1,
                                               .acquisitions_over_threshold = 1,
                                               .total_acquisitions = 1,
                                           });

            client_loop.Quit();
          };

  std::function<void(const std::shared_ptr<fdf_power::ManualWakeLease>, async::Loop&,
                     std::shared_ptr<inspect::Inspector>,
                     const std::shared_ptr<SystemActivityGovernor>&, async::Loop&)>
      pre_resume = [&post_resume](const std::shared_ptr<fdf_power::ManualWakeLease>& lease,
                                  async::Loop& client_loop,
                                  std::shared_ptr<inspect::Inspector> inspect,
                                  const std::shared_ptr<SystemActivityGovernor>& sag,
                                  async::Loop& server_loop) {
        lease->Start();
        async::PostTask(server_loop.dispatcher(), [sag]() { sag->SendAfterResume(); });
        post_resume(lease, client_loop, std::move(inspect), sag, server_loop);
      };

  DoWakeLeaseTest<fdf_power::ManualWakeLease>(
      pre_resume, [](const std::shared_ptr<SystemActivityGovernor>& sag) {}, long_duration_ms);
}

// Tests that inspect data updates as expected when a lease held less time than the threshold
// is started while suspended and then dropped after resume.
TEST_F(WakeLeaseTest, TestShortInspectAcrossSuspend) {
  uint32_t sleep_us = 1000;                              // 1 ms
  uint32_t long_duration_ms = (sleep_us * 1000) / 1000;  // 1 sec

  std::function<void(const std::shared_ptr<fdf_power::ManualWakeLease>, async::Loop&,
                     std::shared_ptr<inspect::Inspector>,
                     const std::shared_ptr<SystemActivityGovernor>&, async::Loop&)>
      post_resume =
          [sleep_us, &post_resume](
              const std::shared_ptr<fdf_power::ManualWakeLease>& lease, async::Loop& client_loop,
              const std::shared_ptr<inspect::Inspector>& inspect,
              const std::shared_ptr<SystemActivityGovernor>& sag, async::Loop& server_loop) {
            if (lease->IsSuspended()) {
              async::PostDelayedTask(
                  client_loop.dispatcher(),
                  [post_resume, lease, &client_loop, inspect, sag, &server_loop]() {
                    post_resume(lease, client_loop, inspect, sag, server_loop);
                  },
                  zx::msec(10));
              return;
            }

            usleep(sleep_us);
            zx::result<zx::eventpair> ep = lease->End();

            CheckLeaseInspectData(inspect, {
                                               .requests_over_threshold = 0,
                                               .acquisitions_over_threshold = 0,
                                               .total_acquisitions = 1,
                                           });

            client_loop.Quit();
          };

  std::function<void(const std::shared_ptr<fdf_power::ManualWakeLease>, async::Loop&,
                     std::shared_ptr<inspect::Inspector>,
                     const std::shared_ptr<SystemActivityGovernor>&, async::Loop&)>
      pre_resume = [&post_resume](const std::shared_ptr<fdf_power::ManualWakeLease>& lease,
                                  async::Loop& client_loop,
                                  std::shared_ptr<inspect::Inspector> inspect,
                                  const std::shared_ptr<SystemActivityGovernor>& sag,
                                  async::Loop& server_loop) {
        lease->Start();
        async::PostTask(server_loop.dispatcher(), [sag]() { sag->SendAfterResume(); });
        post_resume(lease, client_loop, std::move(inspect), sag, server_loop);
      };

  DoWakeLeaseTest<fdf_power::ManualWakeLease>(
      pre_resume, [](const std::shared_ptr<SystemActivityGovernor>& sag) {}, long_duration_ms);
}

// Tests what happens happens when an ManualWakeLease observes a resume signal
// and then we start an ManualWakeLease. We expect no lease is taken. After
// verifying that, send a suspend signal which should trigger the
// ManualWakeLease to claim a wake lease. We then verify that a wake lease was
// actually taken.
TEST_F(WakeLeaseTest, TestManagedWakeLeaseWhenResumedThenSuspend) {
  async::Loop server_loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  server_loop.StartThread("server-loop");

  async::Loop client_loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  client_loop.StartThread("client-loop");

  // The server needs to outlive the client, so create references that exist
  // until after the client's work concludes
  std::shared_ptr<SystemActivityGovernor> sag_server;
  std::shared_ptr<fidl::ServerBindingGroup<fuchsia_power_system::ActivityGovernor>> bindings;
  fbl::RefPtr<fs::Service> sag;

  async::PostTask(server_loop.dispatcher(), [&client_loop, &server_loop, &sag_server, &bindings,
                                             &sag]() mutable {
    // First create SAG and related entities.
    PrepFakeSag(sag, bindings, server_loop, sag_server);

    // Create a channel connected to client and server.
    fidl::Endpoints<fuchsia_power_system::ActivityGovernor> sag_endpoints =
        fidl::Endpoints<fuchsia_power_system::ActivityGovernor>::Create();
    sag->ConnectService(sag_endpoints.server.TakeChannel());

    // Extract the channel from the client end, because passing a ClientEnd to
    // another thread causes problems with thread unsafe FIDL bindings.
    zx::channel client = sag_endpoints.client.TakeChannel();

    // Tell the client to do its work.
    async::PostTask(client_loop.dispatcher(), [&client_loop, &server_loop,
                                               client = std::move(client), &sag_server]() mutable {
      std::shared_ptr<fdf_power::ManualWakeLease> op = std::make_shared<fdf_power::ManualWakeLease>(
          client_loop.dispatcher(), "test-operation",
          fidl::ClientEnd<fuchsia_power_system::ActivityGovernor>(std::move(client)));

      // We want to test what ManualWakeLease does while the system is resumed,
      // so have the server send the resume event.
      async::PostTask(server_loop.dispatcher(), [&sag_server]() { sag_server->SendAfterResume(); });

      // Trigger the start of the atomic operation.
      async::PostTask(client_loop.dispatcher(), [op, &client_loop, &server_loop, &sag_server]() {
        StartOperationWhenResumedThenSuspend(op, client_loop, sag_server, server_loop,
                                             CheckLeaseAcquired);
      });
    });
  });

  // The client will quit its loop after doing its work, so wait for it.
  client_loop.JoinThreads();

  // On the server thread, destroy the server objects.
  async::PostTask(server_loop.dispatcher(), [&sag_server, &bindings, &sag, &server_loop]() {
    // Destroy all the shared objects on the thread where they were created.
    sag.reset();
    bindings.reset();
    sag_server.reset();
    server_loop.Quit();
  });

  // Shut down the server thread.
  server_loop.JoinThreads();
}

// Verifies that a WakeLeaseProvider and WakeLease
// manage underlying resources as expected including that new
// WakeLeases are created when appropriate and the underlying
// WakeLease object is preserved.
TEST_F(WakeLeaseTest, WakeLeaseProviderTest) {
  async::Loop server_loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  server_loop.StartThread("server-loop");

  async::Loop client_loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  client_loop.StartThread("client-loop");

  // The server needs to outlive the client, so create references that exist
  // until after the client's work concludes
  std::shared_ptr<SystemActivityGovernor> sag_server;
  std::shared_ptr<fidl::ServerBindingGroup<fuchsia_power_system::ActivityGovernor>> bindings;
  fbl::RefPtr<fs::Service> sag;

  async::PostTask(server_loop.dispatcher(), [&client_loop, &server_loop, &sag_server, &bindings,
                                             &sag]() mutable {
    // First create SAG and related entities.
    PrepFakeSag(sag, bindings, server_loop, sag_server);

    // Create a channel connected to client and server.
    fidl::Endpoints<fuchsia_power_system::ActivityGovernor> sag_endpoints =
        fidl::Endpoints<fuchsia_power_system::ActivityGovernor>::Create();
    sag->ConnectService(sag_endpoints.server.TakeChannel());

    // Extract the channel from the client end, because passing a ClientEnd to
    // another thread causes problems with thread unsafe FIDL bindings.
    zx::channel client = sag_endpoints.client.TakeChannel();

    // Tell the client to do its work.
    async::PostTask(client_loop.dispatcher(), [&client_loop, client = std::move(client)]() mutable {
      std::shared_ptr<fdf_power::WakeLeaseProvider> operation_provider =
          std::make_shared<fdf_power::WakeLeaseProvider>(
              client_loop.dispatcher(), "test-operation",
              fidl::ClientEnd<fuchsia_power_system::ActivityGovernor>(std::move(client)));

      std::shared_ptr<fdf_power::WakeLease> op1 = operation_provider->StartOperation();
      std::shared_ptr<fdf_power::WakeLease> op2 = operation_provider->StartOperation();

      EXPECT_EQ(op1, op2);

      // Get a raw pointer to the current WakeLease to check
      // against a different pointer later which will help us prove we dropped
      // the WakeLease after all strong pointers to it were
      // dropped.
      fdf_power::WakeLease* old_addr = op1.get();
      std::shared_ptr<fdf_power::ManualWakeLease> first_lease = op1->GetWakeLease();
      op1.reset();
      op2.reset();

      // It should be that we currently have no system wake lease since there
      // are no valid WakeLeases.
      EXPECT_TRUE(first_lease->GetWakeLeaseCopy().is_error());

      // Start a new operation, which should create a new
      // WakeLease.
      std::shared_ptr<fdf_power::WakeLease> op3 = operation_provider->StartOperation();
      EXPECT_NE(old_addr, op3.get());

      // The fdf_power::WakeLease should be the same, even though the
      // WakeLease changed.
      std::shared_ptr<fdf_power::ManualWakeLease> second_lease = op3->GetWakeLease();
      EXPECT_EQ(first_lease, second_lease);

      client_loop.Quit();
    });
  });

  // The client will quit its loop after doing its work, so wait for it.
  client_loop.JoinThreads();

  // On the server thread, destroy the server objects.
  async::PostTask(server_loop.dispatcher(), [&sag_server, &bindings, &sag, &server_loop]() {
    // Destroy all the shared objects on the thread where they were created.
    sag.reset();
    bindings.reset();
    sag_server.reset();
    server_loop.Quit();
  });

  // Shut down the server thread.
  server_loop.JoinThreads();
}

// Verify that an active TimeoutWakeLease doesn't take a system wake lease
// when we're resumed, but acquires a system wake lease across a
// resume->suspend transition:
//   * Create a TimeoutWakeLease
//   * Make the TimeoutWakeLease observe a resume signal
//   * Call HandleInterrupt
//   * Verify the TimeoutWakeLease has no system wake lease
//   * Make the TimeoutWakeLease observe a suspend
//   * Verify the TimeoutWakeLease now has a system wake lease
TEST_F(WakeLeaseTest, ActiveTimeoutWakeLeaseGetsLeaseOnSuspend) {
  // This function gets run AFTER the one defined below. It calls itself again
  // until the lease observes we're suspended.
  const std::function<void(const std::shared_ptr<fdf_power::TimeoutWakeLease>, async::Loop&)>
      run_after_suspend_observed =
          [&run_after_suspend_observed](const std::shared_ptr<fdf_power::TimeoutWakeLease> op,
                                        async::Loop& client_loop) {
            if (op->IsResumed()) {
              async::PostDelayedTask(
                  client_loop.dispatcher(),
                  [&run_after_suspend_observed, op, &client_loop]() {
                    run_after_suspend_observed(op, client_loop);
                  },
                  zx::msec(100));
              return;
            }

            // Should have acquired a wake lease on suspension.
            EXPECT_FALSE(op->TakeWakeLease().is_error());
            client_loop.Quit();
          };

  // This function is run after the test environment is set up. It waits until
  // the lease observes we've resumed. After that it uses HandleInterrupt,
  // which won't take a wake lease until we see a suspend. After that it
  // triggers a suspend and starts the run_after_suspend_observed function.
  const std::function<void(const std::shared_ptr<fdf_power::TimeoutWakeLease>, async::Loop&,
                           const std::shared_ptr<inspect::Inspector>,
                           const std::shared_ptr<SystemActivityGovernor>&, async::Loop&)>
      run_after_resume_observed =
          [&run_after_resume_observed, &run_after_suspend_observed](
              const std::shared_ptr<fdf_power::TimeoutWakeLease> op, async::Loop& client_loop,
              const std::shared_ptr<inspect::Inspector>& inspector,
              const std::shared_ptr<SystemActivityGovernor>& sag, async::Loop& sag_loop) {
            if (!op->IsResumed()) {
              async::PostDelayedTask(
                  client_loop.dispatcher(),
                  [op, &client_loop, inspector, &sag, &sag_loop, &run_after_resume_observed]() {
                    run_after_resume_observed(op, client_loop, inspector, sag, sag_loop);
                  },
                  zx::msec(100));
              return;
            }

            // This shouldn't acquire a lease because the system is resumed.
            op->HandleInterrupt(zx::duration::infinite());
            EXPECT_TRUE(op->GetWakeLeaseCopy().is_error());

            // Have the fake SAG tell teh wake lease we've suspended.
            async::PostTask(sag_loop.dispatcher(), [&sag]() { sag->SendBeforeSuspend(); });

            // Run the function that will wait to observe the suspend and then
            // confirms we didn't take a wake lease.
            run_after_suspend_observed(op, client_loop);
          };

  DoWakeLeaseTest<fdf_power::TimeoutWakeLease>(
      run_after_resume_observed,
      [](const std::shared_ptr<SystemActivityGovernor>& sag) { sag->SendAfterResume(); });
}

// Verify that an inactive TimeoutWakeLease does nothing across a
// resume->suspend transition:
//   * Create a TimeoutWakeLease
//   * Make the TimeoutWakeLease observe a resume signal
//   * Give it a lease event pair
//   * Take the lease event pair
//   * Make the TimeoutWakeLease observe a suspend
//   * Verify the TimeoutWakeLease contains no actual wake lease.
TEST_F(WakeLeaseTest, InactiveTimeoutWakeLeaseDoesNothingOnSuspend) {
  // This function gets run AFTER the one defined below. It calls itself again
  // until the lease observes we're suspended.
  const std::function<void(const std::shared_ptr<fdf_power::TimeoutWakeLease>, async::Loop&)>
      run_after_suspend_observed =
          [&run_after_suspend_observed](const std::shared_ptr<fdf_power::TimeoutWakeLease> op,
                                        async::Loop& client_loop) {
            if (op->IsResumed()) {
              async::PostDelayedTask(
                  client_loop.dispatcher(),
                  [&run_after_suspend_observed, op, &client_loop]() {
                    run_after_suspend_observed(op, client_loop);
                  },
                  zx::msec(100));
              return;
            }

            // It should be that the system suspending did NOT cause us to acquire
            // a new wake lease and therefore we should have none. Check that we
            // get an error.
            EXPECT_TRUE(op->TakeWakeLease().is_error());
            client_loop.Quit();
          };

  zx::eventpair h1, h2;
  zx::eventpair::create(0, &h1, &h2);

  // This function is run after the test environment is set up. It waits until
  // the lease observes we've resumed. After that it deposits a wait lease and
  // takes it back, meaning the wake lease should not be active. After that it
  // triggers a suspend and starts running `run_after_suspend_observed`.
  const std::function<void(const std::shared_ptr<fdf_power::TimeoutWakeLease>, async::Loop&,
                           const std::shared_ptr<inspect::Inspector>,
                           const std::shared_ptr<SystemActivityGovernor>&, async::Loop&)>
      run_after_resume_observed =
          [&run_after_resume_observed, &h1, &run_after_suspend_observed](
              const std::shared_ptr<fdf_power::TimeoutWakeLease> op, async::Loop& client_loop,
              const std::shared_ptr<inspect::Inspector>& inspector,
              const std::shared_ptr<SystemActivityGovernor>& sag, async::Loop& sag_loop) {
            if (!op->IsResumed()) {
              async::PostDelayedTask(
                  client_loop.dispatcher(),
                  [op, &client_loop, inspector, &sag, &sag_loop, &run_after_resume_observed]() {
                    run_after_resume_observed(op, client_loop, inspector, sag, sag_loop);
                  },
                  zx::msec(100));
              return;
            }

            op->DepositWakeLease(std::move(h1), zx::time::infinite());
            // By taking the wake lease here we expect that when we suspend
            // we won't acquire a new wake lease.
            auto discard = op->TakeWakeLease();

            // Have the fake SAG tell teh wake lease we've suspended.
            async::PostTask(sag_loop.dispatcher(), [&sag]() { sag->SendBeforeSuspend(); });

            // Run the function that will wait to observe the suspend and then
            // confirms we didn't take a wake lease.
            run_after_suspend_observed(op, client_loop);
          };

  DoWakeLeaseTest<fdf_power::TimeoutWakeLease>(
      run_after_resume_observed,
      [](const std::shared_ptr<SystemActivityGovernor>& sag) { sag->SendAfterResume(); });
}

TEST_F(WakeLeaseTest, TestWakeLeaseTimeouts) {
  fidl::Endpoints<fuchsia_power_system::ActivityGovernor> endpoints =
      fidl::Endpoints<fuchsia_power_system::ActivityGovernor>();
  // We're exploiting properties of the WakeLease implementation here, in
  // particular that it only talks to SAG, which we aren't faking, if it
  // needs a lease, not if it already has one. This means that WakeLease can
  // hold the contradictory thoughts in its head that we have a wake lease,
  // because it was deposited, but we are also suspended, because it hasn't
  // been told otherwise. Neat!

  fdf_power::TimeoutWakeLease test_lease = fdf_power::TimeoutWakeLease(
      async_get_default_dispatcher(), "test-lease", std::move(endpoints.client));

  EXPECT_EQ(test_lease.GetNextTimeout(), ZX_TIME_INFINITE);
  zx::eventpair h1, h2;
  zx::eventpair::create(0, &h1, &h2);
  test_lease.DepositWakeLease(std::move(h1), zx::time::infinite());
  h1 = test_lease.TakeWakeLease().value();
  test_lease.SetSuspended(true);
  EXPECT_TRUE(test_lease.TakeWakeLease().is_error());

  test_lease.SetSuspended(false);

  EXPECT_EQ(test_lease.GetNextTimeout(), ZX_TIME_INFINITE);
  zx::eventpair h3;
  zx::eventpair::create(0, &h1, &h2);
  h1.duplicate(ZX_RIGHT_SAME_RIGHTS, &h3);
  zx::time expire_time = zx::clock::get_monotonic() + zx::hour(1);
  test_lease.DepositWakeLease(std::move(h3), expire_time);
  EXPECT_EQ(test_lease.GetNextTimeout(), expire_time.get());

  h1.duplicate(ZX_RIGHT_SAME_RIGHTS, &h3);
  zx::time expire_time2 = expire_time - zx::min(1);
  test_lease.DepositWakeLease(std::move(h3), expire_time2);
  EXPECT_EQ(test_lease.GetNextTimeout(), expire_time.get());

  h1.duplicate(ZX_RIGHT_SAME_RIGHTS, &h3);
  zx::time expire_time3 = expire_time + zx::min(1);
  test_lease.DepositWakeLease(std::move(h3), expire_time3);
  EXPECT_EQ(test_lease.GetNextTimeout(), expire_time3.get());

  // Acquire a wake lease, specifying a timeout. The time the timeout is set
  // for should be between the time before the call and the time after the
  // call, plus the timeout.
  zx::duration timeout = zx::min(10);
  zx::time before = zx::clock::get_monotonic();
  test_lease.AcquireWakeLease(timeout);
  zx::time after = zx::clock::get_monotonic();
  EXPECT_GE(test_lease.GetNextTimeout(), (before + timeout).get());
  EXPECT_LE(test_lease.GetNextTimeout(), (after + timeout).get());

  // Acquire a new wake lease, but with a shorter timeout, which we expect to
  // be changed.
  timeout = zx::min(5);
  before = zx::clock::get_monotonic();
  test_lease.AcquireWakeLease(timeout);
  after = zx::clock::get_monotonic();
  zx_time_t next_timeout = test_lease.GetNextTimeout();
  EXPECT_GE(next_timeout, (before + timeout).get());
  EXPECT_LE(next_timeout, (after + timeout).get());

  // Check that the lease handle zircon object does not change. Since we
  // have a lease handle, we expect that calling HandleInterrupt won't obtain
  // a new one.
  auto lease_handle = test_lease.GetWakeLeaseCopy();
  EXPECT_FALSE(lease_handle.is_error());
  zx_info_handle_basic_t handle_info;
  EXPECT_EQ(ZX_OK, lease_handle.value().get_info(ZX_INFO_HANDLE_BASIC, &handle_info,
                                                 sizeof(handle_info), nullptr, nullptr));
  zx_koid_t koid = handle_info.koid;
  // Since the wake lease has never heard anything about whether the system is
  // suspended or resumed, it assumes we're suspended, tell it we're resumed
  test_lease.SetSuspended(false);

  // Now tell it we need to keep the system awake until a given time
  before = zx::clock::get_monotonic();
  EXPECT_TRUE(test_lease.HandleInterrupt(timeout));

  lease_handle = test_lease.GetWakeLeaseCopy();
  EXPECT_FALSE(lease_handle.is_error());
  EXPECT_EQ(ZX_OK, lease_handle.value().get_info(ZX_INFO_HANDLE_BASIC, &handle_info,
                                                 sizeof(handle_info), nullptr, nullptr));
  EXPECT_EQ(handle_info.koid, koid);
  // Add a little slack here because we get some small delays later on
  // since the implementation translates from an absolute time to an offset
  // and then posts the task at that offset a bit after calculating it. Local
  // testing indicates tens of microseconds of skew.
  after = zx::clock::get_monotonic() + zx::sec(2);

  // Check that we still get the right timeout
  next_timeout = test_lease.GetNextTimeout();
  EXPECT_GE(next_timeout, (before + timeout).get());
  EXPECT_LE(next_timeout, (after + timeout).get());
}
}  // namespace power_lib_test

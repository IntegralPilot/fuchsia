// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include "src/developer/debug/debug_agent/zircon_limbo_provider.h"

#include <fidl/fuchsia.exception/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/fdio/directory.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/status.h>

#include <atomic>
#include <cstddef>
#include <memory>
#include <thread>
#include <utility>

#include <gtest/gtest.h>

#include "src/developer/debug/debug_agent/test_utils.h"

namespace debug_agent {
namespace {

class StubProcessLimbo : public fidl::Server<fuchsia_exception::ProcessLimbo> {
 public:
  void SetActive(SetActiveRequest& request, SetActiveCompleter::Sync&) override {
    FX_NOTREACHED() << "Not needed for tests.";
  }

  void GetActive(GetActiveCompleter::Sync&) override { FX_NOTREACHED() << "Not needed for tests."; }

  void WatchActive(WatchActiveCompleter::Sync& completer) override {
    if (!reply_active_) {
      watch_active_completer_ = completer.ToAsync();
      return;
    }

    completer.Reply(is_active_);
    reply_active_ = false;
  }

  void ListProcessesWaitingOnException(ListProcessesWaitingOnExceptionCompleter::Sync&) override {
    FX_NOTREACHED() << "Not needed for tests.";
  }

  void WatchProcessesWaitingOnException(
      WatchProcessesWaitingOnExceptionCompleter::Sync& completer) override {
    watch_count_++;
    if (!reply_watch_processes_) {
      watch_processes_completer_ = completer.ToAsync();
      reply_watch_processes_ = true;
      return;
    }

    completer.Reply(fit::success(CreateExceptionList()));
    reply_watch_processes_ = false;
  }

  void RetrieveException(RetrieveExceptionRequest& request,
                         RetrieveExceptionCompleter::Sync& completer) override {
    auto it = processes_.find(request.process_koid());
    if (it == processes_.end())
      return completer.Reply(fit::error(ZX_ERR_NOT_FOUND));

    // We cannot set any fake handles, as they will fail on the channel write.
    fuchsia_exception::ProcessException exception;
    exception.info(it->second.info());

    processes_.erase(it);
    completer.Reply(fit::success(std::move(exception)));
  }

  void ReleaseProcess(ReleaseProcessRequest& request,
                      ReleaseProcessCompleter::Sync& completer) override {
    auto it = processes_.find(request.process_koid());
    if (it == processes_.end())
      return completer.Reply(fit::error(ZX_ERR_NOT_FOUND));

    processes_.erase(it);
    completer.Reply(fit::success());

    if (reply_watch_processes_ && watch_processes_completer_) {
      watch_processes_completer_->Reply(fit::success(CreateExceptionList()));
      watch_processes_completer_.reset();
      reply_watch_processes_ = false;
      return;
    }
  }

  void ReleaseProcess(uint64_t process_koid) {
    auto it = processes_.find(process_koid);
    if (it == processes_.end())
      return;

    processes_.erase(it);

    if (reply_watch_processes_ && watch_processes_completer_) {
      watch_processes_completer_->Reply(fit::success(CreateExceptionList()));
      watch_processes_completer_.reset();
      reply_watch_processes_ = false;
      return;
    }
  }

  void AppendException(zx_koid_t process_koid, zx_koid_t thread_koid,
                       fuchsia_exception::ExceptionType exception_type) {
    fuchsia_exception::ExceptionInfo info = {};
    info.process_koid() = process_koid;
    info.thread_koid() = thread_koid;
    info.type() = exception_type;

    // Track the metadata in the limbo.
    fuchsia_exception::ProcessExceptionMetadata metadata = {};
    metadata.info(std::move(info));

    // Sadly we cannot send bad handles over a channel, so we cannot actually send the "invented"
    // handles for this test. Setting the info is enough though.
    // metadata.set_process(...);
    // metadata.set_thread(...);
    processes_[info.process_koid()] = std::move(metadata);

    // If there is a callback, only send the new exceptions over.
    if (watch_processes_completer_) {
      watch_processes_completer_->Reply(fit::success(CreateExceptionList()));
      watch_processes_completer_.reset();
      reply_watch_processes_ = false;
    }
  }

  std::vector<fuchsia_exception::ProcessExceptionMetadata> CreateExceptionList() {
    std::vector<fuchsia_exception::ProcessExceptionMetadata> processes;
    processes.reserve(processes_.size());
    for (auto& [process_koid, metadata] : processes_) {
      fuchsia_exception::ProcessExceptionMetadata new_metadata = {};
      new_metadata.info(metadata.info());
      processes.push_back(std::move(new_metadata));
    }

    return processes;
  }

  // Not used for now.
  void GetFilters(GetFiltersCompleter::Sync&) override {}
  void AppendFilters(AppendFiltersRequest&, AppendFiltersCompleter::Sync&) override {}
  void RemoveFilters(RemoveFiltersRequest&, RemoveFiltersCompleter::Sync&) override {}

  const std::map<zx_koid_t, fuchsia_exception::ProcessExceptionMetadata>& processes() const {
    return processes_;
  }

  void set_is_active(bool is_active) { is_active_ = is_active; }

  void set_reply_active(bool reply) { reply_active_ = reply; }
  void set_reply_watch_processes(bool reply) { reply_watch_processes_ = reply; }

  bool has_watch_processes_callback() const { return !!watch_processes_completer_; }

  int watch_count() const { return watch_count_.load(); }

 private:
  std::map<zx_koid_t, fuchsia_exception::ProcessExceptionMetadata> processes_;

  bool is_active_ = true;

  bool reply_active_ = true;
  std::optional<WatchActiveCompleter::Async> watch_active_completer_;

  bool reply_watch_processes_ = true;
  std::optional<WatchProcessesWaitingOnExceptionCompleter::Async> watch_processes_completer_;

  std::atomic<int> watch_count_ = 0;
};

void RunUntil(async::Loop* loop, fit::function<bool()> condition,
              zx::duration step = zx::msec(10)) {
  while (!condition()) {
    loop->Run(zx::deadline_after(step));
  }
}

// Setup a mock service root directory that can be used in |component::ConnectAt|.
template <typename Protocol, typename ServerImpl>
fidl::ClientEnd<fuchsia_io::Directory> SetupServiceRoot(std::unique_ptr<ServerImpl> impl,
                                                        async_dispatcher_t* dispatcher) {
  auto [root_client_end, root_server_end] = fidl::Endpoints<fuchsia_io::Directory>::Create();
  async::PostTask(dispatcher, [dispatcher, impl = std::move(impl),
                               server_end = std::move(root_server_end)]() mutable {
    // |component::OutgoingDirectory| is not thread-safe, so we have to construct and destruct in
    // the executor thread.
    auto outgoing_dir = std::make_unique<component::OutgoingDirectory>(dispatcher);
    ASSERT_TRUE(outgoing_dir->AddProtocol<Protocol>(std::move(impl)).is_ok());
    ASSERT_TRUE(outgoing_dir->Serve(std::move(server_end)).is_ok());
    // Defer the destructing until the loop destructs.
    async::PostTaskForTime(dispatcher, [dir = std::move(outgoing_dir)]() {}, zx::time::infinite());
  });
  auto [svc_client_end, svc_server_end] = fidl::Endpoints<fuchsia_io::Directory>::Create();
  EXPECT_ZX_EQ(
      fdio_open3_at(root_client_end.channel().release(), "svc",
                    uint64_t{fuchsia_io::kPermReadable | fuchsia_io::Flags::kProtocolDirectory},
                    svc_server_end.channel().release()),
      ZX_OK);
  return std::move(svc_client_end);
}

}  // namespace

// Tests -------------------------------------------------------------------------------------------

TEST(ZirconLimboProvider, WatchProcessesOnException) {
  auto process_limbo_ptr = std::make_unique<StubProcessLimbo>();
  auto process_limbo = process_limbo_ptr.get();

  constexpr zx_koid_t kProc1Koid = 100;
  constexpr zx_koid_t kThread1Koid = 101;
  process_limbo->AppendException(kProc1Koid, kThread1Koid,
                                 fuchsia_exception::ExceptionType::kFatalPageFault);

  constexpr zx_koid_t kProc2Koid = 102;
  constexpr zx_koid_t kThread2Koid = 103;
  process_limbo->AppendException(kProc2Koid, kThread2Koid,
                                 fuchsia_exception::ExceptionType::kUnalignedAccess);

  // Setup the async loop to respond to the async call.
  async::Loop remote_loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  auto svc_dir = SetupServiceRoot<fuchsia_exception::ProcessLimbo>(std::move(process_limbo_ptr),
                                                                   remote_loop.dispatcher());
  ASSERT_ZX_EQ(remote_loop.StartThread("process-limbo-thread"), ZX_OK);

  // Setup local loop to run ZirconLimboProvider.
  async::Loop local_loop(&kAsyncLoopConfigAttachToCurrentThread);
  ZirconLimboProvider limbo_provider(svc_dir);
  ASSERT_TRUE(limbo_provider.Valid());

  process_limbo->set_reply_active(false);

  local_loop.RunUntilIdle();

  // Validate that both exceptions came through. The handles aren't real so the values will not
  // be useful, but we can verify that two come out the other end.
  const auto& processes = limbo_provider.GetLimboRecords();
  ASSERT_EQ(processes.size(), 2u);
  EXPECT_NE(processes.find(kProc1Koid), processes.end());
  EXPECT_NE(processes.find(kProc2Koid), processes.end());
}

TEST(ZirconLimboProvider, WatchProcessesCallback) {
  constexpr zx_koid_t kProc1Koid = 100;
  constexpr zx_koid_t kThread1Koid = 101;
  constexpr auto kException1Type = fuchsia_exception::ExceptionType::kFatalPageFault;
  auto process_limbo_ptr = std::make_unique<StubProcessLimbo>();
  auto process_limbo = process_limbo_ptr.get();
  process_limbo->AppendException(kProc1Koid, kThread1Koid, kException1Type);

  // These will be appended later.
  constexpr zx_koid_t kProc2Koid = 102;
  constexpr zx_koid_t kThread2Koid = 103;
  constexpr auto kException2Type = fuchsia_exception::ExceptionType::kUnalignedAccess;

  // Setup the async loop to respond to the async call.
  async::Loop remote_loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  auto svc_dir = SetupServiceRoot<fuchsia_exception::ProcessLimbo>(std::move(process_limbo_ptr),
                                                                   remote_loop.dispatcher());
  ASSERT_ZX_EQ(remote_loop.StartThread("process-limbo-thread"), ZX_OK);

  // Setup local loop to run ZirconLimboProvider.
  async::Loop local_loop(&kAsyncLoopConfigAttachToCurrentThread);
  ZirconLimboProvider limbo_provider(svc_dir);
  ASSERT_TRUE(limbo_provider.Valid());

  local_loop.RunUntilIdle();

  RunUntil(&local_loop,
           [&process_limbo]() { return process_limbo->has_watch_processes_callback(); });

  {
    // There should be one exception in limbo.
    const auto& limbo = limbo_provider.GetLimboRecords();
    ASSERT_EQ(limbo.size(), 1u);
    EXPECT_NE(limbo.find(kProc1Koid), limbo.end());
  }

  // Set the callback.
  int called_count = 0;
  limbo_provider.set_on_enter_limbo(
      [&called_count](const ZirconLimboProvider::Record& record) { called_count++; });

  // The event should've not been signaled.
  ASSERT_EQ(called_count, 0);

  // We post an exception on the limbo's loop.
  {
    zx::event exception_posted;
    ASSERT_ZX_EQ(zx::event::create(0, &exception_posted), ZX_OK);
    async::PostTask(remote_loop.dispatcher(), [&process_limbo, &exception_posted]() {
      // Add the new exception.
      process_limbo->AppendException(kProc2Koid, kThread2Koid, kException2Type);
      exception_posted.signal(0, ZX_USER_SIGNAL_0);
    });

    // Wait until it was posted.
    ASSERT_ZX_EQ(exception_posted.wait_one(ZX_USER_SIGNAL_0, zx::time::infinite(), nullptr), ZX_OK);
  }

  // Process the callback.
  RunUntil(&local_loop, [&called_count]() { return called_count > 0; });

  // Should've called the callback.
  {
    ASSERT_EQ(called_count, 1);
    const auto& records = limbo_provider.GetLimboRecords();
    ASSERT_EQ(records.size(), 2u);
    EXPECT_NE(records.find(kProc1Koid), records.end());
    EXPECT_NE(records.find(kProc2Koid), records.end());
  }

  // Releasing an exception should also call the enter limbo callback.
  called_count = 0;

  {
    zx::event release_event;
    ASSERT_ZX_EQ(zx::event::create(0, &release_event), ZX_OK);
    async::PostTask(remote_loop.dispatcher(), [&process_limbo, &release_event]() {
      // Add the new exception.
      process_limbo->ReleaseProcess(kProc2Koid);
      release_event.signal(0, ZX_USER_SIGNAL_0);
    });

    // Wait until it was posted.
    ASSERT_ZX_EQ(release_event.wait_one(ZX_USER_SIGNAL_0, zx::time::infinite(), nullptr), ZX_OK);
  }

  // The enter limbo callback should not have been called.
  ASSERT_EQ(called_count, 0);

  // We wait until the limbo have had a time to issue the other watch, thus having processes the
  // release callback.
  RunUntil(&local_loop, [&process_limbo]() { return process_limbo->watch_count() == 4; });

  // The limbo should be updated.
  {
    const auto& records = limbo_provider.GetLimboRecords();
    ASSERT_EQ(records.size(), 1u);
    EXPECT_NE(records.find(kProc1Koid), records.end());
  }
}

}  // namespace debug_agent

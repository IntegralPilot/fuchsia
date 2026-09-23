// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "src/developer/debug/zxdb/client/async_task.h"
#include "src/developer/debug/zxdb/client/mock_frame.h"
#include "src/developer/debug/zxdb/client/process.h"
#include "src/developer/debug/zxdb/client/thread.h"
#include "src/developer/debug/zxdb/common/scoped_temp_file.h"
#include "src/developer/debug/zxdb/debug_adapter/context_test.h"
#include "src/developer/debug/zxdb/symbols/compile_unit.h"
#include "src/developer/debug/zxdb/symbols/dwarf_lang.h"
#include "src/developer/debug/zxdb/symbols/function.h"
#include "src/developer/debug/zxdb/symbols/location.h"
#include "src/developer/debug/zxdb/symbols/symbol_context.h"
#include "src/developer/debug/zxdb/symbols/symbol_test_parent_setter.h"

namespace zxdb {
namespace {

class AsyncBacktraceSubscriptionTest : public DebugAdapterContextTest {
 public:
  void SetUp() override {
    DebugAdapterContextTest::SetUp();
    temp_file_ = std::make_unique<ScopedTempFile>();
    fake_file_path_ = temp_file_->name();
  }

  std::unique_ptr<ScopedTempFile> temp_file_;
  std::string fake_file_path_;

  void DeinitializeAsyncBacktraceSubscription() override {
    // Override the default behavior of disabling the `AsyncBacktraceSubscription` since we want to
    // test async backtrace behavior.
  }
};

class FakeAsyncTask : public AsyncTask {
 public:
  FakeAsyncTask(Session* session, uint64_t id, std::string name, Location loc = Location())
      : AsyncTask(session), id_(id), name_(std::move(name)), loc_(std::move(loc)) {}

  uint64_t GetId() const override { return id_; }
  Type GetType() const override { return Type::kFuture; }
  const Location& GetLocation() const override { return loc_; }
  const Identifier& GetIdentifier() const override {
    if (ident_.empty()) {
      ident_ = Identifier(IdentifierComponent(name_));
    }
    return ident_;
  }
  std::string GetState() const override { return "Pending"; }
  const std::vector<NamedValue>& GetValues() const override { return values_; }
  std::vector<Ref> GetChildren() const override {
    std::vector<Ref> refs;
    for (const auto& child : children_) {
      refs.push_back(*child);
    }
    return refs;
  }

  void AddChild(std::unique_ptr<FakeAsyncTask> child) { children_.push_back(std::move(child)); }

 private:
  uint64_t id_;
  std::string name_;
  std::vector<std::unique_ptr<FakeAsyncTask>> children_;
  Location loc_;
  mutable Identifier ident_;
  std::vector<NamedValue> values_;
};

class FakeAsyncTaskProvider : public AsyncTaskProvider {
 public:
  explicit FakeAsyncTaskProvider(std::string fake_file_path)
      : fake_file_path_(std::move(fake_file_path)) {}

  bool CanHandle(Frame* frame) const override { return true; }

  void GetTasks(
      Frame* frame,
      fit::callback<void(const Err&, std::vector<std::unique_ptr<AsyncTask>>)> cb) override {
    std::vector<std::unique_ptr<AsyncTask>> tasks;
    Location loc(0x1234, FileLine(fake_file_path_, 42), 0, SymbolContext::ForRelativeAddresses());
    auto root = std::make_unique<FakeAsyncTask>(frame->session(), 1, "root", std::move(loc));
    auto child = std::make_unique<FakeAsyncTask>(frame->session(), 2, "child");
    child->AddChild(std::make_unique<FakeAsyncTask>(frame->session(), 3, "grandchild"));
    root->AddChild(std::move(child));
    tasks.push_back(std::move(root));
    auto zero_id_task = std::make_unique<FakeAsyncTask>(frame->session(), 0, "zero_id_task");
    tasks.push_back(std::move(zero_id_task));
    cb(Err(), std::move(tasks));
  }

 private:
  std::string fake_file_path_;
};

class RaceConditionAsyncTaskProvider : public AsyncTaskProvider {
 public:
  bool CanHandle(Frame* frame) const override { return true; }

  void GetTasks(
      Frame* frame,
      fit::callback<void(const Err&, std::vector<std::unique_ptr<AsyncTask>>)> cb) override {
    callbacks_.push_back({.session = frame->session(), .cb = std::move(cb)});
  }

  struct SavedCallback {
    Session* session;
    fit::callback<void(const Err&, std::vector<std::unique_ptr<AsyncTask>>)> cb;
  };
  std::vector<SavedCallback> callbacks_;
};

}  // namespace

TEST_F(AsyncBacktraceSubscriptionTest, SingleThreadLifecycle) {
  InitializeDebugging();

  std::vector<dap::AsyncBacktraceUpdate> updates;
  client().registerHandler(
      [&](const dap::AsyncBacktraceUpdate& event) { updates.push_back(event); });

  Process* process = InjectProcessWithModule(kProcessKoid, 0x1000);
  process->AddAsyncTaskProviderForTesting(ExprLanguage::kRust,
                                          std::make_unique<FakeAsyncTaskProvider>(fake_file_path_));
  RunClient();

  InjectThread(kProcessKoid, kThreadKoid);

  context().OnStreamReadable();
  loop().RunUntilNoTasks();
  RunPendingClientCalls();

  // Expect an update from `ThreadObserver::DidCreateThread`.
  ASSERT_EQ(updates.size(), 1u);
  EXPECT_EQ(updates[0].id, static_cast<double>(kThreadKoid));
  EXPECT_EQ(updates[0].name, "test 19028730");
  EXPECT_TRUE(updates[0].tasks.has_value());
  EXPECT_EQ(updates[0].tasks.value().size(), 0u);

  updates.clear();
  Thread* thread = process->GetThreadFromKoid(kThreadKoid);
  ASSERT_NE(thread, nullptr);

  std::vector<std::unique_ptr<Frame>> frames;
  auto cu = fxl::MakeRefCounted<CompileUnit>(DwarfTag::kCompileUnit, fxl::WeakPtr<ModuleSymbols>(),
                                             fxl::RefPtr<DwarfUnit>(), DwarfLang::kRust, "test.rs",
                                             std::optional<uint64_t>());
  auto func = fxl::MakeRefCounted<Function>(DwarfTag::kSubprogram);
  func->set_assigned_name("executor");
  SymbolTestParentSetter parent_setter(func, cu);
  Location loc(0x1234, FileLine(), 0, SymbolContext::ForRelativeAddresses(), func);
  frames.push_back(std::make_unique<MockFrame>(&session(), thread, loc, 0x1000));

  InjectExceptionWithStack(kProcessKoid, kThreadKoid, debug_ipc::ExceptionType::kSingleStep,
                           std::move(frames), /*has_all_frames=*/false);

  // Force the thread to sync its stack frames.
  //
  // This results in the invocation order `OnThreadStopped` -> `DidUpdateStackFrames` ->
  // `CollectAndReportAsyncBacktrace`'s `Sync` callback, which allows us to verify that
  // `DidUpdateStackFrames` does not cancel the pending async-backtrace collection callback when
  // the thread has not yet resumed (i.e. when `CurrentStopSupportsFrames()` is true).
  EXPECT_TRUE(thread->CurrentStopSupportsFrames());

  context().OnStreamReadable();
  loop().RunUntilNoTasks();
  RunPendingClientCalls();

  // Expect a non-cancelled update from `ThreadObserver::OnThreadStopped`.
  ASSERT_EQ(updates.size(), 1u);
  EXPECT_EQ(updates[0].id, static_cast<double>(kThreadKoid));
  EXPECT_EQ(updates[0].name, "test 19028730");
  EXPECT_TRUE(updates[0].tasks.has_value());
  EXPECT_EQ(updates[0].tasks.value().size(), 2u);
  ASSERT_TRUE(updates[0].tasks.value()[0].id.has_value());
  EXPECT_EQ(updates[0].tasks.value()[0].id.value(), "0x1");
  EXPECT_EQ(updates[0].tasks.value()[0].name, "root");

  EXPECT_FALSE(updates[0].tasks.value()[1].id.has_value());
  EXPECT_EQ(updates[0].tasks.value()[1].name, "zero_id_task");

  updates.clear();
  thread->Continue(false);

  // Process the Continue response to update thread state.
  loop().RunUntilNoTasks();
  RunPendingClientCalls();

  EXPECT_FALSE(thread->CurrentStopSupportsFrames());

  // Expect an update from `ThreadObserver::DidUpdateStackFrames` (Thread Resumed).
  ASSERT_EQ(updates.size(), 1u);
  EXPECT_EQ(updates[0].id, static_cast<double>(kThreadKoid));
  EXPECT_EQ(updates[0].name, "test 19028730");
  EXPECT_TRUE(updates[0].tasks.has_value());
  EXPECT_EQ(updates[0].tasks.value().size(), 0u);

  updates.clear();
  debug_ipc::NotifyThreadExiting notify_exit;
  notify_exit.record.id = {.process = kProcessKoid, .thread = kThreadKoid};
  notify_exit.record.state = debug_ipc::ThreadRecord::State::kDead;

  session().DispatchNotifyThreadExiting(notify_exit);

  context().OnStreamReadable();
  loop().RunUntilNoTasks();
  RunPendingClientCalls();

  // Expect an update from `ThreadObserver::WillDestroyThread`.
  ASSERT_EQ(updates.size(), 1u);
  EXPECT_EQ(updates[0].id, static_cast<double>(kThreadKoid));
  EXPECT_EQ(updates[0].name, "test 19028730");
  EXPECT_FALSE(updates[0].tasks.has_value());
}

TEST_F(AsyncBacktraceSubscriptionTest, NestedAsyncTaskStructure) {
  InitializeDebugging();

  std::vector<dap::AsyncBacktraceUpdate> updates;
  client().registerHandler(
      [&](const dap::AsyncBacktraceUpdate& event) { updates.push_back(event); });

  Process* process = InjectProcessWithModule(kProcessKoid, 0x1000);
  process->AddAsyncTaskProviderForTesting(ExprLanguage::kRust,
                                          std::make_unique<FakeAsyncTaskProvider>(fake_file_path_));

  Thread* thread = InjectThread(kProcessKoid, kThreadKoid);

  context().OnStreamReadable();
  loop().RunUntilNoTasks();
  RunPendingClientCalls();

  // Expect an update from `ThreadObserver::DidCreateThread`.
  ASSERT_EQ(updates.size(), 1u);
  EXPECT_EQ(updates[0].id, static_cast<double>(kThreadKoid));
  EXPECT_EQ(updates[0].name, "test 19028730");
  EXPECT_TRUE(updates[0].tasks.has_value());
  EXPECT_EQ(updates[0].tasks.value().size(), 0u);

  updates.clear();

  std::vector<std::unique_ptr<Frame>> frames;
  auto cu = fxl::MakeRefCounted<CompileUnit>(DwarfTag::kCompileUnit, fxl::WeakPtr<ModuleSymbols>(),
                                             fxl::RefPtr<DwarfUnit>(), DwarfLang::kRust, "test.rs",
                                             std::optional<uint64_t>());
  auto func = fxl::MakeRefCounted<Function>(DwarfTag::kSubprogram);
  func->set_assigned_name("executor");
  SymbolTestParentSetter parent_setter(func, cu);
  Location loc(0x1234, FileLine(), 0, SymbolContext::ForRelativeAddresses(), func);
  frames.push_back(std::make_unique<MockFrame>(&session(), thread, loc, 0));

  InjectExceptionWithStack(kProcessKoid, kThreadKoid, debug_ipc::ExceptionType::kSingleStep,
                           std::move(frames), true);

  context().OnStreamReadable();
  loop().RunUntilNoTasks();
  RunPendingClientCalls();

  // Expect an update from `ThreadObserver::OnThreadStopped`.
  ASSERT_EQ(updates.size(), 1u);
  EXPECT_EQ(updates[0].id, static_cast<double>(kThreadKoid));
  EXPECT_EQ(updates[0].name, "test 19028730");
  EXPECT_TRUE(updates[0].tasks.has_value());
  EXPECT_EQ(updates[0].tasks.value().size(), 2u);
  ASSERT_TRUE(updates[0].tasks.value()[0].id.has_value());
  EXPECT_EQ(updates[0].tasks.value()[0].id.value(), "0x1");
  EXPECT_EQ(updates[0].tasks.value()[0].name, "root");
  EXPECT_EQ(updates[0].tasks.value()[0].children.size(), 1u);
  ASSERT_TRUE(updates[0].tasks.value()[0].children[0].id.has_value());
  EXPECT_EQ(updates[0].tasks.value()[0].children[0].id.value(), "0x2");
  EXPECT_EQ(updates[0].tasks.value()[0].children[0].name, "child");
  EXPECT_EQ(updates[0].tasks.value()[0].children[0].children.size(), 1u);
  ASSERT_TRUE(updates[0].tasks.value()[0].children[0].children[0].id.has_value());
  EXPECT_EQ(updates[0].tasks.value()[0].children[0].children[0].id.value(), "0x3");
  EXPECT_EQ(updates[0].tasks.value()[0].children[0].children[0].name, "grandchild");
  EXPECT_EQ(updates[0].tasks.value()[0].children[0].children[0].children.size(), 0u);

  EXPECT_FALSE(updates[0].tasks.value()[1].id.has_value());
  EXPECT_EQ(updates[0].tasks.value()[1].name, "zero_id_task");

  // Verify that the file and line fields are correctly handled.
  EXPECT_TRUE(updates[0].tasks.value()[0].file.has_value());
  EXPECT_EQ(updates[0].tasks.value()[0].file.value(), fake_file_path_);
  EXPECT_TRUE(updates[0].tasks.value()[0].line.has_value());
  EXPECT_EQ(updates[0].tasks.value()[0].line.value(), 42);
}

TEST_F(AsyncBacktraceSubscriptionTest, ConcurrentBacktracesSameThread) {
  InitializeDebugging();

  std::vector<dap::AsyncBacktraceUpdate> updates;
  client().registerHandler(
      [&](const dap::AsyncBacktraceUpdate& event) { updates.push_back(event); });

  Process* process = InjectProcessWithModule(kProcessKoid, 0x1000);
  auto provider = std::make_unique<RaceConditionAsyncTaskProvider>();
  auto* provider_ptr = provider.get();
  process->AddAsyncTaskProviderForTesting(ExprLanguage::kRust, std::move(provider));

  Thread* thread = InjectThread(kProcessKoid, kThreadKoid);

  context().OnStreamReadable();
  loop().RunUntilNoTasks();
  RunPendingClientCalls();

  updates.clear();

  auto cu = fxl::MakeRefCounted<CompileUnit>(DwarfTag::kCompileUnit, fxl::WeakPtr<ModuleSymbols>(),
                                             fxl::RefPtr<DwarfUnit>(), DwarfLang::kRust, "test.rs",
                                             std::optional<uint64_t>());
  auto func = fxl::MakeRefCounted<Function>(DwarfTag::kSubprogram);
  func->set_assigned_name("executor");
  SymbolTestParentSetter parent_setter(func, cu);
  Location loc(0x1234, FileLine(), 0, SymbolContext::ForRelativeAddresses(), func);

  std::vector<std::unique_ptr<Frame>> frames1;
  frames1.push_back(std::make_unique<MockFrame>(&session(), thread, loc, 0));
  InjectExceptionWithStack(kProcessKoid, kThreadKoid, debug_ipc::ExceptionType::kSingleStep,
                           std::move(frames1), true);

  // Triggering a second update should cancel the first callback.
  std::vector<std::unique_ptr<Frame>> frames2;
  frames2.push_back(std::make_unique<MockFrame>(&session(), thread, loc, 0));
  InjectExceptionWithStack(kProcessKoid, kThreadKoid, debug_ipc::ExceptionType::kSingleStep,
                           std::move(frames2), true);

  context().OnStreamReadable();
  loop().RunUntilNoTasks();
  RunPendingClientCalls();

  ASSERT_EQ(provider_ptr->callbacks_.size(), 2u);

  // Execute the first callback. It shouldn't emit an update because it was cancelled.
  auto cb1 = std::move(provider_ptr->callbacks_[0]);
  std::vector<std::unique_ptr<AsyncTask>> tasks1;
  tasks1.push_back(std::make_unique<FakeAsyncTask>(cb1.session, 10, "task_1"));
  cb1.cb(Err(), std::move(tasks1));

  context().OnStreamReadable();
  loop().RunUntilNoTasks();
  RunPendingClientCalls();

  ASSERT_EQ(updates.size(), 0u);

  // Execute the second callback. It should emit an update.
  auto cb2 = std::move(provider_ptr->callbacks_[1]);
  std::vector<std::unique_ptr<AsyncTask>> tasks2;
  tasks2.push_back(std::make_unique<FakeAsyncTask>(cb2.session, 11, "task_2"));
  cb2.cb(Err(), std::move(tasks2));

  context().OnStreamReadable();
  loop().RunUntilNoTasks();
  RunPendingClientCalls();

  ASSERT_EQ(updates.size(), 1u);
  EXPECT_TRUE(updates[0].tasks.has_value());
  EXPECT_EQ(updates[0].tasks.value().size(), 1u);
  ASSERT_TRUE(updates[0].tasks.value()[0].id.has_value());
  EXPECT_EQ(updates[0].tasks.value()[0].id.value(), "0xb");
  EXPECT_EQ(updates[0].tasks.value()[0].name, "task_2");
}

TEST_F(AsyncBacktraceSubscriptionTest, GateBacktraceOnProcessStop) {
  InitializeDebugging();

  std::vector<dap::AsyncBacktraceUpdate> updates;
  client().registerHandler(
      [&](const dap::AsyncBacktraceUpdate& event) { updates.push_back(event); });

  Process* process = InjectProcessWithModule(kProcessKoid, 0x1000);
  auto provider = std::make_unique<RaceConditionAsyncTaskProvider>();
  auto* provider_ptr = provider.get();
  process->AddAsyncTaskProviderForTesting(ExprLanguage::kRust, std::move(provider));

  constexpr uint64_t kThreadKoid1 = kThreadKoid;
  constexpr uint64_t kThreadKoid2 = kThreadKoid + 1;

  Thread* thread1 = InjectThread(kProcessKoid, kThreadKoid1);
  Thread* thread2 = InjectThread(kProcessKoid, kThreadKoid2);

  context().OnStreamReadable();
  loop().RunUntilNoTasks();
  RunPendingClientCalls();

  updates.clear();

  auto cu = fxl::MakeRefCounted<CompileUnit>(DwarfTag::kCompileUnit, fxl::WeakPtr<ModuleSymbols>(),
                                             fxl::RefPtr<DwarfUnit>(), DwarfLang::kRust, "test.rs",
                                             std::optional<uint64_t>());
  auto func = fxl::MakeRefCounted<Function>(DwarfTag::kSubprogram);
  func->set_assigned_name("executor");
  SymbolTestParentSetter parent_setter(func, cu);
  Location loc(0x1234, FileLine(), 0, SymbolContext::ForRelativeAddresses(), func);

  // Triggering updates should only occur when all threads in the process are stopped.
  // Stopping thread1 while thread2 is running should skip collection.
  std::vector<std::unique_ptr<Frame>> frames1;
  frames1.push_back(std::make_unique<MockFrame>(&session(), thread1, loc, 0));
  InjectExceptionWithStack(kProcessKoid, kThreadKoid1, debug_ipc::ExceptionType::kSingleStep,
                           std::move(frames1), true);

  context().OnStreamReadable();
  loop().RunUntilNoTasks();
  RunPendingClientCalls();

  ASSERT_EQ(provider_ptr->callbacks_.size(), 0u);

  // Stopping thread2 brings all threads in the process to a stopped state, triggering collection.
  std::vector<std::unique_ptr<Frame>> frames2;
  frames2.push_back(std::make_unique<MockFrame>(&session(), thread2, loc, 0));
  InjectExceptionWithStack(kProcessKoid, kThreadKoid2, debug_ipc::ExceptionType::kSingleStep,
                           std::move(frames2), true);

  context().OnStreamReadable();
  loop().RunUntilNoTasks();
  RunPendingClientCalls();

  ASSERT_EQ(provider_ptr->callbacks_.size(), 2u);

  // Execute the callbacks and expect updates for both stopped threads.
  auto cb1 = std::move(provider_ptr->callbacks_[0]);
  std::vector<std::unique_ptr<AsyncTask>> tasks1;
  tasks1.push_back(std::make_unique<FakeAsyncTask>(cb1.session, 101, "task_1"));
  cb1.cb(Err(), std::move(tasks1));

  auto cb2 = std::move(provider_ptr->callbacks_[1]);
  std::vector<std::unique_ptr<AsyncTask>> tasks2;
  tasks2.push_back(std::make_unique<FakeAsyncTask>(cb2.session, 102, "task_2"));
  cb2.cb(Err(), std::move(tasks2));

  context().OnStreamReadable();
  loop().RunUntilNoTasks();
  RunPendingClientCalls();

  ASSERT_EQ(updates.size(), 2u);
  EXPECT_EQ(updates[0].processId, static_cast<dap::integer>(kProcessKoid));
  EXPECT_TRUE(updates[0].tasks.has_value());
  EXPECT_EQ(updates[0].tasks.value().size(), 1u);
  ASSERT_TRUE(updates[0].tasks.value()[0].id.has_value());
  EXPECT_EQ(updates[0].tasks.value()[0].id.value(), "0x65");
  EXPECT_EQ(updates[0].tasks.value()[0].name, "task_1");

  EXPECT_EQ(updates[1].processId, static_cast<dap::integer>(kProcessKoid));
  EXPECT_TRUE(updates[1].tasks.has_value());
  EXPECT_EQ(updates[1].tasks.value().size(), 1u);
  ASSERT_TRUE(updates[1].tasks.value()[0].id.has_value());
  EXPECT_EQ(updates[1].tasks.value()[0].id.value(), "0x66");
  EXPECT_EQ(updates[1].tasks.value()[0].name, "task_2");
}

TEST_F(AsyncBacktraceSubscriptionTest, DestroyingRunningThreadTriggersCollectionForStoppedThread) {
  InitializeDebugging();

  std::vector<dap::AsyncBacktraceUpdate> updates;
  client().registerHandler(
      [&](const dap::AsyncBacktraceUpdate& event) { updates.push_back(event); });

  Process* process = InjectProcessWithModule(kProcessKoid, 0x1000);
  auto provider = std::make_unique<RaceConditionAsyncTaskProvider>();
  auto* provider_ptr = provider.get();
  process->AddAsyncTaskProviderForTesting(ExprLanguage::kRust, std::move(provider));

  constexpr uint64_t kThreadKoid1 = kThreadKoid;
  constexpr uint64_t kThreadKoid2 = kThreadKoid + 1;

  Thread* thread1 = InjectThread(kProcessKoid, kThreadKoid1);
  std::ignore = InjectThread(kProcessKoid, kThreadKoid2);

  context().OnStreamReadable();
  loop().RunUntilNoTasks();
  RunPendingClientCalls();

  updates.clear();

  auto cu = fxl::MakeRefCounted<CompileUnit>(DwarfTag::kCompileUnit, fxl::WeakPtr<ModuleSymbols>(),
                                             fxl::RefPtr<DwarfUnit>(), DwarfLang::kRust, "test.rs",
                                             std::optional<uint64_t>());
  auto func = fxl::MakeRefCounted<Function>(DwarfTag::kSubprogram);
  func->set_assigned_name("executor");
  SymbolTestParentSetter parent_setter(func, cu);
  Location loc(0x1234, FileLine(), 0, SymbolContext::ForRelativeAddresses(), func);

  // Stop thread1 while thread2 is running.
  std::vector<std::unique_ptr<Frame>> frames1;
  frames1.push_back(std::make_unique<MockFrame>(&session(), thread1, loc, 0));
  InjectExceptionWithStack(kProcessKoid, kThreadKoid1, debug_ipc::ExceptionType::kSingleStep,
                           std::move(frames1), true);

  context().OnStreamReadable();
  loop().RunUntilNoTasks();
  RunPendingClientCalls();

  // Collection shouldn't trigger yet because thread2 is running.
  ASSERT_EQ(provider_ptr->callbacks_.size(), 0u);

  // Destroy running thread2. Now thread1 is the only thread left and it is stopped.
  debug_ipc::NotifyThreadExiting notify_exit;
  notify_exit.record.id = {.process = kProcessKoid, .thread = kThreadKoid2};
  notify_exit.record.state = debug_ipc::ThreadRecord::State::kDead;
  session().DispatchNotifyThreadExiting(notify_exit);

  context().OnStreamReadable();
  loop().RunUntilNoTasks();
  RunPendingClientCalls();

  // Destroying thread2 should trigger collection for surviving stopped thread1.
  ASSERT_EQ(provider_ptr->callbacks_.size(), 1u);
}

TEST_F(AsyncBacktraceSubscriptionTest, ResumingThreadCancelsPendingBacktracesForProcess) {
  InitializeDebugging();

  std::vector<dap::AsyncBacktraceUpdate> updates;
  client().registerHandler(
      [&](const dap::AsyncBacktraceUpdate& event) { updates.push_back(event); });

  Process* process = InjectProcessWithModule(kProcessKoid, 0x1000);
  auto provider = std::make_unique<RaceConditionAsyncTaskProvider>();
  auto* provider_ptr = provider.get();
  process->AddAsyncTaskProviderForTesting(ExprLanguage::kRust, std::move(provider));

  constexpr uint64_t kThreadKoid1 = kThreadKoid;
  constexpr uint64_t kThreadKoid2 = kThreadKoid + 1;

  Thread* thread1 = InjectThread(kProcessKoid, kThreadKoid1);
  Thread* thread2 = InjectThread(kProcessKoid, kThreadKoid2);

  context().OnStreamReadable();
  loop().RunUntilNoTasks();
  RunPendingClientCalls();

  updates.clear();

  auto cu = fxl::MakeRefCounted<CompileUnit>(DwarfTag::kCompileUnit, fxl::WeakPtr<ModuleSymbols>(),
                                             fxl::RefPtr<DwarfUnit>(), DwarfLang::kRust, "test.rs",
                                             std::optional<uint64_t>());
  auto func = fxl::MakeRefCounted<Function>(DwarfTag::kSubprogram);
  func->set_assigned_name("executor");
  SymbolTestParentSetter parent_setter(func, cu);
  Location loc(0x1234, FileLine(), 0, SymbolContext::ForRelativeAddresses(), func);

  // Stop both threads so collection triggers for both.
  std::vector<std::unique_ptr<Frame>> frames1;
  frames1.push_back(std::make_unique<MockFrame>(&session(), thread1, loc, 0));
  InjectExceptionWithStack(kProcessKoid, kThreadKoid1, debug_ipc::ExceptionType::kSingleStep,
                           std::move(frames1), true);

  std::vector<std::unique_ptr<Frame>> frames2;
  frames2.push_back(std::make_unique<MockFrame>(&session(), thread2, loc, 0));
  InjectExceptionWithStack(kProcessKoid, kThreadKoid2, debug_ipc::ExceptionType::kSingleStep,
                           std::move(frames2), true);

  context().OnStreamReadable();
  loop().RunUntilNoTasks();
  RunPendingClientCalls();

  ASSERT_EQ(provider_ptr->callbacks_.size(), 2u);

  // Resume thread1. This should cancel pending backtraces for all threads in the process.
  thread1->Continue(false);

  loop().RunUntilNoTasks();
  RunPendingClientCalls();

  EXPECT_FALSE(thread1->CurrentStopSupportsFrames());

  updates.clear();

  // Executing the pending callback for thread2 should NOT emit an async backtrace update.
  auto cb2 = std::move(provider_ptr->callbacks_[1]);
  std::vector<std::unique_ptr<AsyncTask>> tasks2;
  tasks2.push_back(std::make_unique<FakeAsyncTask>(cb2.session, 102, "task_2"));
  cb2.cb(Err(), std::move(tasks2));

  context().OnStreamReadable();
  loop().RunUntilNoTasks();
  RunPendingClientCalls();

  EXPECT_EQ(updates.size(), 0u);
}

}  // namespace zxdb

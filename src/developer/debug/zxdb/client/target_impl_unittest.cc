// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/client/target_impl.h"

#include <gtest/gtest.h>

#include "src/developer/debug/shared/platform_message_loop.h"
#include "src/developer/debug/shared/test_stream_buffer.h"
#include "src/developer/debug/shared/zx_status.h"
#include "src/developer/debug/zxdb/client/process.h"
#include "src/developer/debug/zxdb/client/process_observer.h"
#include "src/developer/debug/zxdb/client/remote_api.h"
#include "src/developer/debug/zxdb/client/session.h"
#include "src/developer/debug/zxdb/client/thread.h"
#include "src/developer/debug/zxdb/client/thread_observer.h"
#include "src/developer/debug/zxdb/common/test_with_loop.h"

namespace zxdb {

namespace {

using debug::MessageLoop;

class TargetSink : public RemoteAPI {
 public:
  TargetSink() = default;
  ~TargetSink() = default;

  void set_launch_err(const Err& err) { launch_err_ = err; }
  void set_launch_reply(const debug_ipc::RunBinaryReply& reply) { launch_reply_ = reply; }

  void set_attach_err(const Err& err) { attach_err_ = err; }
  void set_attach_reply(const debug_ipc::AttachReply& reply) { attach_reply_ = reply; }

  void RunBinary(const debug_ipc::RunBinaryRequest& request,
                 fit::callback<void(const Err&, debug_ipc::RunBinaryReply)> cb) override {
    MessageLoop::Current()->PostTask(
        FROM_HERE, [this, cb = std::move(cb)]() mutable { cb(launch_err_, launch_reply_); });
  }

  void Kill(const debug_ipc::KillRequest& request,
            fit::callback<void(const Err&, debug_ipc::KillReply)> cb) override {
    MessageLoop::Current()->PostTask(FROM_HERE, [cb = std::move(cb)]() mutable {
      // For now, always report success.
      cb(Err(), debug_ipc::KillReply());
    });
  }

  void Attach(const debug_ipc::AttachRequest& request,
              fit::callback<void(const Err&, debug_ipc::AttachReply)> cb) override {
    MessageLoop::Current()->PostTask(
        FROM_HERE, [this, cb = std::move(cb)]() mutable { cb(attach_err_, attach_reply_); });
  }

  void Detach(const debug_ipc::DetachRequest& request,
              fit::callback<void(const Err&, debug_ipc::DetachReply)> cb) override {
    MessageLoop::Current()->PostTask(FROM_HERE, [cb = std::move(cb)]() mutable {
      // For now, always report success.
      cb(Err(), debug_ipc::DetachReply());
    });
  }

 private:
  // These two variables are returned from Launch().
  Err launch_err_;
  debug_ipc::RunBinaryReply launch_reply_;

  // These two variables are returned from Attach().
  Err attach_err_;
  debug_ipc::AttachReply attach_reply_;
};

class TargetImplTest : public TestWithLoop {
 public:
  TargetImplTest() {
    sink_ = new TargetSink;
    session_ = std::make_unique<Session>(std::unique_ptr<RemoteAPI>(sink_), debug::Arch::kX64,
                                         debug::Platform::kFuchsia, 4096);
  }
  ~TargetImplTest() { session_.reset(); }

  debug::TestStreamBuffer& stream() { return stream_; }
  TargetSink& sink() { return *sink_; }
  Session& session() { return *session_; }

 private:
  debug::TestStreamBuffer stream_;

  TargetSink* sink_;  // Owned by the session_.

  std::unique_ptr<Session> session_;
};

}  // namespace

// Tests that the process state is updated when trying to launch with no
// connection to the remote system.
TEST_F(TargetImplTest, LaunchNoConnection) {
  auto target_impls = session().system().GetTargetImpls();
  ASSERT_EQ(1u, target_impls.size());
  TargetImpl* target = target_impls[0];

  Err expected_err(ErrType::kNoConnection, "No connection.");
  sink().set_launch_err(expected_err);

  Err out_err;
  target->SetArgs(std::vector<std::string>({"foo_test", "--arg"}));
  target->Launch([&out_err](fxl::WeakPtr<Target> target, const Err& err, uint64_t timestamp) {
    out_err = err;
    MessageLoop::Current()->QuitNow();
  });

  // Should be in the process of launching.
  EXPECT_EQ(Target::State::kStarting, target->GetState());

  loop().Run();

  // Expect the launch to have failed and be in a non-running state.
  EXPECT_EQ(expected_err, out_err);
  EXPECT_EQ(Target::State::kNone, target->GetState());
}

// Tests a successful launch and kill.
TEST_F(TargetImplTest, LaunchKill) {
  auto target_impls = session().system().GetTargetImpls();
  ASSERT_EQ(1u, target_impls.size());
  TargetImpl* target = target_impls[0];

  // Specify a successful reply.
  const uint64_t kKoid = 1234;
  sink().set_launch_err(Err());
  debug_ipc::RunBinaryReply requested_reply;
  requested_reply.process_id = kKoid;
  requested_reply.process_name = "my name";
  sink().set_launch_reply(requested_reply);

  Err out_err;
  target->SetArgs(std::vector<std::string>({"foo_test", "--arg"}));
  target->Launch([&out_err](fxl::WeakPtr<Target> target, const Err& err, uint64_t timestamp) {
    out_err = err;
    MessageLoop::Current()->QuitNow();
  });

  // Should be in the process of launching.
  EXPECT_EQ(Target::State::kStarting, target->GetState());

  // Try to launch another one in the pending state.
  Err second_launch_err;
  target->Launch(
      [&second_launch_err](fxl::WeakPtr<Target> target, const Err& err, uint64_t timestamp) {
        second_launch_err = err;
        MessageLoop::Current()->QuitNow();
      });

  // We should have two tasks posted the first pending attach and the second pending launch (that we
  // expect to fail). They will both quit so the loop need to be run twice.
  loop().Run();
  loop().Run();

  // Expect good launch, it should have made a process.
  EXPECT_FALSE(out_err.has_error());
  EXPECT_EQ(Target::State::kRunning, target->GetState());
  ASSERT_TRUE(target->GetProcess());
  EXPECT_EQ(kKoid, target->GetProcess()->GetKoid());

  // The second launch should have failed.
  EXPECT_TRUE(second_launch_err.has_error());

  // Should not be able to launch another one. It should error out before trying to send IPC.
  target->Launch([&out_err](fxl::WeakPtr<Target> target, const Err& err, uint64_t timestamp) {
    out_err = err;
    MessageLoop::Current()->QuitNow();
  });

  loop().Run();
  EXPECT_TRUE(out_err.has_error());

  // Should not be able to attach.
  target->Attach(1234, debug_ipc::AttachConfig(),
                 [&out_err](fxl::WeakPtr<Target> target, const Err& err, uint64_t timestamp) {
                   out_err = err;
                   MessageLoop::Current()->QuitNow();
                 });

  loop().Run();
  EXPECT_TRUE(out_err.has_error());

  // Kill the process.
  target->Kill([&out_err](fxl::WeakPtr<Target> target, const Err& err) {
    out_err = err;
    MessageLoop::Current()->QuitNow();
  });
  loop().Run();

  // Should have succeeded.
  EXPECT_FALSE(out_err.has_error());
  EXPECT_EQ(Target::State::kNone, target->GetState());
}

// Tests a successful attach and detach.
TEST_F(TargetImplTest, AttachDetach) {
  auto target_impls = session().system().GetTargetImpls();
  ASSERT_EQ(1u, target_impls.size());
  TargetImpl* target = target_impls[0];

  // Specify a successful reply.
  const uint64_t kKoid = 1234;
  sink().set_attach_err(Err());

  Err out_err;
  target->Attach(kKoid, debug_ipc::AttachConfig(),
                 [&out_err](fxl::WeakPtr<Target> target, const Err& err, uint64_t timestamp) {
                   out_err = err;
                   MessageLoop::Current()->QuitNow();
                 });

  // Should be in the process of launching.
  EXPECT_EQ(Target::State::kAttaching, target->GetState());

  // Try to launch another one in the pending state.
  Err second_launch_err;
  target->Launch(
      [&second_launch_err](fxl::WeakPtr<Target> target, const Err& err, uint64_t timestamp) {
        second_launch_err = err;
        MessageLoop::Current()->QuitNow();
      });

  // We should have two tasks posted the first pending attach and the second pending launch (that we
  // expect to fail). They will both quit so the loop need to be run twice.
  loop().Run();
  loop().Run();

  // Expect good attach, it should have made a process.
  EXPECT_FALSE(out_err.has_error());
  EXPECT_EQ(Target::State::kRunning, target->GetState());
  ASSERT_TRUE(target->GetProcess());
  EXPECT_EQ(kKoid, target->GetProcess()->GetKoid());

  // The second we launch should have failed.
  EXPECT_TRUE(second_launch_err.has_error());

  // Detach.
  target->Detach([&out_err](fxl::WeakPtr<Target> target, const Err& err) {
    out_err = err;
    MessageLoop::Current()->QuitNow();
  });
  loop().Run();

  // Should be in a non-running state.
  EXPECT_EQ(Target::State::kNone, target->GetState());
}

TEST_F(TargetImplTest, AssignPreviousConnectedProcessOrder) {
  // An observer that requires the process to be created before any threads are created.
  class EnforcedOrderObserver : public ProcessObserver, public ThreadObserver {
   public:
    void DidCreateProcess(Process* process, uint64_t timestamp) override {
      process_created = true;
      last_process = process;
    }
    void DidCreateThread(Thread* thread) override {
      // This is the core requirement: Process should be created before threads.
      EXPECT_TRUE(process_created);
      EXPECT_EQ(last_process, thread->GetProcess());
      thread_created = true;
      last_thread_koid = thread->GetKoid();
    }

    bool process_created = false;
    bool thread_created = false;
    Process* last_process = nullptr;
    uint64_t last_thread_koid = 0;
  };

  EnforcedOrderObserver observer;
  session().process_observers().AddObserver(&observer);
  session().thread_observers().AddObserver(&observer);

  constexpr uint64_t kProcessKoid = 1234;
  constexpr uint64_t kThreadKoid = 5678;

  debug_ipc::ProcessRecord record;
  record.process_koid = kProcessKoid;
  record.process_name = "process";
  record.threads.resize(1);
  record.threads[0].id = {.process = kProcessKoid, .thread = kThreadKoid};

  TargetImpl* target = session().system().GetTargetImpls()[0];
  target->AssignPreviousConnectedProcess(record);

  EXPECT_TRUE(observer.process_created);
  EXPECT_TRUE(observer.thread_created);

  EXPECT_EQ(target->GetProcess(), observer.last_process);
  EXPECT_EQ(kThreadKoid, observer.last_thread_koid);

  session().process_observers().RemoveObserver(&observer);
  session().thread_observers().RemoveObserver(&observer);
}

}  // namespace zxdb

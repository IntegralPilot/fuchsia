// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fit/defer.h>
#include <lib/zx/job.h>
#include <lib/zx/process.h>
#include <lib/zx/socket.h>
#include <lib/zx/thread.h>
#include <zircon/syscalls/debug.h>
#include <zircon/syscalls/exception.h>

#include <gtest/gtest.h>

#include "src/developer/debug/shared/channel_watcher.h"
#include "src/developer/debug/shared/platform_message_loop.h"
#include "src/developer/debug/shared/socket_watcher.h"
#include "src/developer/debug/shared/zircon_exception_watcher.h"

namespace debug {

namespace {
class ReadableWatcher : public ChannelWatcher, public SocketWatcher {
 public:
  explicit ReadableWatcher(MessageLoop* loop) : loop_(loop) {}
  void OnSocketReadable(zx_handle_t socket_handle) override { loop_->QuitNow(); }
  void OnChannelReadable(zx_handle_t channel_handle) override {
    char buf[6] = {'\0'};
    uint32_t actual_bytes = 0;

    // We could get two Readable notifications, once when the sender writes to the channel, and then
    // again when the channel is closed. The first will have the "Hello" message, which will be
    // consumed. The second can come when the channel is closed, but there may or may not be any
    // data left in the channel.
    zx_channel_read(channel_handle, 0, &buf, nullptr, 5, 0, &actual_bytes, nullptr);

    // We haven't received any data before this, so we should have gotten something from the
    // channel here. The next time this function is called should not have anything else from the
    // sender.
    if (actual_bytes > 0) {
      ASSERT_EQ(actual_bytes, 5u);
      ASSERT_STREQ(buf, "Hello");
      have_gotten_channel_data_ = true;
    }
  }
  void OnPeerClosed(zx_handle_t) override {
    // We should have always drained the channel data before acknowledging the PEER_CLOSED event.
    ASSERT_TRUE(have_gotten_channel_data_);
    loop_->QuitNow();
  }

 private:
  bool have_gotten_channel_data_ = false;
  MessageLoop* loop_;
};
}  // namespace

TEST(MessageLoop, ZirconSocket) {
  zx::socket sender, receiver;
  ASSERT_EQ(ZX_OK, zx::socket::create(ZX_SOCKET_STREAM, &sender, &receiver));

  PlatformMessageLoop loop;
  std::string error_message;
  ASSERT_TRUE(loop.Init(&error_message)) << error_message;

  // Scope everything to before MessageLoop::Cleanup().
  {
    ReadableWatcher watcher(&loop);

    MessageLoop::WatchHandle watch_handle;
    ASSERT_EQ(ZX_OK, loop.WatchSocket(MessageLoop::WatchMode::kRead, receiver.get(), &watcher,
                                      &watch_handle));
    ASSERT_TRUE(watch_handle.watching());

    // Enqueue a task that should cause receiver to become readable.
    loop.PostTask(FROM_HERE, [&sender]() { sender.write(0, "Hello", 5, nullptr); });

    // This will quit on success because the OnSocketReadable callback called
    // QuitNow, or hang forever on failure.
    // TODO(brettw) add a timeout when timers are supported in the message loop.
    loop.Run();
  }
  loop.Cleanup();
}

TEST(MessageLoop, ZirconChannel) {
  zx::channel sender, receiver;
  ASSERT_EQ(ZX_OK, zx::channel::create(0, &sender, &receiver));

  PlatformMessageLoop loop;
  std::string error_message;
  ASSERT_TRUE(loop.Init(&error_message)) << error_message;

  // Scope everything to before MessageLoop::Cleanup().
  {
    ReadableWatcher watcher(&loop);

    MessageLoop::WatchHandle watch_handle;
    ASSERT_EQ(ZX_OK, loop.WatchChannel(receiver.get(), &watcher, &watch_handle));
    ASSERT_TRUE(watch_handle.watching());

    // Send a message across the channel.
    loop.PostTask(FROM_HERE, [&sender]() { sender.write(0, "Hello", 5, nullptr, 0); });

    // Close the channel to signal we're done.
    loop.PostTask(FROM_HERE, [&sender]() { sender.reset(); });

    // This will quit on success because the OnChannelReadable callback called
    // QuitNow, or hang forever on failure.
    loop.Run();
  }
  loop.Cleanup();
}

TEST(MessageLoop, DrainChannelBeforeClose) {
  zx::channel sender, receiver;
  ASSERT_EQ(ZX_OK, zx::channel::create(0, &sender, &receiver));

  PlatformMessageLoop loop;
  std::string error_message;
  ASSERT_TRUE(loop.Init(&error_message)) << error_message;

  // Scope everything to before MessageLoop::Cleanup().
  {
    ReadableWatcher watcher(&loop);

    MessageLoop::WatchHandle watch_handle;
    ASSERT_EQ(ZX_OK, loop.WatchChannel(receiver.get(), &watcher, &watch_handle));
    ASSERT_TRUE(watch_handle.watching());

    // Synchronously send a message across the channel, then immediately hang up. The reader should
    // always drain the channel before acknowledging the peer closed event.
    sender.write(0, "Hello", 5, nullptr, 0);
    sender.reset();

    // This will quit on success because the OnChannelReadable callback called
    // QuitNow, or hang forever on failure.
    loop.Run();
  }
  loop.Cleanup();
}

extern "C" void CrashingThreadEntryPoint(uintptr_t, uintptr_t) { __builtin_trap(); }

class ExceptionWatcherForTest : public ZirconExceptionWatcher {
 public:
  ExceptionWatcherForTest(MessageLoop* loop, MessageLoop::WatchHandle* watch_handle,
                          zx::thread* thread)
      : loop_(loop), watch_handle_(watch_handle), thread_(thread) {}

  void OnException(zx::exception exception_token, zx_exception_info_t exception_info) override {
    called_ = true;

    // Delete this WatchHandle while handling the exception.
    watch_handle_->StopWatching();

    uint32_t state = ZX_EXCEPTION_STATE_HANDLED;
    exception_token.set_property(ZX_PROP_EXCEPTION_STATE, &state, sizeof(state));

    // Set the thread's instruction pointer to zx_thread_exit, this is to prevent the thread from
    // resuming from us handling the exception and immediately faulting again without us catching it
    // and causing crash output to be surfaced in syslog output.
    zx_thread_state_general_regs_t regs;
    if (thread_->read_state(ZX_THREAD_STATE_GENERAL_REGS, &regs, sizeof(regs)) == ZX_OK) {
#if defined(__x86_64__)
      regs.rip = reinterpret_cast<uintptr_t>(&zx_thread_exit);
#elif defined(__aarch64__) || defined(__riscv)
      regs.pc = reinterpret_cast<uintptr_t>(&zx_thread_exit);
#endif
      thread_->write_state(ZX_THREAD_STATE_GENERAL_REGS, &regs, sizeof(regs));
    }

    loop_->QuitNow();
  }

  bool called() const { return called_; }

 private:
  MessageLoop* loop_;
  MessageLoop::WatchHandle* watch_handle_;
  zx::thread* thread_;
  zx::suspend_token suspend_token_;
  bool called_ = false;
};

TEST(MessageLoop, StopWatchingInExceptionCallback) {
  PlatformMessageLoop loop;
  std::string error_message;
  ASSERT_TRUE(loop.Init(&error_message)) << error_message;

  {
    MessageLoop::WatchHandle watch_handle;
    zx::thread thread;
    ExceptionWatcherForTest watcher(&loop, &watch_handle, &thread);

    zx_info_handle_basic_t info;
    ASSERT_EQ(ZX_OK, zx::process::self()->get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info),
                                                   nullptr, nullptr));

    PlatformMessageLoop::WatchProcessConfig config;
    config.process_name = "test_process";
    config.process_handle = zx::process::self()->get();
    config.process_koid = info.koid;
    config.watcher = &watcher;
    config.claim_exception_channel = true;

    ASSERT_EQ(ZX_OK, loop.WatchProcessExceptions(config, &watch_handle));
    ASSERT_TRUE(watch_handle.watching());

    ASSERT_EQ(ZX_OK, zx::thread::create(*zx::process::self(), "crasher", 7, 0, &thread));

    alignas(16) static uint8_t stack[8192];
    uintptr_t stack_top = reinterpret_cast<uintptr_t>(stack + sizeof(stack));

    ASSERT_EQ(ZX_OK, thread.start(reinterpret_cast<uintptr_t>(&CrashingThreadEntryPoint), stack_top,
                                  0, 0));

    loop.Run();
    EXPECT_TRUE(watcher.called());
  }
  loop.Cleanup();
}

}  // namespace debug

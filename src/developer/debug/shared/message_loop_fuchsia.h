// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_SHARED_MESSAGE_LOOP_FUCHSIA_H_
#define SRC_DEVELOPER_DEBUG_SHARED_MESSAGE_LOOP_FUCHSIA_H_

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fdio/unsafe.h>
#include <lib/zx/event.h>
#include <lib/zx/port.h>
#include <lib/zx/thread.h>
#include <zircon/syscalls/exception.h>

#include <vector>

#include "src/developer/debug/shared/event_handlers.h"
#include "src/developer/debug/shared/message_loop.h"

namespace debug {

class ChannelWatcher;
class EventPairWatcher;
class ExceptionHandler;
class SignalHandler;
class SocketWatcher;
class ZirconExceptionWatcher;

enum class WatchType : uint32_t {
  kChannel,
  kFdio,
  kProcessExceptions,
  kJobExceptions,
  kSocket,
  kEventPair
};
const char* WatchTypeToString(WatchType);

// MessageLoop is a virtual class to enable tests to intercept watch messages.
// See debug_agent/debug_agent_unittest.cc for an example.
class MessageLoopFuchsia : public MessageLoop {
 public:
  // Associated struct to track information about what type of resource a watch handle is following.
  //
  // EventHandlers need access to the WatchInfo implementation, hence the reason for it to be
  // public.
  //
  // Definition at the end of the header.
  struct WatchInfo;

  using SignalHandlerMap = std::map<const async_wait_t*, SignalHandler>;
  using ChannelExceptionHandlerMap = std::map<const async_wait_t*, ChannelExceptionHandler>;

  MessageLoopFuchsia();
  ~MessageLoopFuchsia();

  void Cleanup() override;

  // Returns the current message loop or null if there isn't one.
  static MessageLoopFuchsia* Current();

  // MessageLoop implementation.
  WatchHandle WatchFD(WatchMode mode, int fd, FDWatcher watcher) override;

  // Watches the given socket for read/write status. The watcher must outlive the returned
  // WatchHandle. Must only be called on the message loop thread.
  //
  // The WatchHandle must not unregister from a callback. The handle might become both readable and
  // writable at the same time which will necessitate calling both callbacks. The code does not
  // expect the SocketWatcher to disappear in between these callbacks.
  virtual zx_status_t WatchSocket(WatchMode mode, zx_handle_t socket_handle, SocketWatcher* watcher,
                                  WatchHandle* out);

  // Similar to |WatchSocket| above, but watches a channel. There is no current usecase for anything
  // other than reading from channels, so there is no mode argument. The watcher must outlive the
  // returned WatchHandle, and this can only be called from the MessageLoop thread. The WatchHandle
  // may unregister from a callback, since we only care about when the channel becomes readable. If
  // this is modified in the future to watch for both readable and writable events, then the same
  // restriction as |WatchSocket| will apply.
  virtual zx_status_t WatchChannel(zx_handle_t channel, ChannelWatcher* watcher, WatchHandle* out);

  virtual zx_status_t WatchEventPair(zx_handle_t event_pair, EventPairWatcher* watcher,
                                     WatchHandle* out);

  // Expose the underlying dispatcher from the async loop.
  async_dispatcher_t* dispatcher() { return loop_.dispatcher(); }

  // Attaches to the exception port of the given process and issues callbacks on the given watcher.
  // The watcher must outlive the returned WatchHandle. Must only be called on the message loop
  // thread.
  struct WatchProcessConfig {
    std::string process_name;
    zx_handle_t process_handle;
    zx_koid_t process_koid;
    ZirconExceptionWatcher* watcher = nullptr;
    bool claim_exception_channel = true;
  };
  virtual zx_status_t WatchProcessExceptions(WatchProcessConfig config, WatchHandle* out);

  // Attaches to the exception port of the given job and issues callbacks on the given watcher. The
  // watcher must outlive the returned WatchHandle. Must only be called on the message loop thread.
  struct WatchJobConfig {
    std::string job_name;
    zx_handle_t job_handle;
    zx_koid_t job_koid;
    // When true, use the "JobDebugger" exception channel. The "JobDebugger" channel registers for
    // additional notifications such as process starting events, but not for fatal exceptions that
    // are traveling up the job tree. When false, use the "normal" exception channel, which will be
    // delivered fatal exception notifications for this job.
    bool use_debugger_channel;
    ZirconExceptionWatcher* watcher;
  };
  virtual zx_status_t WatchJobExceptions(WatchJobConfig config, WatchHandle* out);

  void QuitNow() override;

  const SignalHandlerMap& signal_handlers() const { return signal_handlers_; }

  const ChannelExceptionHandlerMap& channel_exception_handlers() const {
    return channel_exception_handlers_;
  }

 private:
  const WatchInfo* FindWatchInfo(int id) const;

  // Safely increment |next_watch_id| to the next value.
  int GetNextWatchID();

  // MessageLoop protected implementation.
  uint64_t GetMonotonicNowNS() const override;
  void RunImpl() override;
  void StopWatching(int id) override;
  void SetHasTasks() override;

  // Handlers exceptions channel.
  void HandleChannelException(const ChannelExceptionHandler&, zx::exception exception,
                              zx_exception_info_t exception_info);

  // Handle an event of the given type.
  void OnFdioSignal(int watch_id, const WatchInfo& info, zx_signals_t observed);

  void OnJobException(const WatchInfo& info, zx::exception exception,
                      zx_exception_info_t exception_info);

  void OnProcessException(const WatchInfo& info, zx::exception exception,
                          zx_exception_info_t exception_info);

  void OnProcessTerminated(const WatchInfo&, zx_signals_t observed);

  void OnSocketSignal(int watch_id, const WatchInfo& info, zx_signals_t observed);
  void OnChannelSignal(int watch_id, const WatchInfo& info, zx_signals_t observed);
  void OnEventPairSignal(const WatchInfo& info, zx_signals_t observed);

  std::map<int, WatchInfo> watches_;

  // ID used as an index into watches_.
  int next_watch_id_ = 1;

  async::Loop loop_;

  SignalHandlerMap signal_handlers_;
  // See SignalHandler constructor.
  // |associated_info| needs to be updated with the fact that it has an associated SignalHandler.
  zx_status_t AddSignalHandler(int, zx_handle_t, zx_signals_t, WatchInfo* info);
  void RemoveSignalHandler(WatchInfo* info);

  // Channel Exception Handlers are similar to SignalHandlers, but have different handling
  // semantics. Particularly, they are meant to return out exception_tokens to their handlers.
  ChannelExceptionHandlerMap channel_exception_handlers_;

  // Listens to the exception channel. Will call |HandleChannelException| on this message loop.
  // |options| are the options to be passed to |zx_task_create_exception_channel|.
  zx_status_t AddChannelExceptionHandler(int id, zx_handle_t object, uint32_t options,
                                         WatchInfo* info);
  void RemoveChannelExceptionHandler(WatchInfo*);

  FXL_DISALLOW_COPY_AND_ASSIGN(MessageLoopFuchsia);

  friend class SignalHandler;
  friend class ChannelExceptionHandler;
};

// EventHandlers need access to the WatchInfo implementation.
struct MessageLoopFuchsia::WatchInfo {
  // Name of the resource being watched.
  // Mostly tracked for debugging purposes.
  std::string resource_name;

  WatchType type = WatchType::kFdio;

  // Used when the type is FDIO or socket.
  WatchMode mode = WatchMode::kReadWrite;

  // FDIO-specific watcher parameters.
  int fd = -1;
  fdio_t* fdio = nullptr;
  FDWatcher fd_watcher = nullptr;
  zx_handle_t fd_handle = ZX_HANDLE_INVALID;

  // Socket-specific parameters.
  SocketWatcher* socket_watcher = nullptr;
  zx_handle_t socket_handle = ZX_HANDLE_INVALID;

  // Channel-specific parameters. Currently this is only going to support mode == WatchMode::kRead.
  ChannelWatcher* channel_watcher = nullptr;
  zx_handle_t channel_handle = ZX_HANDLE_INVALID;

  // Task-exception-specific parameters, can be of job or process type.
  ZirconExceptionWatcher* exception_watcher = nullptr;
  zx_koid_t task_koid = 0;
  zx_handle_t task_handle = ZX_HANDLE_INVALID;
  bool uses_exception_channel = true;

  EventPairWatcher* event_pair_watcher = nullptr;
  zx_handle_t event_pair_handle = ZX_HANDLE_INVALID;

  // This makes easier the lookup of the associated ExceptionHandler with this watch id.
  const async_wait_t* signal_handler_key = nullptr;
  const async_wait_t* exception_channel_handler_key = nullptr;
};

}  // namespace debug

#endif  // SRC_DEVELOPER_DEBUG_SHARED_MESSAGE_LOOP_FUCHSIA_H_

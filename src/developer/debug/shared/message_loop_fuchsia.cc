// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/shared/message_loop_fuchsia.h"

#include <lib/fdio/io.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/clock.h>
#include <lib/zx/handle.h>
#include <lib/zx/job.h>
#include <lib/zx/process.h>
#include <stdio.h>
#include <zircon/syscalls/exception.h>
#include <zircon/types.h>

#include "src/developer/debug/shared/channel_watcher.h"
#include "src/developer/debug/shared/event_handlers.h"
#include "src/developer/debug/shared/event_pair_watcher.h"
#include "src/developer/debug/shared/logging/logging.h"
#include "src/developer/debug/shared/message_loop.h"
#include "src/developer/debug/shared/socket_watcher.h"
#include "src/developer/debug/shared/zircon_exception_watcher.h"
#include "src/developer/debug/shared/zx_status.h"

namespace debug {

MessageLoopFuchsia::MessageLoopFuchsia() : loop_(&kAsyncLoopConfigAttachToCurrentThread) {}

MessageLoopFuchsia::~MessageLoopFuchsia() {
  FX_DCHECK(Current() != this);  // Cleanup should have been called.
}

void MessageLoopFuchsia::Cleanup() {
  DEBUG_LOG(MessageLoop) << "Cleaning up the message loop.";

  // Destruct the FDWatcher first, because they may call StopWatching and expect some key is still
  // in watches_ and signal_handlers_.
  std::vector<FDWatcher> to_delete;
  to_delete.reserve(watches_.size());
  for (auto& [key, info] : watches_) {
    to_delete.push_back(std::move(info.fd_watcher));
  }
  to_delete.clear();
  watches_.clear();

  // We need to remove the signal/exception handlers/watches before the message loop
  // goes away.
  signal_handlers_.clear();
  channel_exception_handlers_.clear();

  MessageLoop::Cleanup();
}

// static
MessageLoopFuchsia* MessageLoopFuchsia::Current() {
  return reinterpret_cast<MessageLoopFuchsia*>(MessageLoop::Current());
}

const MessageLoopFuchsia::WatchInfo* MessageLoopFuchsia::FindWatchInfo(int id) const {
  auto it = watches_.find(id);
  if (it == watches_.end())
    return nullptr;
  return &it->second;
}

int MessageLoopFuchsia::GetNextWatchID() {
  std::lock_guard<std::mutex> guard(mutex_);

  int watch_id = next_watch_id_;
  next_watch_id_++;
  return watch_id;
}

zx_status_t MessageLoopFuchsia::AddSignalHandler(int id, zx_handle_t object, zx_signals_t signals,
                                                 WatchInfo* associated_info) {
  SignalHandler handler;
  zx_status_t status = handler.Init(id, object, signals);
  if (status != ZX_OK)
    return status;

  // The handler should not be there already.
  FX_DCHECK(signal_handlers_.find(handler.handle()) == signal_handlers_.end());

  associated_info->signal_handler_key = handler.handle();
  signal_handlers_[handler.handle()] = std::move(handler);

  return ZX_OK;
}

zx_status_t MessageLoopFuchsia::AddChannelExceptionHandler(int id, zx_handle_t object,
                                                           uint32_t options, WatchInfo* info) {
  ChannelExceptionHandler handler;
  zx_status_t status = handler.Init(id, object, options);
  if (status != ZX_OK)
    return status;

  // The handler should not be there already.
  FX_DCHECK(channel_exception_handlers_.find(handler.handle()) ==
            channel_exception_handlers_.end());

  info->exception_channel_handler_key = handler.handle();
  channel_exception_handlers_[handler.handle()] = std::move(handler);

  return ZX_OK;
}

MessageLoop::WatchHandle MessageLoopFuchsia::WatchFD(WatchMode mode, int fd, FDWatcher watcher) {
  WatchInfo info;
  info.type = WatchType::kFdio;
  info.mode = mode;
  info.fd_watcher = std::move(watcher);
  info.fd = fd;
  info.fdio = fdio_unsafe_fd_to_io(fd);
  if (!info.fdio)
    return WatchHandle();

  uint32_t events = 0;
  switch (mode) {
    case WatchMode::kRead:
      events = POLLIN;
      break;
    case WatchMode::kWrite:
      events = POLLOUT;
      break;
    case WatchMode::kReadWrite:
      events = POLLIN | POLLOUT;
      break;
  }

  zx_signals_t signals = ZX_SIGNAL_NONE;
  fdio_unsafe_wait_begin(info.fdio, events, &info.fd_handle, &signals);
  if (info.fd_handle == ZX_HANDLE_INVALID)
    return WatchHandle();

  int watch_id = GetNextWatchID();

  zx_status_t status = AddSignalHandler(watch_id, info.fd_handle, signals, &info);
  if (status != ZX_OK)
    return WatchHandle();

  watches_[watch_id] = std::move(info);
  return WatchHandle(this, watch_id);
}

zx_status_t MessageLoopFuchsia::WatchSocket(WatchMode mode, zx_handle_t socket_handle,
                                            SocketWatcher* watcher, MessageLoop::WatchHandle* out) {
  WatchInfo info;
  info.type = WatchType::kSocket;
  info.mode = mode;
  info.socket_watcher = watcher;
  info.socket_handle = socket_handle;

  int watch_id = GetNextWatchID();

  zx_signals_t signals = ZX_SOCKET_PEER_CLOSED;
  if (mode == WatchMode::kRead || mode == WatchMode::kReadWrite)
    signals |= ZX_SOCKET_READABLE;

  if (mode == WatchMode::kWrite || mode == WatchMode::kReadWrite)
    signals |= ZX_SOCKET_WRITABLE;

  zx_status_t status = AddSignalHandler(watch_id, socket_handle, signals, &info);
  if (status != ZX_OK)
    return status;

  watches_[watch_id] = std::move(info);
  *out = WatchHandle(this, watch_id);
  return ZX_OK;
}

zx_status_t MessageLoopFuchsia::WatchChannel(zx_handle_t channel, ChannelWatcher* watcher,
                                             MessageLoop::WatchHandle* out) {
  WatchInfo info;
  info.type = WatchType::kChannel;
  info.mode = debug::MessageLoop::WatchMode::kRead;
  info.channel_watcher = watcher;
  info.channel_handle = channel;

  int watch_id = GetNextWatchID();

  zx_signals_t signals = ZX_CHANNEL_READABLE | ZX_CHANNEL_PEER_CLOSED;

  zx_status_t status = AddSignalHandler(watch_id, channel, signals, &info);
  if (status != ZX_OK)
    return status;

  watches_[watch_id] = std::move(info);
  *out = WatchHandle(this, watch_id);
  return ZX_OK;
}

zx_status_t MessageLoopFuchsia::WatchEventPair(zx_handle_t event_pair, EventPairWatcher* watcher,
                                               MessageLoop::WatchHandle* out) {
  WatchInfo info;
  info.type = WatchType::kEventPair;
  info.event_pair_watcher = watcher;
  info.event_pair_handle = event_pair;

  int watch_id = GetNextWatchID();

  zx_signals_t signals = ZX_EVENTPAIR_PEER_CLOSED;

  zx_status_t status = AddSignalHandler(watch_id, event_pair, signals, &info);
  if (status != ZX_OK) {
    return status;
  }

  watches_[watch_id] = std::move(info);
  *out = WatchHandle(this, watch_id);

  return ZX_OK;
}

zx_status_t MessageLoopFuchsia::WatchProcessExceptions(WatchProcessConfig config,
                                                       MessageLoop::WatchHandle* out) {
  WatchInfo info;
  info.resource_name = config.process_name;
  info.type = WatchType::kProcessExceptions;
  info.exception_watcher = config.watcher;
  info.task_koid = config.process_koid;
  info.task_handle = config.process_handle;
  info.uses_exception_channel = config.claim_exception_channel;

  int watch_id;
  {
    std::lock_guard<std::mutex> guard(mutex_);

    watch_id = next_watch_id_;
    next_watch_id_++;
  }

  zx_status_t status;

  // Watch all exceptions for the process, if we're configured to take the exception channel.
  if (info.uses_exception_channel) {
    status = AddChannelExceptionHandler(watch_id, config.process_handle,
                                        ZX_EXCEPTION_CHANNEL_DEBUGGER, &info);
    if (status != ZX_OK)
      return status;
  }

  // Always watch for the process terminated signal.
  status = AddSignalHandler(watch_id, config.process_handle, ZX_PROCESS_TERMINATED, &info);
  if (status != ZX_OK)
    return status;

  DEBUG_LOG(MessageLoop) << "Watching process " << info.resource_name;

  watches_[watch_id] = std::move(info);
  *out = WatchHandle(this, watch_id);
  return ZX_OK;
}

zx_status_t MessageLoopFuchsia::WatchJobExceptions(WatchJobConfig config,
                                                   MessageLoop::WatchHandle* out) {
  WatchInfo info;
  info.resource_name = config.job_name;
  info.type = WatchType::kJobExceptions;
  info.exception_watcher = config.watcher;
  info.task_koid = config.job_koid;
  info.task_handle = config.job_handle;

  int watch_id;
  {
    std::lock_guard<std::mutex> guard(mutex_);

    watch_id = next_watch_id_;
    next_watch_id_++;
  }

  // Create and track the exception handle.
  zx_status_t status;
  uint32_t options = config.use_debugger_channel ? ZX_EXCEPTION_CHANNEL_DEBUGGER : 0;
  status = AddChannelExceptionHandler(watch_id, config.job_handle, options, &info);
  if (status != ZX_OK)
    return status;

  DEBUG_LOG(MessageLoop) << "Watching job " << info.resource_name;

  watches_[watch_id] = std::move(info);
  *out = WatchHandle(this, watch_id);
  return ZX_OK;
}

void MessageLoopFuchsia::HandleChannelException(const ChannelExceptionHandler& handler,
                                                zx::exception exception,
                                                zx_exception_info_t exception_info) {
  WatchInfo* watch_info = nullptr;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    auto it = watches_.find(handler.watch_info_id());
    FX_DCHECK(it != watches_.end());
    watch_info = &it->second;
  }

  if (watch_info->type != WatchType::kProcessExceptions &&
      watch_info->type != WatchType::kJobExceptions) {
    FX_NOTREACHED() << "Should only receive exceptions.";
    return;
  }

  FX_DCHECK(watch_info->exception_watcher);

  // We should only receive exceptions here.
  switch (watch_info->type) {
    case WatchType::kChannel:
    case WatchType::kEventPair:
    case WatchType::kFdio:
    case WatchType::kSocket:
      FX_NOTREACHED() << "Should only receive exceptions.";
      return;
    case WatchType::kProcessExceptions:
      OnProcessException(*watch_info, std::move(exception), exception_info);
      return;
    case WatchType::kJobExceptions:
      OnJobException(*watch_info, std::move(exception), exception_info);
      return;
  }

  FX_NOTREACHED();
}

uint64_t MessageLoopFuchsia::GetMonotonicNowNS() const {
  zx::time ret = zx::clock::get_monotonic();

  return ret.get();
}

// To keep similar interfaces and behaviors between MessageLoopPoll and MessageLoopFuchsia,
// we handle tasks and timers ourselves instead of posting them as async_task_t to async::Loop.
void MessageLoopFuchsia::RunImpl() {
  // Init should have been called.
  FX_DCHECK(Current() == this);
  zx_status_t status;

  while (!should_quit()) {
    zx::time time;
    uint64_t delay = DelayNS();
    if (delay == MessageLoop::kMaxDelay) {
      time = zx::time::infinite();
    } else {
      time = zx::deadline_after(zx::nsec(delay));
    }
    status = loop_.Run(time);
    FX_DCHECK(status == ZX_OK || status == ZX_ERR_CANCELED || status == ZX_ERR_TIMED_OUT)
        << "Expected ZX_OK || ZX_ERR_CANCELED || ZX_ERR_TIMED_OUT, got "
        << ZxStatusToString(status);

    std::lock_guard<std::mutex> guard(mutex_);
    if (!ProcessPendingTask())
      loop_.ResetQuit();
  }
}

void MessageLoopFuchsia::QuitNow() {
  MessageLoop::QuitNow();
  loop_.Quit();
}

void MessageLoopFuchsia::StopWatching(int id) {
  // The dispatch code for watch callbacks requires this be called on the same thread as the message
  // loop is.
  FX_DCHECK(Current() == this);

  std::lock_guard<std::mutex> guard(mutex_);

  auto found = watches_.find(id);
  FX_DCHECK(found != watches_.end());

  WatchInfo& info = found->second;
  // BufferedFD constantly creates and destroys FD handles, flooding the log with non-helpful
  // logging statements.
  if (info.type != WatchType::kFdio) {
    DEBUG_LOG(MessageLoop) << "Stop watching " << WatchTypeToString(info.type) << " "
                           << info.resource_name;
  }

  switch (info.type) {
    case WatchType::kProcessExceptions: {
      if (info.uses_exception_channel) {
        RemoveChannelExceptionHandler(&info);
      }
      RemoveSignalHandler(&info);
      break;
    }
    case WatchType::kJobExceptions: {
      RemoveChannelExceptionHandler(&info);
      break;
    }
    case WatchType::kFdio:
      fdio_unsafe_release(info.fdio);
      __FALLTHROUGH;
    case WatchType::kChannel:
    case WatchType::kEventPair:
    case WatchType::kSocket:
      RemoveSignalHandler(&info);
      break;
  }
  watches_.erase(found);
}

void MessageLoopFuchsia::SetHasTasks() {
  // Since async::Loop is cancellable, we just need to quit it to wake up the loop.
  loop_.Quit();
}

void MessageLoopFuchsia::OnFdioSignal(int watch_id, const WatchInfo& info, zx_signals_t observed) {
  uint32_t events = 0;
  fdio_unsafe_wait_end(info.fdio, observed, &events);

  if ((events & POLLERR) || (events & POLLHUP) || (events & POLLNVAL) || (events & POLLRDHUP)) {
    info.fd_watcher(info.fd, false, false, true);

    // Don't dispatch any other notifications when there's an error. Zircon seems to set readable
    // and writable on error even if there's nothing there.
    return;
  }

  // observed is a bitmap of ALL of the signals asserted on the file descripter, which could be a
  // superset of what we expected. Check the watch mode so we don't notify unwanted events.
  bool readable = !!(events & POLLIN) && (info.mode != WatchMode::kWrite);
  bool writable = !!(events & POLLOUT) && (info.mode != WatchMode::kRead);
  info.fd_watcher(info.fd, readable, writable, false);
  // info might be invalid because fd_watcher could have called StopWatching().
}

void MessageLoopFuchsia::RemoveSignalHandler(WatchInfo* info) {
  const async_wait_t* key = info->signal_handler_key;
  FX_DCHECK(key);

  size_t erase_count = signal_handlers_.erase(key);
  FX_DCHECK(erase_count == 1u);

  info->signal_handler_key = nullptr;
}

void MessageLoopFuchsia::RemoveChannelExceptionHandler(WatchInfo* info) {
  const async_wait_t* key = info->exception_channel_handler_key;
  FX_DCHECK(key);

  size_t erase_count = channel_exception_handlers_.erase(key);
  FX_DCHECK(erase_count == 1u);

  info->exception_channel_handler_key = nullptr;
}

void MessageLoopFuchsia::OnProcessException(const WatchInfo& info, zx::exception exception,
                                            zx_exception_info_t exception_info) {
  switch (exception_info.type) {
    case ZX_EXCP_THREAD_STARTING:
      info.exception_watcher->OnThreadStarting(std::move(exception), exception_info);
      break;
    case ZX_EXCP_THREAD_EXITING:
      info.exception_watcher->OnThreadExiting(std::move(exception), exception_info);
      break;
    case ZX_EXCP_GENERAL:
    case ZX_EXCP_FATAL_PAGE_FAULT:
    case ZX_EXCP_UNDEFINED_INSTRUCTION:
    case ZX_EXCP_SW_BREAKPOINT:
    case ZX_EXCP_HW_BREAKPOINT:
    case ZX_EXCP_UNALIGNED_ACCESS:
    case ZX_EXCP_POLICY_ERROR:
      info.exception_watcher->OnException(std::move(exception), exception_info);
      break;
    default:
      FX_NOTREACHED();
  }
}

void MessageLoopFuchsia::OnProcessTerminated(const WatchInfo& info, zx_signals_t observed) {
  FX_DCHECK(observed & ZX_PROCESS_TERMINATED);
  info.exception_watcher->OnProcessTerminated(info.task_koid);
}

void MessageLoopFuchsia::OnJobException(const WatchInfo& info, zx::exception exception,
                                        zx_exception_info_t exception_info) {
  info.exception_watcher->OnJobException(std::move(exception), exception_info);
}

void MessageLoopFuchsia::OnSocketSignal(int watch_id, const WatchInfo& info,
                                        zx_signals_t observed) {
  if (observed & ZX_SOCKET_PEER_CLOSED) {
    info.socket_watcher->OnPeerClosed(info.socket_handle);
    return;
  }

  // observed is a bitmap of ALL of the signals asserted on the socket, which could be a superset of
  // what we expected. Check the watch mode so we don't notify unwanted events.
  bool readable = !!(observed & ZX_SOCKET_READABLE) && (info.mode != WatchMode::kWrite);
  bool writable = !!(observed & ZX_SOCKET_WRITABLE) && (info.mode != WatchMode::kRead);

  // Dispatch readable signal.
  if (readable)
    info.socket_watcher->OnSocketReadable(info.socket_handle);

  // When signaling both readable and writable, make sure the readable handler didn't remove the
  // watch.
  if (readable && writable) {
    std::lock_guard<std::mutex> guard(mutex_);
    if (watches_.find(watch_id) == watches_.end())
      return;
  }

  // Dispatch writable signal.
  if (writable)
    info.socket_watcher->OnSocketWritable(info.socket_handle);
}

void MessageLoopFuchsia::OnChannelSignal(int watch_id, const WatchInfo& info,
                                         zx_signals_t observed) {
  // Dispatch readable signal. Note there's no need to check the mode since ChannelWatcher only
  // cares about readable (and peer closed) events.
  //
  // Note: checking the READABLE bit before checking PEER_CLOSED is important. If we check for
  // PEER_CLOSED first and return, there may be outstanding data left in the channel that hasn't
  // been read. By checking READABLE first and returning, we ensure that the channel is completely
  // drained before acknowledging the PEER_CLOSED signal.
  if (observed & ZX_CHANNEL_READABLE) {
    info.channel_watcher->OnChannelReadable(info.channel_handle);
    return;
  }

  if (observed & ZX_CHANNEL_PEER_CLOSED)
    info.channel_watcher->OnPeerClosed(info.channel_handle);
}

void MessageLoopFuchsia::OnEventPairSignal(const WatchInfo& info, zx_signals_t observed) {
  if (observed & ZX_EVENTPAIR_PEER_CLOSED)
    info.event_pair_watcher->OnPeerClosed(info.event_pair_handle);
}

const char* WatchTypeToString(WatchType type) {
  switch (type) {
    case debug::WatchType::kChannel:
      return "Channel";
    case WatchType::kFdio:
      return "FDIO";
    case WatchType::kJobExceptions:
      return "Job";
    case WatchType::kProcessExceptions:
      return "Process";
    case WatchType::kSocket:
      return "Socket";
    case WatchType::kEventPair:
      return "EventPair";
  }

  FX_NOTREACHED();
  return "";
}

}  // namespace debug

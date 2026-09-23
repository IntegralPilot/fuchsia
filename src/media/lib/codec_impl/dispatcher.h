// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_LIB_CODEC_IMPL_DISPATCHER_H_
#define SRC_MEDIA_LIB_CODEC_IMPL_DISPATCHER_H_

#include <lib/async/dispatcher.h>
#include <threads.h>

#include <memory>
#include <optional>
#include <string>

namespace codec_impl {

class Dispatcher;

// For this class we don't need an abstract base interface since neither impl has
// any data.
class DispatcherFactory {
 public:
  // The name is for naming the async::Loop thread or fdf::Dispatcher. For now,
  // name is expected to be a static string.
  //
  // This can fail, in which case the returned unique_ptr<> will hold nullptr.
  //
  // Passing {} for scheduler_role skips attempting to set a scheduler role.
  static std::unique_ptr<Dispatcher> Create(const char* name, std::string_view scheduler_role);
};

// This interface has two different impls depending on non-driver/DFv1 vs. DFv2.
// The impl is selected by codec_impl client targets choosing to dep on
// codec_impl or codec_impl_dfv2.
//
// It might be nice to avoid virtual methods for this since we're already
// holding Dispatcher in a std::unique_ptr<>, but flip side, using virtual
// methods avoids some #define-based static polymorphism (or whatever you'd like
// to call weird #define stuff), so seems worth it for that reason.
//
// Any added method must be implementable for both a single-threaded async::Loop
// and fdf::SynchronizedDispatcher.
class Dispatcher {
 protected:
  Dispatcher() = default;

 public:
  // no copy, no move (at least for now)
  Dispatcher(const Dispatcher& to_copy) = delete;
  Dispatcher& operator=(const Dispatcher& to_copy) = delete;
  Dispatcher(Dispatcher&& to_move) = delete;
  Dispatcher& operator=(Dispatcher&& to_move) = delete;

  virtual ~Dispatcher() = default;

  // True iff the caller is running under this dispatcher.
  //
  // async::Loop -> if caller is running on the async::Loop's one thread
  //
  // fdf::SynchronizedDispatcher -> fdf::Dispatcher::GetCurrent() matches this
  // Dispatcher's impl.
  virtual bool IsCurrent() = 0;

  // The caller can post tasks to this dispatcher, but should use ClosureQueue
  // or similar to ensure that the tasks won't touch anything that's already
  // gone.
  virtual async_dispatcher_t* dispatcher() = 0;

  // This tells the dispatcher to stop running tasks after any currently-running
  // task. Any pending tasks are deleted at some time between when this call
  // starts and when Join() completes.
  //
  // This can be called on any thread, including the Dispatcher's only/current
  // thread (in which case no further work beyond the currently-running
  // task/callback will run on the Dispatcher).
  virtual void QuitAsync() = 0;

  // Join must not be called on the Dispatcher thread.
  //
  // This is only allowed to be called when the caller knows (via
  // caller-specific means) that no currently-running task on the Dispatcher
  // will block (for any significant duration on anything other than a quick
  // lock or quick futex), and that no currently-running task will wait for the
  // current thread.
  //
  // Returns when there isn't any currently-executing task, all pending tasks
  // have been deleted, and any uniquely-owned thread (if any) has completed.
  //
  // The caller requirements above are satisfied by CodecImpl callers because
  // all of the following are true:
  //   * StreamControl will stop blocking thanks to actions taken near top of
  //     CodecImpl::UnbindLocked which happens before Join().
  //   * CoreCodecStopStream() is quick even if the HW can't be told to
  //     immediately cancel a current frame (in DFv2, the need to wait on HW is
  //     deferred until async PrepareStop handling).
  //   * When sharing the fidl thread for core codec processing, clients of
  //     CodecImpl use CodecImpl::UnbindAsync before ~CodecImpl, which means the
  //     Join() happens after StreamControl is done with any blocking on the
  //     shared fidl thread (also the thread which calls Join).
  //
  // If async::Loop had notification of async::Loop::Quit completion instead of
  // requiring the caller to call async::Loop::Shutdown and
  // async::Loop::JoinThreads which will synchronously wait, we'd have an async
  // mechanism here as well (analogous to fdf::Dispatcher::ShutdownAsync).
  // However, that wouldn't really eliminate much of the stuff referenced in the
  // list above, since we'd still want StreamControl to be reasonably quick
  // about stopping and deleting to avoid delays closing the StreamControl
  // server end.
  virtual void Join() = 0;

  // DFv1 only, DispatcherViaAsyncLoop only. DispatcherViaFdfDispatcher returns
  // nullopt. This is only here to allow for fallback to
  // CoreCodecSetStreamControlProfile when CoreCodecGetSchedulerProfileName
  // returns empty string.
  virtual std::optional<thrd_t> maybe_thrd() = 0;
};

}  // namespace codec_impl

#endif  // SRC_MEDIA_LIB_CODEC_IMPL_DISPATCHER_H_

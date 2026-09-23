// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_CLOSURE_QUEUE_CLOSURE_QUEUE_H_
#define LIB_CLOSURE_QUEUE_CLOSURE_QUEUE_H_

#include <lib/async/cpp/sequence_checker.h>
#include <lib/async/dispatcher.h>
#include <lib/fit/function.h>
#include <threads.h>
#include <zircon/compiler.h>

#include <memory>
#include <mutex>
#include <queue>

class ClosureQueue {
 public:
  // This must happen on a thread managed by dispatcher. Do not call SetDispatcher() after this
  // constructor.
  explicit ClosureQueue(async_dispatcher_t* dispatcher);

  // Must call SetDispatcher() before using the queue.
  ClosureQueue();

  // This call must happen on a thread managed by dispatcher.
  void SetDispatcher(async_dispatcher_t* dispatcher);

  // This must be called only on the dispatcher.
  ~ClosureQueue();

  // If StopAndClear() isn't called yet, runs to_run on dispatcher.
  // If StopAndClear() has already been called, deletes to_run on this thread.
  // If StopAndClear() is called after Enqueue() returns but before to_run has
  // been run on dispatcher, deletes to_run on thread that calls StopAndClear().
  //
  // If run, to_run will run on the dispatcher.
  //
  // This can be called on any thread.
  //
  // TODO(dustingreen): Consider adding an over-full threshold that permits
  // client code to notice when the queue is more full than expected (more full
  // than makes any sense for a given protocol).
  void Enqueue(fit::closure to_run);

  // This can only be called on the dispatcher.  This prevents any additional
  // calls to Enqueue() from actually enqueuing anything, and deletes any
  // previously-queued tasks that haven't already run.
  //
  // This is idempotent, and will run automatically at the start of
  // ~ClosureQueue, but client code is encouraged to call StopAndClear() earlier
  // than ~ClosureQueue if that helps ensure that all the captures of all the
  // queued lambdas will still be fully usable up to the point where
  // StopAndClear() is called.
  //
  // This must be called only on the dispatcher.
  void StopAndClear();
  bool is_stopped();

  bool IsSynchronized();

 private:
  class Impl {
   public:
    // This call must happen on a thread managed by dispatcher.
    static std::shared_ptr<Impl> Create(async_dispatcher_t* dispatcher);
    ~Impl();
    void Enqueue(std::shared_ptr<Impl> self_shared, fit::closure to_run);
    void StopAndClear();
    bool is_stopped();
    void RunOneHere();
    bool IsSynchronized();

   private:
    explicit Impl(async_dispatcher_t* dispatcher);
    void TryRunAll();

    std::mutex lock_;
    // Starts non-nullptr.  Set to nullptr to indicate that StopAndClear() has
    // run.
    //
    // TODO(dustingreen): __TA_GUARDED(lock_), when I can figure out why it doesn't seem to work.
    // __TA_GUARDED(lock_)
    async_dispatcher_t* dispatcher_ = {};
    // TODO(dustingreen): __TA_GUARDED(lock_), when I can figure out why it doesn't seem to work.
    // __TA_GUARDED(lock_)
    std::queue<fit::closure> pending_;

    // Only touched on dispatcher_.  This is a member so that StopAndClear() will really
    // clear synchronously.
    std::queue<fit::closure> pending_on_dispatcher_;

    std::condition_variable pending_not_empty_condition_;

    async::synchronization_checker checker_;
  };

  std::shared_ptr<Impl> impl_;
};

#endif  // LIB_CLOSURE_QUEUE_CLOSURE_QUEUE_H_

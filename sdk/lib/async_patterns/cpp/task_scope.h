// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_ASYNC_PATTERNS_CPP_TASK_SCOPE_H_
#define LIB_ASYNC_PATTERNS_CPP_TASK_SCOPE_H_

#include <lib/async/dispatcher.h>
#include <lib/async_patterns/cpp/internal/task_queue.h>
#include <lib/fit/function.h>
#include <lib/zx/time.h>
#include <zircon/compiler.h>

namespace async_patterns {

/// |TaskScope| lets you post asynchronous tasks that are silently discarded when
/// the |TaskScope| object is destroyed. In contrast, tasks posted using
/// |async::PostTask|, |async::PostDelayedTask|, and so on will always run unless
/// the dispatcher is shutdown -- there is no separate mechanism to cancel them.
///
/// The task scope is usually a member of some bigger async business logic
/// object. By associating the lifetime of the posted async tasks with this
/// object, one can conveniently capture or borrow other members from the async
/// tasks without the need for reference counting to achieve memory safety.
/// Example:
///
///     class AsyncCounter {
///      public:
///       explicit AsyncCounter(async_dispatcher_t* dispatcher) : tasks_(dispatcher) {
///         // Post some tasks to asynchronously count upwards.
///         //
///         // It is okay if |AsyncCounter| is destroyed before some of these tasks
///         // come due. Those tasks will be discarded so they will not end up accessing
///         // a destroyed object.
///         tasks_.Post([this] { count_++ });
///         tasks_.PostDelayed([this] { count_++ }, zx::sec(1));
///         tasks_.PostDelayed(fit::bind_member<&AsyncCounter::CheckCount>(this), zx::sec(5));
///       }
///
///       void CheckCount() {
///         assert(count_ == 2);
///       }
///
///      private:
///       TaskScope tasks_;
///       int count_ = 0;
///     };
///
/// |TaskScope| is thread-unsafe, and must be used and managed from a
/// [synchronized dispatcher][synchronized-dispatcher].
///
/// [synchronized-dispatcher]:
/// https://fuchsia.dev/fuchsia-src/development/languages/c-cpp/thread-safe-async#synchronized-dispatcher
class TaskScope {
 private:
  // |F| must take zero arguments ard return void.
  template <typename F>
  using require_nullary_fn = std::enable_if_t<std::is_void_v<std::invoke_result_t<F>>>;

 public:
  /// Creates a |TaskScope| that will post all tasks to the provided |dispatcher|.
  explicit TaskScope(async_dispatcher_t* dispatcher);

  /// Destroying the |TaskScope| synchronously destroys all pending tasks. If a
  /// task attempts to reentrantly post more tasks into the |TaskScope| within
  /// its destructor, those tasks will be synchronously destroyed too.
  ~TaskScope();

  /// Schedules to invoke |handler| with a deadline of now.
  ///
  /// |handler| should be a |void()| callable object.
  ///
  /// The handler will not run if |TaskScope| is destroyed before it comes due.
  /// The handler will not run if the dispatcher shuts down before it comes due.
  /// In both cases, the handler will be synchronously destroyed during task scope
  /// destruction/dispatcher shutdown.
  ///
  /// This is a drop-in replacement for |async::PostTask|.
  template <typename Closure>
  require_nullary_fn<Closure> Post(Closure&& handler) {
    PostImpl(internal::Task::Box(std::forward<Closure>(handler)));
  }

  /// Schedules to invoke |handler| with a deadline expressed as a |delay| from now.
  ///
  /// |handler| should be a |void()| callable object.
  ///
  /// The handler will not run if |TaskScope| is destroyed before it comes due.
  /// The handler will not run if the dispatcher shuts down before it comes due.
  /// In both cases, the handler will be synchronously destroyed during task scope
  /// destruction/dispatcher shutdown.
  ///
  /// This is a drop-in replacement for |async::PostDelayedTask|.
  template <typename Closure>
  require_nullary_fn<Closure> PostDelayed(Closure&& handler, zx::duration delay) {
    PostImpl(DelayedTask::Box(std::forward<Closure>(handler), this), delay);
  }

  /// Schedules to invoke |handler| with the specified |deadline|.
  ///
  /// |handler| should be a |void()| callable object.
  ///
  /// The handler will not run if |TaskScope| is destroyed before it comes due.
  /// The handler will not run if the dispatcher shuts down before it comes due.
  /// In both cases, the handler will be synchronously destroyed during task scope
  /// destruction/dispatcher shutdown.
  ///
  /// This is a drop-in replacement for |async::PostTaskForTime|.
  template <typename Closure>
  require_nullary_fn<Closure> PostForTime(Closure&& handler, zx::time deadline) {
    PostImpl(DelayedTask::Box(std::forward<Closure>(handler), this), deadline);
  }

 private:
  class DelayedTask : public list_node_t, public async_task_t {
   public:
    explicit DelayedTask(TaskScope* owner);
    virtual ~DelayedTask() = default;

    bool Post(async_dispatcher_t* dispatcher, zx::time deadline);
    virtual void Run() = 0;

    template <typename Callable>
    static std::unique_ptr<DelayedTask> Box(Callable&& callable, TaskScope* owner) {
      class TaskImpl final : public DelayedTask {
       public:
        explicit TaskImpl(Callable&& callable, TaskScope* owner)
            : DelayedTask(owner), callable_(std::forward<Callable>(callable)) {}
        void Run() final { callable_(); }

       private:
        Callable callable_;
      };
      return std::make_unique<TaskImpl>(std::forward<Callable>(callable), owner);
    }

   private:
    static void Handler(async_dispatcher_t* dispatcher, async_task_t* task, zx_status_t status);

    TaskScope* owner_;
  };

  void PostImpl(std::unique_ptr<internal::Task> task);
  void PostImpl(std::unique_ptr<DelayedTask> task, zx::duration delay);
  void PostImpl(std::unique_ptr<DelayedTask> task, zx::time deadline);
  void RemoveDelayedTask(DelayedTask* task) __TA_EXCLUDES(checker_);
  void RemoveDelayedTaskLocked(DelayedTask* task) __TA_REQUIRES(checker_);

  async_dispatcher_t* dispatcher_;
  async::synchronization_checker checker_;
  internal::TaskQueue queue_ __TA_GUARDED(checker_);

  // A list of |DelayedTask|.
  list_node_t delayed_task_list_ __TA_GUARDED(checker_) = LIST_INITIAL_VALUE(delayed_task_list_);

  // If true, tasks are discarded instead of posted.
  bool stopped_ __TA_GUARDED(checker_) = false;
};

}  // namespace async_patterns

#endif  // LIB_ASYNC_PATTERNS_CPP_TASK_SCOPE_H_

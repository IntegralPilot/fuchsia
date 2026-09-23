// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_ASYNC_PATTERNS_CPP_INTERNAL_TASK_QUEUE_H_
#define LIB_ASYNC_PATTERNS_CPP_INTERNAL_TASK_QUEUE_H_

#include <lib/async/cpp/sequence_checker.h>
#include <lib/async/dispatcher.h>
#include <lib/async/task.h>
#include <lib/fit/function.h>
#include <lib/sync/cpp/mutex.h>
#include <zircon/listnode.h>

#include <atomic>
#include <memory>

namespace async_patterns::internal {

class TaskQueueHandle;

// |Task| implementations allocate their captured state along with an intrusive
// list node header on the heap. Since each task is of indeterminate size, this
// design ensures that each task uses exactly one heap allocation.
class Task : public list_node_t {
 public:
  Task() : list_node_t(LIST_INITIAL_CLEARED_VALUE) {}
  virtual ~Task() = default;
  virtual void Run() = 0;

  template <typename Callable>
  static std::unique_ptr<Task> Box(Callable&& callable) {
    class TaskImpl final : public Task {
     public:
      explicit TaskImpl(Callable&& callable) : callable_(std::move(callable)) {}
      void Run() final { callable_(); }

     private:
      Callable callable_;
    };
    return std::make_unique<TaskImpl>(std::forward<Callable>(callable));
  }
};

// A task queue that offers thread-safe task insertion and inversion of control:
// tasks added to the queue are not immediately/synchronously run. Rather, they
// are run (or canceled) when the synchronize dispatcher comes around to
// process them.
//
// If the dispatcher is shut down, all tasks will be silently destroyed.
class TaskQueue : private async_task_t {
 public:
  // Constructs an empty task queue. |dispatcher| will be used to run
  // tasks added to the queue.
  //
  // Thread safety: |TaskQueue| must be constructed on the associated
  // synchronized dispatcher.
  static std::shared_ptr<TaskQueue> Create(async_dispatcher_t* dispatcher, const char* description);

  // Destroys the queue.
  //
  // Invariant: |Stop| must be called before destruction.
  // Thread safety: |TaskQueue| may be destroyed on any thread.
  ~TaskQueue();

  // Schedules a task onto the queue. Wakes the dispatcher if not already.
  //
  // Thread safety: |Add| can be called from any threads.
  void Add(std::unique_ptr<Task> task);

  // Atomically puts the queue into a state where no future tasks will be run.
  // Tasks that are pending will be destroyed during |Stop|. Tasks added after
  // |Stop| will be immediately destroyed in their respective |Add| calls.
  //
  // Cancels the pending wake in the dispatcher if any.
  //
  // Thread safety: |Stop| must be called from the synchronized dispatcher that
  // is running the tasks.
  void Stop();

  explicit TaskQueue(async_dispatcher_t* dispatcher, const char* description);
  TaskQueue(const TaskQueue&) = delete;
  TaskQueue& operator=(const TaskQueue&) = delete;
  TaskQueue(TaskQueue&&) = delete;
  TaskQueue& operator=(TaskQueue&&) = delete;

 private:
  // |OnWake| is called from the synchronized dispatcher when there are tasks
  // in the queue that need running.
  void OnWake(zx_status_t status);
  void StopLocked() __TA_REQUIRES(mutex_);

  static void DropTasks(list_node_t* tasks) __TA_EXCLUDES(mutex_);
  static void RunTasks(list_node_t* tasks);

  bool WakeDispatcher() __TA_REQUIRES(mutex_);
  void CancelWakeDispatcher() __TA_REQUIRES(mutex_);

  async_dispatcher_t* const dispatcher_;
  const async::synchronization_checker checker_;

  libsync::Mutex mutex_;
  // A list of |Task|.
  list_node_t task_list_ __TA_GUARDED(mutex_) = LIST_INITIAL_VALUE(task_list_);

  bool stopped_ __TA_GUARDED(mutex_) = false;

  // Whether |OnWake| will be called by the dispatcher at some point.
  bool wake_pending_ __TA_GUARDED(mutex_) = false;
};

// |TaskQueueHandle| are references vended out to possibly arbitrary threads
// that let them add tasks to the underlying queue. |TaskQueueHandle|s may be
// held on to indefinitely. The added tasks might be discarded if the underlying
// queue is stopped.
class TaskQueueHandle {
 public:
  // Adds a |task| to the referenced task queue.
  template <typename Callable>
  void Add(Callable&& task) const {
    queue_->Add(Task::Box(std::forward<Callable>(task)));
  }

  bool has_value() const { return queue_ != nullptr; }
  void reset() { queue_.reset(); }

  explicit TaskQueueHandle(std::shared_ptr<TaskQueue> queue) : queue_(std::move(queue)) {}

 private:
  // The |TaskQueue| will clear out all of its expensive internal state before
  // destruction. Thus holding onto a stopped |TaskQueue| object is cheap and
  // reduces the atomic overhead of regular task posting compared to |weak_ptr|.
  std::shared_ptr<TaskQueue> queue_;
};

// A type that submits tasks using |TaskQueueHandle| that is meant to work
// together with |PendingCall|.
class SubmitWithTaskQueueHandle {
 public:
  explicit SubmitWithTaskQueueHandle(TaskQueueHandle handle) : handle_(std::move(handle)) {}

  template <typename Task>
  void operator()(Task&& task) {
    handle_.Add(std::forward<Task>(task));
    reset();
  }

  bool has_value() const { return handle_.has_value(); }
  void reset() { handle_.reset(); }

 private:
  TaskQueueHandle handle_;
};

}  // namespace async_patterns::internal

#endif  // LIB_ASYNC_PATTERNS_CPP_INTERNAL_TASK_QUEUE_H_

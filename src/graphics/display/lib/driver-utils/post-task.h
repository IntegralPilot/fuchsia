// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_DRIVER_UTILS_POST_TASK_H_
#define SRC_GRAPHICS_DISPLAY_LIB_DRIVER_UTILS_POST_TASK_H_

#include <lib/async/dispatcher.h>
#include <lib/async/task.h>
#include <lib/fit/function.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <cstddef>
#include <memory>
#include <type_traits>
#include <utility>

#include <fbl/alloc_checker.h>

namespace display {

template <size_t inline_target_size>
class PostTaskState;

// Arranges for `callback` to run on `dispatcher` as a posted task.
//
// This overload only performs checked dynamic memory allocation. All memory
// allocation errors will be reported via the PostTask() returned result. So,
// once PostTask() succeeds, no further dynamic memory allocation will be
// performed.
//
// `inline_target_size` is the capacity for storing `callback`'s captures. A
// compilation error will occur when attempting to use a callback whose captured
// state size exceeds this capacity.
//
// `callback` must be callable until it is called, or until `dispatcher` is shut
// down, whichever comes first. This implies that `callback` must be non-null.
//
// `callback` will be run when `dispatcher` invokes the posted task. So,
// `callback` will always be executed from one of the dispatcher's threads. The
// state captured in `callback` will be destroyed right after `callback` is run,
// on the same thread.
//
// `callback` must be moveable (implying that the captured context must be
// moveable) while it is callable. The move operators may be called on the
// thread used to call `PostTask()`, or on any of the dispatcher's threads.
//
// `callback` will not be run if `dispatcher` is shut down before it processes
// the posted task. In that case, the state captured in `callback` will be
// destroyed while the dispatcher is shutting down. The captured state will
// either be destroyed synchronously in PostTask() or on a dispatcher thread,
// depending on the relative timing of PostTask() and the dispatcher shutdown.
//
// Returns ZX_ERR_NO_MEMORY if dynamic memory allocation fails. Also exhibits
// all failure modes of the underlying C API `async_post_task()`.
template <size_t inline_target_size = fit::default_inline_target_size>
zx::result<> PostTask(async_dispatcher_t& dispatcher,
                      fit::inline_callback<void(), inline_target_size> callback);

// Arranges for `callback` to run on `dispatcher` as a posted task.
//
// This overload does not perform any dynamic memory allocation. The consumed
// `post_task_state` is used instead. The caller must not attempt to access
// `post_task_state` in any way (such as a stashed raw pointer) after passing it
// into this call.
//
// `inline_target_size` is the capacity for storing `callback`'s captures. The
// compiler can infer this argument from the type of `post_task_state`. A
// compilation error will occur when attempting to use a callback whose captured
// state size exceeds this capacity.
//
// `callback` must be callable until it is called, or until `dispatcher` is shut
// down, whichever comes first. This implies that `callback` must be non-null.
//
// `callback` will be run when `dispatcher` invokes the posted task. So,
// `callback` will always be executed from one of the dispatcher's threads. The
// state captured in `callback` will be destroyed right after `callback` is run,
// on the same thread.
//
// `callback` must be moveable (implying that the captured context must be
// moveable) while it is callable. The move operators may be called on the
// thread used to call `PostTask()`, or on any of the dispatcher's threads.
//
// `callback` will not be run if `dispatcher` is shut down before it processes
// the posted task. In that case, the state captured in `callback` will be
// destroyed while the dispatcher is shutting down. The captured state will
// either be destroyed synchronously in PostTask() or on a dispatcher thread,
// depending on the relative timing of PostTask() and the dispatcher shutdown.
//
// Returns any error produced by the underlying C API `async_post_task()`.
template <size_t inline_target_size>
zx::result<> PostTask(std::unique_ptr<PostTaskState<inline_target_size>> post_task_state,
                      async_dispatcher_t& dispatcher,
                      fit::inline_callback<void(), inline_target_size> task_callback);

// All the context needed to run a callback as a task on an async dispatcher.
//
// After a PostTaskState is allocated, no further memory allocation is needed to
// post the task and run the callback.
//
// `inline_target_size` is the capacity for storing the callback's captures. A
// compilation error will occur when attempting to call `Post()` using a
// callback whose captured state size exceeds this capacity.
//
// This class is exposed to facilitate separating the memory allocation from the
// callback capture computation. This supports the common pattern of dividing a
// complex piece of work into a fallible resource acquisition stage, a "commit
// point", and an infallible computation stage.
//
// `PostTaskState::Post()` assumes that PostTaskState instances are owned using
// std::unique_ptr.
template <size_t inline_target_size>
class PostTaskState {
 public:
  // Creates an instance that can be used to execute one callback.
  PostTaskState();

  // PostTaskState instances are not movable or copyable. The underlying state's
  // address is passed to a C library, so the instance must be pinned in memory.
  PostTaskState(const PostTaskState&) = delete;
  PostTaskState(PostTaskState&&) = delete;
  PostTaskState& operator=(const PostTaskState&) = delete;
  PostTaskState& operator=(PostTaskState&&) = delete;

  ~PostTaskState() = default;

  // `PostTask()` implementation. The arguments have identical semantics.
  static zx::result<> Post(std::unique_ptr<PostTaskState> post_task_state,
                           async_dispatcher_t& dispatcher,
                           fit::inline_callback<void(), inline_target_size> callback);

 private:
  // The `handler` member in an `async_task_t` structure.
  //
  // See `async_task_handler_t` and `async_post_task()` for argument semantics.
  static void TaskHandler(async_dispatcher_t* dispatcher, async_task_t* task, zx_status_t status);

  // `async_task` must be the return value of an `AsyncTaskPtr()` call.
  static PostTaskState* ToPostTaskState(async_task_t* async_task);

  // The returned pointer is non-null, and can be used in a `ToPostTaskState()` call.
  async_task_t* AsyncTaskPtr();

  // Cleans up this instance so it can be used in a new `PostTask()` call.
  //
  // Returns the instance's current callback. The caller is responsible for
  // calling the callback before destroying it.
  [[nodiscard]] fit::inline_callback<void(), inline_target_size> Rearm();

  // State exclusively used by the `async_post_task()` API. Thread safety
  // concerns are deferred to that API's implementation.
  async_task_t async_task_;

  // This data member has a complex thread-safety argument.
  //
  // The data is initialized by `PostTaskState::Post()`, and later modified
  // (moved out and destroyed) in `PostTaskState::TaskHandler()`, possibly on a
  // different thread. These data accesses are not protected by any explicit
  // synchronization mechanism.
  //
  // Instead, we assume that the `async_post_task()` implementation must use a
  // memory barrier that makes the calling thread's writes visible to the
  // thread processing the task. We think the barrier is needed so the thread
  // that executes the task handler can read the code pointer.
  fit::inline_callback<void(), inline_target_size> callback_;
};

// Move-only holder that calls a callback when being destroyed.
//
// This helper was designed for performing cleanup that must be ordered after
// other code submitted via `PostTask()`. In particular, instances are suitable
// to be captured by lambdas used to construct the fit::inline_callback
// arguments passed to `PostTask()`.
template <typename Callable>
class CallFromDestructor {
 public:
  static_assert(std::is_invocable_r_v<void, Callable>,
                "CallFromDestructor requires an argument-less function that returns void");

  // `callback` will be called when this instance is destroyed.
  explicit CallFromDestructor(Callable callback);

  // Move construction support is needed for the fit::inline_callback instances
  // that use CallFromDestructor in lambda captures.
  CallFromDestructor(const CallFromDestructor&) = delete;
  CallFromDestructor(CallFromDestructor&& rhs);
  CallFromDestructor& operator=(const CallFromDestructor&) = delete;
  CallFromDestructor& operator=(CallFromDestructor&& rhs) = delete;

  ~CallFromDestructor();

 private:
  Callable callback_;
  bool moved_from_ = false;
};

template <size_t inline_target_size>
zx::result<> PostTask(std::unique_ptr<PostTaskState<inline_target_size>> post_task_state,
                      async_dispatcher_t& dispatcher,
                      fit::inline_callback<void(), inline_target_size> task_callback) {
  ZX_DEBUG_ASSERT(post_task_state != nullptr);
  ZX_DEBUG_ASSERT(task_callback);

  return PostTaskState<inline_target_size>::Post(std::move(post_task_state), dispatcher,
                                                 std::move(task_callback));
}

template <size_t inline_target_size>
zx::result<> PostTask(async_dispatcher_t& dispatcher,
                      fit::inline_callback<void(), inline_target_size> task_callback) {
  ZX_DEBUG_ASSERT(task_callback);

  fbl::AllocChecker alloc_checker;
  auto task_state = fbl::make_unique_checked<PostTaskState<inline_target_size>>(&alloc_checker);
  if (!alloc_checker.check()) {
    return zx::error_result(ZX_ERR_NO_MEMORY);
  }
  return PostTaskState<inline_target_size>::Post(std::move(task_state), dispatcher,
                                                 std::move(task_callback));
}

template <size_t inline_target_size>
PostTaskState<inline_target_size>::PostTaskState()
    : async_task_({
          .state = ASYNC_STATE_INIT,
          .handler = &PostTaskState::TaskHandler,
      }) {}

// static
template <size_t inline_target_size>
zx::result<> PostTaskState<inline_target_size>::Post(
    std::unique_ptr<PostTaskState> post_task_state, async_dispatcher_t& dispatcher,
    fit::inline_callback<void(), inline_target_size> callback) {
  ZX_DEBUG_ASSERT(post_task_state != nullptr);
  ZX_DEBUG_ASSERT(callback);

  ZX_DEBUG_ASSERT_MSG(!post_task_state->callback_, "Post() called twice");
  post_task_state->callback_ = std::move(callback);

  async_task_t* const async_task = post_task_state->AsyncTaskPtr();
  zx_status_t post_status = async_post_task(&dispatcher, async_task);
  if (post_status != ZX_OK) {
    return zx::error_result(post_status);
  }

  // `PostTaskState::TaskHandler()` will get the PostTaskState instance back into
  // a unique_ptr, at which point the compiler will help us avoid leaking it.
  // The `async_post_task()` API guarantees that the task handler will run, even
  // if the dispatcher is shut down.
  post_task_state.release();
  return zx::ok();
}

// static
template <size_t inline_target_size>
void PostTaskState<inline_target_size>::TaskHandler(async_dispatcher_t* dispatcher,
                                                    async_task_t* task, zx_status_t status) {
  ZX_DEBUG_ASSERT(task != nullptr);

  // The PostTaskState instance will get deleted when this method returns.
  PostTaskState* const post_task_state_ptr = ToPostTaskState(task);
  std::unique_ptr<PostTaskState> post_task_state(post_task_state_ptr);

  // Don't run the callback if the dispatcher is shutting down.
  if (status == ZX_ERR_CANCELED) {
    return;
  }
  ZX_DEBUG_ASSERT(status == ZX_OK);

  fit::inline_callback<void(), inline_target_size> callback = post_task_state->Rearm();

  // In the future, we may pass `post_task_state` to the callback, so the
  // PostTaskState instance can be reused for a different task.
  callback();
}

// static
template <size_t inline_target_size>
PostTaskState<inline_target_size>* PostTaskState<inline_target_size>::ToPostTaskState(
    async_task_t* async_task) {
  // The pointer of a standard layout class can be converted to/from a pointer
  // to its first non-static member.
  static_assert(std::is_standard_layout_v<PostTaskState>);
  static_assert(offsetof(PostTaskState, async_task_) == 0);
  PostTaskState* return_value = reinterpret_cast<PostTaskState*>(async_task);

  ZX_DEBUG_ASSERT(&return_value->async_task_ == async_task);
  return return_value;
}

template <size_t inline_target_size>
async_task_t* PostTaskState<inline_target_size>::AsyncTaskPtr() {
  async_task_t* const return_value = &async_task_;
  ZX_DEBUG_ASSERT(PostTaskState::ToPostTaskState(return_value) == this);
  return return_value;
}

template <size_t inline_target_size>
fit::inline_callback<void(), inline_target_size> PostTaskState<inline_target_size>::Rearm() {
  async_task_.state = ASYNC_STATE_INIT;

  ZX_DEBUG_ASSERT(callback_);
  return std::move(callback_);
}

template <typename Callable>
CallFromDestructor<Callable>::CallFromDestructor(Callable callback)
    : callback_(std::move(callback)) {}

template <typename Callable>
CallFromDestructor<Callable>::CallFromDestructor(CallFromDestructor&& rhs)
    : callback_(std::move(rhs.callback_)) {
  rhs.moved_from_ = true;
}

template <typename Callable>
CallFromDestructor<Callable>::~CallFromDestructor() {
  if (!moved_from_) {
    callback_();
  }
}

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_DRIVER_UTILS_POST_TASK_H_

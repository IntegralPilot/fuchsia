// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_TESTING_CPP_DRIVER_RUNTIME_H_
#define LIB_DRIVER_TESTING_CPP_DRIVER_RUNTIME_H_

#include <lib/async/cpp/executor.h>
#include <lib/driver/runtime/testing/cpp/internal/dispatcher.h>
#include <lib/fit/defer.h>
#include <lib/fit/function.h>
#include <lib/fpromise/bridge.h>

#include <list>
#include <set>

namespace fdf_testing {

namespace internal {

// Starts the runtime environment on creation, and resets it on destruction.
class DriverRuntimeEnv final {
 public:
  DriverRuntimeEnv();
  ~DriverRuntimeEnv();
};

}  // namespace internal

// |DriverRuntime| is a RAII client to the driver runtime for a unit test. It can be used to
// start background dispatchers, and run the foreground dispatcher. On destruction it will shutdown
// all created dispatchers, and reset the driver runtime.
//
// There can only be one active instance of this class during a test case. The instance is commonly
// the first member of a test fixture class. After constructing an instance of |DriverRuntime|,
// other code may get the instance via the static |GetInstance| accessor.
//
// # Thread safety
//
// This class is thread-unsafe. Instances must be created and used from the unit test's main thread.
// The exception to this is the |Quit| and |ResetQuit| functions which can be called from any
// thread.
class DriverRuntime final {
 public:
  // This class wraps a fpromise::promise for an asynchronous task. This prevents the user from
  // being exposed directly to the fpromise type inside.
  // Use |DriverRuntime::RunToCompletion| to get the result of the task.
  template <typename ResultType>
  class [[nodiscard]] AsyncTask {
   public:
    explicit AsyncTask(fpromise::promise<ResultType> promise) : promise_(std::move(promise)) {}

   private:
    friend DriverRuntime;
    fpromise::promise<ResultType> promise_;
  };

  // Starts the driver runtime environment. If the env fails to start, this will throw an assert.
  // This will also create and attach a foreground driver dispatcher to the current thread.
  // Foreground dispatchers will not be ran in the background on the driver runtime's managed
  // thread pool, but instead must be run using the Run* methods.
  explicit DriverRuntime();

  // Resets the driver runtime environment.
  ~DriverRuntime();

  // Gets the active instance of the DriverRuntime. The instance is stored in thread_local storage.
  static DriverRuntime* GetInstance();

  // Returns the current foreground driver dispatcher.
  fdf::UnownedSynchronizedDispatcher GetForegroundDispatcher();

  // Starts a background driver dispatcher and returns it. Background dispatchers are automatically
  // ran on the managed thread pool inside the driver runtime. Objects that live on background
  // dispatchers are not safe to access directly from a test, but instead must be wrapped inside of
  // an |async_patterns::TestDispatcherBound|.
  fdf::UnownedSynchronizedDispatcher StartBackgroundDispatcher();

  // Shuts down all of the driver dispatchers that exist. This will return when all shutdowns
  // have completed. If this has already been done returns immediately.
  //
  // |dut_initial_dispatcher| will be set as the current dispatcher context before this returns
  // to help with synchronization_checkers inside of the driver during teardown afterwards.
  // It should be the same dispatcher that the dut (driver under test) was created with.
  // If a nullptr is given then there will not be a current dispatcher context after this returns.
  void ShutdownAllDispatchers(fdf_dispatcher_t* dut_initial_dispatcher);

  // Shuts down the foreground dispatcher and runs the given closure afterwards in the context of
  // the dispatcher. Creates a new foreground dispatcher to replace it.
  // Returns when the shutdown has completed fully and the new dispatcher is created.
  void ResetForegroundDispatcher(fit::closure post_shutdown);

  // Shuts down the given dispatcher and runs the given closure afterwards in the context of
  // the dispatcher. Returns when the shutdown has completed fully.
  void ShutdownBackgroundDispatcher(fdf_dispatcher_t* dispatcher, fit::closure post_shutdown);

  // Runs the foreground dispatcher until it is quit.
  void Run();

  // Runs the foreground dispatcher for at most |timeout|. Returns |true| if the timeout has been
  // reached before it is quit.
  bool RunWithTimeout(zx::duration timeout = zx::sec(1));

  // Runs the foreground dispatcher until the condition returns true.
  //
  // Providing this for parity with async::Loop. Avoid this when possible as it is a bad pattern,
  // the condition callback may have data race and use after free problems.
  // Prefer to use the |Run| and |Quit| combination, or the |RunUntilIdle| call.
  //
  // |step| specifies the interval at which this method should wake up to poll
  // |condition|. If |step| is |zx::duration::infinite()|, no polling timer is
  // set. Instead, the condition is checked initially and after anything happens
  // on the dispatcher (e.g. a task executes). This is useful when the caller knows
  // that |condition| will be made true by a task running on the dispatcher. This will
  // generally be the case unless |condition| is made true on a different
  // thread.
  void RunUntil(fit::function<bool()> condition, zx::duration step = zx::msec(10));

  // Runs the foreground dispatcher until the condition returns true or the timeout is reached.
  // Returns |true| if the condition was met, and |false| if the timeout was
  // reached.
  //
  // Providing this for parity with async::Loop. Avoid this when possible as it is a bad pattern,
  // the condition callback may have data race and use after free problems.
  // Prefer to use the |RunWithTimeout| and |Quit| combination, or the |RunUntilIdle| call.
  //
  // |step| specifies the interval at which this method should wake up to poll
  // |condition|. If |step| is |zx::duration::infinite()|, no polling timer is
  // set. Instead, the condition is checked initially and after anything happens
  // on the dispatcher (e.g. a task executes). This is useful when the caller knows
  // that |condition| will be made true by a task running on the dispatcher. This will
  // generally be the case unless |condition| is made true on a different
  // thread.
  bool RunWithTimeoutOrUntil(fit::function<bool()> condition, zx::duration timeout = zx::sec(1),
                             zx::duration step = zx::msec(10));

  // Runs the foreground dispatcher until idle.
  void RunUntilIdle();

  // Runs the foreground dispatcher until the |task| has completed.
  // Returns the result of the |task| when complete.
  template <typename Result>
  Result RunToCompletion(AsyncTask<Result> async_task) {
    AssertCurrentThreadIsInitialThread();
    return RunPromise(std::move(async_task.promise_)).take_value();
  }

  // Runs the foreground dispatcher until the given |fpromise::promise| completes, and returns the
  // |fpromise::result| it produced.
  //
  // If the |fpromise::promise| never completes, this method will run forever.
  template <typename PromiseType>
  typename PromiseType::result_type RunPromise(PromiseType promise);

  // Runs the foreground dispatcher while spawning a new thread to perform |work|, and
  // returns the result of calling |work|. Runs the foreground dispatcher until |work| returns.
  //
  // |Callable| should take zero arguments.
  template <typename Callable>
  auto PerformBlockingWork(Callable&& work) {
    using R = std::invoke_result_t<Callable>;
    if constexpr (std::is_same_v<R, void>) {
      // Use an |std::monostate| such that |PerformBlockingWorkImpl| does not
      // have to contend with void values which are tricky to work with.
      PerformBlockingWorkImpl<std::monostate>([work = std::forward<Callable>(work)]() mutable {
        work();
        return std::monostate{};
      });
      return;
    } else {
      return PerformBlockingWorkImpl<R>(std::forward<Callable>(work));
    }
  }

  // Quits the foreground dispatcher.
  // Active invocations of |Run| will eventually terminate upon completion of their
  // current unit of work.
  //
  // Subsequent calls to |Run| will return immediately until |ResetQuit| is called.
  void Quit();

  // Resets the quit state of the foreground dispatcher so that it can be restarted
  // using |Run|.
  //
  // This function must only be called when the foreground dispatcher is not running.
  // The caller must ensure all active invocations of |Run| have terminated before
  // resetting the quit state.
  //
  // Returns |ZX_OK| if the foreground dispatcher's quit state was correctly reset.
  // Returns |ZX_ERR_BAD_STATE| if the foreground dispatcher was shutting down or if
  // it was currently actively running
  zx_status_t ResetQuit();

  // Creates a closure that calls |Quit|.
  fit::closure QuitClosure();

 private:
  // Runs the foreground dispatcher. Dispatches events until there are none remaining, and then
  // returns without waiting. This is useful for unit testing, because the behavior doesn't depend
  // on time.
  //
  // Returns |ZX_OK| if the dispatcher reaches an idle state.
  // Returns |ZX_ERR_CANCELED| if it is quit.
  zx_status_t RunUntilIdleInternal();

  // Runs the foreground dispatcher.
  //
  // Dispatches events until the |deadline| expires or it is quit.
  // Use |ZX_TIME_INFINITE| to dispatch events indefinitely.
  //
  // If |once| is true, performs a single unit of work then returns.
  //
  // Returns |ZX_OK| if the dispatcher returns after one cycle.
  // Returns |ZX_ERR_TIMED_OUT| if the deadline expired.
  // Returns |ZX_ERR_CANCELED| if it quit.
  zx_status_t RunInternal(zx::time deadline = zx::time::infinite(), bool once = false);

  template <typename Result, typename Callable>
  Result PerformBlockingWorkImpl(Callable&& work);

  // Asserts that the current thread is the same as the initial thread. Used to enforce
  // calls are only made from the main test thread.
  void AssertCurrentThreadIsInitialThread();

  internal::DriverRuntimeEnv env_;

  // We will use this ensure calls are being made only on the main thread.
  std::thread::id initial_thread_id;

  // This is an optional so we can replace it through |ResetForegroundDispatcher|.
  std::optional<fdf_internal::TestSynchronizedDispatcher> foreground_dispatcher_;
  std::list<fdf_internal::TestSynchronizedDispatcher> background_dispatchers_;
  std::set<const void*> shutdown_background_dispatcher_owners_;

  // We will use this prevent doing ShutdownAllDispatchers multiple times.
  bool is_shutdown_ = false;
};

// Internal template implementation details
// This is mirrored from:
// `//sdk/lib/async-loop-testing/cpp/include/lib/async-loop/testing/cpp/real_loop.h`
// ========================================

template <typename PromiseType>
typename PromiseType::result_type DriverRuntime::RunPromise(PromiseType promise) {
  async::Executor e(foreground_dispatcher_->dispatcher());
  std::optional<typename PromiseType::result_type> res;
  e.schedule_task(
      promise.then([&res](typename PromiseType::result_type& v) { res = std::move(v); }));

  // We set |step| to infinity, as the automatic-wake-up timer shouldn't be
  // necessary. A well-behaved promise must always wake up the executor when
  // there's more work to be done.
  RunUntil([&res]() { return res.has_value(); }, zx::duration::infinite());
  return std::move(res.value());
}

template <typename Result, typename Callable>
Result DriverRuntime::PerformBlockingWorkImpl(Callable&& work) {
  fpromise::bridge<Result> bridge;
  std::thread t([completer = std::move(bridge.completer),
                 work = std::forward<Callable>(work)]() mutable { completer.complete_ok(work()); });
  auto defer = fit::defer([&] { t.join(); });
  return RunPromise(bridge.consumer.promise()).take_value();
}

}  // namespace fdf_testing

#endif  // LIB_DRIVER_TESTING_CPP_DRIVER_RUNTIME_H_

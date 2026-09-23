// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_CAPTIVE_THREAD_INCLUDE_LIB_CAPTIVE_THREAD_CAPTIVE_THREAD_H_
#define SRC_LIB_CAPTIVE_THREAD_INCLUDE_LIB_CAPTIVE_THREAD_CAPTIVE_THREAD_H_

#include <lib/fit/function.h>
#include <lib/zx/channel.h>
#include <lib/zx/exception.h>
#include <lib/zx/result.h>
#include <lib/zx/thread.h>
#include <lib/zx/time.h>
#include <zircon/syscalls/debug.h>
#include <zircon/syscalls/exception.h>

#include <atomic>
#include <concepts>
#include <cstdint>
#include <memory>
#include <optional>
#include <ostream>
#include <thread>
#include <tuple>

namespace captive_thread {

// kTrapException is what __builtin_trap() produces.
constexpr zx_excp_type_t kTrapException =
#ifdef __aarch64__
    ZX_EXCP_SW_BREAKPOINT
#elif defined(__x86_64__) || defined(__riscv)
    ZX_EXCP_UNDEFINED_INSTRUCTION
#endif
    ;

// kSingleStepException is what zx_thread_state_single_step_t{1} produces.
constexpr zx_excp_type_t kSingleStepException = ZX_EXCP_HW_BREAKPOINT;

constexpr uint64_t kTrapInstructionSize =
#if defined(__x86_64__) || defined(__riscv_c)
    2
#elif defined(__aarch64__) || defined(__riscv)
    4
#endif
    ;

template <template <class...> class Template>
using OnRegisterTypes = Template<zx_thread_state_general_regs_t,  //
                                 zx_thread_state_fp_regs_t,       //
                                 zx_thread_state_vector_regs_t,   //
                                 zx_thread_state_debug_regs_t>;

template <class... Regs>
struct IsRegistersType {
  template <typename T>
  static constexpr bool value = (std::same_as<T, Regs> || ...);
};

template <typename T>
concept RegistersType = OnRegisterTypes<IsRegistersType>::value<T>;

// CaptiveThread is a large and immovable object.  To move one around, create
// it with std::make_unique<CaptiveThread>(...) and use it via std::unique_ptr.
//
// CaptiveThread is constructed just like a std::thread to launch a thread.
// Its ForceJoin() method forces the thread to exit before doing
// std::thread::join.  The thread is always joined this way on CaptiveThread
// destruction.
//
// Other methods provide for catching exceptions the thread hits or for
// suspending it asynchronously; and for easily accessing its register state
// while it's stopped for an exception or suspension.
class CaptiveThread {
 public:
  // The routine is guaranteed to be called only once, but we don't use
  // fit::callback so as not to complicate allocation issues.
  using Routine = fit::function<void()>;

  // These are the zx::thread::start arguments for StartRaw() and CreateRaw().
  struct Raw {
    uint64_t pc = 0;
    uint64_t sp = 0;
    uint64_t arg1 = 0;
    uint64_t arg2 = 0;
  };

  CaptiveThread(const CaptiveThread&) = delete;
  CaptiveThread(CaptiveThread&& other) noexcept = delete;

  // Create a thread that runs the given function (can be move-only).
  explicit CaptiveThread(Routine);

  // The given function can take any kind of args that can be captured as
  // perfect forwards.
  template <typename F, typename... Args>
    requires(std::invocable<F, Args...> &&  // Anything not already coercible.
             !std::constructible_from<Routine, F, Args...>)
  explicit CaptiveThread(F f, Args&&... args)
      : CaptiveThread(Routine([f = std::move(f), ... args = std::forward<Args>(args)] mutable {
          std::move(f)(std::forward<Args>(args)...);
        })) {}

  // This starts a new raw thread made with zx::thread::create().  The
  // arguments are the initial register values passed to zx::thread::start().
  // With the optional suspend flag set, Suspend() will be done before the
  // thread starts (it's still necessary to use WaitForStop() to examine it).
  //
  // When a "raw" thread is started via CreateRaw() or StartRaw(), it must run
  // code that's pure assembly or otherwise refrains from any interaction with
  // normal ABI code or any libc expectations of any kind.  If it's allowed to
  // run to completion, it must use zx_thread_exit() directly.  When it's
  // forcibly "joined", that won't use std::thread::join() or run any normal
  // C++ or libc thread exit code.  Instead, it will force the thread into an
  // exception state if not already there, and then force it to exit via the
  // exception handling mechanism.
  static zx::result<std::unique_ptr<CaptiveThread>> CreateRaw(  //
      std::string_view name, Raw regs, bool suspended = false);

  // This is like CreateRaw(), but takes ownership of a zx::thread already
  // created but not yet started.  This allows the thread to be created in some
  // special fashion (even in another process), or have properties set,
  // etc. before it starts.  To suspend the thread before it starts, pass in an
  // existing zx::suspend_token for it that's taken over as if from Suspend().
  static zx::result<std::unique_ptr<CaptiveThread>> StartRaw(  //
      zx::thread thread, Raw regs, zx::suspend_token = {});

  // After destruction, the thread is guaranteed to be exited and joined.
  ~CaptiveThread();

  // If the thread is not already exiting, then force it to exit.  Then join
  // with it as in std::thread::join.  Other methods are not necessarily valid
  // after ForceJoin(), but it is always safe to call ForceJoin() again or to
  // call ForceJoin() after BlockUntilSuccess().
  void ForceJoin();

  // True if ForceJoin() or BlockUntilSuccess() has already been called.
  bool Joined() const { return !thread_handle_.is_valid(); }

  // Borrow the thread's kernel handle.  This handle is valid for the life of
  // the CaptiveThread object, even after the actual thread dies.
  zx::unowned_thread thread_handle() const { return thread_handle_.borrow(); }

  // Borrow the handle for the current exception.  This handle is only valid
  // while InException() is true, until ResolveException() is called.
  zx::unowned_exception exception() const { return exception_.borrow(); }

  // Wait for the thread to get an exception or exit.  If this succeeds, then
  // either InException() is true, or the thread has exited.  If the thread is
  // suspended, this will wait until it resumes and hits an exception or exits.
  //
  // On success, the result value is just the `this` pointer.  This return
  // value is accepted by the <lib/captive-thread/testing/matchers.h> gmock
  // matchers for `EXPECT_THAT(thread.WaitForException(), ...);` use in tests.
  zx::result<CaptiveThread*> WaitForException(zx::time deadline = zx::time::infinite()) {
    return Wait(deadline, false);
  }

  // Request a thread suspension.  If it's already stopped in any fashion, this
  // returns immediate success.
  zx::result<> Suspend();

  // Wait for the thread to be stopped in any fashion.  If Suspend() hasn't
  // been called, then this is similar to WaitForException().
  zx::result<CaptiveThread*> WaitForStop(zx::time deadline = zx::time::infinite()) {
    return Wait(deadline, true);
  }

  // This presumes the thread will finish running the function and waits until
  // it has done so.  If the thread gets an exception, it will not be caught.
  // Must not be called when IsStopped().
  void BlockUntilSuccess();

  // Report if the thread is currently stopped in exception or suspension, or
  // has a suspension in progress.  If Suspend() has been called, then
  // InSuspend() and IsStopped() are true even if WaitForStop() is still needed
  // to actually synchronize and be able access registers, etc.
  bool InException() const { return exception_.is_valid(); }
  bool InSuspend() const { return suspend_.is_valid(); }
  bool IsStopped() const { return InException() || InSuspend(); }

  // Return the report for the exception, or std::nullopt if !InException().
  std::optional<zx_exception_report_t> ExceptionReport() const { return exception_report_; }

  // Fetch the thread registers.  This caches the value until the next
  // resumption or SetRegisters(), so it's cheap to call repeatedly.
  template <RegistersType Regs = zx_thread_state_general_regs_t>
  zx::result<Regs> Registers();

  // Modify the thread registers.  This clears any values previously cached and
  // does zx::thread::write_state, so the next Registers<Regs>() call will read
  // the normalized values back with zx::thread::read_state.
  template <RegistersType Regs = zx_thread_state_general_regs_t>
  zx::result<> SetRegisters(const Regs& regs);

  // Resume and resolve the exception so no other handler will see it.  Must be
  // called when InException() is true.
  void ResolveException();

  // Resume from being stopped.  Must be called when IsStopped() is true.  When
  // InException(), this results in cascading to the next exception handler
  // (system crash service, etc.).
  void Resume();

  // As above, but resume with single-step enabled.
  zx::result<> ResolveExceptionSingleStep();
  zx::result<> ResumeSingleStep();

  // This is shorthand for ResolveExceptionSingleStep() and WaitForException().
  zx::result<CaptiveThread*> StepToException(zx::time deadline = zx::time::infinite());

  friend void PrintTo(const CaptiveThread&, std::ostream* os);

 private:
  class FakeStep;

  template <class... T>
  using TupleOfPtrs = std::tuple<std::unique_ptr<T>...>;
  using RegsTuple = OnRegisterTypes<TupleOfPtrs>;

  CaptiveThread() noexcept = default;

  void ResumeInternal();
  zx::result<> StepInternal();
  zx::result<CaptiveThread*> Wait(zx::time deadline, bool suspend_ok);

  Routine routine_;
  std::atomic_int state_;
  zx::thread thread_handle_;
  zx::channel channel_;
  zx::exception exception_;
  zx::suspend_token suspend_;
  zx_thread_state_general_regs_t exit_regs_;
  std::optional<zx_exception_report_t> exception_report_;
  RegsTuple stopped_regs_;
  std::unique_ptr<FakeStep> fake_step_;
  bool singlestep_ = false;

  // Note this member is declared last so others are initialized first.
  // It's only ever std::nullopt in an object created by StartRaw.
  std::optional<std::thread> thread_;
};
static_assert(!std::default_initializable<CaptiveThread>);
static_assert(!std::movable<CaptiveThread>);
static_assert(!std::copyable<CaptiveThread>);

constexpr uint64_t FaultAddress(const zx_exception_report_t& report) {
  const auto& arch_context = report.context.arch.u;
#ifdef __aarch64__
  return arch_context.arm_64.far;
#elifdef __riscv
  return arch_context.riscv_64.tval;
#elifdef __x86_64__
  return arch_context.x86_64.cr2;
#endif
}

}  // namespace captive_thread

#endif  // SRC_LIB_CAPTIVE_THREAD_INCLUDE_LIB_CAPTIVE_THREAD_CAPTIVE_THREAD_H_

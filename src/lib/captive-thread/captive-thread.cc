// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "lib/captive-thread/captive-thread.h"

#include <lib/captive-thread/registers.h>
#include <lib/stdcompat/inplace_vector.h>
#include <zircon/assert.h>
#include <zircon/exception.h>
#include <zircon/status.h>
#include <zircon/threads.h>
#include <zircon/tls.h>

#include <atomic>

#include "fake-step.h"

namespace captive_thread {
namespace {

template <RegistersType Regs>
constexpr uint32_t kRegisterKind = 0;
template <>
constexpr uint32_t kRegisterKind<zx_thread_state_general_regs_t> = ZX_THREAD_STATE_GENERAL_REGS;
template <>
constexpr uint32_t kRegisterKind<zx_thread_state_fp_regs_t> = ZX_THREAD_STATE_FP_REGS;
template <>
constexpr uint32_t kRegisterKind<zx_thread_state_vector_regs_t> = ZX_THREAD_STATE_VECTOR_REGS;
template <>
constexpr uint32_t kRegisterKind<zx_thread_state_debug_regs_t> = ZX_THREAD_STATE_DEBUG_REGS;

// This returns zero after filling in the current register values.  It ensures
// the return value register saved is nonzero.  If these register values are
// all restored, then this call will return a second time.
uint64_t Checkpoint(zx_thread_state_general_regs_t& exit_regs);

#ifdef __aarch64__

static_assert(  // lr and sp are adjacent.
    offsetof(zx_thread_state_general_regs_t, lr) + sizeof(uint64_t) ==
    offsetof(zx_thread_state_general_regs_t, sp));

static_assert(  // pc and cpsr are adjacent.
    offsetof(zx_thread_state_general_regs_t, pc) + sizeof(uint64_t) ==
    offsetof(zx_thread_state_general_regs_t, cpsr));

#if __has_attribute(naked)
[[gnu::naked, clang::no_sanitize("all")]]
uint64_t Checkpoint(zx_thread_state_general_regs_t& exit_regs) {
#endif
  // Report the x30 value as the pc, so the checkpoint is as of our return.
  // Use zero for CPSR.  The return value also is the argument register, so it
  // starts as nonzero.
  __asm__(
#if !__has_attribute(naked)
      R"""(
      .pushsection .text, "axG", %cc[fn]
      .hidden %cc[fn]
      .globl %cc[fn]
      .type %cc[fn], %%function
      %cc[fn]:
      .cfi_startproc
      )"""
#endif
      R"""(
      stp  x0,  x1, [x0, #%cc[r] + (0 * 8)]
      stp  x2,  x3, [x0, #%cc[r] + (2 * 8)]
      stp  x4,  x5, [x0, #%cc[r] + (4 * 8)]
      stp  x6,  x7, [x0, #%cc[r] + (6 * 8)]
      stp  x8,  x9, [x0, #%cc[r] + (8 * 8)]
      stp x10, x11, [x0, #%cc[r] + (10 * 8)]
      stp x12, x13, [x0, #%cc[r] + (12 * 8)]
      stp x14, x15, [x0, #%cc[r] + (14 * 8)]
      stp x16, x17, [x0, #%cc[r] + (16 * 8)]
      stp x18, x19, [x0, #%cc[r] + (18 * 8)]
      stp x20, x21, [x0, #%cc[r] + (20 * 8)]
      stp x22, x23, [x0, #%cc[r] + (22 * 8)]
      stp x24, x25, [x0, #%cc[r] + (24 * 8)]
      stp x26, x27, [x0, #%cc[r] + (26 * 8)]
      stp x28, x29, [x0, #%cc[r] + (28 * 8)]
      mov x16, sp
      mrs x17, TPIDR_EL0
      stp x30, x16, [x0, #%cc[lr]]
      stp x30, xzr, [x0, #%cc[pc]]
      str x17, [x0, #%cc[tpidr]]
      mov x0, xzr
      ret
      )"""
#if !__has_attribute(naked)
      R"""(
      .cfi_endproc
      .size %cc[fn], . - %cc[fn]
      .popsection
      )"""
#endif
      :
      :
#if !__has_attribute(naked)
      [fn] ":"(Checkpoint),
#endif
      [r] "i"(offsetof(zx_thread_state_general_regs_t, r)),
      [lr] "i"(offsetof(zx_thread_state_general_regs_t, lr)),
      [pc] "i"(offsetof(zx_thread_state_general_regs_t, pc)),
      [tpidr] "i"(offsetof(zx_thread_state_general_regs_t, tpidr)));
#if __has_attribute(naked)
}
#endif

#elifdef __riscv

[[gnu::naked, clang::no_sanitize("all")]]
uint64_t Checkpoint(zx_thread_state_general_regs_t& exit_regs) {
  // Report the ra value as the pc, so the checkpoint is as of our return.  The
  // a0 value stored will be the return value on restore; it's already nonzero.
  __asm__(
      R"""(
      sd ra, (a0)
      .irp n,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
        sd x\n, (\n * 8)(a0)
      .endr
      mv a0, zero
      ret
      )""");
}

#elifdef __x86_64__

// The checkpoint saves all the registers when it's called, and later the
// controlling thread writes them all back faithfully while this thread is
// stopped.  However, when this thread resumes, the kernel doesn't always
// restore quite all the registers.  If the thread stops in exception, then the
// register values passed to zx_thread_write_state will indeed all be restored.
// However, if the thread is stopped for suspension while inside a system call,
// rather than either by exception or by a suspension that interrupts the
// thread running user code, then the kernel returns to user mode--putatively
// with the register values passed to zx_thread_write_state--using SYSRET
// rather than IRET.  This instruction unavoidably always clobbers %rcx (with
// the user PC where it resumes) and %r11 (with the %rflags value).  So when
// this thread picks up running at the restored checkpoint location, those two
// register values might not be the ones saved on entry.

[[gnu::naked, clang::no_sanitize("all")]]
uint64_t Checkpoint(zx_thread_state_general_regs_t& exit_regs) {
  // Clobber %rdx with the unsafe SP so it's restored like a register.
  __asm__("mov %%fs:%cc0, %%rdx" : : "i"(ZX_TLS_UNSAFE_SP_OFFSET));

  // Clobber %rax with the address of the label below, and store that as the PC
  // to restore.
  __asm__("lea 0f(%rip), %rax");
  __asm__("mov %%rax, %cc0(%%rdi)" : : "i"(offsetof(zx_thread_state_general_regs_t, rip)));

  // Now clobber %rax with the return address from the stack.
  // This is the %rax value that will be restored later.
  __asm__("mov (%rsp), %rax");

#define SAVE_REG(reg) \
  __asm__("mov %%" #reg ", %cc0(%%rdi)" : : "i"(offsetof(zx_thread_state_general_regs_t, reg)))

  SAVE_REG(rax);
  SAVE_REG(rbx);
  SAVE_REG(rcx);  // Might be clobbered.
  SAVE_REG(rdx);
  SAVE_REG(rsi);
  SAVE_REG(rdi);
  SAVE_REG(rbp);
  SAVE_REG(rsp);
  SAVE_REG(r8);
  SAVE_REG(r9);
  SAVE_REG(r10);
  SAVE_REG(r11);  // Might be clobbered.
  SAVE_REG(r12);
  SAVE_REG(r13);
  SAVE_REG(r14);
  SAVE_REG(r15);
#undef SAVE_REG

  // Don't bother trying to get the true %fs.base or %gs.base values.  Only
  // %fs.base really matters and we can presume the %fs:0 protocol, but cannot
  // presume rdfsbase and rdgsbase instructions.
  __asm__("mov %fs:0, %rax");
  __asm__("mov %%rax, %cc0(%%rdi)" : : "i"(offsetof(zx_thread_state_general_regs_t, fs_base)));
  __asm__("movq $0, %cc0(%%rdi)" : : "i"(offsetof(zx_thread_state_general_regs_t, gs_base)));

  // Clear the return value register and return here.  On resumption below,
  // that register will be reloaded with the saved return address.
  __asm__("xor %eax, %eax");
  __asm__("ret");

  // The checkpoint will be restored to running at this PC.
  // Restore the return address onto the stack.
  __asm__("0: mov %rax, (%rsp)");

  // Restore the unsafe SP value saved / restored in %rdx.
  __asm__("mov %%rdx, %%fs:%cc0" : : "i"(ZX_TLS_UNSAFE_SP_OFFSET));

  __asm__("ret");
}

#endif

// The atomic state starts as 0, then goes to 1 while running the routine, then
// -1 while on the exit path.  The routine is owned by the CaptiveThread only
// so that there is no destructor work or deallocation that this thread might
// fail to do, e.g. resulting in a memory leak.
void RunThread(CaptiveThread::Routine& routine, zx::thread& thread_handle,
               zx_thread_state_general_regs_t& exit_regs, zx::channel& exception_channel,
               std::atomic_int& state) {
  if (Checkpoint(exit_regs) == 0) {
    // Duplicate the thread handle so it can be used safely after exit.
    zx_status_t status = zx::thread::self()->duplicate(ZX_RIGHT_SAME_RIGHTS, &thread_handle);
    ZX_ASSERT_MSG(status == ZX_OK, "duplicate: %s", zx_status_get_string(status));
    ZX_DEBUG_ASSERT(thread_handle.is_valid());

    // Bind the thread exception channel.
    status = thread_handle.create_exception_channel(0, &exception_channel);
    ZX_ASSERT_MSG(status == ZX_OK, "create_exception_channel: %s", zx_status_get_string(status));
    ZX_DEBUG_ASSERT(exception_channel.is_valid());

    // Wake up the constructor that created this thread.
    state.store(1, std::memory_order_release);
    state.notify_one();

    // Run the user function.  If an exception is caught, ForceThread() will
    // restore the registers so Checkpoint() returns a second time, nonzero.
    routine();
  }

  // Record that we're not "running": don't get reset to the exit path twice.
  state.store(-1, std::memory_order_release);
}

void MarkHandled(zx::unowned_exception exception,
                 uint32_t disposition = ZX_EXCEPTION_STATE_HANDLED) {
  zx_status_t status =
      exception->set_property(ZX_PROP_EXCEPTION_STATE, &disposition, sizeof(disposition));
  ZX_ASSERT_MSG(status == ZX_OK, "ZX_PROP_EXCEPTION_STATE: %s", zx_status_get_string(status));
}

template <RegistersType Regs>
auto& StoppedRegs(auto& regs) {
  return std::get<std::unique_ptr<Regs>>(regs);
}

zx::result<> SetSingleStep(zx::unowned_thread thread, bool on) {
  const zx_thread_state_single_step_t kStep = on ? 1 : 0;
  return zx::make_result(thread->write_state(ZX_THREAD_STATE_SINGLE_STEP, &kStep, sizeof(kStep)));
}

}  // namespace

CaptiveThread::~CaptiveThread() { ForceJoin(); }

// Start the thread and wait for it to get ready.  Until it's ready,
// it has exclusive access to exit_regs_ and channel_.
CaptiveThread::CaptiveThread(Routine f)
    : routine_{std::move(f)},
      thread_(std::in_place, RunThread, std::ref(routine_), std::ref(thread_handle_),
              std::ref(exit_regs_), std::ref(channel_), std::ref(state_)) {
  // This synchronizes with the thread filling in thread_handle_, channel_, and
  // exit_regs_; and constitutes reacquiring the lock on those.
  state_.wait(0, std::memory_order_acquire);
}

void CaptiveThread::ForceJoin() {
  if (Joined()) {
    // Already called Join or BlockUntilSuccess.
    return;
  }

  if (!InException()) {
    // The thread might be running, so get it safely stopped.
    if (zx::result result = Suspend(); result.is_error()) {
      // Ignore the error if the thread has already died.  WaitForStop() will
      // see the ZX_THREAD_TERMINATED signal immediately.
      ZX_ASSERT_MSG(result.error_value() == ZX_ERR_BAD_STATE, "zx::task::suspend: %s",
                    result.status_string());
    }

    // Wait for suspension, but it could still hit an exception first.
    zx::result result = WaitForStop();
    ZX_ASSERT_MSG(result.is_ok(), "wait: %s", result.status_string());
  }

  if constexpr (!FakeStep::kImplemented) {
    ZX_DEBUG_ASSERT(!fake_step_);
  } else if (fake_step_) {
    zx::result result = std::exchange(fake_step_, {})->ClearBreakpoints();
    ZX_ASSERT_MSG(result.is_ok(), "clearing fake-step breakpoints: %s", result.status_string());
  }

  auto write_exit_regs = [this] {
    zx_status_t status =
        thread_handle_.write_state(ZX_THREAD_STATE_GENERAL_REGS, &exit_regs_, sizeof(exit_regs_));
    ZX_ASSERT_MSG(status == ZX_OK, "zx::thread::write_state: %s", zx_status_get_string(status));
  };

  if (!thread_) {
    // For a raw thread, it just needs to be in an exception.  If it's not
    // already, then it's suspended and we can get it into an exception by
    // writing the exit_regs_, which is all zero and so gets an immediate page
    // fault for the PC.
    if (!InException()) {
      write_exit_regs();
      zx::result result = WaitForException();
      ZX_ASSERT_MSG(result.is_ok(), "%s", result.status_string());
    }

    // Tell the kernel that resuming from exception means immediate exit.
    // This thread should never run another user instruction.
    MarkHandled(exception_.borrow(), ZX_EXCEPTION_STATE_THREAD_EXIT);

    // Let the thread resume to exit.
    ResumeInternal();

    // Stop handling exceptions before waiting for it to exit.
    // This has to happen after resuming it, however.
    channel_.reset();

    // Now wait for the kernel thread to exit.
    BlockUntilSuccess();
    return;
  }

  // If the thread was asynchronously suspended rather than hitting an
  // exception, then don't perturb it if it's already on the exit path.
  if (InException() || state_.load(std::memory_order_acquire) >= 0) {
    write_exit_regs();
  }

  if (InException()) {
    // After warping the thread to the exit path, ignore the pending exception.
    MarkHandled(exception_.borrow());
  }

  // Unbind the exception channel.  If the thread hits another exception
  // hereafter, that will be a normal crash.
  channel_.reset();

  // Drop the thread handle, indicating it's been joined.
  thread_handle_.reset();

  // Let the thread run its exit path.
  ResumeInternal();

  // Do the normal thread join.
  std::exchange(thread_, {})->join();

  // Clear out any allocations or other destructor work for the routine_ object
  // itself.  The routine_ object was kept alive in the CaptiveThread object so
  // that the thread itself wouldn't be responsible for any cleanup.
  routine_ = nullptr;
}

template <RegistersType Regs>
zx::result<Regs> CaptiveThread::Registers() {
  if (!IsStopped()) {
    return zx::error{ZX_ERR_BAD_STATE};
  }
  auto& cached_regs = StoppedRegs<Regs>(stopped_regs_);
  if (!cached_regs) {
    std::unique_ptr regs = std::make_unique_for_overwrite<Regs>();
    // This can still fail if a suspension has started but not been waited for,
    // but the caller can deal with that.
    zx_status_t status = thread_handle_.read_state(kRegisterKind<Regs>, regs.get(), sizeof(*regs));
    if (status != ZX_OK) {
      return zx::error{status};
    }
    cached_regs = std::move(regs);
  }
  return zx::ok(*cached_regs);
}

template zx::result<zx_thread_state_general_regs_t>
CaptiveThread::Registers<zx_thread_state_general_regs_t>();
template zx::result<zx_thread_state_fp_regs_t>
CaptiveThread::Registers<zx_thread_state_fp_regs_t>();
template zx::result<zx_thread_state_vector_regs_t>
CaptiveThread::Registers<zx_thread_state_vector_regs_t>();
template zx::result<zx_thread_state_debug_regs_t>
CaptiveThread::Registers<zx_thread_state_debug_regs_t>();

template <RegistersType Regs>
zx::result<> CaptiveThread::SetRegisters(const Regs& regs) {
  // Clear any cached values, which are no longer likely to be correct.
  StoppedRegs<Regs>(stopped_regs_).reset();

  return zx::make_result(thread_handle_.write_state(kRegisterKind<Regs>, &regs, sizeof(regs)));
}

template zx::result<> CaptiveThread::SetRegisters<zx_thread_state_general_regs_t>(
    const zx_thread_state_general_regs_t&);
template zx::result<> CaptiveThread::SetRegisters<zx_thread_state_fp_regs_t>(
    const zx_thread_state_fp_regs_t&);
template zx::result<> CaptiveThread::SetRegisters<zx_thread_state_vector_regs_t>(
    const zx_thread_state_vector_regs_t&);
template zx::result<> CaptiveThread::SetRegisters<zx_thread_state_debug_regs_t>(
    const zx_thread_state_debug_regs_t&);

zx::result<> CaptiveThread::Suspend() {
  if (IsStopped()) {
    return zx::ok();
  }
  return zx::make_result(thread_handle_.suspend(&suspend_));
}

void CaptiveThread::ResolveException() {
  ZX_ASSERT(InException());
  MarkHandled(exception_.borrow());
  ResumeInternal();
}

zx::result<> CaptiveThread::ResolveExceptionSingleStep() {
  ZX_ASSERT(InException());
  if (zx::result step = StepInternal(); step.is_error()) {
    return step;
  }
  MarkHandled(exception_.borrow());
  ResumeInternal();
  return zx::ok();
}

void CaptiveThread::Resume() {
  ZX_ASSERT(IsStopped());
  ResumeInternal();
}

zx::result<> CaptiveThread::ResumeSingleStep() {
  ZX_ASSERT(IsStopped());
  if (zx::result step = StepInternal(); step.is_error()) {
    return step;
  }
  ResumeInternal();
  return zx::ok();
}

zx::result<> CaptiveThread::StepInternal() {
  ZX_DEBUG_ASSERT(!singlestep_);

  auto fake = [this] -> zx::result<> {
    if constexpr (FakeStep::kImplemented) {
      zx::result regs = Registers();
      if (regs.is_error()) {
        return regs.take_error();
      }
      return fake_step_->SetBreakpoints(*regs);
    }
    return zx::error{ZX_ERR_NOT_SUPPORTED};
  };

  auto maybe_real = [this, fake] -> zx::result<> {
    zx::result result = SetSingleStep(thread_handle_.borrow(), true);
    if (FakeStep::kImplemented && result.status_value() == ZX_ERR_NOT_SUPPORTED) {
      fake_step_ = std::make_unique<FakeStep>();
      return fake();
    }
    return result;
  };

  zx::result result = fake_step_ ? fake() : maybe_real();
  singlestep_ = result.is_ok();

  return result;
}

void CaptiveThread::ResumeInternal() {
  suspend_.reset();
  exception_.reset();
  exception_report_.reset();

  // Discard any old cached registers.
  stopped_regs_ = RegsTuple{};
}

void CaptiveThread::BlockUntilSuccess() {
  ZX_ASSERT(!IsStopped());
  ZX_ASSERT(!Joined());
  ZX_DEBUG_ASSERT(!singlestep_);
  stopped_regs_ = RegsTuple{};
  fake_step_.reset();
  if (thread_) {
    channel_.reset();
    thread_handle_.reset();
    std::exchange(thread_, {})->join();
  } else {
    zx::result result = WaitForException();
    ZX_ASSERT_MSG(result.is_ok(), "%s", result.status_string());
    ZX_ASSERT_MSG(!exception_report_, "%s",
                  zx_exception_get_string(exception_report_->header.type));
    channel_.reset();
    thread_handle_.reset();
  }
}

zx::result<CaptiveThread*> CaptiveThread::Wait(zx::time deadline, bool suspend_ok) {
  ZX_DEBUG_ASSERT(!Joined());
  if (InException()) {
    return zx::ok(this);
  }
  cpp26::inplace_vector<zx_wait_item_t, 2> items{
      {
          .handle = thread_handle_.get(),
          .waitfor = ZX_THREAD_TERMINATED | (suspend_ok ? ZX_THREAD_SUSPENDED : 0),
      },
  };
  const zx_signals_t& thread_pending = items.front().pending;
  const zx_signals_t* channel_pending = nullptr;
  if (channel_.is_valid()) {
    items.push_back({.handle = channel_.get(), .waitfor = ZX_CHANNEL_READABLE});
    channel_pending = &items.back().pending;
  }
  if (zx_status_t status =
          zx::handle::wait_many(items.data(), static_cast<uint32_t>(items.size()), deadline);
      status != ZX_OK) {
    return zx::error{status};
  }
  if (channel_pending && (*channel_pending & ZX_CHANNEL_READABLE)) {
    exception_report_.emplace();
    zx_status_t status =
        thread_handle_.get_info(ZX_INFO_THREAD_EXCEPTION_REPORT, std::addressof(*exception_report_),
                                sizeof(*exception_report_), nullptr, nullptr);
    if (status != ZX_OK) {
      exception_report_.reset();
      return zx::error{status};
    }
    zx_exception_info_t info;
    uint32_t bytes, handles;
    status = channel_.read(0, &info, exception_.reset_and_get_address(), sizeof(info), 1, &bytes,
                           &handles);
    if (status != ZX_OK) {
      exception_report_.reset();
      return zx::error{status};
    }
    ZX_DEBUG_ASSERT(bytes == sizeof(info));
    ZX_DEBUG_ASSERT(handles == 1);
    ZX_DEBUG_ASSERT(info.type == exception_report_->header.type);
    if (singlestep_) {
      // Reset single-step state after any stop.
      auto clear_step = [this] -> zx::result<> {
        if constexpr (FakeStep::kImplemented) {
          if (fake_step_) {
            // Normalize the report in case it was a FakeStep breakpoint.
            fake_step_->FixupException(*this, *exception_report_);
            return fake_step_->ClearBreakpoints();
          }
        }
        return SetSingleStep(thread_handle_.borrow(), false);
      };
      if (auto result = clear_step(); result.is_error()) {
        return result.take_error();
      }
      singlestep_ = false;
    }
  } else if (thread_pending & ZX_THREAD_SUSPENDED) {
    ZX_DEBUG_ASSERT(suspend_ok);
  } else {
    ZX_DEBUG_ASSERT(thread_pending & ZX_THREAD_TERMINATED);
  }
  return zx::ok(this);
}

void PrintTo(const CaptiveThread& thread, std::ostream* os) {
  if (auto report = thread.ExceptionReport()) {
    *os << "in exception " << zx_exception_get_string(report->header.type);
  } else if (thread.InSuspend()) {
    *os << "in suspension>";
  } else if (thread.Joined()) {
    *os << "already joined";
  } else {
    *os << "no exception";
  }
}

zx::result<CaptiveThread*> CaptiveThread::StepToException(zx::time deadline) {
  if (zx::result step = ResolveExceptionSingleStep(); step.is_error()) {
    return step.take_error();
  }
  return WaitForException(deadline);
}

zx::result<std::unique_ptr<CaptiveThread>> CaptiveThread::CreateRaw(  //
    std::string_view name, Raw regs, bool suspended) {
  zx::thread thread;
  if (zx_status_t status = zx::thread::create(*zx::process::self(), name.data(),
                                              static_cast<uint32_t>(name.size()), 0, &thread);
      status != ZX_OK) {
    return zx::error{status};
  }
  zx::suspend_token token;
  if (suspended) {
    if (zx_status_t status = thread.suspend(&token); status != ZX_OK) {
      return zx::error{status};
    }
  }
  return StartRaw(std::move(thread), regs, std::move(token));
}

zx::result<std::unique_ptr<CaptiveThread>> CaptiveThread::StartRaw(  //
    zx::thread thread_handle, Raw regs, zx::suspend_token token) {
  std::unique_ptr<CaptiveThread> thread{new CaptiveThread};
  thread->thread_handle_ = std::move(thread_handle);
  thread->suspend_ = std::move(token);

  if (zx_status_t status = thread->thread_handle_.create_exception_channel(0, &thread->channel_);
      status != ZX_OK) {
    return zx::error{status};
  }

  if (zx_status_t status = thread->thread_handle_.start(regs.pc, regs.sp, regs.arg1, regs.arg2);
      status != ZX_OK) {
    return zx::error{status};
  }

  return zx::ok(std::move(thread));
}

}  // namespace captive_thread

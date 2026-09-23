// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-options/boot-options.h>
#include <lib/counters.h>
#include <lib/object-constants.h>
#include <lib/user_copy/user_ptr.h>

#include <kernel/ffi.h>
#include <object/process_dispatcher.h>
#include <object/thread_dispatcher.h>

KCOUNTER(thread_legacy_yield, "thread.legacy_yield")

extern "C" {

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE bool cpp_thread_dispatcher_is_current(const ThreadDispatcher* thread) {
  return thread == ThreadDispatcher::GetCurrent();
}

zx_status_t cpp_thread_dispatcher_create(
    ProcessDispatcher* process, uint32_t flags, const char* name_ptr, size_t name_len,
    ffi::Uninitialized<KernelHandle<ThreadDispatcher>>* out_handle,
    ffi::Uninitialized<zx_rights_t>* out_rights) {
  KernelHandle<ThreadDispatcher> handle;
  zx_rights_t rights;
  zx_status_t status =
      ThreadDispatcher::Create(fbl::ImportFromRawPtr(process), flags,
                               ktl::string_view(name_ptr, name_len), &handle, &rights);
  if (status == ZX_OK) {
    out_handle->Initialize(ktl::move(handle));
    out_rights->Initialize(rights);
  }
  return status;
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t cpp_thread_dispatcher_initialize(ThreadDispatcher* thread) {
  return thread->Initialize();
}

zx_status_t cpp_thread_dispatcher_start(ThreadDispatcher* thread, zx_vaddr_t entry,
                                        zx_vaddr_t stack, uint64_t arg1, uint64_t arg2, uint64_t tp,
                                        uint64_t abi_reg, bool ensure_initial_thread) {
  return thread->Start(
      {
          .pc = entry,
          .sp = stack,
          .arg1 = arg1,
          .arg2 = arg2,
          .tp = tp,
          .abi_reg = abi_reg,
      },
      ensure_initial_thread);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_thread_dispatcher_exit_current() { ThreadDispatcher::ExitCurrent(); }

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_thread_dispatcher_kill_current() { ThreadDispatcher::KillCurrent(); }

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_thread_dispatcher_kill(ThreadDispatcher* thread) { thread->Kill(); }

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t cpp_thread_dispatcher_suspend(ThreadDispatcher* thread) {
  return thread->Suspend();
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_thread_dispatcher_resume(ThreadDispatcher* thread) { thread->Resume(); }

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t cpp_thread_dispatcher_restricted_kick(ThreadDispatcher* thread) {
  return thread->RestrictedKick();
}

zx_status_t cpp_thread_dispatcher_read_state(ThreadDispatcher* thread, uint32_t state_kind,
                                             void* buffer, size_t buffer_size) {
  return thread->ReadState(static_cast<zx_thread_state_topic_t>(state_kind),
                           make_user_out_ptr(buffer), buffer_size);
}

zx_status_t cpp_thread_dispatcher_write_state(ThreadDispatcher* thread, uint32_t state_kind,
                                              const void* buffer, size_t buffer_size) {
  if ((state_kind & ZX_THREAD_STATE_DEBUG_REGS) && !BootOptions::Get()->enable_debugging_syscalls) {
    return ZX_ERR_NOT_SUPPORTED;
  }
  return thread->WriteState(static_cast<zx_thread_state_topic_t>(state_kind),
                            make_user_in_ptr(buffer), buffer_size);
}

zx_status_t cpp_thread_dispatcher_set_base_profile(ThreadDispatcher* thread,
                                                   const SchedulerState::BaseProfile* profile) {
  static_assert(sizeof(SchedulerState::BaseProfile) == kSchedulerStateBaseProfileSize,
                "SchedulerState::BaseProfile size mismatch");
  static_assert(alignof(SchedulerState::BaseProfile) == kSchedulerStateBaseProfileAlign,
                "SchedulerState::BaseProfile align mismatch");
  return thread->SetBaseProfile(*profile);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t cpp_thread_dispatcher_set_soft_affinity(ThreadDispatcher* thread,
                                                                      cpu_mask_t mask) {
  return thread->SetSoftAffinity(mask);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_thread_dispatcher_get_info_for_userspace(
    const ThreadDispatcher* thread, ffi::Uninitialized<zx_info_thread_t>* out_info) {
  out_info->Initialize(thread->GetInfoForUserspace());
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t cpp_thread_dispatcher_get_stats_for_userspace(
    ThreadDispatcher* thread, ffi::Uninitialized<zx_info_thread_stats_t>* out_info) {
  return thread->GetStatsForUserspace(out_info->GetAddressUnchecked());
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_thread_dispatcher_get_runtime_stats(
    const ThreadDispatcher* thread, ffi::Uninitialized<zx_info_task_runtime_t>* out_info) {
  out_info->Initialize(thread->GetRuntimeStats());
}

zx_status_t cpp_sys_thread_raise_exception(uint32_t options, zx_excp_type_t type,
                                           const zx_exception_context_t* user_context) {
  if (options != ZX_EXCEPTION_TARGET_JOB_DEBUGGER || type != ZX_EXCP_USER || !user_context) {
    return ZX_ERR_INVALID_ARGS;
  }

  arch_exception_context_t context = {};
  context.user_synth_code = user_context->synth_code;
  context.user_synth_data = user_context->synth_data;

  auto thread = ThreadDispatcher::GetCurrent();
  thread->process()->OnUserExceptionForJobDebugger(thread, &context);
  return ZX_OK;
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t cpp_sys_thread_legacy_yield(uint32_t options) {
  if (options != 0) {
    return ZX_ERR_INVALID_ARGS;
  }
  kcounter_add(thread_legacy_yield, 1);
  Thread::Current::Yield();
  return ZX_OK;
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE ThreadDispatcher::Blocked cpp_thread_dispatcher_set_blocked_reason(
    ThreadDispatcher::Blocked reason) {
  return ThreadDispatcher::SetBlockedReason(reason);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t cpp_thread_dispatcher_get_exception_report(
    const ThreadDispatcher* thread, ffi::Uninitialized<zx_exception_report_t>* out_report) {
  return const_cast<ThreadDispatcher*>(thread)->GetExceptionReport(
      out_report->GetAddressUnchecked());
}

}  // extern "C"

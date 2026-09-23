// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <kernel/ffi.h>
#include <object/handle.h>
#include <object/job_dispatcher.h>
#include <object/process_dispatcher.h>
#include <object/thread_dispatcher.h>
#include <object/vm_address_region_dispatcher.h>
#include <object/vm_object_dispatcher.h>

extern "C" {

zx_status_t cpp_process_dispatcher_create(
    JobDispatcher* job, const char* name_ptr, size_t name_len, uint32_t flags,
    ffi::Uninitialized<KernelHandle<ProcessDispatcher>>* out_handle,
    ffi::Uninitialized<zx_rights_t>* out_rights,
    ffi::Uninitialized<KernelHandle<VmAddressRegionDispatcher>>* out_vmar_handle,
    ffi::Uninitialized<zx_rights_t>* out_vmar_rights) {
  KernelHandle<ProcessDispatcher> handle;
  KernelHandle<VmAddressRegionDispatcher> vmar_handle;
  zx_rights_t rights;
  zx_rights_t vmar_rights;
  zx_status_t status =
      ProcessDispatcher::Create(fbl::ImportFromRawPtr(job), ktl::string_view(name_ptr, name_len),
                                flags, &handle, &rights, &vmar_handle, &vmar_rights);
  if (status == ZX_OK) {
    out_handle->Initialize(ktl::move(handle));
    out_rights->Initialize(rights);
    out_vmar_handle->Initialize(ktl::move(vmar_handle));
    out_vmar_rights->Initialize(vmar_rights);
  }
  return status;
}

zx_status_t cpp_process_dispatcher_create_shared(
    ProcessDispatcher* shared_proc, const char* name_ptr, size_t name_len, uint32_t flags,
    ffi::Uninitialized<KernelHandle<ProcessDispatcher>>* out_proc_handle,
    ffi::Uninitialized<zx_rights_t>* out_proc_rights,
    ffi::Uninitialized<KernelHandle<VmAddressRegionDispatcher>>* out_restricted_vmar_handle,
    ffi::Uninitialized<zx_rights_t>* out_restricted_vmar_rights) {
  ktl::string_view sp(name_ptr, name_len);
  KernelHandle<ProcessDispatcher> proc_handle;
  KernelHandle<VmAddressRegionDispatcher> restricted_vmar_handle;
  zx_rights_t proc_rights;
  zx_rights_t restricted_vmar_rights;
  zx_status_t status = ProcessDispatcher::CreateShared(
      fbl::ImportFromRawPtr(shared_proc), sp, flags, &proc_handle, &proc_rights,
      &restricted_vmar_handle, &restricted_vmar_rights);
  if (status != ZX_OK) {
    return status;
  }
  out_proc_handle->Initialize(ktl::move(proc_handle));
  out_restricted_vmar_handle->Initialize(ktl::move(restricted_vmar_handle));
  out_proc_rights->Initialize(proc_rights);
  out_restricted_vmar_rights->Initialize(restricted_vmar_rights);
  return ZX_OK;
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE ProcessDispatcher* cpp_process_dispatcher_current() {
  return ProcessDispatcher::GetCurrent();
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE bool cpp_process_dispatcher_is_current(const ProcessDispatcher* process) {
  return process == ProcessDispatcher::GetCurrent();
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t cpp_process_dispatcher_start(ProcessDispatcher* process,
                                                           ThreadDispatcher* thread, zx_vaddr_t pc,
                                                           zx_vaddr_t sp, Handle* arg_handle,
                                                           uintptr_t arg2) {
  return process->Start(fbl::ImportFromRawPtr(thread), pc, sp, HandleOwner(arg_handle), arg2);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_process_dispatcher_kill(ProcessDispatcher* process, int64_t retcode) {
  process->Kill(retcode);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t cpp_process_dispatcher_suspend(ProcessDispatcher* process) {
  return process->Suspend();
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_process_dispatcher_resume(ProcessDispatcher* process) {
  process->Resume();
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t cpp_process_dispatcher_make_and_add_handle(
    ProcessDispatcher* process, KernelHandle<Dispatcher>* handle, zx_rights_t rights,
    zx_handle_t* out_handle) {
  return process->MakeAndAddHandle(ktl::move(*handle), rights, out_handle);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t cpp_process_dispatcher_make_and_add_handle_from_ref(
    ProcessDispatcher* process, Dispatcher* dispatcher, zx_rights_t rights,
    zx_handle_t* out_handle) {
  return process->MakeAndAddHandle(fbl::ImportFromRawPtr(dispatcher), rights, out_handle);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t cpp_handle_table_get_dispatcher(
    zx_handle_t handle, ffi::Uninitialized<fbl::RefPtr<Dispatcher>>* out_disp,
    zx_rights_t* out_rights) {
  auto up = ProcessDispatcher::GetCurrent();
  fbl::RefPtr<Dispatcher> disp;
  zx_status_t status = up->handle_table().GetDispatcherAndRights(*up, handle, &disp, out_rights);
  if (status == ZX_OK) {
    out_disp->Initialize(ktl::move(disp));
  }
  return status;
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE Handle* cpp_process_dispatcher_remove_handle(ProcessDispatcher* process,
                                                               zx_handle_t handle_value) {
  return process->handle_table().RemoveHandle(*process, handle_value).release();
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t
cpp_process_dispatcher_enforce_basic_policy(ProcessDispatcher* process, uint32_t policy) {
  return process->EnforceBasicPolicy(policy);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE int64_t
cpp_process_dispatcher_get_timer_slack_policy_amount(const ProcessDispatcher* process) {
  return process->GetTimerSlackPolicy().amount();
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_process_dispatcher_get_timer_slack_policy(
    const ProcessDispatcher* process, TimerSlack* out_slack) {
  *out_slack = process->GetTimerSlackPolicy();
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void* cpp_process_dispatcher_handle_table_lock(const ProcessDispatcher* process) {
  return process->handle_table().get_lock();
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE Handle* cpp_process_dispatcher_handle_table_get_handle_locked(
    ProcessDispatcher* process, zx_handle_t handle_value) TA_NO_THREAD_SAFETY_ANALYSIS {
  return process->handle_table().GetHandleLocked(*process, handle_value);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_info_process_t
cpp_process_dispatcher_get_info(const ProcessDispatcher* process) {
  return process->GetInfo();
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t cpp_process_dispatcher_set_critical_to_job(ProcessDispatcher* process,
                                                                         JobDispatcher* job,
                                                                         bool retcode_nonzero) {
  return process->SetCriticalToJob(fbl::ImportFromRawPtr(job), retcode_nonzero);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t cpp_process_futex_grow_pool(ProcessDispatcher* process) {
  return process->futex_context().GrowFutexStatePool();
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t cpp_process_futex_shrink_pool(ProcessDispatcher* process) {
  process->futex_context().ShrinkFutexStatePool();
  return ZX_OK;
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
[[noreturn]] FFI_ALWAYS_INLINE void cpp_process_dispatcher_exit_current(int64_t retcode) {
  ProcessDispatcher::ExitCurrent(retcode);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE JobDispatcher* cpp_process_dispatcher_job(ProcessDispatcher* process) {
  fbl::RefPtr<JobDispatcher> job = process->job();
  return fbl::ExportToRawPtr(&job);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE VmAspace* cpp_process_dispatcher_aspace_at(ProcessDispatcher* process,
                                                             zx_vaddr_t va) {
  return process->aspace_at(va);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE uintptr_t
cpp_process_dispatcher_get_debug_addr(const ProcessDispatcher* process) {
  return process->get_debug_addr();
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t cpp_process_dispatcher_set_debug_addr(ProcessDispatcher* process,
                                                                    uintptr_t addr) {
  return process->set_debug_addr(addr);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE uintptr_t
cpp_process_dispatcher_get_dyn_break_on_load(const ProcessDispatcher* process) {
  return process->get_dyn_break_on_load();
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t
cpp_process_dispatcher_set_dyn_break_on_load(ProcessDispatcher* process, uintptr_t break_on_load) {
  return process->set_dyn_break_on_load(break_on_load);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE uintptr_t
cpp_process_dispatcher_vdso_base_address(const ProcessDispatcher* process) {
  return process->vdso_base_address();
}

#if ARCH_X86
// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE uintptr_t
cpp_process_dispatcher_hw_trace_context_id(const ProcessDispatcher* process) {
  return process->hw_trace_context_id();
}
#endif

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t cpp_process_attach_aspace_to_thread(ProcessDispatcher* process,
                                                                  Thread* core_thread) {
  VmAspace* aspace = process->normal_aspace();
  aspace->AttachToThread(core_thread);
  return ZX_OK;
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_koid_t cpp_process_get_job_koid(const ProcessDispatcher* process) {
  return process->job()->get_koid();
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t cpp_process_add_initialized_thread(ProcessDispatcher* process,
                                                                 ThreadDispatcher* thread,
                                                                 bool ensure_initial_thread,
                                                                 const UserEntryState* entry) {
  return process->AddInitializedThread(thread, ensure_initial_thread, *entry);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_process_remove_thread(ProcessDispatcher* process,
                                                 ThreadDispatcher* thread) {
  process->RemoveThread(thread);
}

}  // extern "C"

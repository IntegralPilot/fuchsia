// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <kernel/ffi.h>
#include <object/handle.h>
#include <object/job_dispatcher.h>

extern "C" {

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE bool cpp_job_dispatcher_is_root(const JobDispatcher* job) {
  return job == GetRootJobDispatcher().get();
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t cpp_job_dispatcher_create(
    uint32_t flags, JobDispatcher* parent, ffi::Uninitialized<KernelHandle<JobDispatcher>>* handle,
    ffi::Uninitialized<zx_rights_t>* rights) {
  KernelHandle<JobDispatcher> new_handle;
  zx_rights_t new_rights;
  zx_status_t status =
      JobDispatcher::Create(flags, fbl::ImportFromRawPtr(parent), &new_handle, &new_rights);
  if (status == ZX_OK) {
    handle->Initialize(ktl::move(new_handle));
    rights->Initialize(new_rights);
  }
  return status;
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t cpp_job_dispatcher_set_basic_policy_v1(
    JobDispatcher* job, uint32_t mode, const zx_policy_basic_v1_t* policy, size_t count) {
  return job->SetBasicPolicy(mode, policy, count);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t cpp_job_dispatcher_set_basic_policy_v2(
    JobDispatcher* job, uint32_t mode, const zx_policy_basic_v2_t* policy, size_t count) {
  return job->SetBasicPolicy(mode, policy, count);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t cpp_job_dispatcher_set_timer_slack_policy(
    JobDispatcher* job, const zx_policy_timer_slack_t* policy) {
  return job->SetTimerSlackPolicy(*policy);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE JobDispatcher* cpp_job_dispatcher_get_root_job() {
  auto root = GetRootJobDispatcher();
  return fbl::ExportToRawPtr(&root);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE JobDispatcher* cpp_job_dispatcher_parent(const JobDispatcher* job) {
  auto parent = const_cast<JobDispatcher*>(job)->parent();
  return fbl::ExportToRawPtr(&parent);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE uint32_t cpp_job_dispatcher_max_height(const JobDispatcher* job) {
  return job->max_height();
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE bool cpp_job_dispatcher_kill(JobDispatcher* job, int64_t return_code) {
  return job->Kill(return_code);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_job_dispatcher_set_kill_on_oom(JobDispatcher* job, bool value) {
  job->set_kill_on_oom(value);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE bool cpp_job_dispatcher_get_kill_on_oom(const JobDispatcher* job) {
  return job->get_kill_on_oom();
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE bool cpp_job_dispatcher_kill_job_with_kill_on_oom(JobDispatcher* job) {
  return job->KillJobWithKillOnOOM();
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_job_dispatcher_get_info(const JobDispatcher* job,
                                                   zx_info_job_t* info_out) {
  *info_out = job->GetInfo();
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_info_task_runtime_t
cpp_job_dispatcher_get_runtime_stats(const JobDispatcher* job) {
  return job->GetRuntimeStats();
}

namespace {

// Gathers the koids of a job's descendants.
class SimpleJobEnumerator final : public JobEnumerator {
 public:
  // If |jobs| is true, only records job koids; otherwise, only
  // records process koids.
  SimpleJobEnumerator(user_out_ptr<zx_koid_t> ptr, size_t max, bool jobs)
      : jobs_(jobs), ptr_(ptr), max_(max) {}

  size_t get_avail() const { return avail_; }
  size_t get_count() const { return count_; }

 private:
  bool OnJob(JobDispatcher* job) override {
    if (!jobs_) {
      return true;
    }
    return RecordKoid(job->get_koid());
  }

  bool OnProcess(ProcessDispatcher* proc) override {
    if (jobs_) {
      return true;
    }
    // Hide any processes that are both still in the INITIAL state, and have a handle count of 0.
    // Such processes have not yet had their zx_process_create call complete yet, and making it
    // visible and allowing handles to be constructed via object_get_child, could spuriously destroy
    // it. Once a process either has a handle, or has left the initial state, handles can freely be
    // constructed since any additional on_zero_handles invocations will be idempotent.
    // TODO(https://fxbug.dev/42175105): Consider whether long term needing to allow multiple
    // on_zero_handles transitions is the correct strategy.
    if (proc->state() == ProcessDispatcher::State::INITIAL && Handle::Count(*proc) == 0) {
      return true;
    }
    return RecordKoid(proc->get_koid());
  }

  bool RecordKoid(zx_koid_t koid) {
    avail_++;
    if (count_ < max_) {
      // TODO: accumulate batches and do fewer user copies.
      if (ptr_.copy_array_to_user(&koid, 1, count_) != ZX_OK) {
        return false;
      }
      count_++;
    }
    return true;
  }

  const bool jobs_;
  const user_out_ptr<zx_koid_t> ptr_;
  const size_t max_;

  size_t count_ = 0;
  size_t avail_ = 0;
};

}  // namespace

zx_status_t cpp_job_dispatcher_enumerate_children(const JobDispatcher* job, zx_koid_t* user_koids,
                                                  size_t max, bool is_jobs, size_t* out_count,
                                                  size_t* out_avail) {
  user_out_ptr<zx_koid_t> ptr(user_koids);
  SimpleJobEnumerator sje(ptr, max, is_jobs);
  if (!const_cast<JobDispatcher*>(job)->EnumerateChildren(&sje)) {
    return ZX_ERR_INVALID_ARGS;
  }
  *out_count = sje.get_count();
  *out_avail = sje.get_avail();
  return ZX_OK;
}

}  // extern "C"

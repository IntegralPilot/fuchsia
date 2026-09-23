// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_CPU_PROFILER_TARGETS_H_
#define SRC_PERFORMANCE_CPU_PROFILER_TARGETS_H_

#include <elf-search.h>
#include <lib/fit/function.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/job.h>
#include <lib/zx/process.h>
#include <lib/zx/result.h>
#include <lib/zx/thread.h>
#include <zircon/types.h>

#include <memory>
#include <optional>
#include <string_view>
#include <unordered_map>
#include <vector>

#include <src/lib/unwinder/elf_module_cache.h>
#include <src/lib/unwinder/fp_unwinder.h>
#include <src/lib/unwinder/fuchsia.h>
#include <src/lib/unwinder/unwind.h>

#include "memory.h"
#include "symbolization_context.h"

namespace profiler {
inline std::string GetThreadName(const zx::thread& thread) {
  char name_buf[ZX_MAX_NAME_LEN];
  if (zx_status_t status = thread.get_property(ZX_PROP_NAME, name_buf, sizeof(name_buf));
      status != ZX_OK) {
    name_buf[0] = '\0';
  }
  return std::string(name_buf);
}

struct ThreadTarget {
  zx::thread handle;
  zx_koid_t tid;
  std::string name;
  mutable std::optional<uint64_t> restricted_state_addr;
};

// The unwinding library receives pointers and references. We place the relevant structs together to
// ensure they don't get copied/moved and invalidate the references.
struct UnwinderData {
  explicit UnwinderData(const zx::unowned_process& process)
      : memory(process->get()),
        cached_memory(&memory),
        elf_module_cache_(this->modules),
        fp_unwinder(this->elf_module_cache_) {}

  UnwinderData(const UnwinderData&) = delete;
  UnwinderData(UnwinderData&&) = delete;
  UnwinderData& operator=(const UnwinderData&) = delete;
  UnwinderData& operator=(UnwinderData&&) = delete;

  unwinder::FuchsiaMemory memory;
  profiler::CachedModuleMemory cached_memory;
  std::vector<unwinder::Module> modules;

  // Must keep the cache of loaded modules available for the duration of unwinding.
  unwinder::ElfModuleCache elf_module_cache_;
  // mutable, because stepping the unwinder causes state to change
  mutable unwinder::FramePointerUnwinder fp_unwinder;
};

struct ProcessTarget {
  ProcessTarget(zx::process process, zx_koid_t pid, std::string name,
                std::unordered_map<zx_koid_t, ThreadTarget> threads)
      : handle(std::move(process)),
        pid(pid),
        name(std::move(name)),
        threads(std::move(threads)),
        unwinder_data(std::make_unique<UnwinderData>(handle.borrow())) {}

  zx::process handle;
  zx_koid_t pid;
  std::string name;
  std::unordered_map<zx_koid_t, ThreadTarget> threads;
  std::unique_ptr<UnwinderData> unwinder_data;
  mutable std::vector<zx_info_maps_t> cached_mappings;
};

zx::result<std::map<std::vector<std::byte>, profiler::Module>> GetProcessModules(
    const zx::process&, elf_search::Searcher& searcher);

// Given a process, create a process target containing it and all its threads
zx::result<profiler::ProcessTarget> MakeProcessTarget(zx::process process,
                                                      elf_search::Searcher& searcher);

struct JobTarget {
  explicit JobTarget(zx::job job, zx_koid_t job_id, std::span<const zx_koid_t> ancestry)
      : job(std::move(job)), job_id(job_id), ancestry(ancestry.begin(), ancestry.end()) {}
  JobTarget(zx::job job, zx_koid_t job_id, std::unordered_map<zx_koid_t, ProcessTarget> processes,
            std::unordered_map<zx_koid_t, JobTarget> child_jobs,
            std::span<const zx_koid_t> ancestry)
      : job(std::move(job)),
        job_id(job_id),
        processes(std::move(processes)),
        child_jobs(std::move(child_jobs)),
        ancestry(ancestry.begin(), ancestry.end()) {}

  JobTarget(const JobTarget&) = delete;
  JobTarget(JobTarget&&) = default;

  zx::job job;
  zx_koid_t job_id;
  std::unordered_map<zx_koid_t, ProcessTarget> processes;
  std::unordered_map<zx_koid_t, JobTarget> child_jobs;

  // The list of ancestor jobs encountered while traversing starting at root job to this job.
  // Contains the root job, but does not contain this job.
  std::vector<zx_koid_t> ancestry;

  // Do a depth first search to call f on each process in the modeled job tree.
  zx::result<> ForEachProcess(
      const fit::function<zx::result<>(std::span<const zx_koid_t> job_path,
                                       const ProcessTarget& target)>& f) const;
  // Do a depth first search to call f on each child job in the modeled job tree.
  zx::result<> ForEachJob(const fit::function<zx::result<>(const JobTarget& target)>& f) const;

  // Add `job` into the job tree as a child to the nested jobs specified by `ancestry`.
  //
  // Returns zx::ok if the job was successfully added,
  // ZX_ERR_NOT_FOUND if there is no matching ancestry
  // ZX_ERR_ALREADY_EXISTS if there is already an existing job at the location with the same job_id
  //
  // Note: `ancestry` is the jobs that `job` will be placed under and does not include the job_id of
  // `job` itself.
  zx::result<> AddJob(std::span<const zx_koid_t> ancestry, JobTarget&& job);

  // Add `process` into the job tree as a child to the job specified by `job_path`
  //
  // Returns zx::ok if the process was successfully added,
  // ZX_ERR_NOT_FOUND if there is no matching job_path
  // ZX_ERR_ALREADY_EXISTS if there is already an existing process at the location with the same pid
  zx::result<> AddProcess(std::span<const zx_koid_t> job_path, ProcessTarget&& process);

  // Get `process` from the job tree as a child to the job specified by `job_path`
  //
  // Returns zx::ok if the process was found,
  // ZX_ERR_NOT_FOUND if there is no matching job_path or pid
  zx::result<ProcessTarget*> GetProcess(std::span<const zx_koid_t> job_path, zx_koid_t pid);

  // Add `thread` into the job tree as a child to process in job specified by `pid` and `job_path`.
  //
  // Returns zx::ok if the thread was successfully added,
  // ZX_ERR_NOT_FOUND if there is no matching job_path
  // ZX_ERR_ALREADY_EXISTS if there is already an existing thread at the location with the same tid
  zx::result<> AddThread(std::span<const zx_koid_t> job_path, zx_koid_t pid, ThreadTarget&& thread);
  zx::result<> RemoveThread(std::span<const zx_koid_t> job_path, zx_koid_t pid, zx_koid_t tid);

  // Search for the name of the process specified by `pid`.
  std::optional<std::string_view> GetProcessName(zx_koid_t pid) const;

  // Search for the name of the thread specified by `tid`.
  std::optional<std::string_view> GetThreadName(zx_koid_t tid) const;
};

// Given a job, create a job target containing it, its processes, their threads, and its child
// jobs. Additionally, the created job will be given the ancestry specified by `ancestry`.
zx::result<profiler::JobTarget> MakeJobTarget(zx::job job, std::span<const zx_koid_t> ancestry,
                                              elf_search::Searcher& searcher);

// Given a job, create a job target containing it, its processes, their threads, and its child
// jobs. The resulting job will have no parent or ancestors.
zx::result<profiler::JobTarget> MakeJobTarget(zx::job job, elf_search::Searcher& searcher);

class TargetTree {
 public:
  TargetTree() = default;
  TargetTree(const TargetTree&) = delete;
  TargetTree& operator=(const TargetTree&) = delete;

  TargetTree(TargetTree&&) = default;
  TargetTree& operator=(TargetTree&&) = default;

  // Add `job` into top level set of jobs in the tree.
  //
  // Returns zx::ok if the job was successfully added,
  // ZX_ERR_ALREADY_EXISTS if there is already an existing job with the same job_id
  zx::result<> AddJob(JobTarget&& job);

  // Add `thread` into the tree as a child to the process specified by `pid` with no parent job
  //
  // Returns zx::ok if the thread was successfully added,
  // ZX_ERR_NOT_FOUND if there is no process matching `pid`
  // ZX_ERR_ALREADY_EXISTS if there is already an existing thread at the location
  zx::result<> AddThread(zx_koid_t pid, ThreadTarget&& thread);
  zx::result<> RemoveThread(zx_koid_t pid, zx_koid_t tid);

  // Add `process` into the job tree as a process with no parent job
  //
  // Returns zx::ok if the process was successfully added,
  // ZX_ERR_ALREADY_EXISTS if there is already an existing process with the same pid
  zx::result<> AddProcess(ProcessTarget&& process);

  // Add `job` into the job tree as a child to the job specified by `ancestry`.
  //
  // Returns zx::ok if the job was successfully added,
  // ZX_ERR_NOT_FOUND if there is no matching ancestry
  // ZX_ERR_ALREADY_EXISTS if there is already an existing job at the location with the same job_id
  //
  // Note: `ancestry` is the jobs that `job` will be placed under and does not include the job_id of
  // `job` itself.
  zx::result<> AddJob(std::span<const zx_koid_t> ancestry, JobTarget&& job);

  zx::result<> RemoveThread(std::span<const zx_koid_t> job_path, zx_koid_t pid, zx_koid_t tid);

  // Add `thread` into the job tree as a child to the job specified by `job_path` in the process
  // `pid`
  //
  // Returns zx::ok if the thread was successfully added,
  // ZX_ERR_NOT_FOUND if there is no matching job_path
  // ZX_ERR_ALREADY_EXISTS if there is already an existing thread at the location with the same tid
  zx::result<> AddThread(std::span<const zx_koid_t> job_path, zx_koid_t pid, ThreadTarget&& thread);

  // Add `process` into the job tree as a child in the job by `job_path`.
  //
  // Returns zx::ok if the process was successfully added,
  // ZX_ERR_NOT_FOUND if there is no matching job_path
  // ZX_ERR_ALREADY_EXISTS if there is already an existing thread at the location with the same tid
  zx::result<> AddProcess(std::span<const zx_koid_t> job_path, ProcessTarget&& process);

  // Search `pid` as a ProcessTarget from the job tree indexed by `job_path`.
  //
  // ZX_ERR_NOT_FOUND if there is no matching job_path and pid
  zx::result<ProcessTarget*> GetProcess(std::span<const zx_koid_t> job_path, zx_koid_t pid);
  void Clear();

  // Call `f` on each Job in the TargetTree. The order each job is visited is unspecified. If `f`
  // returns an error, the function will short circuit and immediately return the error code without
  // visiting any remaining jobs.
  zx::result<> ForEachJob(const fit::function<zx::result<>(const JobTarget& target)>& f);

  // Call `f` on each top level unparented process as well as every process in each added job. The
  // order each process is visited is unspecified. If `f` returns an error, the function will short
  // circuit and immediately return the error code without visiting any remaining processes.
  zx::result<> ForEachProcess(const fit::function<zx::result<>(std::span<const zx_koid_t> job_path,
                                                               const ProcessTarget& target)>& f);

  // Search for the name of the process specified by `pid`.
  std::optional<std::string_view> GetProcessName(zx_koid_t pid) const;

  // Search for the name of the thread specified by `tid`.
  std::optional<std::string_view> GetThreadName(zx_koid_t tid) const;

 private:
  std::unordered_map<zx_koid_t, JobTarget> jobs_;
  std::unordered_map<zx_koid_t, ProcessTarget> processes_;
};

}  // namespace profiler
#endif  // SRC_PERFORMANCE_CPU_PROFILER_TARGETS_H_

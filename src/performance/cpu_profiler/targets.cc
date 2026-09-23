// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "targets.h"

#include <elf-search.h>
#include <lib/fit/function.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>
#include <lib/zx/job.h>
#include <lib/zx/process.h>
#include <lib/zx/result.h>
#include <lib/zx/thread.h>
#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/types.h>

#include <algorithm>
#include <cstddef>
#include <span>
#include <unordered_map>
#include <utility>
#include <vector>

#include <src/lib/unwinder/module.h>

zx::result<> profiler::JobTarget::ForEachProcess(
    const fit::function<zx::result<>(std::span<const zx_koid_t> job_path,
                                     const ProcessTarget& target)>& f) const {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  std::vector<zx_koid_t> job_path{ancestry.begin(), ancestry.end()};
  job_path.push_back(job_id);
  for (const auto& [_, process] : processes) {
    zx::result<> res = f(job_path, process);
    if (res.is_error()) {
      return res;
    }
  }
  for (const auto& [_, job] : child_jobs) {
    zx::result<> res = job.ForEachProcess(f);
    if (res.is_error()) {
      return res;
    }
  }
  return zx::ok();
}

zx::result<> profiler::JobTarget::ForEachJob(
    const fit::function<zx::result<>(const JobTarget& target)>& f) const {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  zx::result res = f(*this);
  if (res.is_error()) {
    return res;
  }

  for (const auto& [_, job] : child_jobs) {
    zx::result res = job.ForEachJob(f);
    if (res.is_error()) {
      return res;
    }
  }
  return zx::ok();
}

zx::result<> profiler::JobTarget::AddJob(std::span<const zx_koid_t> ancestry, JobTarget&& job) {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  if (ancestry.empty()) {
    zx_koid_t job_id = job.job_id;
    auto [_, emplaced] = child_jobs.try_emplace(job_id, std::move(job));
    return zx::make_result(emplaced ? ZX_OK : ZX_ERR_ALREADY_EXISTS);
  }
  auto it = child_jobs.find(ancestry[0]);
  if (it == child_jobs.end()) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  return it->second.AddJob(ancestry.subspan(1), std::move(job));
}

zx::result<> profiler::JobTarget::AddProcess(std::span<const zx_koid_t> job_path,
                                             ProcessTarget&& process) {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  if (job_path.empty()) {
    zx_koid_t pid = process.pid;
    auto [_, emplaced] = processes.try_emplace(pid, std::move(process));
    return zx::make_result(emplaced ? ZX_OK : ZX_ERR_ALREADY_EXISTS);
  }
  auto next_child = child_jobs.find(job_path[0]);
  if (next_child == child_jobs.end()) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  return next_child->second.AddProcess(job_path.subspan(1), std::move(process));
}

zx::result<profiler::ProcessTarget*> profiler::JobTarget::GetProcess(
    std::span<const zx_koid_t> job_path, zx_koid_t pid) {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  if (job_path.empty()) {
    if (processes.contains(pid)) {
      auto it = processes.find(pid);
      return zx::ok(&it->second);
    }
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  auto next_child = child_jobs.find(job_path[0]);
  if (next_child == child_jobs.end()) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  return next_child->second.GetProcess(job_path.subspan(1), pid);
}

zx::result<> profiler::JobTarget::AddThread(std::span<const zx_koid_t> job_path, zx_koid_t pid,
                                            ThreadTarget&& thread) {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  if (job_path.empty()) {
    auto process = processes.find(pid);
    if (process == processes.end()) {
      return zx::error(ZX_ERR_NOT_FOUND);
    }
    zx_koid_t tid = thread.tid;
    auto [_, emplaced] = process->second.threads.try_emplace(tid, std::move(thread));
    return zx::make_result(emplaced ? ZX_OK : ZX_ERR_ALREADY_EXISTS);
  }

  auto next_child = child_jobs.find(job_path[0]);
  if (next_child == child_jobs.end()) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  return next_child->second.AddThread(job_path.subspan(1), pid, std::move(thread));
}

zx::result<> profiler::JobTarget::RemoveThread(std::span<const zx_koid_t> job_path, zx_koid_t pid,
                                               zx_koid_t tid) {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  if (job_path.empty()) {
    auto process = processes.find(pid);
    if (process == processes.end()) {
      return zx::error(ZX_ERR_NOT_FOUND);
    }
    size_t num_removed = process->second.threads.erase(tid);
    return zx::make_result(num_removed == 1 ? ZX_OK : ZX_ERR_NOT_FOUND);
  }

  auto next_child = child_jobs.find(job_path[0]);
  if (next_child == child_jobs.end()) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  return next_child->second.RemoveThread(job_path.subspan(1), pid, tid);
}

std::optional<std::string_view> profiler::JobTarget::GetProcessName(zx_koid_t pid) const {
  auto it = processes.find(pid);
  if (it != processes.end()) {
    return it->second.name;
  }
  for (const auto& [_, child_job] : child_jobs) {
    if (auto name = child_job.GetProcessName(pid); name.has_value()) {
      return name;
    }
  }
  return std::nullopt;
}

std::optional<std::string_view> profiler::JobTarget::GetThreadName(zx_koid_t tid) const {
  for (const auto& [_, process] : processes) {
    auto it = process.threads.find(tid);
    if (it != process.threads.end()) {
      return it->second.name;
    }
  }
  for (const auto& [_, child_job] : child_jobs) {
    if (auto name = child_job.GetThreadName(tid); name.has_value()) {
      return name;
    }
  }
  return std::nullopt;
}

zx::result<std::vector<zx_koid_t>> GetChildrenTids(const zx::process& process) {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  size_t num_threads;
  zx_status_t status = process.get_info(ZX_INFO_PROCESS_THREADS, nullptr, 0, nullptr, &num_threads);
  if (status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "failed to get process thread info (#threads)";
    return zx::error(status);
  }
  if (num_threads < 1) {
    // A job or process in early initialization may not have threads yet.
    // That's okay, we'll attach to them when they are created.
    return zx::ok(std::vector<zx_koid_t>{});
  }

  auto threads = std::make_unique<zx_koid_t[]>(num_threads);
  size_t records_read;
  status = process.get_info(ZX_INFO_PROCESS_THREADS, threads.get(), num_threads * sizeof(zx_koid_t),
                            &records_read, nullptr);

  if (status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "failed to get process thread info";
    return zx::error(status);
  }

  std::vector<zx_koid_t> children{threads.get(), threads.get() + records_read};
  return zx::ok(children);
}

zx::result<profiler::ProcessTarget> profiler::MakeProcessTarget(zx::process process,
                                                                elf_search::Searcher& searcher) {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  zx_info_handle_basic_t handle_info;
  zx_status_t res =
      process.get_info(ZX_INFO_HANDLE_BASIC, &handle_info, sizeof(handle_info), nullptr, nullptr);
  if (res != ZX_OK) {
    return zx::error(res);
  }
  FX_LOGS(DEBUG) << "Creating process target for " << handle_info.koid << ".";

  char name[ZX_MAX_NAME_LEN];
  if (zx_status_t status = process.get_property(ZX_PROP_NAME, name, sizeof(name));
      status != ZX_OK) {
    name[0] = '\0';
  }

  zx::result<std::vector<zx_koid_t>> children = GetChildrenTids(process);
  if (children.is_error()) {
    return children.take_error();
  }

  std::unordered_map<zx_koid_t, profiler::ThreadTarget> threads;
  for (auto child_tid : *children) {
    zx::thread child_thread;
    zx_status_t res = process.get_child(child_tid, ZX_DEFAULT_THREAD_RIGHTS, &child_thread);
    if (res != ZX_OK) {
      FX_PLOGS(ERROR, res) << "Failed to get handle for child (tid: " << child_tid << ")";
      continue;
    }
    std::string thread_name = profiler::GetThreadName(child_thread);
    threads.try_emplace(child_tid, profiler::ThreadTarget{.handle = std::move(child_thread),
                                                          .tid = child_tid,
                                                          .name = std::move(thread_name)});
  }
  profiler::ProcessTarget process_target{std::move(process), handle_info.koid, std::string(name),
                                         std::move(threads)};

  zx::result<std::map<std::vector<std::byte>, profiler::Module>> modules =
      GetProcessModules(*zx::unowned_process{process_target.handle}, searcher);
  if (modules.is_error()) {
    return zx::error(modules.error_value());
  }
  for (const auto& [build_id, module] : *modules) {
    process_target.unwinder_data->modules.emplace_back(module.vaddr,
                                                       &process_target.unwinder_data->cached_memory,
                                                       unwinder::Module::AddressMode::kProcess);
  }
  return zx::ok(std::move(process_target));
}

zx::result<profiler::JobTarget> profiler::MakeJobTarget(zx::job job,
                                                        std::span<const zx_koid_t> ancestry,
                                                        elf_search::Searcher& searcher) {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  zx_info_handle_basic_t info;
  if (zx_status_t status =
          job.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
      status != ZX_OK) {
    FX_PLOGS(WARNING, status) << "failed to make process_target";
    return zx::error(status);
  }
  zx_koid_t job_id = info.koid;
  FX_LOGS(DEBUG) << "Creating job target  for " << job_id << ".";

  size_t num_child_jobs;
  if (zx_status_t status = job.get_info(ZX_INFO_JOB_CHILDREN, nullptr, 0, nullptr, &num_child_jobs);
      status != ZX_OK) {
    FX_PLOGS(WARNING, status) << "failed to query number of job children";
    return zx::error(status);
  }

  // Provide each of this job's children their ancestry, which is this job's ancestry, prepended to
  // this job's job id.
  std::vector<zx_koid_t> child_job_ancestry{ancestry.begin(), ancestry.end()};
  child_job_ancestry.push_back(job_id);

  // A job can contain child jobs as well as processes directly. We're going to scan through both
  // here to build the job tree.
  //
  // We do need to be a little bit careful here: If a job has short lived processes or child jobs,
  // or we simply get unlucky when a process/job exits, we could query the list child jobs, but find
  // that one or more of the children is gone by the time we query the child itself for its handle.
  // This is especially likely when we're doing system wide profiling and traversing the whole job
  // tree.
  //
  // As such, we want to distinguish between failing an operation on the job we're trying to build
  // the JobTarget for, and failing for a child. If we fail an operation on the job itself, we
  // should abort, the job is no longer accessible to us. However if we fail to query a child, the
  // overall job may still be alive so we want to be resilient and continue on, trying the remaining
  // children.
  std::unordered_map<zx_koid_t, profiler::JobTarget> child_job_targets;
  if (num_child_jobs > 0) {
    auto child_jobs = std::make_unique<zx_koid_t[]>(num_child_jobs);
    if (zx_status_t status = job.get_info(ZX_INFO_JOB_CHILDREN, child_jobs.get(),
                                          num_child_jobs * sizeof(zx_koid_t), nullptr, nullptr);
        status != ZX_OK) {
      FX_PLOGS(WARNING, status) << "failed to get job children";
      return zx::error(status);
    }

    for (size_t i = 0; i < num_child_jobs; i++) {
      zx_koid_t child_koid = child_jobs[i];
      zx::job child_job;
      if (zx_status_t status = job.get_child(child_koid, ZX_DEFAULT_JOB_RIGHTS, &child_job);
          status != ZX_OK) {
        FX_PLOGS(WARNING, status) << "failed to get job: " << child_koid;
        continue;
      }
      zx::result<profiler::JobTarget> child_job_target =
          MakeJobTarget(std::move(child_job), child_job_ancestry, searcher);
      if (child_job_target.is_error()) {
        FX_PLOGS(WARNING, child_job_target.status_value()) << "failed to make job_target";
        continue;
      }
      child_job_target->ancestry = std::move(child_job_ancestry);
      child_job_targets.try_emplace(child_koid, std::move(*child_job_target));
    }
  }

  size_t num_processes;
  if (zx_status_t status = job.get_info(ZX_INFO_JOB_PROCESSES, nullptr, 0, nullptr, &num_processes);
      status != ZX_OK) {
    FX_PLOGS(WARNING, status) << "failed to query number of job processes";
    return zx::error(status);
  }
  std::unordered_map<zx_koid_t, profiler::ProcessTarget> process_targets;
  if (num_processes > 0) {
    auto processes = std::make_unique<zx_koid_t[]>(num_processes);
    if (zx_status_t status = job.get_info(ZX_INFO_JOB_PROCESSES, processes.get(),
                                          num_processes * sizeof(zx_koid_t), nullptr, nullptr);
        status != ZX_OK) {
      FX_PLOGS(WARNING, status) << "failed to get job processes";
      return zx::error(status);
    }

    for (size_t i = 0; i < num_processes; i++) {
      zx_koid_t process_koid = processes[i];
      zx::process process;
      if (zx_status_t status = job.get_child(process_koid, ZX_DEFAULT_PROCESS_RIGHTS, &process);
          status != ZX_OK) {
        FX_PLOGS(WARNING, status) << "failed to get process: " << process_koid;
        continue;
      }
      zx::result<profiler::ProcessTarget> process_target =
          MakeProcessTarget(std::move(process), searcher);

      if (process_target.is_error()) {
        FX_PLOGS(WARNING, process_target.status_value()) << "failed to make process_target";
        continue;
      }
      process_targets.try_emplace(process_koid, std::move(*process_target));
    }
  }
  return zx::ok(profiler::JobTarget{std::move(job), job_id, std::move(process_targets),
                                    std::move(child_job_targets), ancestry});
}

zx::result<profiler::JobTarget> profiler::MakeJobTarget(zx::job job,
                                                        elf_search::Searcher& searcher) {
  return MakeJobTarget(std::move(job), std::span<const zx_koid_t>{}, searcher);
}

zx::result<> profiler::TargetTree::AddJob(JobTarget&& job) {
  return AddJob(std::span<const zx_koid_t>{}, std::move(job));
}

zx::result<> profiler::TargetTree::AddJob(std::span<const zx_koid_t> ancestry, JobTarget&& job) {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  if (ancestry.empty()) {
    zx_koid_t job_id = job.job_id;
    auto [it, emplaced] = jobs_.try_emplace(job_id, std::move(job));
    return zx::make_result(emplaced ? ZX_OK : ZX_ERR_ALREADY_EXISTS);
  }

  zx_koid_t next_child_koid = ancestry[0];
  auto it = jobs_.find(next_child_koid);
  return it == jobs_.end() ? zx::error(ZX_ERR_NOT_FOUND)
                           : it->second.AddJob(ancestry.subspan(1), std::move(job));
}

zx::result<> profiler::TargetTree::AddProcess(ProcessTarget&& process) {
  return AddProcess(std::span<const zx_koid_t>{}, std::move(process));
}

zx::result<> profiler::TargetTree::AddProcess(std::span<const zx_koid_t> job_path,
                                              ProcessTarget&& process) {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  if (job_path.empty()) {
    zx_koid_t pid = process.pid;
    auto [it, emplaced] = processes_.try_emplace(pid, std::move(process));
    return zx::make_result(emplaced ? ZX_OK : ZX_ERR_ALREADY_EXISTS);
  }
  zx_koid_t next_child_koid = job_path[0];
  auto it = jobs_.find(next_child_koid);
  return it == jobs_.end() ? zx::error(ZX_ERR_NOT_FOUND)
                           : it->second.AddProcess(job_path.subspan(1), std::move(process));
}

zx::result<profiler::ProcessTarget*> profiler::TargetTree::GetProcess(
    std::span<const zx_koid_t> job_path, zx_koid_t pid) {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  if (job_path.empty()) {
    if (processes_.contains(pid)) {
      auto it = processes_.find(pid);
      return zx::ok(&it->second);
    }
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  zx_koid_t next_child_koid = job_path[0];
  auto it = jobs_.find(next_child_koid);
  return it == jobs_.end() ? zx::error(ZX_ERR_NOT_FOUND)
                           : it->second.GetProcess(job_path.subspan(1), pid);
}

zx::result<> profiler::TargetTree::AddThread(zx_koid_t pid, ThreadTarget&& thread) {
  return AddThread(std::span<const zx_koid_t>{}, pid, std::move(thread));
}

zx::result<> profiler::TargetTree::RemoveThread(zx_koid_t pid, zx_koid_t tid) {
  return RemoveThread(std::span<const zx_koid_t>{}, pid, tid);
}

zx::result<> profiler::TargetTree::AddThread(std::span<const zx_koid_t> job_path, zx_koid_t pid,
                                             ThreadTarget&& thread) {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  if (job_path.empty()) {
    auto it = processes_.find(pid);
    if (it == processes_.end()) {
      return zx::error(ZX_ERR_NOT_FOUND);
    }
    zx_koid_t tid = thread.tid;
    auto [_, emplaced] = it->second.threads.try_emplace(tid, std::move(thread));
    return zx::make_result(emplaced ? ZX_OK : ZX_ERR_ALREADY_EXISTS);
  }

  zx_koid_t next_child_koid = job_path[0];
  auto it = jobs_.find(next_child_koid);
  return it == jobs_.end() ? zx::error(ZX_ERR_NOT_FOUND)
                           : it->second.AddThread(job_path.subspan(1), pid, std::move(thread));
}

zx::result<> profiler::TargetTree::RemoveThread(std::span<const zx_koid_t> job_path, zx_koid_t pid,
                                                zx_koid_t tid) {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  if (job_path.empty()) {
    auto it = processes_.find(pid);
    if (it == processes_.end()) {
      return zx::error(ZX_ERR_NOT_FOUND);
    }
    size_t num_erased = it->second.threads.erase(tid);
    return zx::make_result(num_erased == 1 ? ZX_OK : ZX_ERR_NOT_FOUND);
  }

  zx_koid_t next_child_koid = job_path[0];
  auto it = jobs_.find(next_child_koid);
  return it == jobs_.end() ? zx::error(ZX_ERR_NOT_FOUND)
                           : it->second.RemoveThread(job_path.subspan(1), pid, tid);
}

void profiler::TargetTree::Clear() {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  jobs_.clear();
  processes_.clear();
}

zx::result<> profiler::TargetTree::ForEachJob(
    const fit::function<zx::result<>(const JobTarget& target)>& f) {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  for (const auto& [_, job] : jobs_) {
    zx::result<> res = job.ForEachJob(f);
    if (res.is_error()) {
      return res;
    }
  }
  return zx::ok();
}

zx::result<> profiler::TargetTree::ForEachProcess(
    const fit::function<zx::result<>(std::span<const zx_koid_t> job_path,
                                     const ProcessTarget& target)>& f) {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  for (const auto& [_, process] : processes_) {
    zx::result res = f(std::span<const zx_koid_t>{}, process);
    if (res.is_error()) {
      return res;
    }
  }
  for (const auto& [_, job] : jobs_) {
    zx::result<> res = job.ForEachProcess(f);
    if (res.is_error()) {
      return res;
    }
  }
  return zx::ok();
}

std::optional<std::string_view> profiler::TargetTree::GetProcessName(zx_koid_t pid) const {
  auto it = processes_.find(pid);
  if (it != processes_.end()) {
    return it->second.name;
  }
  for (const auto& [_, job] : jobs_) {
    if (auto name = job.GetProcessName(pid); name.has_value()) {
      return name;
    }
  }
  return std::nullopt;
}

std::optional<std::string_view> profiler::TargetTree::GetThreadName(zx_koid_t tid) const {
  for (const auto& [_, process] : processes_) {
    auto it = process.threads.find(tid);
    if (it != process.threads.end()) {
      return it->second.name;
    }
  }
  for (const auto& [_, job] : jobs_) {
    if (auto name = job.GetThreadName(tid); name.has_value()) {
      return name;
    }
  }
  return std::nullopt;
}

zx::result<std::map<std::vector<std::byte>, profiler::Module>> profiler::GetProcessModules(
    const zx::process& process, elf_search::Searcher& searcher) {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  std::map<std::vector<std::byte>, profiler::Module> modules;
  zx_status_t search_result =
      searcher.ForEachModule(process, [&modules](const elf_search::ModuleInfo& info) mutable {
        TRACE_DURATION("cpu_profiler", "ForEachModule");
        std::vector<std::byte> build_id;
        std::ranges::transform(info.build_id, std::back_inserter(build_id),
                               [](const uint8_t byte) { return std::byte{byte}; });
        auto [it, inserted] = modules.try_emplace(build_id);
        if (inserted) {
          it->second.module_name = info.name;
          it->second.vaddr = info.vaddr;

          for (const auto& phdr : info.phdrs) {
            if (phdr.p_type != PT_LOAD) {
              continue;
            }
            it->second.loads.push_back({phdr.p_vaddr, phdr.p_memsz, phdr.p_flags});
          }
        }
      });
  return zx::make_result(search_result, std::move(modules));
}

// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "profiler_controller_impl.h"

#include <elf-search.h>
#include <elf.h>
#include <fidl/fuchsia.kernel/cpp/fidl.h>
#include <fidl/fuchsia.sys2/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fit/result.h>
#include <lib/fxt/serializer.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>
#include <lib/zx/handle.h>
#include <lib/zx/job.h>
#include <lib/zx/process.h>
#include <lib/zx/result.h>
#include <lib/zx/thread.h>
#include <lib/zx/time.h>
#include <zircon/errors.h>
#include <zircon/syscalls.h>
#include <zircon/types.h>

#include <algorithm>
#include <cstddef>
#include <memory>
#include <unordered_map>
#include <utility>
#include <vector>

#include <src/lib/fsl/socket/strings.h>
#include <src/lib/unwinder/module.h>

#include "component.h"
#include "fxt_writer.h"
#include "kernel_sampler.h"
#include "sampler.h"
#include "stack_sampler.h"
#include "symbolization_context.h"
#include "targets.h"
#include "taskfinder.h"
#include "test_component.h"
#include "unowned_component.h"

namespace {
zx::result<> PopulateTargets(profiler::TargetTree& tree, TaskFinder::FoundTasks&& tasks,
                             elf_search::Searcher& searcher) {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  for (auto&& [koid, job] : tasks.jobs) {
    TRACE_DURATION("cpu_profiler", "PopulateTargets/EachJob");
    zx::result<profiler::JobTarget> job_target = profiler::MakeJobTarget(std::move(job), searcher);
    if (job_target.is_error()) {
      // A job might exit in the time between us walking the tree and attempting to find its
      // children. Skip it in this case.
      continue;
    }
    if (zx::result res = tree.AddJob(std::move(*job_target)); res.is_error()) {
      FX_PLOGS(ERROR, res.status_value()) << "Failed to add job target";
      return res;
    }
  }

  for (auto&& [koid, process] : tasks.processes) {
    TRACE_DURATION("cpu_profiler", "PopulateTargets/EachProcess");
    zx_info_handle_basic_t handle_info;
    if (process.get_info(ZX_INFO_HANDLE_BASIC, &handle_info, sizeof(handle_info), nullptr,
                         nullptr) != ZX_OK) {
      // A process might exit in the time between us walking the tree and attempting to find its
      // children. Skip it in this case.
      continue;
    }
    zx::result<profiler::ProcessTarget> process_target =
        profiler::MakeProcessTarget(std::move(process), searcher);
    if (process_target.is_error()) {
      continue;
    }

    if (zx::result res = tree.AddProcess(std::move(*process_target)); res.is_error()) {
      FX_PLOGS(ERROR, res.status_value()) << "Failed to add process target";
      return res;
    }
  }
  for (auto&& [koid, thread] : tasks.threads) {
    TRACE_DURATION("cpu_profiler", "PopulateTargets/EachThread");
    zx_info_handle_basic_t handle_info;
    if (thread.get_info(ZX_INFO_HANDLE_BASIC, &handle_info, sizeof(handle_info), nullptr,
                        nullptr) != ZX_OK) {
      continue;
    }

    // If we have a thread, we need to know what its parent process is
    // and then get a handle to it. Unfortunately, the "easiest" way to
    // do this is to walk the job tree again.
    TaskFinder finder;
    finder.AddProcess(handle_info.related_koid);
    zx::result<TaskFinder::FoundTasks> found_tasks = finder.FindHandles();
    if (found_tasks.is_error()) {
      continue;
    }
    if (found_tasks->processes.size() != 1) {
      FX_LOGS(ERROR) << "Found the wrong number of processes for thread: " << thread.get();
      return zx::error(ZX_ERR_NOT_FOUND);
    }
    auto [pid, process] = std::move(found_tasks->processes[0]);

    char name[ZX_MAX_NAME_LEN];
    if (zx_status_t status = process.get_property(ZX_PROP_NAME, name, sizeof(name));
        status != ZX_OK) {
      name[0] = '\0';
    }
    profiler::ProcessTarget process_target{std::move(process), pid, std::string(name),
                                           std::unordered_map<zx_koid_t, profiler::ThreadTarget>{}};
    FX_LOGS(DEBUG) << "Collecting process modules for process " << pid << ".";
    zx::result<std::map<std::vector<std::byte>, profiler::Module>> modules =
        profiler::GetProcessModules(process_target.handle, searcher);
    if (modules.is_error()) {
      return zx::error(modules.error_value());
    }
    for (const auto& [build_id, module] : *modules) {
      process_target.unwinder_data->modules.emplace_back(module.vaddr,
                                                         &process_target.unwinder_data->memory,
                                                         unwinder::Module::AddressMode::kProcess);
    }

    if (zx::result<> res = tree.AddProcess(std::move(process_target)); res.is_error()) {
      // If the process already exists, then we'll just append to the existing one below
      if (res.status_value() != ZX_ERR_ALREADY_EXISTS) {
        FX_PLOGS(ERROR, res.status_value()) << "Failed to add process target";
        return res;
      }
    }

    std::string thread_name = profiler::GetThreadName(thread);
    if (zx::result res =
            tree.AddThread(pid, profiler::ThreadTarget{.handle = std::move(thread),
                                                       .tid = koid,
                                                       .name = std::move(thread_name)});
        res.is_error()) {
      FX_PLOGS(ERROR, res.status_value()) << "Failed to add thread target: " << koid;
      return res;
    }
  }
  return zx::ok();
}
}  // namespace

void profiler::ProfilerControllerImpl::Configure(ConfigureRequest& request,
                                                 ConfigureCompleter::Sync& completer) {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  FX_LOGS(DEBUG) << "Configuring profiler.";
  if (state_ != ProfilingState::Unconfigured) {
    completer.Reply(fit::error(fuchsia_cpu_profiler::SessionConfigureError::kBadState));
    return;
  }
  if (!request.output()) {
    completer.Reply(fit::error(fuchsia_cpu_profiler::SessionConfigureError::kBadSocket));
    return;
  }
  writer_ = std::make_unique<FxtWriter>(std::move(*request.output()));

  if (!request.config() || !request.config()->target()) {
    FX_LOGS(ERROR) << "No Target Specified and System Wide profiling isn't yet implemented!";
    completer.Reply(fit::error(fuchsia_cpu_profiler::SessionConfigureError::kMissingTargetConfigs));
    return;
  }

  if (!request.config()->configs()) {
    FX_LOGS(ERROR) << "No sampling configs specified!";
    completer.Reply(fit::error(fuchsia_cpu_profiler::SessionConfigureError::kMissingSampleConfigs));
    return;
  }

  sample_specs_.clear();
  for (auto&& sampling_config : request.config()->configs().value()) {
    sample_specs_.push_back(std::move(sampling_config));
  }

  // We're given pids/tids/jobids for each of our targets. We'll need handles to each of these
  // targets in order to suspend them and read their memory. We'll walk the root job tree looking
  // for anything that has a koid that matches the ones we've been given.
  TaskFinder finder;
  std::optional<std::pair<zx_koid_t, zx::job>> root_job_info;
  switch (request.config()->target()->Which()) {
    case fuchsia_cpu_profiler::TargetConfig::Tag::kTasks: {
      for (auto& t : request.config()->target()->tasks().value()) {
        switch (t.Which()) {
          case fuchsia_cpu_profiler::Task::Tag::kProcess:
            finder.AddProcess(t.process().value());
            break;
          case fuchsia_cpu_profiler::Task::Tag::kThread:
            finder.AddThread(t.thread().value());
            break;
          case fuchsia_cpu_profiler::Task::Tag::kJob:
            finder.AddJob(t.job().value());
            break;
          case fuchsia_cpu_profiler::Task::Tag::kSystemWide: {
            // We treat the root job like any other job we are asked for:
            //
            // Once we get the handle, we query it for its koid and add it to the list.
            auto client_end = component::Connect<fuchsia_kernel::RootJob>();
            if (client_end.is_error()) {
              completer.Reply(
                  fit::error(fuchsia_cpu_profiler::SessionConfigureError::kInvalidConfiguration));
              FX_PLOGS(ERROR, client_end.error_value())
                  << "System wide profiling was requested, but the "
                     "profiler could not obtain the root job";
              return;
            }
            auto result = fidl::SyncClient(std::move(*client_end))->Get();
            if (result.is_error()) {
              FX_PLOGS(ERROR, result.error_value().status())
                  << "System wide profiling was requested, but the "
                     "profiler could not obtain the root job";
              completer.Reply(
                  fit::error(fuchsia_cpu_profiler::SessionConfigureError::kInvalidConfiguration));
              return;
            }
            zx::job root_job = std::move(result->job());
            zx_info_handle_basic_t handle_info;
            if (zx_status_t res = root_job.get_info(ZX_INFO_HANDLE_BASIC, &handle_info,
                                                    sizeof(handle_info), nullptr, nullptr);
                res != ZX_OK) {
              FX_PLOGS(ERROR, res) << "System wide profiling was requested, but the "
                                   << "profiler could not obtain the root job's koid";
              completer.Reply(
                  fit::error(fuchsia_cpu_profiler::SessionConfigureError::kInvalidConfiguration));
              return;
            }
            root_job_info = std::make_pair(handle_info.koid, std::move(root_job));
            break;
          }
          default:
            FX_LOGS(ERROR) << "Invalid task!";
            completer.Reply(
                fit::error(fuchsia_cpu_profiler::SessionConfigureError::kInvalidConfiguration));
            return;
        }
      }
      break;
    }
    case fuchsia_cpu_profiler::TargetConfig::Tag::kComponent: {
      auto attach_config = request.config()->target()->component();
      switch (attach_config->Which()) {
        case fuchsia_cpu_profiler::AttachConfig::Tag::kLaunchComponent: {
          auto launch_config = attach_config->launch_component();
          if (!launch_config->url()) {
            FX_LOGS(ERROR) << "Cannot launch a component without a specified url!";
            completer.Reply(
                fit::error(fuchsia_cpu_profiler::SessionConfigureError::kMissingComponentUrl));
            return;
          }
          auto url = launch_config->url().value();

          std::string moniker;
          if (launch_config->moniker()) {
            moniker = launch_config->moniker().value();
          } else {
            // If we are launching the component and a moniker isn't specified, default to
            // core/ffx-laboratory.

            // url: fuchsia-pkg://fuchsia.com/package#meta/component.cm
            const size_t name_start = url.find_last_of('/');
            const size_t name_end = url.find_last_of('.');
            if (name_start == std::string::npos || name_end == std::string::npos) {
              FX_LOGS(ERROR) << "Invalid url: " << url;
              completer.Reply(
                  fit::error(fuchsia_cpu_profiler::SessionConfigureError::kInvalidConfiguration));
              return;
            }
            // name: component
            const std::string name = url.substr(name_start + 1, name_end - (name_start + 1));
            moniker = "./core/ffx-laboratory:" + name;
          }

          zx::result<std::unique_ptr<profiler::ComponentTarget>> res =
              profiler::ControlledComponent::Create(url, moniker, component_watcher_);
          if (res.is_error()) {
            FX_PLOGS(INFO, res.error_value())
                << "No access to fuchsia.sys2.LifecycleController.root. Component launching and attaching is disabled";
            completer.Reply(
                fit::error(fuchsia_cpu_profiler::SessionConfigureError::kInvalidConfiguration));
            return;
          }
          component_target_ = std::move(*res);
          break;
        }
        case fuchsia_cpu_profiler::AttachConfig::Tag::kLaunchTest: {
          auto test_config = attach_config->launch_test();
          if (!test_config->url()) {
            FX_LOGS(ERROR) << "Cannot launch a component without a specified url!";
            completer.Reply(
                fit::error(fuchsia_cpu_profiler::SessionConfigureError::kMissingComponentUrl));
            return;
          }

          zx::result<std::unique_ptr<TestComponent>> res =
              TestComponent::Create(dispatcher_, test_config->url().value(),
                                    std::move(test_config->options()), component_watcher_);
          if (res.is_error()) {
            FX_PLOGS(ERROR, res.error_value()) << "Failed to launch test: " << *test_config->url();
            completer.Reply(fit::error(fuchsia_cpu_profiler::SessionConfigureError::kBadState));
            return;
          }
          component_target_ = std::move(*res);
          break;
        }

        case fuchsia_cpu_profiler::AttachConfig::Tag::kAttachToComponentMoniker: {
          auto attach_moniker = attach_config->attach_to_component_moniker();
          zx::result<std::unique_ptr<profiler::ComponentTarget>> res =
              profiler::UnownedComponent::Create(attach_moniker, std::nullopt, component_watcher_);
          if (res.is_error()) {
            FX_PLOGS(ERROR, res.error_value())
                << "Failed to attach to component: " << attach_moniker.value_or("<unspecified>");
            completer.Reply(
                fit::error(fuchsia_cpu_profiler::SessionConfigureError::kInvalidConfiguration));
            return;
          }
          component_target_ = std::move(*res);
          break;
        }
        case fuchsia_cpu_profiler::AttachConfig::Tag::kAttachToComponentUrl: {
          auto attach_url = attach_config->attach_to_component_url();
          zx::result<std::unique_ptr<profiler::ComponentTarget>> res =
              profiler::UnownedComponent::Create(std::nullopt, attach_url, component_watcher_);
          if (res.is_error()) {
            completer.Reply(
                fit::error(fuchsia_cpu_profiler::SessionConfigureError::kInvalidConfiguration));
            return;
          }
          component_target_ = std::move(*res);
          break;
        }
        default: {
          completer.Reply(
              fit::error(fuchsia_cpu_profiler::SessionConfigureError::kInvalidConfiguration));
          return;
        }
      }
      break;
    }
    default:
      completer.Reply(
          fit::error(fuchsia_cpu_profiler::SessionConfigureError::kInvalidConfiguration));
      return;
  }

  targets_.Clear();
  if (root_job_info.has_value()) {
    TaskFinder::FoundTasks tasks;
    tasks.jobs.push_back(std::move(*root_job_info));

    if (auto res = PopulateTargets(targets_, std::move(tasks), searcher_); res.is_error()) {
      FX_PLOGS(ERROR, res.error_value()) << "Populate Targets failed";
      completer.Reply(
          fit::error(fuchsia_cpu_profiler::SessionConfigureError::kInvalidConfiguration));
      return;
    }
  } else if (!finder.Empty()) {
    zx::result<TaskFinder::FoundTasks> handles_result = finder.FindHandles();
    if (handles_result.is_error()) {
      FX_PLOGS(ERROR, handles_result.error_value()) << "Failed to walk job tree";
      completer.Reply(
          fit::error(fuchsia_cpu_profiler::SessionConfigureError::kInvalidConfiguration));
      return;
    }
    if (handles_result->empty()) {
      FX_LOGS(ERROR) << "Found no relevant handles";
      completer.Reply(
          fit::error(fuchsia_cpu_profiler::SessionConfigureError::kInvalidConfiguration));
      return;
    }

    if (auto res = PopulateTargets(targets_, std::move(*handles_result), searcher_);
        res.is_error()) {
      FX_PLOGS(ERROR, res.error_value()) << "Populate Targets failed";
      completer.Reply(
          fit::error(fuchsia_cpu_profiler::SessionConfigureError::kInvalidConfiguration));
      return;
    }
  }

  state_ = ProfilingState::Stopped;
  completer.Reply(fit::ok());
}

void profiler::ProfilerControllerImpl::Start(StartRequest& request,
                                             StartCompleter::Sync& completer) {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  FX_LOGS(DEBUG) << "Starting profiler.";
  if (state_ != ProfilingState::Stopped) {
    completer.Reply(fit::error(fuchsia_cpu_profiler::SessionStartError::kBadState));
    return;
  }
  if (sample_specs_.empty()) {
    FX_LOGS(ERROR) << "No sampling configuration found.";
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }

  const auto& config = sample_specs_[0];
  if (!config.sample().has_value() || !config.sample()->callgraph().has_value() ||
      !config.sample()->callgraph()->strategy().has_value()) {
    FX_LOGS(ERROR) << "Missing callgraph strategy in configuration";
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }

  auto strategy = config.sample()->callgraph()->strategy().value();

  num_samples_ = 0;
  first_sample_timestamp_ = zx::ticks(0);
  pids_seen_.clear();
  tids_seen_.clear();

  FX_LOGS(DEBUG) << "Sending profiler headers.";
  fxt::WriteMagicNumberRecord(writer_.get());
  fxt::WriteInitializationRecord(writer_.get(), zx::ticks::per_second().get());

  SampleCallback on_sample = [this](const profiler::Sample& sample) {
    num_samples_++;
    if (first_sample_timestamp_ == zx::ticks(0)) {
      first_sample_timestamp_ = sample.timestamp;
    }
    if (pids_seen_.insert(sample.pid).second) {
      if (sampler_) {
        if (auto name = sampler_->GetProcessName(sample.pid); name.has_value()) {
          fxt::WriteKernelObjectRecord(
              writer_.get(), fxt::Koid(sample.pid), ZX_OBJ_TYPE_PROCESS,
              fxt::StringRef<fxt::RefType::kInline>(name->data(), name->size()));
        }
      }
    }
    if (tids_seen_.insert(sample.tid).second) {
      if (sampler_) {
        if (auto name = sampler_->GetThreadName(sample.tid); name.has_value()) {
          fxt::WriteKernelObjectRecord(
              writer_.get(), fxt::Koid(sample.tid), ZX_OBJ_TYPE_THREAD,
              fxt::StringRef<fxt::RefType::kInline>(name->data(), name->size()));
        }
      }
    }
    if (!sample.stack_memory.empty()) {
      fxt::WriteLargeBlobRecordWithMetadata(
          writer_.get(), sample.timestamp.get(),
          fxt::StringRef<fxt::RefType::kInline>("cpu_profiler"),
          fxt::StringRef<fxt::RefType::kInline>("stack_sample"),
          fxt::ThreadRef<fxt::RefType::kInline>(fxt::Koid(sample.pid), fxt::Koid(sample.tid)),
          sample.stack_memory.data(), sample.stack_memory.size());
    } else {
      fxt::WriteProfilerBacktraceRecord(
          writer_.get(), sample.timestamp.get(),
          fxt::ThreadRef<fxt::RefType::kInline>(fxt::Koid(sample.pid), fxt::Koid(sample.tid)),
          sample.stack.data(), sample.stack.size() * sizeof(uint64_t));
    }
  };

  if (strategy == fuchsia_cpu_profiler::CallgraphStrategy::kDwarf) {
    sampler_ = fxl::MakeRefCounted<StackSampler>(dispatcher_, std::move(targets_),
                                                 std::move(sample_specs_), std::move(on_sample));
  } else if (strategy == fuchsia_cpu_profiler::CallgraphStrategy::kFramePointer) {
    sampler_ = fxl::MakeRefCounted<KernelSampler>(dispatcher_, std::move(targets_),
                                                  std::move(sample_specs_), std::move(on_sample));
  } else {
    FX_LOGS(ERROR) << "Unsupported callgraph strategy: "
                   << static_cast<int>(fidl::ToUnderlying(strategy));
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }
  sample_specs_.clear();
  targets_.Clear();

  if (component_target_) {
    if (zx::result<> res = component_target_->Start(sampler_->GetWeakPtr()); res.is_error()) {
      FX_PLOGS(ERROR, res.error_value()) << "Failed to start!";
      completer.Close(res.error_value());
      Reset();
      return;
    }
  }

  size_t buffer_size_mb = request.buffer_size_mb().value_or(8);
  zx::result<> start_res = sampler_->Start(buffer_size_mb);
  if (start_res.is_error()) {
    FX_PLOGS(ERROR, start_res.status_value()) << "Failed to start sampler";
    Reset();
    completer.Close(start_res.status_value());
    return;
  }
  state_ = ProfilingState::Running;
  completer.Reply(fit::ok());
}

void profiler::ProfilerControllerImpl::Stop(StopCompleter::Sync& completer) {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  FX_LOGS(DEBUG) << "Stopping profiler.";

  component_watcher_.Clear();
  if (zx::result stop_res = sampler_->Stop(); stop_res.is_error()) {
    FX_PLOGS(ERROR, stop_res.status_value()) << "Sampler failed to stop";
    completer.Close(stop_res.status_value());
    Reset();
    return;
  }

  if (component_target_) {
    if (zx::result<> res = component_target_->Stop(); res.is_error()) {
      FX_PLOGS(WARNING, res.error_value()) << "Failed to stop launched components";
    }

    if (zx::result<> res = component_target_->Destroy(); res.is_error()) {
      FX_PLOGS(WARNING, res.error_value()) << "Failed to destroy launched components";
    }
  }

  std::vector<zx::ticks> inspecting_durations = sampler_->SamplingDurations();

  zx::result<profiler::SymbolizationContext> modules = sampler_->GetContexts();
  if (modules.is_error()) {
    FX_PLOGS(ERROR, modules.status_value()) << "Failed to get modules";
    Reset();
    completer.Close(modules.status_value());
    return;
  }

  fuchsia_cpu_profiler::SessionResult stats{{
      .samples_collected = num_samples_,
      .missing_process_mappings = std::vector<zx_koid_t>(),
  }};

  // Verify that we were able to grab module information for each process we sampled. If we find any
  // processes without associated modules, report them back to the caller.
  std::set<zx_koid_t> pids_with_missing_modules;
  for (const auto& pid : pids_seen_) {
    if (!modules->process_contexts.contains(pid) && !pids_with_missing_modules.contains(pid)) {
      stats.missing_process_mappings()->push_back(pid);
      pids_with_missing_modules.insert(pid);
    }
  }

  if (!inspecting_durations.empty()) {
    auto ticks_per_second = zx::ticks::per_second();
    auto ticks_per_us = ticks_per_second / 1'000'000;

    zx::ticks total_ticks_inspecting;
    for (zx::ticks ticks : inspecting_durations) {
      total_ticks_inspecting += ticks;
    }
    std::ranges::sort(inspecting_durations, [](zx::ticks a, zx::ticks b) { return a < b; });
    zx::ticks mean_inspecting = total_ticks_inspecting / inspecting_durations.size();

    stats.mean_sample_time() = mean_inspecting / ticks_per_us;
    stats.median_sample_time() =
        inspecting_durations[inspecting_durations.size() / 2] / ticks_per_us;
    stats.min_sample_time() = inspecting_durations.front() / ticks_per_us;
    stats.max_sample_time() = inspecting_durations.back() / ticks_per_us;
  }
  // Allow the caller to move on before writing to the socket. This way, if the caller is
  // synchronous, they can start reading from the socket without worrying about deadlocking due to a
  // full socket.
  completer.Reply(std::move(stats));

  FX_LOGS(DEBUG) << "Sending modules.";
  uint16_t next_module_id = 0;
  zx::ticks process_timestamp = first_sample_timestamp_;

  for (const auto& pid : pids_seen_) {
    if (modules->process_contexts.contains(pid)) {
      auto process_modules = modules->process_contexts[pid];
      for (const auto& [build_id, mod] : process_modules) {
        uint16_t module_id = next_module_id++;
        const uint8_t name_size =
            static_cast<uint8_t>(std::min<size_t>(mod.module_name.size(), 255));
        const std::string truncated_name = mod.module_name.substr(0, name_size);
        fxt::WriteProfilerModuleRecord(
            writer_.get(), process_timestamp.get(),
            fxt::ThreadRef<fxt::RefType::kInline>(fxt::Koid(pid), fxt::Koid(0)), module_id,
            truncated_name.c_str(), name_size, reinterpret_cast<const uint8_t*>(build_id.data()),
            build_id.size());

        for (const auto& segment : mod.loads) {
          fxt::WriteProfilerMmapRecord(
              writer_.get(), process_timestamp.get(),
              fxt::ThreadRef<fxt::RefType::kInline>(fxt::Koid(pid), fxt::Koid(0)), module_id,
              mod.vaddr + segment.p_vaddr, segment.p_memsz, segment.p_vaddr,
              static_cast<uint8_t>(segment.p_flags));
        }
      }
    }
  }

  Reset();
}

void profiler::ProfilerControllerImpl::Reset(ResetCompleter::Sync& completer) {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  Reset();
  completer.Reply();
}

void profiler::ProfilerControllerImpl::Reset() {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  sampler_.reset();
  writer_.reset();
  searcher_ = elf_search::Searcher();
  targets_.Clear();
  state_ = ProfilingState::Unconfigured;
  component_target_.reset();
  sample_specs_.clear();
  component_watcher_.Clear();
}

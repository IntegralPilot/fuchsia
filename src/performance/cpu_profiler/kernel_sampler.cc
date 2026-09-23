// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "kernel_sampler.h"

#include <fidl/fuchsia.kernel/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fit/defer.h>
#include <lib/trace/event.h>
#include <zircon/syscalls-next.h>
#include <zircon/syscalls.h>

#include <unordered_set>

#include <trace-reader/reader.h>
#include <trace-reader/records.h>

zx::result<std::unique_ptr<profiler::KernelSamplerSession>>
profiler::KernelSamplerSession::CreateAndInit(const zx_sampler_config_t& config) {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  auto sampling_client_end = component::Connect<fuchsia_kernel::SamplingResource>();
  if (sampling_client_end.is_error()) {
    FX_PLOGS(ERROR, sampling_client_end.error_value())
        << "Failed to get connect to sampling resource";
    return zx::error(sampling_client_end.status_value());
  }
  auto sampling_result = fidl::SyncClient(std::move(*sampling_client_end))->Get();
  if (!sampling_result.is_ok()) {
    FX_LOGS(ERROR) << sampling_result.error_value() << " Failed to get sampling resource";
    return zx::error(sampling_result.error_value().status());
  }

  zx::resource sampling_resource = std::move(sampling_result->resource());

  zx::handle sampler;

  FX_LOGS(DEBUG) << "Creating kernel sampler.";
  if (zx_status_t init_status =
          zx_sampler_create(sampling_resource.get(), 0, &config, sampler.reset_and_get_address());
      init_status != ZX_OK) {
    FX_PLOGS(ERROR, init_status) << "Failed to create the kernel sampler.";
    return zx::error(init_status);
  }

  return zx::ok(std::make_unique<profiler::KernelSamplerSession>(std::move(sampler)));
}

zx::result<> profiler::KernelSamplerSession::Start() {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  if (running_) {
    return zx::error(ZX_ERR_BAD_STATE);
  }
  FX_LOGS(DEBUG) << "Starting kernel sampler.";
  running_ = true;
  return zx::make_result(zx_sampler_start(sampler_.get()));
}

void profiler::KernelSampler::ServiceBuffers() {
  if (session_ && session_->is_running()) {
    service_buffers_task_.set_handler([this]() {
      if (session_ && session_->is_running()) {
        if (zx::result<> res = ForwardBuffers(); res.is_error()) {
          FX_PLOGS(WARNING, res.error_value()) << "Failed to forward buffers";
          return;
        }
        ServiceBuffers();
      }
    });
    service_buffers_task_.PostDelayed(dispatcher_, zx::sec(1));
  }
}

zx::result<> profiler::KernelSamplerSession::Stop() {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  if (!running_) {
    return zx::error(ZX_ERR_BAD_STATE);
  }
  running_ = false;
  return zx::make_result(zx_sampler_stop(sampler_.get()));
}

zx::result<> profiler::KernelSampler::Start(size_t buffer_size_mb) {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  // Verify we support the requested samples
  // We currently only support 1 samplespec, and that's backtraces via frame pointers
  if (sample_specs_.size() != 1) {
    FX_LOGS(ERROR) << "Kernel sampling currently only supports one sampling approach at a time. ("
                   << sample_specs_.size() << " approaches specified)";
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  if (sample_specs_[0].timebase() != fuchsia_cpu_profiler::Counter::WithPlatformIndependent(
                                         fuchsia_cpu_profiler::CounterId::kNanoseconds)) {
    FX_LOGS(ERROR) << "Sampling currently only supports timer based sampling";
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  if (!sample_specs_[0].sample()->callgraph() ||
      !sample_specs_[0].sample()->callgraph()->strategy() ||
      sample_specs_[0].sample()->callgraph()->strategy() !=
          fuchsia_cpu_profiler::CallgraphStrategy::kFramePointer) {
    FX_LOGS(ERROR) << "Sampling currently only supports framepointer based sampling";
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  buffer_size_bytes_ = (1 << 20) * buffer_size_mb;
  zx_sampler_config_t config{
      .period =
          zx::nsec(static_cast<int64_t>(sample_specs_[0].period().value_or(10'000'000))).get(),
      .buffer_size = buffer_size_bytes_};
  zx::result session_result = KernelSamplerSession::CreateAndInit(config);
  if (session_result.is_error()) {
    return session_result.take_error();
  }
  session_ = std::move(session_result).value();

  // Passing a nullptr to zx_sampler_read queries the required buffer size to read out data.
  size_t max_size = 0;
  if (zx_status_t status = zx_sampler_read(session_->BorrowSampler()->get(), nullptr, 0, &max_size);
      status != ZX_OK) {
    return zx::error(status);
  }

  if (max_size == 0) {
    return zx::error(ZX_ERR_BAD_STATE);
  }

  sample_buffer_.resize((max_size + sizeof(uint64_t) - 1) / sizeof(uint64_t));

  FX_LOGS(DEBUG) << "Attaching to known tasks and watching for new ones.";
  zx::result known_threads_res = targets_.ForEachProcess(
      [this](std::span<const zx_koid_t> job_path, const ProcessTarget& p) -> zx::result<> {
        TRACE_DURATION("cpu_profiler", "KernelSampler::Start/ForEachProcess");

        // Before we start sampling the thread, make sure we've recorded information about its
        // process
        CacheModules(p);

        std::vector<zx_koid_t> saved_path{job_path.begin(), job_path.end()};
        auto process_watcher = std::make_unique<ProcessWatcher>(
            p.handle.borrow(),
            [saved_path, this](zx_koid_t pid, zx_koid_t tid, zx::thread t) {
              AddThread(saved_path, pid, tid, std::move(t));
            },
            [saved_path, this](zx_koid_t pid, zx_koid_t tid) {
              RemoveThread(saved_path, pid, tid);
            });

        auto [it, emplaced] = process_watchers_.emplace(p.pid, std::move(process_watcher));
        if (emplaced) {
          zx::result watch_result = it->second->Watch(dispatcher_);
          if (watch_result.is_error()) {
            if (watch_result.error_value() == ZX_ERR_BAD_STATE) {
              FX_LOGS(DEBUG) << "Process terminated before being watched.";
            } else {
              FX_PLOGS(ERROR, watch_result.status_value()) << "Failed to watch process: " << p.pid;
              job_watchers_.clear();
              process_watchers_.clear();
              return watch_result.take_error();
            }
          }
        }
        return zx::ok();
      });
  if (known_threads_res.is_error()) {
    FX_PLOGS(ERROR, known_threads_res.error_value()) << "Failed to set up all known processes.";
    return known_threads_res;
  }

  // If a watched job launches a new process, we want to add it to the set
  zx::result watch_result =
      targets_.ForEachJob([this](const JobTarget& target) { return WatchTarget(target); });
  if (watch_result.is_error()) {
    return watch_result;
  }
  zx::result<> res = session_->Start();
  if (res.is_ok()) {
    ServiceBuffers();
  }
  return res;
}

zx::result<> profiler::KernelSampler::AddTarget(JobTarget&& target) {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  if (session_) {
    if (zx::result<> watch_res = WatchTarget(target); watch_res.is_error()) {
      return watch_res;
    }
    zx::result<> res = target.ForEachProcess(
        [this](std::span<const zx_koid_t> job_path, const ProcessTarget& p) -> zx::result<> {
          TRACE_DURATION("cpu_profiler", "KernelSampler::AddTarget/ForEachProcess");

          // Before we start sampling the thread, make sure we've recorded information about its
          // process
          CacheModules(p);
          return zx::ok();
        });
    if (res.is_error()) {
      return res;
    }
  }
  return targets_.AddJob(std::move(target));
}

zx::result<> profiler::KernelSampler::ForwardBuffers() {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  if (!session_) {
    return zx::ok();
  }
  zx::unowned_handle sampler = session_->BorrowSampler();

  // Flatten the watched threads so that we can filter out the records that aren't relevant.
  std::unordered_set<zx_koid_t> profiled_threads;

  // This will always return zx::ok;
  auto _ = targets_.ForEachProcess(
      [&profiled_threads](std::span<const zx_koid_t> job_path, const ProcessTarget& p) {
        for (const auto& [koid, _] : p.threads) {
          profiled_threads.insert(koid);
        }
        return zx::ok();
      });

  trace::TraceReader::RecordConsumer consume_record = [this, &profiled_threads](trace::Record rec) {
    if (rec.type() != trace::RecordType::kProfiler) {
      FX_LOGS(WARNING) << "Unhandled record type: " << static_cast<uint64_t>(rec.type());
      return;
    }
    const trace::Record::Profiler& profiler = rec.GetProfiler();
    if (profiler.type() != trace::ProfilerRecordType::kBacktrace) {
      FX_LOGS(WARNING) << "Unhandled profiler record type: "
                       << static_cast<uint64_t>(profiler.type());
      return;
    }

    const trace::Record::Profiler::Backtrace& backtrace = profiler.backtrace();
    const zx_koid_t pid = backtrace.process_thread.process_koid();
    const zx_koid_t tid = backtrace.process_thread.thread_koid();
    if (profiled_threads.contains(tid)) {
      if (sample_cb_) {
        sample_cb_({pid, tid, backtrace.backtrace, zx::ticks{backtrace.timestamp}, {}});
      }
    }
  };
  zx_status_t encountered_error = ZX_OK;
  trace::TraceReader::ErrorHandler handle_error = [&encountered_error](std::string_view err) {
    FX_LOGS(ERROR) << "Encountered malformed data: " << err;
    encountered_error = ZX_ERR_BAD_STATE;
  };
  trace::TraceReader reader{std::move(consume_record), std::move(handle_error)};

  size_t bytes_read = 0;
  if (zx_status_t status = zx_sampler_read(sampler->get(), sample_buffer_.data(),
                                           sample_buffer_.size() * sizeof(uint64_t), &bytes_read);
      status != ZX_OK) {
    return zx::error(status);
  }
  if (bytes_read == 0) {
    return zx::ok();
  }

  trace::Chunk chunk{sample_buffer_.data(), bytes_read / sizeof(uint64_t)};
  if (!reader.ReadRecords(chunk)) {
    FX_LOGS(ERROR) << "Buffer data corrupted";
    encountered_error = ZX_ERR_BAD_STATE;
  }

  return zx::make_result(encountered_error);
}

profiler::KernelSampler::~KernelSampler() { std::ignore = Stop(); }

zx::result<> profiler::KernelSampler::Stop() {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  FX_LOGS(DEBUG) << "Stopping kernel sampler.";
  service_buffers_task_.Cancel();
  job_watchers_.clear();
  process_watchers_.clear();
  if (!session_) {
    sample_cb_ = nullptr;
    return zx::ok();
  }
  if (zx::result res = session_->Stop(); res.is_error()) {
    FX_PLOGS(WARNING, res.error_value()) << "Failed to stop";
    session_.reset();
    sample_buffer_.clear();
    sample_buffer_.shrink_to_fit();
    sample_cb_ = nullptr;
    return res;
  }
  zx::result res = ForwardBuffers();
  session_.reset();
  sample_buffer_.clear();
  sample_buffer_.shrink_to_fit();
  sample_cb_ = nullptr;
  return res;
}

void profiler::KernelSampler::AddThread(std::vector<zx_koid_t> job_path, zx_koid_t pid,
                                        zx_koid_t tid, zx::thread t) {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__, "pid", pid, "tid", tid);
  // Before we start sampling the thread, make sure we've recorded information about its process
  if (!contexts_.contains(pid)) {
    zx::result<ProcessTarget*> target = targets_.GetProcess(job_path, pid);
    if (target.is_ok()) {
      CacheModules(**target);
    } else {
      FX_PLOGS(ERROR, target.status_value()) << "Failed to search up process: " << pid;
    }
  }

  // Add the thread so we can later grab its address space and module information for
  // symbolization purposes.
  std::string thread_name = profiler::GetThreadName(t);
  if (zx::result res = targets_.AddThread(
          job_path, pid,
          ThreadTarget{.handle = std::move(t), .tid = tid, .name = std::move(thread_name)});
      res.is_error()) {
    FX_PLOGS(ERROR, res.status_value()) << "Failed to add thread to session: " << tid;
  }
}

void profiler::KernelSampler::RemoveThread(std::vector<zx_koid_t> job_path, zx_koid_t pid,
                                           zx_koid_t tid) {
  // Skip removing the thread, we need its metadata to remain around for when we read through the
  // samples. Once we have streaming and we won't have any more samplings coming from the thread any
  // more, we can properly remove it.
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__, "pid", pid, "tid", tid);
}

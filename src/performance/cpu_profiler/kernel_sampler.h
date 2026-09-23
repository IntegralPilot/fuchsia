// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_CPU_PROFILER_KERNEL_SAMPLER_H_
#define SRC_PERFORMANCE_CPU_PROFILER_KERNEL_SAMPLER_H_

#include <fidl/fuchsia.kernel/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/zx/vmo.h>
#include <zircon/syscalls-next.h>

#include "sampler.h"

namespace profiler {

// Wrapper over zx_sampler_start
class KernelSamplerSession {
 public:
  explicit KernelSamplerSession(zx::handle sampler) : sampler_(std::move(sampler)) {}
  static zx::result<std::unique_ptr<KernelSamplerSession>> CreateAndInit(
      const zx_sampler_config_t& config);

  zx::result<> Start();
  zx::result<> Stop();
  zx::unowned_handle BorrowSampler() { return zx::unowned_handle(sampler_.get()); }

  bool is_running() const { return running_; }

  ~KernelSamplerSession() = default;

 private:
  bool running_ = false;
  zx::handle sampler_;
};

class KernelSampler : public Sampler {
 public:
  explicit KernelSampler(async_dispatcher_t* dispatcher, TargetTree&& targets,
                         std::vector<fuchsia_cpu_profiler::SamplingConfig> sampling_specs,
                         SampleCallback sample_cb = nullptr)
      : Sampler(dispatcher, std::move(targets), std::move(sampling_specs), std::move(sample_cb)) {}

  zx::result<> AddTarget(JobTarget&& target) override;
  zx::result<> Start(size_t buffer_size_mb) override;
  zx::result<> Stop() override;
  ~KernelSampler() override;

 private:
  void AddThread(std::vector<zx_koid_t> job_path, zx_koid_t pid, zx_koid_t tid,
                 zx::thread t) override;
  void RemoveThread(std::vector<zx_koid_t> job_path, zx_koid_t pid, zx_koid_t tid) override;
  zx::result<> ForwardBuffers();
  void ServiceBuffers();

  std::unique_ptr<KernelSamplerSession> session_;
  size_t buffer_size_bytes_;
  async::TaskClosure service_buffers_task_;
  std::vector<uint64_t> sample_buffer_;
};
}  // namespace profiler
#endif  // SRC_PERFORMANCE_CPU_PROFILER_KERNEL_SAMPLER_H_

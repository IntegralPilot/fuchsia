// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/memory/metrics/capture_strategy.h"

#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>
#include <zircon/syscalls/object.h>
#include <zircon/types.h>

#include <algorithm>
#include <iterator>
#include <optional>
#include <tuple>

#include "src/developer/memory/metrics/capture.h"

namespace memory {
namespace {
// GetInfoVector executes an OS::GetInfo call that outputs a list of element inside |buffer|,
// ensuring that |buffer| is big enough to receive the list of elements. |GetInfoVector| returns
// the status of the call and the number of elements effectively returned.
template <typename T>
zx::result<size_t> GetInfoVector(OS& os, zx_handle_t handle, uint32_t topic,
                                 std::vector<T>& buffer) {
  size_t num_entries = 0, available_entries = 0;
  zx_status_t s = os.GetInfo(handle, topic, buffer.data(), buffer.size() * sizeof(T), &num_entries,
                             &available_entries);
  if (s != ZX_OK) {
    return zx::error(s);
  }
  if (num_entries == available_entries) {
    return zx::ok(num_entries);
  }
  buffer.resize(available_entries);
  s = os.GetInfo(handle, topic, buffer.data(), buffer.size() * sizeof(T), &num_entries,
                 &available_entries);
  if (s != ZX_OK) {
    return zx::error(s);
  }
  return zx::ok(num_entries);
}

}  // namespace

zx_status_t BaseCaptureStrategy::OnNewProcess(OS& os, Process process, zx::handle process_handle) {
  size_t num_vmos;
  {
    TRACE_DURATION("memory_metrics", "BaseCaptureStrategy::OnNewProcess::GetVMOs");
    auto result =
        GetInfoVector<zx_info_vmo_t>(os, process_handle.get(), ZX_INFO_PROCESS_VMOS, vmos_);
    // We don't want to show processes for which we don't have data (e.g. because they exited).
    if (result.is_error()) {
      return result.error_value();
    }
    num_vmos = result.value();
  }
  std::unordered_map<zx_koid_t, const zx_info_vmo_t*> unique_vmos;
  {
    TRACE_DURATION("memory_metrics", "BaseCaptureStrategy::OnNewProcess::UniqueProcessVMOs");
    unique_vmos.reserve(num_vmos);
    for (size_t i = 0; i < num_vmos; i++) {
      const auto& vmo_info = vmos_[i];
      unique_vmos.try_emplace(vmo_info.koid, &vmo_info);
    }
  }
  {
    TRACE_DURATION("memory_metrics", "BaseCaptureStrategy::OnNewProcess::UniqueVMOs");
    process.vmos.reserve(unique_vmos.size());
    for (const auto& [vmo_koid, vmo] : unique_vmos) {
      koid_to_vmo_.try_emplace(vmo_koid, *vmo);
      process.vmos.push_back(vmo_koid);
    }
  }
  zx_koid_t process_koid = process.koid;
  koid_to_process_[process_koid] = std::move(process);
  return ZX_OK;
}

std::pair<std::unordered_map<zx_koid_t, Process>, std::unordered_map<zx_koid_t, Vmo>>
BaseCaptureStrategy::Finalize(OS& os, BaseCaptureStrategy&& base_capture_strategy) {
  return {std::move(base_capture_strategy.koid_to_process_),
          std::move(base_capture_strategy.koid_to_vmo_)};
}

StarnixCaptureStrategy::StarnixCaptureStrategy(std::string process_name)
    : process_name_(std::move(process_name)) {}

zx_status_t StarnixCaptureStrategy::OnNewProcess(OS& os, Process process,
                                                 zx::handle process_handle) {
  if (process_name_ == process.name) {
    starnix_jobs_[process.job].kernel_koid = process.koid;
  }

  zx_koid_t process_koid = process.koid;
  process_handles_[process_koid] = std::move(process_handle);
  koid_to_process_[process_koid] = std::move(process);
  return ZX_OK;
}

zx::result<std::tuple<std::unordered_map<zx_koid_t, Process>, std::unordered_map<zx_koid_t, Vmo>>>
StarnixCaptureStrategy::Finalize(OS& os, StarnixCaptureStrategy&& starnix_capture_strategy) {
  TRACE_DURATION("memory_metrics", "StarnixCaptureStrategy::Finalize");

  BaseCaptureStrategy base;

  // The cutoff address between the restricted space and the Starnix kernel depends on the
  // architecture. We rely on the fact that shared processes, as used by Starnix, have two root
  // VMARs, one for the restricted space and one for the Starnix kernel, to determine this cutoff at
  // runtime, instead of having it hardcoded.
  std::optional<zx_vaddr_t> starnix_kernel_cutoff = std::nullopt;
  // Capture the data for each process.
  for (auto& [_, process] : starnix_capture_strategy.koid_to_process_) {
    auto starnix_proc = starnix_capture_strategy.starnix_jobs_.find(process.job);
    if (starnix_proc == starnix_capture_strategy.starnix_jobs_.end()) {
      zx_koid_t koid = process.koid;
      zx_status_t s = base.OnNewProcess(os, std::move(process),
                                        std::move(starnix_capture_strategy.process_handles_[koid]));
      // No error or a process-specific error (e.g.: the process exited), we continue.
      if (s != ZX_OK && s != ZX_ERR_BAD_STATE) {
        return zx::error(s);
      }
      continue;
    }

    // This is the first process in this Starnix job. We get the list of all VMOs in that job only
    // once as we assume this list is shared by all processes in that job.
    if (!starnix_proc->second.vmos_retrieved) {
      TRACE_DURATION("memory_metrics", "StarnixCaptureStrategy::Finalize::StarnixVMOs");
      std::vector<zx_info_vmo_t> vmos;
      auto result = GetInfoVector(os, starnix_capture_strategy.process_handles_[process.koid].get(),
                                  ZX_INFO_PROCESS_VMOS, vmos);
      if (result.status_value() == ZX_ERR_BAD_STATE) {
        continue;
      }
      if (result.is_error()) {
        return result.take_error();
      }
      starnix_proc->second.vmos_retrieved = true;

      // We fill |unmapped_vmos| with all the known VMOs of this process group. Mapped VMOs will be
      // removed from |unmapped_vmos| later, as we learn of the mappings.
      size_t num_vmos = result.value();
      for (size_t i = 0; i < num_vmos; i++) {
        auto [it, is_new] = starnix_proc->second.unmapped_vmos.insert(vmos[i].koid);
        // If we have already seen the VMO in this process, then we have seen it globally and we
        // don't need to insert it again.
        if (is_new) {
          starnix_capture_strategy.koid_to_vmo_.try_emplace(vmos[i].koid, vmos[i]);
        }
      }
    }

    {
      TRACE_DURATION("memory_metrics", "StarnixCaptureStrategy::Finalize::StarnixMappings");
      auto result = GetInfoVector(os, starnix_capture_strategy.process_handles_[process.koid].get(),
                                  ZX_INFO_PROCESS_MAPS, starnix_capture_strategy.mappings_);
      if (result.status_value() == ZX_ERR_BAD_STATE) {
        continue;
      }
      if (result.is_error()) {
        return result.take_error();
      }

      size_t num_mappings = result.value();
      for (size_t i = 0; i < num_mappings; i++) {
        const auto& mapping = starnix_capture_strategy.mappings_[i];
        if (!starnix_kernel_cutoff.has_value() && mapping.type == ZX_INFO_MAPS_TYPE_VMAR &&
            mapping.depth == 1) {
          starnix_kernel_cutoff = mapping.base + mapping.size;
        }
        if (mapping.type == ZX_INFO_MAPS_TYPE_MAPPING) {
          if (!starnix_capture_strategy.koid_to_vmo_.contains(mapping.u.mapping.vmo_koid)) {
            // It is a new VMO that we haven't captured. This can happen if the list of VMOs change
            // while we do the data collection.
            continue;
          }
          FX_DCHECK(starnix_kernel_cutoff.has_value())
              << "starnix_kernel_cutoff should have been set";
          if (mapping.base >= starnix_kernel_cutoff) {
            // This is a Starnix kernel mapping.
            starnix_proc->second.kernel_mapped_vmos.insert(mapping.u.mapping.vmo_koid);
          } else {
            // This is a restricted space mapping.
            starnix_proc->second.process_mapped_vmos[process.koid].insert(
                mapping.u.mapping.vmo_koid);
          }
          starnix_proc->second.unmapped_vmos.erase(mapping.u.mapping.vmo_koid);
        }
      }
    }
  }

  auto [base_koid_to_process, base_koid_to_vmo] =
      BaseCaptureStrategy::Finalize(os, std::move(base));

  // Both |koid_to_process_| and |base_koid_to_process| will contain process entries for
  // non-Starnix processes. However, only the |base_koid_to_process| entries will be filled with
  // the non-Starnix process VMOs. This calls adds the entries for Starnix processes to
  // |base_koid_to_process|, ready to be filled by the loop below..
  base_koid_to_process.merge(starnix_capture_strategy.koid_to_process_);
  base_koid_to_vmo.merge(starnix_capture_strategy.koid_to_vmo_);

  for (auto& [_, starnix_job] : starnix_capture_strategy.starnix_jobs_) {
    for (auto& [process_koid, vmos] : starnix_job.process_mapped_vmos) {
      std::ranges::move(vmos, std::back_inserter(base_koid_to_process[process_koid].vmos));
    }
    std::ranges::move(starnix_job.kernel_mapped_vmos,
                      std::back_inserter(base_koid_to_process[starnix_job.kernel_koid].vmos));
    std::ranges::move(starnix_job.unmapped_vmos,
                      std::back_inserter(base_koid_to_process[starnix_job.kernel_koid].vmos));
  }

  return zx::ok(std::make_tuple(std::move(base_koid_to_process), std::move(base_koid_to_vmo)));
}

}  // namespace memory

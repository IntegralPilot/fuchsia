// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_MEMORY_METRICS_CAPTURE_STRATEGY_H_
#define SRC_DEVELOPER_MEMORY_METRICS_CAPTURE_STRATEGY_H_

#include <lib/zx/result.h>
#include <zircon/syscalls/object.h>
#include <zircon/types.h>

#include <tuple>
#include <unordered_set>

#include <src/lib/fxl/macros.h>

#include "src/developer/memory/metrics/capture.h"

namespace memory {

const char STARNIX_KERNEL_PROCESS_NAME[] = "starnix_kernel.cm";

// Extracts VMO information out of a process tree.
//
// Typically, iterate through each process reported on by the
// |ZX_INFO_PROCESS_VMOS| zx_object_info topic.
//
// Note: To use this class, call |OnNewProcess| for all processes you intend to
// track, then call |Finalize|.
class BaseCaptureStrategy {
 public:
  BaseCaptureStrategy() = default;

  // To be called with each |Process| that needs to be tracked.
  zx_status_t OnNewProcess(OS& os, Process process, zx::handle process_handle);

  // Consumes the |BaseCaptureStrategy| to produce a pair of mappings from
  // |zx_koid_t| to known |Process|es and |Vmo|s.
  static std::pair<std::unordered_map<zx_koid_t, Process>, std::unordered_map<zx_koid_t, Vmo>>
  Finalize(OS& os, BaseCaptureStrategy&& base_capture_strategy);

 private:
  std::unordered_map<zx_koid_t, Process> koid_to_process_;
  std::unordered_map<zx_koid_t, Vmo> koid_to_vmo_;
  std::vector<zx_info_vmo_t> vmos_;

  FXL_DISALLOW_COPY_AND_ASSIGN(BaseCaptureStrategy);
};

// Extracts VMO information out of a process tree.
//
// When a job contains a process named |process_name|, assume all processes within that job are
// shared processes. This strategy attributes the VMOs of processes of these jobs as follows:
//  - Memory mapped to the private, restricted address space of a process is attributed to this
//  process;
//  - Memory mapped into the shared address space is attributed to the process |process_name|;
//  - Handles of unmapped VMOs are attributed to the process |process_name|;
//  - Handles of mapped VMOs are not taken into account (their mapping is).
// For all other processes, this delegates to |BaseCaptureStrategy|.
//
// Note: usage is similar to |BaseCaptureStrategy|; see its documentation.
class StarnixCaptureStrategy {
 public:
  explicit StarnixCaptureStrategy(std::string process_name = STARNIX_KERNEL_PROCESS_NAME);

  zx_status_t OnNewProcess(OS& os, Process process, zx::handle process_handle);
  static zx::result<
      std::tuple<std::unordered_map<zx_koid_t, Process>, std::unordered_map<zx_koid_t, Vmo>>>
  Finalize(OS& os, StarnixCaptureStrategy&& starnix_capture_strategy);

 private:
  std::unordered_map<zx_koid_t, zx::handle> process_handles_;
  std::unordered_map<zx_koid_t, Process> koid_to_process_;
  std::unordered_map<zx_koid_t, Vmo> koid_to_vmo_;

  struct StarnixJob {
    zx_koid_t kernel_koid;
    std::unordered_set<zx_koid_t> kernel_mapped_vmos;
    std::unordered_map<zx_koid_t, std::unordered_set<zx_koid_t>> process_mapped_vmos;
    std::unordered_set<zx_koid_t> unmapped_vmos;
    bool vmos_retrieved = false;
  };

  const std::string process_name_;

  std::vector<zx_info_maps_t> mappings_;
  std::unordered_map<zx_koid_t, StarnixJob> starnix_jobs_;

  FXL_DISALLOW_COPY_AND_ASSIGN(StarnixCaptureStrategy);
};

}  // namespace memory

#endif  // SRC_DEVELOPER_MEMORY_METRICS_CAPTURE_STRATEGY_H_

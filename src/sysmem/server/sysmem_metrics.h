// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SYSMEM_SERVER_SYSMEM_METRICS_H_
#define SRC_SYSMEM_SERVER_SYSMEM_METRICS_H_

#include <lib/sys/cpp/service_directory.h>

#include <src/lib/metrics_buffer/metrics_buffer.h>

#include <src/sysmem/metrics/metrics.cb.h>

namespace sys {
class ServiceDirectory;
}  // namespace sys

class SysmemMetrics {
 public:
  SysmemMetrics();
  cobalt::MetricsBuffer& metrics_buffer();

  void LogUnusedPageCheck(sysmem_metrics::UnusedPageCheckMetricDimensionEvent event);
  void LogUnusedPageCheckCounts(uint32_t succeeded_count, uint32_t failed_count);

  // This is called once a close_weak_asap (per-buffer_index) has taken 5s (so far), regardless of
  // whether all the weak VMOs are ever closed after that.
  void LogCloseWeakAsapTakingTooLong();
  // This is called only once all weak VMOs (per-buffer_index) have been closed. If they're never
  // actually closed, this is never called (in contrast to LogCloseWeakAsapTakingTooLong).
  void LogCloseWeakAsapDuration(zx::duration duration);

 private:
  std::shared_ptr<cobalt::MetricsBuffer> metrics_buffer_;
  cobalt::MetricBuffer unused_page_check_;
  cobalt::MetricBuffer weak_vmo_events_;
  cobalt::HistogramMetricBuffer weak_vmo_histograms_;

  static constexpr zx::duration kUnusedPageCheckFlushSuccessPeriod = zx::min(30);
  uint64_t unused_page_check_pending_success_count_ = 0;
  zx::time unused_page_check_last_flush_time_ = zx::time::infinite_past();
};

#endif  // SRC_SYSMEM_SERVER_SYSMEM_METRICS_H_

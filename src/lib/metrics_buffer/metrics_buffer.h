// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_METRICS_BUFFER_METRICS_BUFFER_H_
#define SRC_LIB_METRICS_BUFFER_METRICS_BUFFER_H_

#include <fidl/fuchsia.io/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async/cpp/task.h>
#include <lib/sys/cpp/service_directory.h>

#include <memory>
#include <unordered_map>
#include <unordered_set>

#include "src/lib/metrics_buffer/metrics_impl.h"

namespace cobalt {

class MetricsBuffer;

// This class is a convenience interface which remembers a metric_id.
class MetricBuffer {
 public:
  MetricBuffer(const MetricBuffer& to_copy) = delete;
  MetricBuffer& operator=(const MetricBuffer& to_copy) = delete;
  MetricBuffer(MetricBuffer&& to_move) = default;
  MetricBuffer& operator=(MetricBuffer&& to_move) = default;
  ~MetricBuffer() = default;

  void LogEvent(std::vector<uint32_t> dimension_values);
  void LogEventCount(std::vector<uint32_t> dimension_values, uint32_t count);

 private:
  friend class MetricsBuffer;
  MetricBuffer(std::shared_ptr<MetricsBuffer> parent, uint32_t metric_id);
  std::shared_ptr<MetricsBuffer> parent_;
  uint32_t metric_id_ = 0;
};

// This class is a convenience interface which remembers a metric_id.
class StringMetricBuffer {
 public:
  StringMetricBuffer(const StringMetricBuffer& to_copy) = delete;
  StringMetricBuffer& operator=(const StringMetricBuffer& to_copy) = delete;
  StringMetricBuffer(StringMetricBuffer&& to_move) = default;
  StringMetricBuffer& operator=(StringMetricBuffer&& to_move) = default;
  ~StringMetricBuffer() = default;

  void LogString(std::vector<uint32_t> dimension_values, std::string string_value);

 private:
  friend class MetricsBuffer;
  StringMetricBuffer(std::shared_ptr<MetricsBuffer> parent, uint32_t metric_id);
  std::shared_ptr<MetricsBuffer> parent_;
  uint32_t metric_id_ = 0;
};

struct ExponentialIntegerBuckets {
  int64_t floor;
  uint32_t num_buckets;
  uint32_t initial_step;
  uint32_t step_multiplier;
};

struct LinearIntegerBuckets {
  int64_t floor;
  uint32_t num_buckets;
  uint32_t step_size;
};

struct HistogramInfo {
  uint32_t metric_id = 0;
  std::variant<ExponentialIntegerBuckets, LinearIntegerBuckets> buckets;
};

// If metrics.cb.h codegen provides a non-macro way in future, switch to that.
// TODO: Update to using the StepMultiplierFloat directly.
#define COBALT_EXPONENTIAL_HISTOGRAM_INFO(base_name)                                           \
  [] {                                                                                         \
    ::cobalt::ExponentialIntegerBuckets buckets;                                               \
    buckets.floor = base_name##IntBucketsFloor;                                                \
    buckets.num_buckets = base_name##IntBucketsNumBuckets;                                     \
    buckets.initial_step = base_name##IntBucketsInitialStep;                                   \
    buckets.step_multiplier = static_cast<uint32_t>(base_name##IntBucketsStepMultiplierFloat); \
    return ::cobalt::HistogramInfo{.metric_id = base_name##MetricId, .buckets = buckets};      \
  }()

#define COBALT_LINEAR_HISTOGRAM_INFO(base_name)                                           \
  [] {                                                                                    \
    ::cobalt::LinearIntegerBuckets buckets;                                               \
    buckets.floor = base_name##IntBucketsFloor;                                           \
    buckets.num_buckets = base_name##IntBucketsNumBuckets;                                \
    buckets.step_size = base_name##IntBucketsStepSize;                                    \
    return ::cobalt::HistogramInfo{.metric_id = base_name##MetricId, .buckets = buckets}; \
  }()

// This class is a convenience interface which remembers metric_id + bucket_config, and calculates
// which bucket for the caller.
class HistogramMetricBuffer {
 public:
  HistogramMetricBuffer(const HistogramMetricBuffer& to_copy) = delete;
  HistogramMetricBuffer& operator=(const HistogramMetricBuffer& to_copy) = delete;
  HistogramMetricBuffer(HistogramMetricBuffer&& to_move) = default;
  HistogramMetricBuffer& operator=(HistogramMetricBuffer&& to_move) = default;
  ~HistogramMetricBuffer() = default;

  // This computes which histogram bucket, and adds 1 to the tally of that bucket.
  void LogValue(std::vector<uint32_t> dimension_values, int64_t value);

 private:
  friend class MetricsBuffer;
  // For the second parameter, see COBALT_EXPONENTIAL_HISTOGRAM_INFO() and
  // COBALT_LINEAR_HISTOGRAM_INFO() defined above.
  HistogramMetricBuffer(std::shared_ptr<MetricsBuffer> parent, HistogramInfo histogram_info);
  std::shared_ptr<MetricsBuffer> parent_;
  HistogramInfo histogram_info_;

  // This info corresponds to fields of cobalt IntegerBucketConfig. We can't depend on
  // cobalt buckets_config from here because it depends on syslog.
  std::vector<int64_t> floors_;
};

// The purpose of this class is to ensure the rate of messages to Cobalt stays reasonable, per
// Cobalt's rate requirement/recommendation in the Cobalt docs.
//
// Typically it'll make sense to only have one of these per process, but that's not enforced.
//
// Methods of this class can be called on any thread.
class MetricsBuffer final : public std::enable_shared_from_this<MetricsBuffer> {
 public:
  // Initially a noop instance, so unit tests don't need to wire up cobalt.  Call
  // SetServiceDirectory() to enable and start logging.
  static std::shared_ptr<MetricsBuffer> Create(uint32_t project_id);

  // !service_directory is ok.  If !service_directory, the instance will be a nop instance until
  // SetServiceDirectory() is called.
  static std::shared_ptr<MetricsBuffer> Create(
      uint32_t project_id, std::shared_ptr<sys::ServiceDirectory> service_directory);

  ~MetricsBuffer() __TA_EXCLUDES(lock_);

  // Set the ServiceDirectory from which to get fuchsia.metrics.MetricEventLoggerFactory.  This can
  // be nullptr. This can be called again, regardless of whether there was already a previous
  // ServiceDirectory. Previously-queued events may be lost (especially recently-queued events) when
  // switching to a new ServiceDirectory.
  void SetServiceDirectory(std::shared_ptr<sys::ServiceDirectory> service_directory)
      __TA_EXCLUDES(lock_);

  // This specifies the minimum amount of time between logging batches to cobalt.  If enough
  // different metrics have accumulated to force more than one message to cobalt, then more than
  // one message is possible, but typically a single message will be sent to cobalt no more often
  // than this.  In unit tests we use this to turn the min_logging_period way down so that tests can
  // finish faster.
  void SetMinLoggingPeriod(zx::duration min_logging_period);

  // Log the event as EVENT_COUNT, with period_duration_micros 0, possibly aggregating with any
  // other calls to this method with the same component and event wihtin a short duration to limit
  // the rate of FIDL calls to Cobalt, per the rate requirement/recommendation in the Cobalt docs.
  void LogEvent(uint32_t metric_id, std::vector<uint32_t> dimension_values) __TA_EXCLUDES(lock_);

  void LogEventCount(uint32_t metric_id, std::vector<uint32_t> dimension_values, uint32_t count);

  // The string_value is not an arbitrary string; it must be one of the potential strings defined in
  // the metric definition, or the string_value ends up getting ignored.
  void LogString(uint32_t metric_id, std::vector<uint32_t> dimension_values,
                 std::string string_value);

  // Use sparingly, only when it's appropriate to force the counts to flush to Cobalt, which will
  // typically only be before orderly exit or in situations like driver suspend.  Over-use of this
  // method will break the purpose of using this class, which is to ensure the rate of messages to
  // Cobalt stays reasonable.
  void ForceFlush() __TA_EXCLUDES(lock_);

  MetricBuffer CreateMetricBuffer(uint32_t metric_id);
  StringMetricBuffer CreateStringMetricBuffer(uint32_t metric_id);
  HistogramMetricBuffer CreateHistogramMetricBuffer(HistogramInfo histogram_info);

 private:
  friend class HistogramMetricBuffer;

  // This computes which histogram bucket, and adds 1 to the tally of that bucket.
  void LogHistogramValue(const HistogramInfo& histogram_info, std::vector<int64_t>& floors,
                         std::vector<uint32_t> dimension_values, int64_t value);

  explicit MetricsBuffer(uint32_t project_id) __TA_EXCLUDES(lock_);

  MetricsBuffer(uint32_t project_id, std::shared_ptr<sys::ServiceDirectory> service_directory)
      __TA_EXCLUDES(lock_);

  class MetricKey {
   public:
    MetricKey(uint32_t metric_id, std::vector<uint32_t> dimension_values);

    uint32_t metric_id() const;
    const std::vector<uint32_t>& dimension_values() const;

   private:
    uint32_t metric_id_ = 0;
    std::vector<uint32_t> dimension_values_;
  };
  struct MetricKeyHash {
    size_t operator()(const MetricKey& key) const noexcept;

   private:
    std::hash<uint32_t> hash_uint32_;
  };
  struct MetricKeyEqual {
    bool operator()(const MetricKey& lhs, const MetricKey& rhs) const noexcept;
  };

  // We don't keep a count per string; instead we collapse out any additional string instances per
  // flush period, since each tally of a string would require an additional item in the batch. We
  // want to avoid creating a large number of items in a batch or a large number of batches due to
  // a noisy string. We also want to avoid using unbounded memory in the MetricsBuffer. We could
  // achieve this with a cap on how many instances of the same string we're willing to buffer and
  // batch, but for now we just cap at buffering 1 of a given string at any given time.
  class PendingStringsItem {
   public:
    PendingStringsItem(uint32_t metric_id, std::vector<uint32_t> dimension_values,
                       std::string string_value);

    uint32_t metric_id() const;
    const std::vector<uint32_t>& dimension_values() const;
    const std::string& string_value() const;

   private:
    uint32_t metric_id_ = 0;
    std::vector<uint32_t> dimension_values_;
    std::string string_value_;
  };
  struct PendingStringsItemHash {
    size_t operator()(const PendingStringsItem& item) const noexcept;

   private:
    std::hash<uint32_t> hash_uint32_;
    std::hash<std::string> hash_string_;
  };
  struct PendingStringsItemEqual {
    bool operator()(const PendingStringsItem& lhs, const PendingStringsItem& rhs) const noexcept;
  };

  void TryPostFlushCountsLocked() __TA_REQUIRES(lock_);
  void FlushPendingEventCounts() __TA_EXCLUDES(lock_);

  static constexpr zx::duration kDefaultMinLoggingPeriod = zx::sec(5);

  std::mutex lock_;

  const uint32_t project_id_{};

  // For historical reasons, we have a separate async::Loop for each instance of
  // cobalt::MetricsImpl. We only SetServiceDirectory() once in non-test scenarios, so outside tests
  // we'll only ever create one async::Loop.
  std::unique_ptr<async::Loop> loop_;

  std::unique_ptr<cobalt::MetricsImpl> cobalt_logger_ __TA_GUARDED(lock_);

  zx::time last_flushed_ __TA_GUARDED(lock_) = zx::time::infinite_past();

  // From component and event to event count.
  using PendingCounts = std::unordered_map<MetricKey, int64_t, MetricKeyHash, MetricKeyEqual>;
  PendingCounts pending_counts_ __TA_GUARDED(lock_);

  // We don't keep a count per string; instead we collapse out any additional string instances per
  // flush period, since each tally of a string would require an additional item in the batch. We
  // want to avoid creating a large number of items in a batch or a large number of batches due to
  // a noisy string. We also want to avoid using unbounded memory in the MetricsBuffer. We could
  // achieve this with a cap on how many instances of the same string we're willing to buffer and
  // batch, but for now we just cap at buffering 1 of a given string at any given time.
  using PendingStrings =
      std::unordered_map<MetricKey, std::unordered_set<std::string>, MetricKeyHash, MetricKeyEqual>;
  PendingStrings pending_strings_ __TA_GUARDED(lock_);

  // key - bucket index
  // value - how many tallies to add to bucket
  using PendingBuckets = std::unordered_map<uint32_t, uint32_t>;
  using PendingHistograms =
      std::unordered_map<MetricKey, PendingBuckets, MetricKeyHash, MetricKeyEqual>;

  PendingHistograms pending_histograms_ __TA_GUARDED(lock_);

  zx::duration min_logging_period_ = kDefaultMinLoggingPeriod;
};

}  // namespace cobalt

#endif  // SRC_LIB_METRICS_BUFFER_METRICS_BUFFER_H_

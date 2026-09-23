// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_POWER_STATE_RECORDER_CPP_INSPECT_BUFFER_INTERNAL_H_
#define LIB_POWER_STATE_RECORDER_CPP_INSPECT_BUFFER_INTERNAL_H_

#include <lib/fit/function.h>
#include <lib/inspect/component/cpp/component.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/power/state_recorder/cpp/concepts.h>
#include <lib/power/state_recorder/cpp/inspect_buffer.h>
#include <lib/zx/clock.h>
#include <lib/zx/time.h>

#include <algorithm>
#include <cstdint>
#include <string>
#include <type_traits>
#include <vector>

namespace power_observability::internal {

inline constexpr size_t kShardCapacity = 200;

template <typename T>
struct InspectArrayTypeTraits;

template <typename T>
  requires WidensToUint64<T>
struct InspectArrayTypeTraits<T> {
  using ArrayProperty = inspect::UintArray;
  using ValueType = uint64_t;
  static ArrayProperty Create(inspect::Node& node, std::string_view name, size_t slots) {
    return node.CreateUintArray(name, slots);
  }
};

template <typename T>
  requires WidensToInt64<T>
struct InspectArrayTypeTraits<T> {
  using ArrayProperty = inspect::IntArray;
  using ValueType = int64_t;
  static ArrayProperty Create(inspect::Node& node, std::string_view name, size_t slots) {
    return node.CreateIntArray(name, slots);
  }
};

template <typename T>
  requires WidensToDouble<T>
struct InspectArrayTypeTraits<T> {
  using ArrayProperty = inspect::DoubleArray;
  using ValueType = double;
  static ArrayProperty Create(inspect::Node& node, std::string_view name, size_t slots) {
    return node.CreateDoubleArray(name, slots);
  }
};

template <typename T>
  requires std::is_enum_v<T>
struct InspectArrayTypeTraits<T> : InspectArrayTypeTraits<std::underlying_type_t<T>> {};

template <typename T>
struct InspectShard {
  inspect::Node node;
  inspect::IntArray times;
  typename InspectArrayTypeTraits<T>::ArrayProperty values;
};

// Fixed-capacity sharded circular buffer in Inspect for eager mode.
template <typename T>
  requires IsRecordableValueType<T>
class EagerShardedBuffer {
 public:
  EagerShardedBuffer(inspect::Node& parent_node, size_t capacity)
      : capacity_(capacity),
        history_node_(parent_node.CreateChild("history")),
        current_index_(history_node_.CreateUint("current_index", 0)),
        current_size_(history_node_.CreateUint("current_size", 0)),
        shards_node_(history_node_.CreateChild("shards")) {
    size_t active_capacity = std::max<size_t>(capacity, 1);
    size_t total_shards = (active_capacity + kShardCapacity - 1) / kShardCapacity;
    shards_.reserve(total_shards);

    for (size_t s = 0; s < total_shards; ++s) {
      size_t start_idx = s * kShardCapacity;
      size_t end_idx = std::min(start_idx + kShardCapacity, capacity);
      size_t shard_size = std::max<size_t>(end_idx - start_idx, 1);

      auto shard_node = shards_node_.CreateChild(std::to_string(s));
      auto times = shard_node.CreateIntArray("times", shard_size);
      auto values = InspectArrayTypeTraits<T>::Create(shard_node, "values", shard_size);

      shards_.push_back(InspectShard<T>{
          .node = std::move(shard_node),
          .times = std::move(times),
          .values = std::move(values),
      });
    }
  }

  EagerShardedBuffer(EagerShardedBuffer&&) = default;
  EagerShardedBuffer& operator=(EagerShardedBuffer&&) = default;
  EagerShardedBuffer(const EagerShardedBuffer&) = delete;
  EagerShardedBuffer& operator=(const EagerShardedBuffer&) = delete;

  void Record(int64_t timestamp_ns, T value) {
    if (capacity_ == 0) {
      return;
    }
    size_t shard_idx = index_tracker_ / kShardCapacity;
    size_t slot_idx = index_tracker_ % kShardCapacity;

    auto& shard = shards_[shard_idx];
    shard.times.Set(slot_idx, timestamp_ns);
    using ValueType = typename InspectArrayTypeTraits<T>::ValueType;
    shard.values.Set(slot_idx, static_cast<ValueType>(value));

    index_tracker_ = (index_tracker_ + 1) % capacity_;
    size_tracker_ = std::min(size_tracker_ + 1, capacity_);

    current_index_.Set(index_tracker_);
    current_size_.Set(size_tracker_);
  }

 private:
  size_t capacity_;
  inspect::Node history_node_;
  inspect::UintProperty current_index_;
  inspect::UintProperty current_size_;
  inspect::Node shards_node_;
  std::vector<InspectShard<T>> shards_;
  size_t index_tracker_ = 0;
  size_t size_tracker_ = 0;
};

// Records data in an underlying TimestampedBuffer to a lazy node in sharded circular buffer format.
template <typename T>
  requires IsRecordableValueType<T>
class LazyInspectRecorderBase {
 public:
  // This class cannot be safely moved because the lazy node callback references `this`.
  LazyInspectRecorderBase(LazyInspectRecorderBase&& other) = delete;
  LazyInspectRecorderBase& operator=(LazyInspectRecorderBase&& other) = delete;

  LazyInspectRecorderBase(const LazyInspectRecorderBase& other) = delete;
  LazyInspectRecorderBase& operator=(const LazyInspectRecorderBase& other) = delete;

  virtual ~LazyInspectRecorderBase() {}

  void AddEntry(T data, int64_t timestamp_ms) { buffer_.AddEntry(data, timestamp_ms); }

 protected:
  LazyInspectRecorderBase(size_t capacity, inspect::Node& parent_node)
      : buffer_(capacity),
        history_node_(parent_node.CreateLazyNode(
            "history", fit::bind_member<&LazyInspectRecorderBase<T>::TimeSeriesToInspect>(this))),
        reset_info_node_(parent_node.CreateLazyNode(
            "reset_info",
            fit::bind_member<&LazyInspectRecorderBase<T>::ResetInfoToInspect>(this))) {}

 private:
  fpromise::promise<inspect::Inspector> ResetInfoToInspect() const {
    inspect::Inspector inspector;
    auto& root = inspector.GetRoot();
    auto& reset_info = buffer_.GetResetInfo();
    root.RecordUint("count", reset_info.count);
    root.RecordInt("last_reset_ns", reset_info.last_reset_ns);
    return fpromise::make_ok_promise(std::move(inspector));
  }

  fpromise::promise<inspect::Inspector> TimeSeriesToInspect() const {
    std::vector<DataPoint<T>> items;
    items.reserve(buffer_.GetCount());
    buffer_.ForEachDataPoint([&](const DataPoint<T>& data_point) { items.push_back(data_point); });

    inspect::Inspector inspector;
    auto& root = inspector.GetRoot();
    size_t size = items.size();

    root.RecordUint("current_index", 0);
    root.RecordUint("current_size", size);

    auto shards_node = root.CreateChild("shards");
    size_t active_capacity = std::max<size_t>(size, 1);
    size_t num_shards = (active_capacity + kShardCapacity - 1) / kShardCapacity;

    for (size_t s = 0; s < num_shards; ++s) {
      size_t shard_start_idx = s * kShardCapacity;
      size_t shard_end_idx = std::min(shard_start_idx + kShardCapacity, size);
      // Since s < num_shards, shard_start_idx <= size. So shard_end_idx >= shard_start_idx.
      size_t shard_size = shard_end_idx - shard_start_idx;
      size_t allocated_size = std::max<size_t>(shard_size, 1);

      auto shard_node = shards_node.CreateChild(std::to_string(s));
      auto times_prop = shard_node.CreateIntArray("times", allocated_size);
      auto values_prop = InspectArrayTypeTraits<T>::Create(shard_node, "values", allocated_size);

      using ValueType = typename InspectArrayTypeTraits<T>::ValueType;
      for (size_t i = 0; i < shard_size; ++i) {
        const auto& item = items[shard_start_idx + i];
        times_prop.Set(i, item.timestamp_ns);
        values_prop.Set(i, static_cast<ValueType>(item.value));
      }

      inspector.emplace(std::move(times_prop));
      inspector.emplace(std::move(values_prop));
      inspector.emplace(std::move(shard_node));
    }

    inspector.emplace(std::move(shards_node));
    return fpromise::make_ok_promise(std::move(inspector));
  }

  TimestampedBuffer<T> buffer_;
  inspect::LazyNode history_node_;
  inspect::LazyNode reset_info_node_;
};

}  // namespace power_observability::internal

#endif  // LIB_POWER_STATE_RECORDER_CPP_INSPECT_BUFFER_INTERNAL_H_

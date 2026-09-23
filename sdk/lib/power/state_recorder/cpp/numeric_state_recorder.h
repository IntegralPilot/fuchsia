// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_POWER_STATE_RECORDER_CPP_NUMERIC_STATE_RECORDER_H_
#define LIB_POWER_STATE_RECORDER_CPP_NUMERIC_STATE_RECORDER_H_

#include <lib/inspect/component/cpp/component.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/power/state_recorder/cpp/common.h>
#include <lib/power/state_recorder/cpp/common_internal.h>
#include <lib/power/state_recorder/cpp/concepts.h>
#include <lib/power/state_recorder/cpp/numeric_state_recorder_internal.h>
#include <lib/trace-engine/context.h>
#include <lib/trace-engine/types.h>
#include <lib/trace/event.h>
#include <lib/zx/clock.h>
#include <lib/zx/result.h>
#include <zircon/compiler.h>

#include <algorithm>
#include <memory>
#include <optional>
#include <string>
#include <type_traits>
#include <variant>

namespace power_observability {

// Decimal prefixes that can be used with most Units.
enum class DecimalPrefix {
  Nano,
  Micro,
  Milli,
  Centi,
  Deci,
  Kilo,
  Mega,
  Giga,
};

// Measurement units that can be used with NumericStateRecorder. Construct using the public
// factory functions.
class Units {
 public:
  static Units AmpHours(std::optional<DecimalPrefix> prefix = std::nullopt) {
    return Units(BaseUnit::AmpHours, prefix);
  }
  static Units Amps(std::optional<DecimalPrefix> prefix = std::nullopt) {
    return Units(BaseUnit::Amps, prefix);
  }
  static Units Hertz(std::optional<DecimalPrefix> prefix = std::nullopt) {
    return Units(BaseUnit::Hertz, prefix);
  }
  static Units Joules(std::optional<DecimalPrefix> prefix = std::nullopt) {
    return Units(BaseUnit::Joules, prefix);
  }
  static Units Seconds(std::optional<DecimalPrefix> prefix = std::nullopt) {
    return Units(BaseUnit::Seconds, prefix);
  }
  static Units Watts(std::optional<DecimalPrefix> prefix = std::nullopt) {
    return Units(BaseUnit::Watts, prefix);
  }
  static Units Volts(std::optional<DecimalPrefix> prefix = std::nullopt) {
    return Units(BaseUnit::Volts, prefix);
  }
  static Units Celsius(std::optional<DecimalPrefix> prefix = std::nullopt) {
    return Units(BaseUnit::Celsius, prefix);
  }
  static Units Number(std::optional<DecimalPrefix> prefix = std::nullopt) {
    return Units(BaseUnit::Number, prefix);
  }
  static Units Percent() { return Units(BaseUnit::Percent, std::nullopt); }

  std::string ToString() const {
    if (prefix_.has_value()) {
      return ToString(prefix_.value()) + ToString(base_);
    }
    return ToString(base_);
  }

 private:
  enum class BaseUnit {
    AmpHours,
    Amps,
    Hertz,
    Joules,
    Seconds,
    Watts,
    Volts,
    Celsius,
    Number,
    Percent,
  };

  static std::string ToString(DecimalPrefix prefix) {
    switch (prefix) {
      case DecimalPrefix::Nano:
        return "n";
      case DecimalPrefix::Micro:
        return "u";
      case DecimalPrefix::Milli:
        return "m";
      case DecimalPrefix::Centi:
        return "c";
      case DecimalPrefix::Deci:
        return "d";
      case DecimalPrefix::Kilo:
        return "k";
      case DecimalPrefix::Mega:
        return "M";
      case DecimalPrefix::Giga:
        return "G";
    }
    __builtin_unreachable();
  }

  static std::string ToString(BaseUnit base) {
    switch (base) {
      case BaseUnit::AmpHours:
        return "Ah";
      case BaseUnit::Amps:
        return "A";
      case BaseUnit::Hertz:
        return "Hz";
      case BaseUnit::Joules:
        return "J";
      case BaseUnit::Seconds:
        return "s";
      case BaseUnit::Watts:
        return "W";
      case BaseUnit::Volts:
        return "V";
      case BaseUnit::Celsius:
        return "C";
      case BaseUnit::Number:
        return "#";
      case BaseUnit::Percent:
        return "%";
    }
    __builtin_unreachable();
  }

  Units(BaseUnit base, std::optional<DecimalPrefix> prefix) : base_(base), prefix_(prefix) {}

  BaseUnit base_;
  std::optional<DecimalPrefix> prefix_;
};

// Metadata for a numeric state.
template <typename T>
  requires IsRecordableNumericType<T>
struct NumericStateMetadata {
  std::string name;
  Units units;
  std::optional<std::pair<T, T>> range;  // Inclusive range, [min, max]
  const char* trace_category_literal;
  bool continuously_averaged = false;
};

// Records time series data of a numeric-valued state.
template <typename T>
  requires IsRecordableNumericType<T>
class NumericStateRecorder final {
 public:
  static zx::result<NumericStateRecorder<T>> Create(NumericStateMetadata<T> metadata,
                                                    RecorderOptions options,
                                                    StateRecorderManager& manager);

  // Records `value`, timestamped either at `event_timestamp` if provided, or at the current time of
  // the boot clock if not.
  void Record(T value, std::optional<zx::time_boot> event_timestamp = std::nullopt);

  NumericStateRecorder(const NumericStateRecorder&) = delete;
  NumericStateRecorder& operator=(const NumericStateRecorder&) = delete;

  NumericStateRecorder& operator=(NumericStateRecorder&& other) noexcept {
    name_ = std::move(other.name_);
    trace_category_literal_ = other.trace_category_literal_;
    root_node_ = std::move(other.root_node_);
    history_ = std::move(other.history_);
    trace_id_ = other.trace_id_;
    trace_name_ref_ = other.trace_name_ref_;
    manager_ = other.manager_;
    continuously_averaged_ = other.continuously_averaged_;
    last_timestamp_ = other.last_timestamp_;
    moved_from_ = other.moved_from_;
    other.moved_from_ = true;
    return *this;
  }

  NumericStateRecorder(NumericStateRecorder&& other) noexcept
      : name_(std::move(other.name_)),
        trace_category_literal_(other.trace_category_literal_),
        root_node_(std::move(other.root_node_)),
        history_(std::move(other.history_)),
        trace_id_(other.trace_id_),
        trace_name_ref_(other.trace_name_ref_),
        manager_(other.manager_),
        continuously_averaged_(other.continuously_averaged_),
        last_timestamp_(other.last_timestamp_),
        moved_from_(other.moved_from_) {
    other.moved_from_ = true;
  }

  ~NumericStateRecorder() {
    if (!moved_from_) {
      manager_->UnregisterName(*name_);
    }
  }

 private:
  NumericStateRecorder(NumericStateMetadata<T> metadata, RecorderOptions options,
                       StateRecorderManager& manager, inspect::Node root_node)
      : name_(std::make_unique<std::string>(metadata.name)),
        trace_category_literal_(metadata.trace_category_literal),
        root_node_(std::move(root_node)),
        history_(options.lazy_record
                     ? History(internal::NumericLazyInspectRecorder<T>::Create(options.capacity,
                                                                               root_node_))
                     : History(internal::EagerShardedBuffer<T>(root_node_, options.capacity))),
        trace_id_(TRACE_NONCE()),
        trace_name_ref_(trace_make_inline_string_ref(name_->c_str(), name_->length())),
        manager_(&manager),
        continuously_averaged_(metadata.continuously_averaged) {
    // In the eager case, record nominal reset info for symmetry with the lazy case.
    if (!options.lazy_record) {
      root_node_.RecordChild("reset_info", [](inspect::Node& node) {
        node.RecordUint("count", 0);
        node.RecordInt("last_reset_ns", zx::clock::get_boot().get());
      });
    }

    root_node_.RecordChild("metadata", [&](inspect::Node& metadata_node) {
      metadata_node.RecordString("format_version", "2.0");
      metadata_node.RecordString("name", *name_);
      metadata_node.RecordString("type", "numeric");
      metadata_node.RecordString("units", metadata.units.ToString());
      if (continuously_averaged_) {
        metadata_node.RecordBool("continuously_averaged", true);
      }
      if (metadata.range.has_value()) {
        metadata_node.RecordChild("range", [&](inspect::Node& range_node) {
          if constexpr (WidensToUint64<T>) {
            range_node.RecordUint("min_inc", static_cast<uint64_t>(metadata.range->first));
            range_node.RecordUint("max_inc", static_cast<uint64_t>(metadata.range->second));
          } else if constexpr (WidensToInt64<T>) {
            range_node.RecordInt("min_inc", static_cast<int64_t>(metadata.range->first));
            range_node.RecordInt("max_inc", static_cast<int64_t>(metadata.range->second));
          } else if constexpr (WidensToDouble<T>) {
            range_node.RecordDouble("min_inc", static_cast<double>(metadata.range->first));
            range_node.RecordDouble("max_inc", static_cast<double>(metadata.range->second));
          } else {
            static_assert(!IsRecordableNumericType<T>, "Unsupported type");
          }
        });
      }
    });
  }

  std::unique_ptr<std::string> name_;  // Use unique_ptr for address stability with trace_name_ref_
  const char* trace_category_literal_;
  inspect::Node root_node_;

  using History = std::variant<internal::EagerShardedBuffer<T>,
                               std::unique_ptr<internal::NumericLazyInspectRecorder<T>>>;
  History history_;

  trace_async_id_t trace_id_;
  trace_string_ref_t trace_name_ref_;
  StateRecorderManager* manager_;
  bool continuously_averaged_;
  std::optional<zx::time_boot> last_timestamp_;
  bool moved_from_ = false;
};

template <typename T>
  requires IsRecordableNumericType<T>
zx::result<NumericStateRecorder<T>> NumericStateRecorder<T>::Create(
    NumericStateMetadata<T> metadata, RecorderOptions options, StateRecorderManager& manager) {
  auto result = manager.RegisterName(metadata.name);
  if (!result.is_ok()) {
    return result.take_error();
  }
  return zx::ok(NumericStateRecorder<T>(metadata, options, manager, std::move(result.value())));
}

template <typename T>
  requires IsRecordableNumericType<T>
void NumericStateRecorder<T>::Record(T value, std::optional<zx::time_boot> event_timestamp) {
  zx::time_boot current_timestamp;
  if (event_timestamp) {
    current_timestamp = *event_timestamp;
  } else {
    current_timestamp = zx::clock::get_boot();
  }

  std::visit(
      [&](auto& history) {
        if constexpr (std::is_same_v<std::decay_t<decltype(history)>,
                                     internal::EagerShardedBuffer<T>>) {
          history.Record(current_timestamp.get(), value);
        } else {
          history->AddEntry(value, internal::to_msecs(current_timestamp));
        }
      },
      history_);

  static trace_site_t trace_site_state;
  trace_string_ref_t category_ref;
  trace_context_t* context = trace_acquire_context_for_category_cached(
      trace_category_literal_, &trace_site_state, &category_ref);

  if (unlikely(context)) {
    trace_thread_ref_t thread_ref;
    trace_context_register_current_thread(context, &thread_ref);

    trace_arg_t arg;
    if constexpr (WidensToUint32<T>) {
      arg = trace_make_arg(trace_make_inline_c_string_ref("value"),
                           trace_make_uint32_arg_value(static_cast<uint32_t>(value)));
    } else if constexpr (std::is_same_v<T, uint64_t>) {
      arg = trace_make_arg(trace_make_inline_c_string_ref("value"),
                           trace_make_uint64_arg_value(value));
    } else if constexpr (WidensToInt32<T>) {
      arg = trace_make_arg(trace_make_inline_c_string_ref("value"),
                           trace_make_int32_arg_value(static_cast<int32_t>(value)));
    } else if constexpr (std::is_same_v<T, int64_t>) {
      arg = trace_make_arg(trace_make_inline_c_string_ref("value"),
                           trace_make_int64_arg_value(value));
    } else if constexpr (WidensToDouble<T>) {
      arg = trace_make_arg(trace_make_inline_c_string_ref("value"),
                           trace_make_double_arg_value(static_cast<double>(value)));
    } else {
      static_assert(!IsRecordableNumericType<T>, "Unsupported type");
    }

    // If we're dealing with a continuously-averaged value, we:
    //  - Skip recording the first value, for which last_timestamp_ is undefined.
    //  - Timestamp the value using last_timestamp_ instead of current_timestamp. If another value
    //    is recorded after this one, then `value` will ultimately be graphed over the interval
    //    [last_timestamp_, current_timestamp].
    if (!continuously_averaged_ || last_timestamp_) {
      auto timestamp = continuously_averaged_ ? *last_timestamp_ : current_timestamp;
      trace_context_write_counter_event_record(context, internal::boot_time_to_ticks(timestamp),
                                               &thread_ref, &category_ref, &trace_name_ref_,
                                               trace_id_, &arg, 1);
    }
    trace_release_context(context);
  }
  last_timestamp_ = current_timestamp;
}

}  // namespace power_observability

#endif  // LIB_POWER_STATE_RECORDER_CPP_NUMERIC_STATE_RECORDER_H_

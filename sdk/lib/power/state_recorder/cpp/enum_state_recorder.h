// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_POWER_STATE_RECORDER_CPP_ENUM_STATE_RECORDER_H_
#define LIB_POWER_STATE_RECORDER_CPP_ENUM_STATE_RECORDER_H_

#include <lib/inspect/cpp/inspect.h>
#include <lib/power/state_recorder/cpp/common.h>
#include <lib/power/state_recorder/cpp/common_internal.h>
#include <lib/power/state_recorder/cpp/concepts.h>
#include <lib/power/state_recorder/cpp/enum_state_recorder_internal.h>
#include <lib/trace-engine/context.h>
#include <lib/trace-engine/types.h>
#include <lib/trace/event.h>
#include <lib/zx/clock.h>
#include <lib/zx/result.h>
#include <zircon/compiler.h>

#include <map>
#include <memory>
#include <optional>
#include <string>
#include <type_traits>

namespace power_observability {

// Metadata for an enum state.
template <typename T>
  requires IsRecordableEnumType<T>
struct EnumStateMetadata {
  // Name of the state.
  // - Inspect: This name will be used for this state's Inspect node, recorded in
  //   a node named "power_observability_state_recorders" within the component inspector's root.
  // - Trace: Time series will be recorded on a *global* track with this name. If names
  //   collide, events for the colliding recorders will be placed on the same track.
  std::string name;

  // Mapping of state IDs to state names.
  std::map<T, std::string> states;

  // Category for trace events associated with this state. This must be a string literal due to
  // constraints of the tracing system.
  const char* trace_category_literal;
};

// Records state changes to Inspect and trace.
template <typename T>
  requires IsRecordableEnumType<T>
class EnumStateRecorder final {
 public:
  // Creates a new StateRecorder.
  //
  // Errors:
  //   - ZX_ERR_ALREADY_EXISTS: `metadata.name` is already in use by a StateRecorder exporting
  //     to the provided inspector.
  static zx::result<EnumStateRecorder<T>> Create(EnumStateMetadata<T> metadata,
                                                 RecorderOptions options,
                                                 StateRecorderManager& manager);

  // Records `value`, timestamped either at `event_timestamp` if provided, or at the current time of
  // the boot clock if not.
  void Record(T value, std::optional<zx::time_boot> event_timestamp = std::nullopt);

  EnumStateRecorder(const EnumStateRecorder&) = delete;
  EnumStateRecorder& operator=(const EnumStateRecorder&) = delete;

  EnumStateRecorder& operator=(EnumStateRecorder&& other) noexcept {
    name_ = std::move(other.name_);
    trace_category_literal_ = other.trace_category_literal_;
    name_lookup_ = other.name_lookup_;
    root_node_ = std::move(other.root_node_);
    history_ = std::move(other.history_);
    trace_id_ = other.trace_id_;
    trace_name_ = std::move(other.trace_name_);
    trace_name_ref_ = other.trace_name_ref_;
    current_state_name_ = other.current_state_name_;
    manager_ = other.manager_;

    other.moved_from_ = true;

    return *this;
  }

  EnumStateRecorder(EnumStateRecorder&& other) noexcept
      : name_(std::move(other.name_)),
        trace_category_literal_(other.trace_category_literal_),
        name_lookup_(other.name_lookup_),
        root_node_(std::move(other.root_node_)),
        history_(std::move(other.history_)),
        trace_id_(other.trace_id_),
        trace_name_(std::move(other.trace_name_)),
        trace_name_ref_(other.trace_name_ref_),
        current_state_name_(other.current_state_name_),
        manager_(other.manager_) {
    other.moved_from_ = true;
  }

  ~EnumStateRecorder() {
    if (!moved_from_) {
      manager_->UnregisterName(name_);
    }
  }

 protected:
  EnumStateRecorder(EnumStateMetadata<T> metadata, RecorderOptions options,
                    StateRecorderManager& manager, inspect::Node root_node)
      : name_(metadata.name),
        trace_category_literal_(metadata.trace_category_literal),
        name_lookup_(std::make_shared<internal::StateNameLookup<T>>(metadata.states)),
        root_node_(std::move(root_node)),
        history_(options.lazy_record
                     ? History(internal::EnumLazyInspectRecorder<T>::Create(
                           name_lookup_, options.capacity, root_node_))
                     : History(internal::EagerShardedBuffer<T>(root_node_, options.capacity))),
        trace_id_(TRACE_NONCE()),
        trace_name_(std::make_unique<std::string>(name_)),
        trace_name_ref_(trace_make_inline_string_ref(trace_name_->c_str(), trace_name_->length())),
        manager_(&manager) {
    // In the eager case, record nominal reset info for symmetry with the lazy case.
    if (!options.lazy_record) {
      root_node_.RecordChild("reset_info", [](inspect::Node& node) {
        node.RecordUint("count", 0);
        node.RecordInt("last_reset_ns", zx::clock::get_boot().get());
      });
    }

    root_node_.RecordChild("metadata", [&](inspect::Node& metadata_node) {
      metadata_node.RecordString("format_version", "2.0");
      metadata_node.RecordString("name", metadata.name);
      metadata_node.RecordString("type", "enum");
      metadata_node.RecordChild("states", [&](inspect::Node& states_node) {
        for (const auto& [state_enum, state_name] : metadata.states) {
          if constexpr (WidensToUint64<std::underlying_type_t<T>>) {
            states_node.RecordUint(state_name, static_cast<std::underlying_type_t<T>>(state_enum));
          } else if constexpr (WidensToInt64<std::underlying_type_t<T>>) {
            states_node.RecordInt(state_name, static_cast<std::underlying_type_t<T>>(state_enum));
          } else {
            static_assert(!IsRecordableEnumType<T>, "Unsupported type");
          }
        }
      });
    });
  }

 private:
  const internal::StateName* GetStateName(T state_enum) const;

  std::string name_;
  const char* trace_category_literal_;

  std::shared_ptr<internal::StateNameLookup<T>> name_lookup_;

  inspect::Node root_node_;

  using History = std::variant<internal::EagerShardedBuffer<T>,
                               std::unique_ptr<internal::EnumLazyInspectRecorder<T>>>;
  History history_;

  trace_async_id_t trace_id_;

  // Store the name string in a unique_ptr to guarantee address stability, so the ref in
  // `trace_name_ref_` remains valid on move.
  //
  // Note that `trace_name_` appends to `name` to ensure uniqueness. As a result, the StateName
  // struct doesn't quite match this use case.
  std::unique_ptr<std::string> trace_name_;
  trace_string_ref_t trace_name_ref_;

  std::optional<const internal::StateName*> current_state_name_;

  // Store a reference to the manager so we can unregister our name on destruction.
  StateRecorderManager* manager_;

  bool moved_from_ = false;
};

template <typename T>
  requires IsRecordableEnumType<T>
zx::result<EnumStateRecorder<T>> EnumStateRecorder<T>::Create(EnumStateMetadata<T> metadata,
                                                              RecorderOptions options,
                                                              StateRecorderManager& manager) {
  auto result = manager.RegisterName(metadata.name);
  if (!result.is_ok()) {
    return result.take_error();
  }

  return zx::ok(EnumStateRecorder<T>(metadata, options, manager, std::move(result.value())));
}

template <typename T>
  requires IsRecordableEnumType<T>
void EnumStateRecorder<T>::Record(T state_enum, std::optional<zx::time_boot> event_timestamp) {
  zx::time_boot current_timestamp;
  if (event_timestamp) {
    current_timestamp = *event_timestamp;
  } else {
    current_timestamp = zx::clock::get_boot();
  }

  // Since our event names are not literals, we're using the trace function API rather than the
  // more common macro API.
  static trace_site_t trace_site_state;
  trace_string_ref_t trace_category_ref;
  trace_context_t* trace_context = trace_acquire_context_for_category_cached(
      trace_category_literal_, &trace_site_state, &trace_category_ref);
  trace_thread_ref_t thread_ref;
  zx_ticks_t trace_ticks = 0;

  if (unlikely(trace_context)) {
    trace_ticks = internal::boot_time_to_ticks(current_timestamp);
    trace_context_register_vthread_by_ref(trace_context, ZX_KOID_INVALID, &trace_name_ref_,
                                          trace_id_, &thread_ref);
    if (current_state_name_.has_value()) {
      trace_context_write_duration_end_event_record(
          trace_context, trace_ticks, &thread_ref, &trace_category_ref,
          &(current_state_name_.value()->trace_name), nullptr, 0);
    }
  }

  current_state_name_ = name_lookup_->GetStateName(state_enum);

  if (unlikely(trace_context)) {
    trace_context_write_duration_begin_event_record(
        trace_context, trace_ticks, &thread_ref, &trace_category_ref,
        &(current_state_name_.value()->trace_name), nullptr, 0);
    trace_release_context(trace_context);
  }

  if (auto* history = std::get_if<internal::EagerShardedBuffer<T>>(&history_)) {
    history->Record(current_timestamp.get(), state_enum);
  } else if (auto* history =
                 std::get_if<std::unique_ptr<internal::EnumLazyInspectRecorder<T>>>(&history_)) {
    (*history)->AddEntry(state_enum, internal::to_msecs(current_timestamp));
  }
}

}  // namespace power_observability

#endif  // LIB_POWER_STATE_RECORDER_CPP_ENUM_STATE_RECORDER_H_

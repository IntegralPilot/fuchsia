// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_POWER_CPP_TYPES_H_
#define LIB_DRIVER_POWER_CPP_TYPES_H_

#include <fidl/fuchsia.hardware.power/cpp/fidl.h>
#include <lib/zx/handle.h>
#include <lib/zx/result.h>

#include <optional>
#include <variant>
#include <vector>

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)

namespace fdf_power {

// The length of time it takes to move to a power level.
struct Transition {
  static zx::result<Transition> FromFidl(const fuchsia_hardware_power::Transition& src);
  static zx::result<Transition> FromFidl(const fuchsia_hardware_power::wire::Transition& src);

  // The power level we're moving to.
  uint8_t target_level;

  // The time it takes to move to the level in microseconds.
  uint32_t latency_us;
};

// A zero-indexed set of levels that a device can assume.
struct PowerLevel {
  static zx::result<PowerLevel> FromFidl(const fuchsia_hardware_power::PowerLevel& src);
  static zx::result<PowerLevel> FromFidl(const fuchsia_hardware_power::wire::PowerLevel& src);

  // The zero-indexed level of this `PowerLevel`.
  uint8_t level;

  // Human-readable label for this `PowerLevel`, used only for debugging.
  std::string name;

  // Describes the levels that are valid transitions from this `PowerLevel`.
  std::vector<Transition> transitions;
};

// Set of `PowerLevel`s and a human-readable identifier. A `PowerLevel` itself contains information
// about valid transitions out of that level.
struct PowerElement {
  static zx::result<PowerElement> FromFidl(const fuchsia_hardware_power::PowerElement& src);
  static zx::result<PowerElement> FromFidl(const fuchsia_hardware_power::wire::PowerElement& src);

  std::string name;
  std::vector<PowerLevel> levels;
};

// Represents a dependency between two power levels of two different `PowerElement`s.
struct LevelTuple {
  static zx::result<LevelTuple> FromFidl(const fuchsia_hardware_power::LevelTuple& src);
  static zx::result<LevelTuple> FromFidl(const fuchsia_hardware_power::wire::LevelTuple& src);

  uint8_t child_level;
  uint8_t parent_level;
};

// NOTE: This is _not_ a complete list of SAG elements. This enum only lists the elements supported
// by the power support library.
enum class SagElement : uint32_t {
  kExecutionState = 1,
  kApplicationActivity = 4,
};

enum class CpuElement : uint32_t {
  kCpu = 1,
};

// Identifier for an element that is another element's parent, in other words an element that the
// other element depends upon.
class ParentElement {
 public:
  static zx::result<ParentElement> FromFidl(const fuchsia_hardware_power::ParentElement& src);
  static zx::result<ParentElement> FromFidl(const fuchsia_hardware_power::wire::ParentElement& src);

  enum class Type {
    // The parent element is one of SAG's elements and the access token should be obtained from the
    // appropriate SAG-related protocol.
    kSag,

    // The parent element's access token should be available from
    // /svc/fuchsia.hardware.power.PowerTokenProvider/{instance_name}`.
    kInstanceName,

    kCpu,
  };

  static ParentElement WithSag(SagElement sag);

  static ParentElement WithInstanceName(std::string instance_name);

  static ParentElement WithCpu(CpuElement cpu);

  // Sets the type to `kSag`.
  void SetSag(SagElement sag);

  // Sets the type to `kInstanceName`.
  void SetInstanceName(std::string instance_name);

  void SetCpu(CpuElement);

  // Returns nullopt if type is not `kSag`.
  std::optional<SagElement> GetSag() const;

  // Returns nullopt if type is not `kInstanceName`.
  std::optional<std::string> GetInstanceName() const;

  std::optional<CpuElement> GetCpu() const;

  Type type() const { return type_; }

  bool operator==(const ParentElement& rhs) const;

 private:
  using ValueType = std::variant<SagElement, std::string, CpuElement>;
  void CheckTypeValueAlignment() const;

  friend std::hash<ParentElement>;

  // The index of the value associated with `type` stored within `value_`. This index isn't stored
  // in `Type` because it is considered an implementation detail.
  static constexpr size_t kSagIndex = 0;
  static constexpr size_t kInstanceNameIndex = 1;
  static constexpr size_t kCpuIndex = 2;

  explicit ParentElement(Type type, ValueType value) : type_(type), value_(std::move(value)) {}

  Type type_;
  ValueType value_;
};

// Describes the relationship between the `PowerLevel`s of two `PowerElement`s. `child` is the name
// of the `PowerElement` which has `PowerLevel`s that depend on `parent`.
struct PowerDependency {
  static zx::result<PowerDependency> FromFidl(const fuchsia_hardware_power::PowerDependency& src);
  static zx::result<PowerDependency> FromFidl(
      const fuchsia_hardware_power::wire::PowerDependency& src);

  // The name for a `PowerElement` which a driver owns.
  std::string child;

  // The name for a `PowerElement` which a driver has access to
  ParentElement parent;

  // The map of level dependencies from `child` to `parent`.
  std::vector<LevelTuple> level_deps;
};

// Contains the `PowerElement` description and any dependencies it has on other `PowerElement`s.
struct PowerElementConfiguration {
  static zx::result<PowerElementConfiguration> FromFidl(
      const fuchsia_hardware_power::PowerElementConfiguration& src);
  static zx::result<PowerElementConfiguration> FromFidl(
      const fuchsia_hardware_power::wire::PowerElementConfiguration& src);

  PowerElement element;
  std::vector<PowerDependency> dependencies;
};

}  // namespace fdf_power

namespace std {

template <>
struct hash<fdf_power::ParentElement> {
  auto operator()(const fdf_power::ParentElement& parent) const -> size_t {
    return hash<fdf_power::ParentElement::ValueType>{}(parent.value_);
  }
};

}  // namespace std

#endif

#endif  // LIB_DRIVER_POWER_CPP_TYPES_H_

// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_POWER_DRIVERS_FUSB302_INSPECTABLE_TYPES_H_
#define SRC_DEVICES_POWER_DRIVERS_FUSB302_INSPECTABLE_TYPES_H_

#include <lib/inspect/cpp/inspect.h>
#include <lib/inspect/cpp/vmo/types.h>

namespace fusb302 {

template <typename T>
class InspectableBool {
 public:
  InspectableBool(inspect::Node* parent, const std::string& name, T initial_value)
      : value_(initial_value), inspect_(parent->CreateBool(name, initial_value)) {}

  T get() const { return value_; }
  void set(T value) {
    value_ = value;
    inspect_.Set(value);
  }

 private:
  T value_;
  inspect::BoolProperty inspect_;
};

template <typename T>
class InspectableUint {
 public:
  InspectableUint(inspect::Node* parent, const std::string& name, T initial_value)
      : value_(initial_value),
        inspect_(parent->CreateUint(name, static_cast<uint64_t>(initial_value))) {}

  T get() const { return value_; }
  void set(T value) {
    value_ = value;
    inspect_.Set(static_cast<uint64_t>(value));
  }

 private:
  T value_;
  inspect::UintProperty inspect_;
};

template <typename T>
class InspectableInt {
 public:
  InspectableInt(inspect::Node* parent, const std::string& name, T initial_value)
      : value_(initial_value),
        inspect_(parent->CreateInt(name, static_cast<int64_t>(initial_value))) {}

  T get() const { return value_; }
  void set(T value) {
    value_ = value;
    inspect_.Set(static_cast<int64_t>(value));
  }

 private:
  T value_;
  inspect::IntProperty inspect_;
};

}  // namespace fusb302

#endif  // SRC_DEVICES_POWER_DRIVERS_FUSB302_INSPECTABLE_TYPES_H_

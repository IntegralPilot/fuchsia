// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/power/cpp/types.h>
#include <zircon/assert.h>
#include <zircon/errors.h>

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)

namespace fdf_power {

zx::result<Transition> Transition::FromFidl(const fuchsia_hardware_power::Transition& src) {
  const auto& target_level = src.target_level();
  if (!target_level.has_value()) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  const auto& latency_us = src.latency_us();
  if (!latency_us.has_value()) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  return zx::ok(Transition{.target_level = target_level.value(), .latency_us = latency_us.value()});
}

zx::result<Transition> Transition::FromFidl(const fuchsia_hardware_power::wire::Transition& src) {
  if (!src.has_target_level() || !src.has_latency_us()) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  return zx::ok(Transition{.target_level = src.target_level(), .latency_us = src.latency_us()});
}

zx::result<LevelTuple> LevelTuple::FromFidl(const fuchsia_hardware_power::LevelTuple& src) {
  const auto& child_level = src.child_level();
  if (!child_level.has_value()) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  const auto& parent_level = src.parent_level();
  if (!parent_level.has_value()) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  return zx::ok(
      LevelTuple{.child_level = child_level.value(), .parent_level = parent_level.value()});
}

zx::result<LevelTuple> LevelTuple::FromFidl(const fuchsia_hardware_power::wire::LevelTuple& src) {
  if (!src.has_child_level() || !src.has_parent_level()) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  return zx::ok(LevelTuple{.child_level = src.child_level(), .parent_level = src.parent_level()});
}

zx::result<PowerLevel> PowerLevel::FromFidl(const fuchsia_hardware_power::PowerLevel& src) {
  const auto& level = src.level();
  if (!level.has_value()) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  const auto& name = src.name();
  if (!name.has_value()) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  std::vector<Transition> transitions;
  const auto& fidl_transitions = src.transitions();
  if (fidl_transitions.has_value()) {
    for (const auto& fidl_transition : fidl_transitions.value()) {
      zx::result transition = Transition::FromFidl(fidl_transition);
      if (transition.is_error()) {
        return transition.take_error();
      }
      transitions.push_back(transition.value());
    }
  }

  return zx::ok(PowerLevel{
      .level = level.value(), .name{name.value()}, .transitions{std::move(transitions)}});
}

zx::result<PowerLevel> PowerLevel::FromFidl(const fuchsia_hardware_power::wire::PowerLevel& src) {
  if (!src.has_level() || !src.has_name()) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  std::vector<Transition> transitions;
  if (src.has_transitions()) {
    for (const auto& fidl_transition : src.transitions()) {
      zx::result transition = Transition::FromFidl(fidl_transition);
      if (transition.is_error()) {
        return transition.take_error();
      }
      transitions.push_back(transition.value());
    }
  }

  return zx::ok(PowerLevel{
      .level = src.level(), .name{src.name().get()}, .transitions{std::move(transitions)}});
}

zx::result<PowerElement> PowerElement::FromFidl(const fuchsia_hardware_power::PowerElement& src) {
  const auto& name = src.name();
  if (!name.has_value()) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  std::vector<PowerLevel> levels;
  const auto& fidl_levels = src.levels();
  if (fidl_levels.has_value()) {
    for (const auto& fidl_level : fidl_levels.value()) {
      zx::result level = PowerLevel::FromFidl(fidl_level);
      if (level.is_error()) {
        return level.take_error();
      }
      levels.push_back(std::move(level.value()));
    }
  }

  return zx::ok(PowerElement{.name{name.value()}, .levels{std::move(levels)}});
}

zx::result<PowerElement> PowerElement::FromFidl(
    const fuchsia_hardware_power::wire::PowerElement& src) {
  if (!src.has_name()) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  std::vector<PowerLevel> levels;
  if (src.has_levels()) {
    for (const auto& fidl_level : src.levels()) {
      zx::result level = PowerLevel::FromFidl(fidl_level);
      if (level.is_error()) {
        return level.take_error();
      }
      levels.push_back(std::move(level.value()));
    }
  }

  return zx::ok(PowerElement{.name{src.name().get()}, .levels{std::move(levels)}});
}

zx::result<ParentElement> ParentElement::FromFidl(
    const fuchsia_hardware_power::ParentElement& src) {
  switch (src.Which()) {
    case fuchsia_hardware_power::ParentElement::Tag::kSag: {
      const auto& fidl_sag = src.sag();
      if (!fidl_sag.has_value()) {
        return zx::error(ZX_ERR_INVALID_ARGS);
      }
      SagElement sag;
      switch (fidl_sag.value()) {
        case fuchsia_hardware_power::SagElement::kExecutionState:
          sag = SagElement::kExecutionState;
          break;
        case fuchsia_hardware_power::SagElement::kApplicationActivity:
          sag = SagElement::kApplicationActivity;
          break;
      }
      return zx::ok(ParentElement::WithSag(sag));
    }
    case fuchsia_hardware_power::ParentElement::Tag::kInstanceName: {
      const auto& fidl_instance_name = src.instance_name();
      if (!fidl_instance_name.has_value()) {
        return zx::error(ZX_ERR_INVALID_ARGS);
      }
      return zx::ok(ParentElement::WithInstanceName(fidl_instance_name.value()));
    }
    case fuchsia_hardware_power::ParentElement::Tag::kCpuControl: {
      if (!src.cpu_control().has_value()) {
        return zx::error(ZX_ERR_INVALID_ARGS);
      }
      return zx::ok(ParentElement::WithCpu(fdf_power::CpuElement::kCpu));
    }
  }
}

zx::result<ParentElement> ParentElement::FromFidl(
    const fuchsia_hardware_power::wire::ParentElement& src) {
  switch (src.Which()) {
    case fuchsia_hardware_power::wire::ParentElement::Tag::kSag: {
      SagElement sag;
      switch (src.sag()) {
        case fuchsia_hardware_power::wire::SagElement::kExecutionState:
          sag = SagElement::kExecutionState;
          break;
        case fuchsia_hardware_power::wire::SagElement::kApplicationActivity:
          sag = SagElement::kApplicationActivity;
          break;
      }
      return zx::ok(ParentElement::WithSag(sag));
    }
    case fuchsia_hardware_power::wire::ParentElement::Tag::kInstanceName: {
      return zx::ok(ParentElement::WithInstanceName(std::string{src.instance_name().get()}));
    }
    case fuchsia_hardware_power::wire::ParentElement::Tag::kCpuControl: {
      switch (src.cpu_control()) {
        case fuchsia_hardware_power::wire::CpuPowerElement::kCpu:
          return zx::ok(ParentElement::WithCpu(CpuElement::kCpu));
        default:
          return zx::error(ZX_ERR_NOT_SUPPORTED);
      }
    }
  }
}

zx::result<PowerDependency> PowerDependency::FromFidl(
    const fuchsia_hardware_power::PowerDependency& src) {
  const auto& child = src.child();
  if (!child.has_value()) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  const auto& fidl_parent = src.parent();
  if (!fidl_parent.has_value()) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  zx::result parent = ParentElement::FromFidl(fidl_parent.value());
  if (parent.is_error()) {
    return parent.take_error();
  }

  std::vector<LevelTuple> level_deps;
  const auto& fidl_level_deps = src.level_deps();
  if (fidl_level_deps.has_value()) {
    for (const auto& fidl_level_dep : fidl_level_deps.value()) {
      zx::result level_dep = LevelTuple::FromFidl(fidl_level_dep);
      if (level_dep.is_error()) {
        return level_dep.take_error();
      }
      level_deps.push_back(level_dep.value());
    }
  }

  return zx::ok(PowerDependency{.child{child.value()},
                                .parent{std::move(parent.value())},
                                .level_deps{std::move(level_deps)}});
}

zx::result<PowerDependency> PowerDependency::FromFidl(
    const fuchsia_hardware_power::wire::PowerDependency& src) {
  if (!src.has_child() || !src.has_parent()) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  zx::result parent = ParentElement::FromFidl(src.parent());
  if (parent.is_error()) {
    return parent.take_error();
  }

  std::vector<LevelTuple> level_deps;
  if (src.has_level_deps()) {
    for (const auto& fidl_level_dep : src.level_deps()) {
      zx::result level_dep = LevelTuple::FromFidl(fidl_level_dep);
      if (level_dep.is_error()) {
        return level_dep.take_error();
      }
      level_deps.push_back(level_dep.value());
    }
  }

  return zx::ok(PowerDependency{.child{std::string{src.child().get()}},
                                .parent{std::move(parent.value())},
                                .level_deps{std::move(level_deps)}});
}

zx::result<PowerElementConfiguration> PowerElementConfiguration::FromFidl(
    const fuchsia_hardware_power::PowerElementConfiguration& src) {
  const auto& fidl_element = src.element();
  if (!fidl_element.has_value()) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  zx::result element = PowerElement::FromFidl(fidl_element.value());
  if (element.is_error()) {
    return element.take_error();
  }

  std::vector<PowerDependency> dependencies;
  const auto& fidl_dependencies = src.dependencies();
  if (fidl_dependencies.has_value()) {
    for (const auto& fidl_dependency : fidl_dependencies.value()) {
      zx::result dependency = PowerDependency::FromFidl(fidl_dependency);
      if (dependency.is_error()) {
        return dependency.take_error();
      }
      dependencies.push_back(std::move(dependency.value()));
    }
  }

  return zx::ok(PowerElementConfiguration{.element{std::move(element.value())},
                                          .dependencies{std::move(dependencies)}});
}

zx::result<PowerElementConfiguration> PowerElementConfiguration::FromFidl(
    const fuchsia_hardware_power::wire::PowerElementConfiguration& src) {
  if (!src.has_element()) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  zx::result element = PowerElement::FromFidl(src.element());
  if (element.is_error()) {
    return element.take_error();
  }

  std::vector<PowerDependency> dependencies;
  if (src.has_dependencies()) {
    for (const auto& fidl_dependency : src.dependencies()) {
      zx::result dependency = PowerDependency::FromFidl(fidl_dependency);
      if (dependency.is_error()) {
        return dependency.take_error();
      }
      dependencies.push_back(std::move(dependency.value()));
    }
  }

  return zx::ok(PowerElementConfiguration{.element{std::move(element.value())},
                                          .dependencies{std::move(dependencies)}});
}

ParentElement ParentElement::WithSag(SagElement sag) {
  ValueType value{std::in_place_index<kSagIndex>, sag};
  return ParentElement{Type::kSag, std::move(value)};
}

ParentElement ParentElement::WithInstanceName(std::string instance_name) {
  ValueType value{std::in_place_index<kInstanceNameIndex>, std::move(instance_name)};
  return ParentElement{Type::kInstanceName, std::move(value)};
}

ParentElement ParentElement::WithCpu(CpuElement cpu) {
  ValueType value{std::in_place_index<kCpuIndex>, cpu};
  return ParentElement{Type::kCpu, std::move(value)};
}

void ParentElement::SetSag(SagElement sag) {
  type_ = Type::kSag;
  value_.emplace<kSagIndex>(sag);
}

void ParentElement::SetInstanceName(std::string instance_name) {
  type_ = Type::kInstanceName;
  value_.emplace<kInstanceNameIndex>(std::move(instance_name));
}

void ParentElement::SetCpu(CpuElement cpu) {
  type_ = Type::kCpu;
  value_.emplace<kCpuIndex>(cpu);
}

std::optional<SagElement> ParentElement::GetSag() const {
  if (type_ != Type::kSag) {
    return std::nullopt;
  }
  CheckTypeValueAlignment();
  return std::get<kSagIndex>(value_);
}

std::optional<std::string> ParentElement::GetInstanceName() const {
  if (type_ != Type::kInstanceName) {
    return std::nullopt;
  }
  CheckTypeValueAlignment();
  return std::get<kInstanceNameIndex>(value_);
}

std::optional<CpuElement> ParentElement::GetCpu() const {
  if (type_ != Type::kCpu) {
    return std::nullopt;
  }

  CheckTypeValueAlignment();
  return std::get<kCpuIndex>(value_);
}

void ParentElement::CheckTypeValueAlignment() const {
  size_t expected_idx = 0;
  switch (type_) {
    case Type::kSag:
      expected_idx = kSagIndex;
      break;
    case Type::kInstanceName:
      expected_idx = kInstanceNameIndex;
      break;
    case Type::kCpu:
      expected_idx = kCpuIndex;
      break;
  }

  ZX_ASSERT_MSG(value_.index() == expected_idx, "Incorrect variant index: Expected %lu but got %lu",
                expected_idx, value_.index());
}

bool ParentElement::operator==(const ParentElement& rhs) const { return value_ == rhs.value_; }

}  // namespace fdf_power

#endif

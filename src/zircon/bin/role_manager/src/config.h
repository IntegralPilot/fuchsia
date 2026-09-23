// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_ZIRCON_BIN_ROLE_MANAGER_SRC_CONFIG_H_
#define SRC_ZIRCON_BIN_ROLE_MANAGER_SRC_CONFIG_H_

#include <fidl/fuchsia.scheduler/cpp/fidl.h>
#include <lib/fit/result.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/zx/profile.h>
#include <lib/zx/time.h>
#include <zircon/syscalls.h>

#include <functional>
#include <map>
#include <span>
#include <string>
#include <string_view>
#include <unordered_map>
#include <variant>
#include <vector>

#include <fbl/enum_bits.h>
#include <fbl/macros.h>
#include <re2/re2.h>

namespace zircon_profile {

enum class ProfileScope {
  None = 0,
  Bringup,
  Core,
  Board,
  Product,
  Builtin,
};
FBL_ENABLE_ENUM_BITS(ProfileScope)

struct Profile {
  Profile(ProfileScope ps, zx_profile_info_t info) : scope(ps), info(info) {}
  Profile(ProfileScope ps, zx_profile_info_t info,
          std::vector<fuchsia_scheduler::Parameter> outputs)
      : scope(ps), info(info), output_parameters(outputs) {}
  ProfileScope scope{ProfileScope::None};
  zx_profile_info_t info{};
  zx::profile profile{};
  std::vector<fuchsia_scheduler::Parameter> output_parameters;

  // Counter exposed via Inspect to track role requests.
  uint64_t request_count = 0;

  void PopulateInspect(inspect::Node& node) const;
};

std::string ScopeToString(ProfileScope scope);

struct MediaRole {
  zx_duration_mono_t capacity;
  zx_duration_mono_t deadline;
};

class Role {
 public:
  // Enforce move semantics.
  Role() = default;
  Role(Role&& other) = default;
  Role& operator=(Role&& other) = default;
  Role(const Role&) = delete;
  Role& operator=(const Role&) = delete;

  // Attempt to create a role with the given name and selectors.
  // `Role`s should always be created with one of these functions, and should never be directly
  // constructed.
  static fit::result<zx_status_t, Role> Create(
      std::string_view name, std::span<const fuchsia_scheduler::Parameter> selectors);
  // TODO(https://fxbug.dev/321037780): Remove ignore_selectors once the MediaProfileProvider (and
  // associated hacks in ProfileProvider) have been deprecated.
  static fit::result<zx_status_t, Role> Create(std::string_view name_with_selectors,
                                               bool ignore_selectors = false);

  // TODO(https://fxbug.dev/321037780): Remove IsTestRole once the ProfileProvider has been
  // deprecated, as the RoleManager has its own integration tests that do not rely on a hardcoded
  // test role.
  bool IsTestRole() const { return name_ == "fuchsia.test-role"; }
  bool HasSelector(std::string_view selector) const;
  std::string_view name() const { return name_; }
  fit::result<fit::failed, MediaRole> ToMediaRole() const;
  bool operator==(const Role& other) const;

 private:
  std::string name_;
  std::map<std::string, fuchsia_scheduler::ParameterValue, std::less<>> selectors_;
  inline static const re2::RE2 kReRoleName{"(\\w[\\w\\-]*(?:\\.\\w[\\w\\-]*)*)"};
  inline static const re2::RE2 kReRoleParts{"(\\w[\\w\\-]*(?:\\.\\w[\\w\\-]*)*)(?::(.+))?"};
  inline static const re2::RE2 kReSelector{"(\\w[\\w\\-]+)(?:=([^,]+))?,?"};
  friend struct RoleHash;
};

struct RoleHash {
  std::size_t operator()(const Role& role) const {
    std::size_t hash = std::hash<std::string_view>{}(role.name_);
    for (const auto& [key, value] : role.selectors_) {
      // Combine the key hash into the overall hash. The hash combination function is taken from
      // boost::hash_combine.
      std::size_t key_hash = std::hash<std::string_view>{}(key);
      hash ^= key_hash + 0x9e3779b9 + (hash << 6) + (hash >> 2);

      // Combine the value hash into the overall hash.
      std::size_t value_hash = 0;
      switch (value.Which()) {
        case fuchsia_scheduler::ParameterValue::Tag::kIntValue:
          value_hash = std::hash<long>{}(value.int_value().value());
          break;
        case fuchsia_scheduler::ParameterValue::Tag::kFloatValue:
          value_hash = std::hash<double>{}(value.float_value().value());
          break;
        case fuchsia_scheduler::ParameterValue::Tag::kStringValue:
          value_hash = std::hash<std::string_view>{}(value.string_value().value());
          break;
        default:
          // We should never hit this case.
          value_hash = 1;
      }
      hash ^= value_hash + 0x9e3779b9 + (hash << 6) + (hash >> 2);
    }
    return hash;
  }
};

using ProfileMap = std::unordered_map<Role, Profile, RoleHash>;

struct ConfiguredProfiles {
  ProfileMap thread;
  ProfileMap memory;
};

fit::result<std::string, ConfiguredProfiles> LoadConfigs(const std::string& config_path);

}  // namespace zircon_profile

#endif  // SRC_ZIRCON_BIN_ROLE_MANAGER_SRC_CONFIG_H_

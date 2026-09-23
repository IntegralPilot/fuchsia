// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "config.h"

#include <fcntl.h>
#include <lib/fit/result.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/profile.h>
#include <lib/zx/time.h>
#include <zircon/assert.h>
#include <zircon/syscalls/profile.h>

#include <algorithm>
#include <charconv>
#include <cmath>
#include <limits>
#include <sstream>
#include <string>
#include <string_view>

#include <fbl/unique_fd.h>
#include <rapidjson/document.h>
#include <rapidjson/error/en.h>
#include <re2/re2.h>
#include <src/lib/files/directory.h>
#include <src/lib/files/file.h>

#include "absl/strings/string_view.h"

using fuchsia_scheduler::Parameter;
using fuchsia_scheduler::ParameterValue;
using zircon_profile::Profile;
using zircon_profile::ProfileScope;
using zircon_profile::Role;

namespace {

constexpr char kConfigFileExtension[] = ".profiles";

std::string ToString(const std::vector<Parameter>& params) {
  std::ostringstream stream;
  stream << "{ ";
  for (const auto& param : params) {
    stream << param.key() << ": ";
    switch (param.value().Which()) {
      case ParameterValue::Tag::kIntValue:
        stream << std::to_string(param.value().int_value().value());
        break;
      case ParameterValue::Tag::kFloatValue:
        stream << std::to_string(param.value().float_value().value());
        break;
      case ParameterValue::Tag::kStringValue:
        stream << param.value().string_value().value();
        break;
      default:
        FX_LOG_KV(WARNING, "invalid output parameter format");
    }
    stream << ", ";
  }
  stream << " }";
  return stream.str();
}

std::string ToString(const zx_profile_info_t& info) {
  std::ostringstream stream;

  stream << "{ ";

  if (info.flags & ZX_PROFILE_INFO_FLAG_PRIORITY) {
    stream << "\"priority\": " << info.priority << ", ";
  }
  if (info.flags & ZX_PROFILE_INFO_FLAG_DEADLINE) {
    stream << "\"capacity\": " << info.deadline_params.capacity
           << ", \"deadline\": " << info.deadline_params.relative_deadline
           << ", \"period\": " << info.deadline_params.period;
    if (info.flags & ZX_PROFILE_INFO_FLAG_CRITICAL) {
      stream << ", \"critical\": true";
    }
    stream << ", ";
  }
  if (info.flags & ZX_PROFILE_INFO_FLAG_CPU_MASK) {
    stream << "\"affinity\": " << info.cpu_affinity_mask.mask[0] << " (0x" << std::hex
           << info.cpu_affinity_mask.mask[0] << "), " << std::dec;
  }

  stream << "}";
  return stream.str();
}

std::string ToString(ProfileScope scope) {
  switch (scope) {
    case ProfileScope::Bringup:
      return "bringup";
    case ProfileScope::Board:
      return "board";
    case ProfileScope::Core:
      return "core";
    case ProfileScope::Product:
      return "product";
    default:
      return "none";
  }
}
// Proxies an iterator over the members of the given value node. As of this
// writing, the version of rapidjson in third_party does not support range-based
// for loops. This adapter provides the missing functionality.
struct IterateMembers {
  explicit IterateMembers(const rapidjson::Value& value)
      : begin_iterator{value.MemberBegin()}, end_iterator{value.MemberEnd()} {}

  auto begin() { return begin_iterator; }
  auto end() { return end_iterator; }

  rapidjson::Value::ConstMemberIterator begin_iterator;
  rapidjson::Value::ConstMemberIterator end_iterator;
};

// Proxies an iterator over the values of the given array node. Provides missing
// functionality similar to the iterator above.
struct IterateValues {
  explicit IterateValues(const rapidjson::Value& value)
      : begin_iterator{value.Begin()}, end_iterator{value.End()} {}

  auto begin() { return begin_iterator; }
  auto end() { return end_iterator; }

  rapidjson::Value::ConstValueIterator begin_iterator;
  rapidjson::Value::ConstValueIterator end_iterator;
};

// Utility to build a fit::result<std::string, ?> in the error state using stream operators.
//
// Example:
//
//   return Error() << "Failed to open file " << filename << "!";
//
class Error {
 public:
  Error() = default;
  explicit Error(const std::string& initial) : stream_{initial} {}

  // Forwards the stream operator argument to the underlying ostream.
  template <typename T>
  Error& operator<<(const T& value) {
    stream_ << value;
    return *this;
  }

  // Implicit conversion to fit::result<std::string, ?> in the error state with accumulated string
  // as the error value.
  template <typename... Vs>
  operator fit::result<std::string, Vs...>() const {
    return fit::error(stream_.str());
  }

  operator fit::error<std::string>() const { return fit::error(stream_.str()); }

 private:
  std::ostringstream stream_;
};

fit::result<std::string, zx::duration> ParseDurationString(const std::string& duration) {
  // Match one or more digits, optionally followed by time units ms, us, or ns.
  static const re2::RE2 kReDuration{"^(\\d+)(ms|us|ns)?$"};

  int64_t scalar;
  std::string units;
  const bool matched = re2::RE2::PartialMatch(duration, kReDuration, &scalar, &units);
  if (!matched) {
    return Error() << "String \"" << duration << "\" is not a valid duration!";
  }

  if (units.empty() || units == "ns") {
    return fit::ok(zx::nsec(scalar));
  }
  if (units == "ms") {
    return fit::ok(zx::msec(scalar));
  }
  if (units == "us") {
    return fit::ok(zx::usec(scalar));
  }
  return Error() << "String duration \"" << duration << "\" has unrecognized units \"" << units
                 << "\"!";
}

fit::result<std::string, zx::duration> ParseDuration(const rapidjson::Value& object) {
  if (object.IsInt()) {
    return fit::ok(zx::nsec(object.GetInt()));
  }
  if (object.IsString()) {
    return ParseDurationString(object.GetString());
  }
  return fit::error("Duration must be an integer or duration string!");
}

struct TextPosition {
  int32_t line;
  int32_t column;
};

TextPosition GetLineAndColumnForOffset(const std::string& input, size_t offset) {
  if (offset == 0) {
    // Errors at position 0 are assumed to be related to the whole file.
    return {.line = 0, .column = 0};
  }

  TextPosition position = {.line = 1, .column = 1};
  for (size_t i = 0; i < input.size() && i < offset; i++) {
    if (input[i] == '\n') {
      position.line += 1;
      position.column = 1;
    } else {
      position.column += 1;
    }
  }

  return position;
}

std::string GetErrorMessage(const rapidjson::Document& document, const std::string& file_data) {
  const auto [line, column] = GetLineAndColumnForOffset(file_data, document.GetErrorOffset());
  std::ostringstream stream;
  stream << line << ":" << column << ": " << GetParseError_En(document.GetParseError());
  return stream.str();
}

template <typename... Context>
auto GetMember(const char* name, const rapidjson::Value& object, Context&&... context)
    -> fit::result<std::string, decltype(std::cref(object[name]))> {
  if (!object.IsObject()) {
    return (Error() << ... << std::forward<Context>(context)) << " must be a JSON object!";
  }
  if (!object.HasMember(name)) {
    return (Error() << ... << std::forward<Context>(context))
           << " must have a \"" << name << "\" member!";
  }
  return fit::ok(std::cref(object[name]));
}

template <typename... Context>
fit::result<std::string, int> GetInt(const char* name, const rapidjson::Value& object,
                                     Context&&... context) {
  auto result = GetMember(name, object, std::forward<Context>(context)...);
  if (result.is_error()) {
    return result.take_error();
  }
  if (!result->get().IsInt()) {
    return (Error() << ... << std::forward<Context>(context))
           << " member \"" << name << "\" must be an integer!";
  }
  return fit::ok(result->get().GetInt());
}

template <typename... Context>
fit::result<std::string, const char*> GetString(const char* name, const rapidjson::Value& object,
                                                Context&&... context) {
  auto result = GetMember(name, object, std::forward<Context>(context)...);
  if (result.is_error()) {
    return result.take_error();
  }
  if (!result->get().IsString()) {
    return (Error() << ... << std::forward<Context>(context))
           << " member \"" << name << "\" must be a string!";
  }
  return fit::ok(result->get().GetString());
}

template <typename... Context>
auto GetArray(const char* name, const rapidjson::Value& object, Context&&... context) {
  auto result = GetMember(name, object, std::forward<Context>(context)...);
  if (result.is_ok() && !result->get().IsArray()) {
    return (Error() << ... << std::forward<Context>(context))
           << " member \"" << name << "\" must be an array!";
  }
  return result;
}

template <typename... Context>
auto GetObject(const char* name, const rapidjson::Value& object, Context&&... context) {
  auto result = GetMember(name, object, std::forward<Context>(context)...);
  if (result.is_ok() && !result->get().IsObject()) {
    return (Error() << ... << std::forward<Context>(context))
           << " member \"" << name << "\" must be a JSON object!";
  }
  return result;
}

template <typename... Context>
fit::result<std::string, unsigned int> GetUint(const char* name, const rapidjson::Value& object,
                                               Context&&... context) {
  auto result = GetMember(name, object, std::forward<Context>(context)...);
  if (result.is_error()) {
    return result.take_error();
  }
  if (!result->get().IsUint()) {
    return (Error() << ... << std::forward<Context>(context)).rdbuf()
           << " member \"" << name << "\" must be an unsigned integer!";
  }
  return fit::ok(result->get().GetUint());
}

std::optional<zx_profile_info_t> ParseThreadProfile(const std::string& filename,
                                                    const char* profile_name,
                                                    const rapidjson::Value::Member& profile) {
  const bool has_priority = profile.value.HasMember("priority");
  const bool has_capacity = profile.value.HasMember("capacity");
  const bool has_deadline = profile.value.HasMember("deadline");
  const bool has_period = profile.value.HasMember("period");
  const bool has_affinity = profile.value.HasMember("affinity");
  const bool has_critical = profile.value.HasMember("critical");

  // The Zircon scheduler treats deadline as equal to period, so consider the profile as complete if
  // either deadline or period (or both) are present.
  const bool has_complete_deadline = has_capacity && (has_deadline || has_period);
  const bool has_some_deadline = has_capacity || has_deadline || has_period || has_critical;

  zx_profile_info_t info{};
  if (has_priority && !has_some_deadline) {
    auto result = GetInt("priority", profile.value, "Profile ", profile_name);
    if (result.is_ok()) {
      info.flags = ZX_PROFILE_INFO_FLAG_PRIORITY;
      info.priority = std::clamp<int32_t>(result.value(), ZX_PRIORITY_LOWEST, ZX_PRIORITY_HIGHEST);
    } else {
      FX_LOG_KV(WARNING, result.error_value().c_str(), FX_KV("profile_name", profile_name),
                FX_KV("tag", "RoleManager"));
      return std::nullopt;
    }
  } else if (!has_priority && has_complete_deadline) {
    auto capacity_result = ParseDuration(profile.value["capacity"]);
    if (capacity_result.is_error()) {
      FX_LOG_KV(WARNING, capacity_result.error_value().c_str(), FX_KV("profile_name", profile_name),
                FX_KV("tag", "RoleManager"));
      return std::nullopt;
    }

    zx_duration_mono_t deadline;
    zx_duration_mono_t period;
    if (has_deadline) {
      auto deadline_result = ParseDuration(profile.value["deadline"]);
      if (deadline_result.is_error()) {
        FX_LOG_KV(WARNING, deadline_result.error_value().c_str(),
                  FX_KV("profile_name", profile_name), FX_KV("tag", "RoleManager"));
        return std::nullopt;
      }
      deadline = deadline_result->get();
    }
    if (has_period) {
      auto period_result = ParseDuration(profile.value["period"]);
      if (period_result.is_error()) {
        FX_LOG_KV(WARNING, period_result.error_value().c_str(), FX_KV("profile_name", profile_name),
                  FX_KV("tag", "RoleManager"));
        return std::nullopt;
      }
      period = period_result->get();
    }

    info.flags = ZX_PROFILE_INFO_FLAG_DEADLINE;
    if (has_critical) {
      const auto& critical_member = profile.value["critical"];
      if (!critical_member.IsBool()) {
        FX_LOG_KV(WARNING, "Profile member \"critical\" must be a boolean!",
                  FX_KV("filename", filename), FX_KV("profile_name", profile_name),
                  FX_KV("tag", "RoleManager"));
        return std::nullopt;
      }
      if (critical_member.GetBool()) {
        info.flags |= ZX_PROFILE_INFO_FLAG_CRITICAL;
      }
    }
    // has_complete_deadline guarantees that at least one of deadline or period exists in profile.
    info.deadline_params =
        zx_sched_deadline_params_t{.capacity = capacity_result->get(),
                                   .relative_deadline = has_deadline ? deadline : period,
                                   .period = has_period ? period : deadline};
  } else if (has_priority && has_some_deadline) {
    FX_LOG_KV(WARNING, "Priority and deadline parameters are mutually exclusive!",
              FX_KV("filename", filename), FX_KV("profile_name", profile_name),
              FX_KV("tag", "RoleManager"));
    return std::nullopt;
  } else if (!has_priority && !has_complete_deadline && has_some_deadline) {
    FX_LOG_KV(WARNING,
              "Deadline profiles must specify \"capacity\" and (\"deadline\" or \"period\")!",
              FX_KV("filename", filename), FX_KV("profile_name", profile_name),
              FX_KV("tag", "RoleManager"));
    return std::nullopt;
  }

  if (has_affinity) {
    const auto& affinity_member = profile.value["affinity"];
    const bool is_uint = affinity_member.IsUint64();
    const bool is_array = affinity_member.IsArray();

    info.flags |= ZX_PROFILE_INFO_FLAG_CPU_MASK;

    if (is_uint) {
      static_assert(std::numeric_limits<uint64_t>::digits <= ZX_CPU_SET_BITS_PER_WORD);
      info.cpu_affinity_mask.mask[0] = affinity_member.GetUint64();
    } else if (is_array) {
      size_t element_count = 0;
      bool failed = false;

      for (const auto& value : IterateValues(affinity_member)) {
        if (!value.IsUint()) {
          FX_LOG_KV(WARNING,
                    "Array element of profile member \"affinity\" must be an "
                    "unsigned integer!",
                    FX_KV("element_count", element_count), FX_KV("filename", filename),
                    FX_KV("profile_name", profile_name), FX_KV("tag", "RoleManager"));
          failed = true;
          break;
        }

        element_count++;
        const size_t cpu_number = value.GetUint();

        if (cpu_number >= ZX_CPU_SET_MAX_CPUS) {
          FX_LOG_KV(WARNING, "Profile member \"affinity\" must be an integer < ZX_CPU_SET_MAX_CPUS",
                    FX_KV("filename", filename), FX_KV("profile_name", profile_name),
                    FX_KV("ZX_CPU_SET_MAX_CPUS", ZX_CPU_SET_MAX_CPUS), FX_KV("tag", "RoleManager"));
          failed = true;
          break;
        }

        info.cpu_affinity_mask.mask[cpu_number / ZX_CPU_SET_BITS_PER_WORD] |=
            uint64_t{1} << (cpu_number % ZX_CPU_SET_BITS_PER_WORD);
      }
      if (failed) {
        return std::nullopt;
      }
    } else {
      FX_LOG_KV(WARNING, "Profile member \"affinity\" must be a uint64 or an array of CPU indices!",
                FX_KV("filename", filename), FX_KV("profile_name", profile_name),
                FX_KV("tag", "RoleManager"));
      return std::nullopt;
    }
  }  // if (has_affinity)

  return info;
}

std::optional<zx_profile_info_t> ParseMemoryProfile(const std::string& filename,
                                                    const char* profile_name,
                                                    const rapidjson::Value::Member& profile) {
  const bool has_priority = profile.value.HasMember("priority");

  zx_profile_info_t info{};
  if (has_priority) {
    auto result = GetInt("priority", profile.value, "Profile ", profile_name);
    if (result.is_ok()) {
      info.flags = ZX_PROFILE_INFO_FLAG_MEMORY_PRIORITY;
      info.priority = std::clamp<int32_t>(result.value(), ZX_PRIORITY_LOWEST, ZX_PRIORITY_HIGHEST);
    } else {
      FX_LOG_KV(WARNING, result.error_value().c_str(), FX_KV("profile_name", profile_name),
                FX_KV("tag", "RoleManager"));
      return std::nullopt;
    }
  }
  return info;
}

using SingleProfileParser = std::optional<zx_profile_info_t>(
    const std::string& filename, const char* profile_name, const rapidjson::Value::Member& profile);

void ParseProfiles(const std::string& filename, const rapidjson::Document& document,
                   const char* type_name, SingleProfileParser parser,
                   zircon_profile::ProfileMap* profiles) {
  if (!document.IsObject()) {
    FX_LOG_KV(WARNING, "The profile config document must be a JSON object!",
              FX_KV("filename", filename), FX_KV("tag", "RoleManager"));
    return;
  }

  if (!document.HasMember(type_name)) {
    return;
  }
  const rapidjson::Value& profile_member = document[type_name];

  ProfileScope scope = ProfileScope::None;
  if (document.HasMember("scope")) {
    auto result = GetString("scope", document);
    if (result.is_ok()) {
      if (!strcmp(*result, "bringup")) {
        scope = ProfileScope::Bringup;
      } else if (!strcmp(*result, "board")) {
        scope = ProfileScope::Board;
      } else if (!strcmp(*result, "core")) {
        scope = ProfileScope::Core;
      } else if (!strcmp(*result, "product")) {
        scope = ProfileScope::Product;
      } else {
        FX_LOG_KV(WARNING, "Invalid role scope, defaulting to none!", FX_KV("filename", filename),
                  FX_KV("scope", *result), FX_KV("tag", "RoleManager"));
      }
    }
  } else {
    FX_LOG_KV(WARNING, "Missing role scope, defaulting to none!", FX_KV("filename", filename));
  }

  for (const auto& profile : IterateMembers(profile_member)) {
    const char* profile_name = profile.name.GetString();
    if (!profile.value.IsObject()) {
      FX_LOG_KV(WARNING, "Profile value must be a JSON object!", FX_KV("filename", filename),
                FX_KV("type", type_name), FX_KV("profile", profile_name),
                FX_KV("tag", "RoleManager"));
      continue;
    }

    fit::result role = Role::Create(profile_name);
    if (role.is_error()) {
      FX_LOG_KV(WARNING, "Failed to create role from JSON object!", FX_KV("filename", filename),
                FX_KV("type", type_name), FX_KV("profile", profile_name),
                FX_KV("tag", "RoleManager"));
      continue;
    }

    auto result = parser(filename, profile_name, profile);
    if (!result.has_value()) {
      continue;
    }

    zx_profile_info_t& info = *result;

    if (info.flags == 0) {
      FX_LOG_KV(WARNING, "Ignoring empty profile.", FX_KV("filename", filename),
                FX_KV("type", type_name), FX_KV("profile_name", profile_name),
                FX_KV("tag", "RoleManager"));
      continue;
    }

    std::vector<Parameter> output_parameters = {};
    if (profile.value.HasMember("output_parameters")) {
      const auto& output_param_member = profile.value["output_parameters"];
      if (!output_param_member.IsObject()) {
        FX_LOG_KV(WARNING, "Output parameters must be a JSON object!", FX_KV("filename", filename),
                  FX_KV("type", type_name), FX_KV("profile", profile_name),
                  FX_KV("tag", "RoleManager"));
        continue;
      }
      for (const auto& m : IterateMembers(output_param_member)) {
        if (m.value.IsInt()) {
          output_parameters.push_back(
              Parameter{m.name.GetString(), ParameterValue::WithIntValue(m.value.GetInt())});
        } else if (m.value.IsDouble()) {
          output_parameters.push_back(
              Parameter{m.name.GetString(), ParameterValue::WithFloatValue(m.value.GetDouble())});
        } else if (m.value.IsString()) {
          output_parameters.push_back(
              Parameter{m.name.GetString(), ParameterValue::WithStringValue(m.value.GetString())});
        } else {
          FX_LOG_KV(WARNING, "Output parameter value must be a float, integer, or string!",
                    FX_KV("parameter_name", m.name.GetString()), FX_KV("filename", filename),
                    FX_KV("type", type_name), FX_KV("profile", profile_name),
                    FX_KV("tag", "RoleManager"));
        }
      }
    }

    Profile p(scope, info, output_parameters);
    const auto [iter, added] = profiles->insert(std::pair{std::move(role.value()), std::move(p)});
    if (!added) {
      const ProfileScope existing_scope = iter->second.scope;
      if (existing_scope >= scope) {
        FX_LOG_KV(WARNING, "Profile already exists at scope.", FX_KV("filename", filename),
                  FX_KV("profile_name", profile_name),
                  FX_KV("existing_scope", ToString(existing_scope)), FX_KV("type", type_name),
                  FX_KV("profile_name", profile_name), FX_KV("scope", ToString(scope)),
                  FX_KV("tag", "RoleManager"));
        continue;
      }
      if (iter->second.scope < scope) {
        FX_LOG_KV(INFO, "Profile overridden at scope.", FX_KV("filename", filename),
                  FX_KV("type", type_name), FX_KV("profile_name", profile_name),
                  FX_KV("scope", ToString(scope)), FX_KV("profile_name", profile_name),
                  FX_KV("tag", "RoleManager"));
        iter->second = Profile(scope, info, std::move(output_parameters));
      }
    }
  }  // for (const auto& profile : IterateMembers(document))
}

}  // anonymous namespace

namespace zircon_profile {

fit::result<std::string, ConfiguredProfiles> LoadConfigs(const std::string& config_path) {
  fbl::unique_fd dir_fd(openat(AT_FDCWD, config_path.c_str(), O_RDONLY | O_DIRECTORY));
  if (!dir_fd.is_valid()) {
    // A non-existent directory is not an error.
    FX_LOG_KV(WARNING, "Failed to open config dir.", FX_KV("config_path", config_path),
              FX_KV("error", strerror(errno)), FX_KV("tag", "RoleManager"));
    return fit::ok(ConfiguredProfiles{});
  }

  std::vector<std::string> dir_entries;
  if (!files::ReadDirContentsAt(dir_fd.get(), ".", &dir_entries)) {
    return Error() << "Could not read directory contents from path " << config_path << " error "
                   << strerror(errno);
  }

  const auto filename_predicate = [](const std::string& filename) {
    const auto pos = filename.rfind(kConfigFileExtension);
    return pos != std::string::npos && pos == (filename.size() - std::strlen(kConfigFileExtension));
  };

  ConfiguredProfiles profiles;

  // Define fuchsia.default at builtin scope to prevent overrides from config files.
  {
    zx_profile_info_t info{};
    info.flags = ZX_PROFILE_INFO_FLAG_PRIORITY;
    info.priority = ZX_PRIORITY_DEFAULT;
    fit::result default_role_result = Role::Create("fuchsia.default");
    ZX_DEBUG_ASSERT(default_role_result.is_ok());
    auto [iter, added] = profiles.thread.emplace(std::move(default_role_result.value()),
                                                 Profile{ProfileScope::Builtin, info});
    ZX_DEBUG_ASSERT(added);
  }
  {
    zx_profile_info_t info{};
    info.flags = ZX_PROFILE_INFO_FLAG_MEMORY_PRIORITY;
    info.priority = ZX_PRIORITY_DEFAULT;
    fit::result default_role_result = Role::Create("fuchsia.default");
    ZX_DEBUG_ASSERT(default_role_result.is_ok());
    auto [iter, added] = profiles.memory.emplace(std::move(default_role_result.value()),
                                                 Profile{ProfileScope::Builtin, info});
    ZX_DEBUG_ASSERT(added);
  }

  for (const auto& entry : dir_entries) {
    if (!files::IsFileAt(dir_fd.get(), entry) || !filename_predicate(entry)) {
      continue;
    }

    FX_LOG_KV(INFO, "Loading config.", FX_KV("config_path", entry), FX_KV("tag", "RoleManager"));

    std::string data;
    if (!files::ReadFileToStringAt(dir_fd.get(), entry, &data)) {
      FX_LOG_KV(WARNING, "Failed to read file.", FX_KV("config_path", entry),
                FX_KV("error", strerror(errno)), FX_KV("tag", "RoleManager"));
      continue;
    }

    rapidjson::Document document;
    const auto kFlags = rapidjson::kParseCommentsFlag | rapidjson::kParseTrailingCommasFlag |
                        rapidjson::kParseIterativeFlag;
    document.Parse<kFlags>(data);

    if (document.HasParseError()) {
      FX_LOG_KV(WARNING, "Failed to parse config.", FX_KV("config_path", entry),
                FX_KV("error", GetErrorMessage(document, data)), FX_KV("tag", "RoleManager"));
      continue;
    }

    ParseProfiles(entry, document, "profiles", ParseThreadProfile, &profiles.thread);
    ParseProfiles(entry, document, "memory", ParseMemoryProfile, &profiles.memory);
  }

  auto log_profiles = [](ProfileMap& profiles) {
    for (const auto& [key, value] : profiles) {
      FX_LOG_KV(DEBUG, "Loaded profile.", FX_KV("key", key.name()),
                FX_KV("scope", ToString(value.scope)), FX_KV("info", ToString(value.info)),
                FX_KV("tag", "RoleManager"),
                FX_KV("output_parameters", ToString(value.output_parameters)));
    }
  };
  FX_LOG_KV(DEBUG, "Loaded thread profiles:");
  log_profiles(profiles.thread);
  FX_LOG_KV(DEBUG, "Defined memory profiles:");
  log_profiles(profiles.memory);

  return fit::ok(std::move(profiles));
}

fit::result<zx_status_t, Role> Role::Create(std::string_view name,
                                            std::span<const Parameter> selectors) {
  // Validate the name. It should have no selectors embedded in it.
  Role role;
  if (!re2::RE2::FullMatch(name, kReRoleName, &role.name_)) {
    FX_LOG_KV(WARNING, "Bad role name.", FX_KV("role_name", name), FX_KV("tag", "RoleManager"));
    return fit::error(ZX_ERR_INVALID_ARGS);
  }
  for (const auto& selector : selectors) {
    role.selectors_.try_emplace(selector.key(), selector.value());
  }
  return fit::ok(std::move(role));
}

// ToLong attempts to convert the given string into a long and populates *out with the result.
// Returns true if str is indeed a long, false otherwise.
bool ToLong(std::string_view str, long* out) {
  long value = 0;
  auto [ptr, ec] = std::from_chars(str.data(), str.data() + str.size(), value);
  if (ec != std::errc{} || ptr != str.data() + str.size()) {
    return false;
  }
  *out = value;
  return true;
}

// ToDouble attempts to convert the given string into a double and populates *out with the result.
// Returns true if str is indeed a double, false otherwise.
// Note that this will return true for any whole number, so it's important that the caller checks
// if the number is a long using ToLong before calling this function.
bool ToDouble(std::string_view str, double* out) {
  double value = 0.0;
  auto [ptr, ec] = std::from_chars(str.data(), str.data() + str.size(), value);
  if (ec != std::errc{} || ptr != str.data() + str.size()) {
    return false;
  }
  *out = value;
  return true;
}

fit::result<zx_status_t, Role> Role::Create(std::string_view name_with_selectors,
                                            bool ignore_selectors) {
  Role role;
  std::string selectors;
  if (!re2::RE2::FullMatch(name_with_selectors, kReRoleParts, &role.name_, &selectors)) {
    FX_LOG_KV(WARNING, "Bad selector.", FX_KV("role_selector", name_with_selectors),
              FX_KV("tag", "RoleManager"));
    return fit::error(ZX_ERR_INVALID_ARGS);
  }

  // If the caller wants to us to ignore the selectors, exit early.
  if (ignore_selectors) {
    return fit::ok(std::move(role));
  }

  // Parse the selectors.
  absl::string_view input{selectors};
  std::string key, raw_value;
  while (re2::RE2::Consume(&input, kReSelector, &key, &raw_value)) {
    ParameterValue value = ParameterValue::WithStringValue(raw_value);
    long l;
    double d;
    if (ToLong(raw_value, &l)) {
      value = ParameterValue::WithIntValue(l);
    } else if (ToDouble(raw_value, &d)) {
      value = ParameterValue::WithFloatValue(d);
    }
    const auto [iter, added] = role.selectors_.emplace(key, value);
    if (!added) {
      FX_LOG_KV(WARNING, "Duplicate key in selector.", FX_KV("key", key), FX_KV("value", raw_value),
                FX_KV("role_selector", name_with_selectors), FX_KV("tag", "RoleManager"));
    }
  }
  return fit::ok(std::move(role));
}

bool Role::HasSelector(std::string_view selector) const {
  auto search = selectors_.find(selector);
  return search != selectors_.end();
}

fit::result<fit::failed, MediaRole> Role::ToMediaRole() const {
  const auto realm_iter = selectors_.find("realm");
  if (realm_iter == selectors_.end() || realm_iter->second.string_value().value() != "media") {
    return fit::failed{};
  }

  const auto capacity_iter = selectors_.find("capacity");
  const auto deadline_iter = selectors_.find("deadline");
  if (capacity_iter == selectors_.end() || deadline_iter == selectors_.end()) {
    return fit::failed{};
  }

  if (capacity_iter->second.Which() != ParameterValue::Tag::kIntValue) {
    FX_LOG_KV(WARNING, "Media role has invalid capacity selector.", FX_KV("role_name", name_),
              FX_KV("tag", "RoleManager"));
    return fit::failed{};
  }
  int64_t capacity = capacity_iter->second.int_value().value();

  if (deadline_iter->second.Which() != ParameterValue::Tag::kIntValue) {
    FX_LOG_KV(WARNING, "Media role has invalid deadline selector.", FX_KV("role_name", name_),
              FX_KV("tag", "RoleManager"));
    return fit::failed{};
  }
  int64_t deadline = deadline_iter->second.int_value().value();

  return fit::ok(MediaRole{.capacity = capacity, .deadline = deadline});
}

bool Role::operator==(const Role& other) const {
  // The role names must be the same.
  if (name_ != other.name_) {
    return false;
  }
  // The other role must have the exact same selectors as this one.
  if (selectors_.size() != other.selectors_.size()) {
    return false;
  }
  for (const auto& [key, value] : selectors_) {
    auto it = other.selectors_.find(key);
    if (it == other.selectors_.end()) {
      return false;
    }
    if (value.Which() != it->second.Which()) {
      return false;
    }
    switch (value.Which()) {
      case ParameterValue::Tag::kIntValue:
        if (value.int_value().value() != it->second.int_value().value()) {
          return false;
        }
        break;
      case ParameterValue::Tag::kFloatValue:
        if (value.float_value().value() != it->second.float_value().value()) {
          return false;
        }
        break;
      case ParameterValue::Tag::kStringValue:
        if (value.string_value().value() != it->second.string_value().value()) {
          return false;
        }
        break;
      default:
        // We should never hit this case.
        return false;
    }
  }
  return true;
}

std::string ScopeToString(ProfileScope scope) {
  switch (scope) {
    case ProfileScope::Bringup:
      return "bringup";
    case ProfileScope::Board:
      return "board";
    case ProfileScope::Core:
      return "core";
    case ProfileScope::Product:
      return "product";
    case ProfileScope::Builtin:
      return "builtin";
    default:
      return "none";
  }
}

void Profile::PopulateInspect(inspect::Node& node) const {
  node.RecordString("scope", ScopeToString(scope));

  if (info.flags & ZX_PROFILE_INFO_FLAG_PRIORITY) {
    node.RecordInt("priority", info.priority);
  }

  if (info.flags & ZX_PROFILE_INFO_FLAG_DEADLINE) {
    node.RecordInt("capacity", info.deadline_params.capacity);
    node.RecordInt("deadline", info.deadline_params.relative_deadline);
    node.RecordInt("period", info.deadline_params.period);
  }

  if (info.flags & ZX_PROFILE_INFO_FLAG_CRITICAL) {
    node.RecordBool("critical", true);
  }

  if (info.flags & ZX_PROFILE_INFO_FLAG_CPU_MASK) {
    std::ostringstream stream;
    stream << "0x" << std::hex << info.cpu_affinity_mask.mask[0];
    node.RecordString("affinity", stream.str());
  }

  if (!output_parameters.empty()) {
    node.RecordChild("output_parameters", [&](inspect::Node& params_node) {
      for (const auto& param : output_parameters) {
        switch (param.value().Which()) {
          case fuchsia_scheduler::ParameterValue::Tag::kIntValue:
            params_node.RecordInt(param.key(), param.value().int_value().value());
            break;
          case fuchsia_scheduler::ParameterValue::Tag::kFloatValue:
            params_node.RecordDouble(param.key(), param.value().float_value().value());
            break;
          case fuchsia_scheduler::ParameterValue::Tag::kStringValue:
            params_node.RecordString(param.key(), param.value().string_value().value());
            break;
          default:
            break;
        }
      }
    });
  }

  node.RecordUint("request_count", request_count);
}

}  // namespace zircon_profile

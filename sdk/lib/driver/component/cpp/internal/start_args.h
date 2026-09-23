// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_COMPONENT_CPP_INTERNAL_START_ARGS_H_
#define LIB_DRIVER_COMPONENT_CPP_INTERNAL_START_ARGS_H_

#include <fidl/fuchsia.component.runner/cpp/fidl.h>
#include <fidl/fuchsia.data/cpp/fidl.h>
#include <lib/driver/symbols/symbols.h>
#include <lib/zx/result.h>
#include <zircon/status.h>

#include <vector>

namespace fdf_internal {

inline zx::result<std::string> ProgramValue(const fuchsia_data::wire::Dictionary& program,
                                            std::string_view key) {
  if (program.has_entries()) {
    for (auto& entry : program.entries()) {
      if (!std::equal(key.begin(), key.end(), entry.key.begin())) {
        continue;
      }
      if (!entry.value.has_value() || !entry.value->is_str()) {
        return zx::error(ZX_ERR_WRONG_TYPE);
      }
      auto& value = entry.value->str();
      return zx::ok(std::string{value.data(), value.size()});
    }
  }
  return zx::error(ZX_ERR_NOT_FOUND);
}

inline zx::result<std::string> ProgramValue(const std::optional<fuchsia_data::Dictionary>& program,
                                            std::string_view key) {
  if (program.has_value() && program->entries().has_value()) {
    for (const auto& entry : *program->entries()) {
      if (key != entry.key()) {
        continue;
      }
      auto value = entry.value()->str();
      if (!value.has_value()) {
        return zx::error(ZX_ERR_WRONG_TYPE);
      }
      return zx::ok(value.value());
    }
  }
  return zx::error(ZX_ERR_NOT_FOUND);
}

// Returns the list of values for |key| as a vector of strings.
inline zx::result<std::vector<std::string>> ProgramValueAsVector(
    const fuchsia_data::wire::Dictionary& program, std::string_view key) {
  if (program.has_entries()) {
    for (auto& entry : program.entries()) {
      if (!std::equal(key.begin(), key.end(), entry.key.begin())) {
        continue;
      }
      if (!entry.value.has_value() || !entry.value->is_str_vec()) {
        return zx::error(ZX_ERR_WRONG_TYPE);
      }
      auto& values = entry.value->str_vec();
      std::vector<std::string> result;
      result.reserve(values.size());
      for (auto& value : values) {
        result.emplace_back(value.data(), value.size());
      }
      return zx::ok(result);
    }
  }
  return zx::error(ZX_ERR_NOT_FOUND);
}

// Returns the list of values for |key| as a vector of strings.
inline zx::result<std::vector<std::string>> ProgramValueAsVector(
    const fuchsia_data::Dictionary& program, std::string_view key) {
  auto program_entries = program.entries();
  if (program_entries.has_value()) {
    for (auto& entry : program_entries.value()) {
      auto& entry_key = entry.key();
      auto& entry_value = entry.value();

      if (key != entry_key) {
        continue;
      }

      if (entry_value->Which() != fuchsia_data::DictionaryValue::Tag::kStrVec) {
        return zx::error(ZX_ERR_WRONG_TYPE);
      }

      return zx::ok(entry_value->str_vec().value());
    }
  }
  return zx::error(ZX_ERR_NOT_FOUND);
}

inline zx::result<std::vector<fuchsia_data::wire::Dictionary>> ProgramValueAsObjVector(
    const fuchsia_data::wire::Dictionary& program, std::string_view key) {
  if (!program.has_entries()) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  for (auto& entry : program.entries()) {
    if (!std::equal(key.begin(), key.end(), entry.key.begin())) {
      continue;
    }
    if (!entry.value.has_value() || !entry.value->is_obj_vec()) {
      return zx::error(ZX_ERR_WRONG_TYPE);
    }
    auto& values = entry.value->obj_vec();
    std::vector<fuchsia_data::wire::Dictionary> result;
    result.reserve(values.size());
    for (auto& value : values) {
      result.emplace_back(value);
    }
    return zx::ok(result);
  }
  return zx::error(ZX_ERR_NOT_FOUND);
}

inline zx::result<std::vector<fuchsia_data::Dictionary>> ProgramValueAsObjVector(
    const fuchsia_data::Dictionary& program, std::string_view key) {
  auto program_entries = program.entries();
  if (!program_entries.has_value()) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  for (auto& entry : program_entries.value()) {
    auto& entry_key = entry.key();
    auto& entry_value = entry.value();

    if (key != entry_key) {
      continue;
    }

    if (entry_value->Which() != fuchsia_data::DictionaryValue::Tag::kObjVec) {
      return zx::error(ZX_ERR_WRONG_TYPE);
    }

    return zx::ok(entry_value->obj_vec().value());
  }
  return zx::error(ZX_ERR_NOT_FOUND);
}

inline zx::result<fidl::UnownedClientEnd<fuchsia_io::Directory>> NsValue(
    const fidl::VectorView<fuchsia_component_runner::wire::ComponentNamespaceEntry>& entries,
    std::string_view path) {
  for (auto& entry : entries) {
    if (std::equal(path.begin(), path.end(), entry.path().begin())) {
      return zx::ok<fidl::UnownedClientEnd<fuchsia_io::Directory>>(entry.directory());
    }
  }
  return zx::error(ZX_ERR_NOT_FOUND);
}

inline zx::result<fidl::UnownedClientEnd<fuchsia_io::Directory>> NsValue(
    const std::vector<fuchsia_component_runner::ComponentNamespaceEntry>& entries,
    std::string_view path) {
  for (auto& entry : entries) {
    auto entry_path = entry.path();
    ZX_ASSERT_MSG(entry_path.has_value(), "The entry's path cannot be empty.");
    if (path == entry_path.value()) {
      auto& entry_directory = entry.directory();
      ZX_ASSERT_MSG(entry_directory.has_value(), "The entry's directory cannot be empty.");
      return zx::ok<fidl::UnownedClientEnd<fuchsia_io::Directory>>(entry_directory.value());
    }
  }
  return zx::error(ZX_ERR_NOT_FOUND);
}

}  // namespace fdf_internal

#endif  // LIB_DRIVER_COMPONENT_CPP_INTERNAL_START_ARGS_H_

// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_COMPONENT_CPP_INTERNAL_SYMBOLS_H_
#define LIB_DRIVER_COMPONENT_CPP_INTERNAL_SYMBOLS_H_

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>

#include <unordered_set>

namespace fdf_internal {

template <typename T>
zx::result<T> SymbolValue(const fuchsia_driver_framework::wire::DriverStartArgs& args,
                          std::string_view name) {
  if (!args.has_symbols()) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  const fidl::VectorView<fuchsia_driver_framework::wire::NodeSymbol>& symbols = args.symbols();
  static_assert(sizeof(T) == sizeof(zx_vaddr_t), "T must match zx_vaddr_t in size");
  for (auto& symbol : symbols) {
    if (std::equal(name.begin(), name.end(), symbol.name().begin())) {
      T value;
      memcpy(&value, &symbol.address(), sizeof(zx_vaddr_t));
      return zx::ok(value);
    }
  }
  return zx::error(ZX_ERR_NOT_FOUND);
}

template <typename T>
zx::result<T> SymbolValue(
    const std::optional<std::vector<fuchsia_driver_framework::NodeSymbol>>& symbols,
    std::string_view name) {
  if (!symbols.has_value()) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  static_assert(sizeof(T) == sizeof(zx_vaddr_t), "T must match zx_vaddr_t in size");
  for (auto& symbol : *symbols) {
    if (!symbol.name().has_value() || name != symbol.name().value()) {
      continue;
    }
    if (!symbol.address().has_value()) {
      continue;
    }
    T value;
    memcpy(&value, &symbol.address().value(), sizeof(zx_vaddr_t));
    return zx::ok(value);
  }
  return zx::error(ZX_ERR_NOT_FOUND);
}

template <typename T>
T GetSymbol(const std::optional<std::vector<fuchsia_driver_framework::NodeSymbol>>& symbols,
            std::string_view name, T default_value = nullptr) {
  auto value = SymbolValue<T>(symbols, name);
  return value.is_ok() ? *value : default_value;
}

template <typename T>
zx::result<T> SymbolValue(
    const std::optional<std::vector<fuchsia_driver_framework::NodeSymbol>>& symbols,
    std::string_view module_name, std::string_view symbol_name) {
  if (!symbols.has_value()) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  static_assert(sizeof(T) == sizeof(zx_vaddr_t), "T must match zx_vaddr_t in size");
  for (auto& symbol : *symbols) {
    if (!symbol.module_name().has_value() || module_name != symbol.module_name().value()) {
      continue;
    }
    if (!symbol.name().has_value() || symbol_name != symbol.name().value()) {
      continue;
    }
    if (!symbol.address().has_value()) {
      continue;
    }
    T value;
    memcpy(&value, &symbol.address().value(), sizeof(zx_vaddr_t));
    return zx::ok(value);
  }
  return zx::error(ZX_ERR_NOT_FOUND);
}

template <typename T>
T GetSymbol(const std::optional<std::vector<fuchsia_driver_framework::NodeSymbol>>& symbols,
            std::string_view module_name, std::string_view symbol_name, T default_value = nullptr) {
  auto value = SymbolValue<T>(symbols, module_name, symbol_name);
  return value.is_ok() ? *value : default_value;
}

inline std::unordered_set<std::string> GetModules(
    const std::optional<std::vector<fuchsia_driver_framework::NodeSymbol>>& symbols) {
  if (!symbols.has_value()) {
    return {};
  }
  std::unordered_set<std::string> modules;
  for (const auto& symbol : *symbols) {
    if (symbol.module_name().has_value()) {
      modules.insert(symbol.module_name().value());
    }
  }
  return modules;
}

}  // namespace fdf_internal

#endif  // LIB_DRIVER_COMPONENT_CPP_INTERNAL_SYMBOLS_H_

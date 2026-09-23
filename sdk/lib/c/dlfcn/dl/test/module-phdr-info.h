// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_DLFCN_DL_TEST_MODULE_PHDR_INFO_H_
#define LIB_C_DLFCN_DL_TEST_MODULE_PHDR_INFO_H_

#include <lib/elfldltl/layout.h>

#include <algorithm>
#include <filesystem>
#include <format>
#include <ostream>
#include <vector>

#include "../../dl_phdr_info.h"

namespace dl::testing {

class ModulePhdrInfo {
 public:
  using Elf = elfldltl::Elf<>;
  using Phdr = Elf::Phdr;

  constexpr ModulePhdrInfo() = default;
  constexpr ModulePhdrInfo(const ModulePhdrInfo&) = default;

  explicit ModulePhdrInfo(const dl_phdr_info& info)
      : name_(info.dlpi_name),
        addr_(info.dlpi_addr),
        phdr_(reinterpret_cast<const Phdr*>(info.dlpi_phdr)),
        phnum_(info.dlpi_phnum),
        tls_modid_(info.dlpi_tls_modid),
        tls_data_(info.dlpi_tls_data) {}

  constexpr auto operator<=>(const ModulePhdrInfo&) const = default;

  constexpr std::string_view name() const { return name_; }

  constexpr uintptr_t addr() const { return addr_; }

  constexpr std::span<const Phdr> phdrs() const { return std::span{phdr_, phnum_}; }

  constexpr uint16_t phnum() const { return phnum_; }

  constexpr size_t tls_modid() const { return tls_modid_; }

  constexpr void* tls_data() const { return tls_data_; }

  std::string basename() const {
    auto path = std::filesystem::path(name_);
    return path.filename();
  }

  // Check whether the given `ptr` is within the address range of any one of
  // this module's PT_LOAD segments.  This is used to test whether this module
  // is responsible for a given symbol pointer.
  constexpr bool contains_addr(uintptr_t ptr) const {
    return std::ranges::any_of(phdrs(), [ptr, this](const Phdr& phdr) {
      return phdr.type == elfldltl::ElfPhdrType::kLoad &&  //
             ptr >= addr_ + phdr.vaddr &&                  //
             ptr - addr_ - phdr.vaddr < phdr.memsz;
    });
  }

 private:
  std::string_view name_;
  uintptr_t addr_;
  const Phdr* phdr_;
  uint16_t phnum_;
  size_t tls_modid_;
  void* tls_data_;
};

inline std::ostream& operator<<(std::ostream& os, const ModulePhdrInfo& info) {
  return os << std::format(
             "{{dlpi_name=\"{}\", dlpi_addr={:x}, dlpi_phdr={}, dlpi_phnum={}, dlpi_tls_modid={}, dlpi_tls_data={}}}",
             info.name(), info.addr(), static_cast<const void*>(info.phdrs().data()),
             info.phdrs().size(), info.tls_modid(), info.tls_data());
}

using ModuleInfoList = std::vector<ModulePhdrInfo>;

// Data collected from the system dl_iterate_phdr at startup.
extern const ModuleInfoList gStartupPhdrInfo;

// A `dl_iterate_phdr` callback that collects the dl_iterate_phdr information
// of all loaded modules. This will push an instance of the `DlPhdrInfo` to the
// `ModuleInfoList*` that is passed in via the `data` pointer.
inline int CollectModulePhdrInfo(dl_phdr_info* phdr_info, size_t size, void* data) {
  static_cast<ModuleInfoList*>(data)->emplace_back(*phdr_info);
  return 0;
}

}  // namespace dl::testing

#endif  // LIB_C_DLFCN_DL_TEST_MODULE_PHDR_INFO_H_

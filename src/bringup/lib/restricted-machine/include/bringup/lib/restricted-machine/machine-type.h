// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_BRINGUP_LIB_RESTRICTED_MACHINE_INCLUDE_BRINGUP_LIB_RESTRICTED_MACHINE_MACHINE_TYPE_H_
#define SRC_BRINGUP_LIB_RESTRICTED_MACHINE_INCLUDE_BRINGUP_LIB_RESTRICTED_MACHINE_MACHINE_TYPE_H_

#include <lib/elfldltl/constants.h>
#include <zircon/types.h>

#include <array>
#include <string_view>

namespace restricted_machine {
// MachineType provides the values possible for configuring an Environment for
// the hosting hardware. It largely maps to elfldltl::ElfMachine except for
// kRiscv where the ElfClass is used to differentiate between 32-bit and 64-bit
// execution modes.
class MachineType {
 public:
  // An enumeration of supported machine architectures.
  enum Types : uint32_t {
    kNone,
    k386,
    kArm,
    kX86_64,
    kAarch64,
    kRiscv,
    kRiscv64,

    // The architecture of the compilation target.
    kNative =
        []() {
#ifdef __aarch64__
          return kAarch64;
#elif defined(__arm__)
          return kArm;
#elif defined(__i386__)
          return k386;
#elif defined(__x86_64__)
          return kX86_64;
#elif defined(__riscv)
          if constexpr (sizeof(uintptr_t) == sizeof(uint64_t)) {
            return kRiscv64;
          } else if constexpr (sizeof(uintptr_t) == sizeof(uint32_t)) {
            return kRiscv;
          }
#endif
          return kNone;
        }()

  };
  constexpr MachineType() : t(kNone) {}
  constexpr MachineType(Types t) : t(t) {}
  constexpr operator Types() const { return t; }
  constexpr Types machine_type() const { return t; }
  explicit operator bool() const = delete;

  // Returns a string representation of the machine type.
  constexpr std::string_view AsString() const {
    switch (t) {
      case kNone:
        return "none";
      case k386:
        return "x86";
      case kArm:
        return "arm";
      case kX86_64:
        return "x64";
      case kAarch64:
        return "arm64";
      case kRiscv:
        return "riscv";
      case kRiscv64:
        return "riscv64";
    }
  }

  // Returns the corresponding elfldltl::ElfMachine value.
  constexpr elfldltl::ElfMachine AsElfMachine() const {
    switch (t) {
      case kNone:
        return elfldltl::ElfMachine::kNone;
      case k386:
        return elfldltl::ElfMachine::k386;
      case kArm:
        return elfldltl::ElfMachine::kArm;
      case kX86_64:
        return elfldltl::ElfMachine::kX86_64;
      case kAarch64:
        return elfldltl::ElfMachine::kAarch64;
      case kRiscv:
        return elfldltl::ElfMachine::kRiscv;
      case kRiscv64:
        return elfldltl::ElfMachine::kRiscv;
    }
  }

 private:
  Types t;
};

// An array of machine types that may be supported by the host.
//
// kNative will always map to the appropriate primary architecture. If a
// sub-architecture is supported, it will be included as well.
#if defined(__ARM_ACLE)
constexpr static std::array<MachineType, 2> kSupportedMachines{
    MachineType::kNative,
    MachineType::kArm,
};
#else
constexpr static std::array<MachineType, 1> kSupportedMachines{
    MachineType::kNative,
};
#endif

}  // namespace restricted_machine

namespace std {
template <>
struct hash<restricted_machine::MachineType> {
  std::size_t operator()(const restricted_machine::MachineType& mt) const {
    return std::hash<uint32_t>{}(mt.machine_type());
  }
};
}  // namespace std

#endif  // SRC_BRINGUP_LIB_RESTRICTED_MACHINE_INCLUDE_BRINGUP_LIB_RESTRICTED_MACHINE_MACHINE_TYPE_H_

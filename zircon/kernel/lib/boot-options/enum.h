// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_BOOT_OPTIONS_ENUM_H_
#define ZIRCON_KERNEL_LIB_BOOT_OPTIONS_ENUM_H_

#include <lib/boot-options/types.h>
#include <stdio.h>
#include <zircon/assert.h>

#include <optional>
#include <string_view>

// This is specialized for each Enum type with a generic lambda that calls
// either EnumParser::Case or EnumPrinter::Case once for each enum value.
template <typename T>
constexpr bool Enum = false;  // Will cause errors since it's not callable.

#if BOOT_OPTIONS_TESTONLY_OPTIONS
template <>
constexpr auto Enum<TestEnum> = [](auto&& Switch) {
  return Switch  //
      .Case("default", TestEnum::kDefault)
      .Case("value1", TestEnum::kValue1)
      .Case("value2", TestEnum::kValue2);
};
#endif

template <>
inline constexpr auto Enum<OomBehavior> = [](auto&& Switch) {
  return Switch  //
      .Case("reboot", OomBehavior::kReboot)
      .Case("jobkill", OomBehavior::kJobKill);
};

template <>
inline constexpr auto Enum<PageTableEvictionPolicy> = [](auto&& Switch) {
  return Switch  //
      .Case("always", PageTableEvictionPolicy::kAlways)
      .Case("never", PageTableEvictionPolicy::kNever)
      .Case("on_request", PageTableEvictionPolicy::kOnRequest);
};

template <>
inline constexpr auto Enum<EntropyTestSource> = [](auto&& Switch) {
  return Switch  //
      .Case("hw_rng", EntropyTestSource::kHwRng)
      .Case("jitterentropy", EntropyTestSource::kJitterEntropy);
};

template <>
inline constexpr auto Enum<SerialDebugSyscalls> = [](auto&& Switch) {
  return Switch  //
      .Case("false", SerialDebugSyscalls::kDisabled)
      .Case("true", SerialDebugSyscalls::kEnabled)
      .Case("output-only", SerialDebugSyscalls::kOutputOnly);
};

template <>
inline constexpr auto Enum<RootJobBehavior> = [](auto&& Switch) {
  return Switch  //
      .Case("halt", RootJobBehavior::kHalt)
      .Case("reboot", RootJobBehavior::kReboot)
      .Case("bootloader", RootJobBehavior::kBootloader)
      .Case("recovery", RootJobBehavior::kRecovery)
      .Case("shutdown", RootJobBehavior::kShutdown);
};

template <>
inline constexpr auto Enum<WallclockType> = [](auto&& Switch) {
  return Switch  //
      .Case("auto", WallclockType::kAutoDetect)
      .Case("tsc", WallclockType::kTsc)
      .Case("pit", WallclockType::kPit)
      .Case("hpet", WallclockType::kHpet);
};

template <>
inline constexpr auto Enum<ScannerLruAction> = [](auto&& Switch) {
  return Switch  //
      .Case("none", ScannerLruAction::kNone)
      .Case("evict_only", ScannerLruAction::kEvictOnly)
      .Case("compress_only", ScannerLruAction::kCompressOnly)
      .Case("evict_and_compress", ScannerLruAction::kEvictAndCompress);
};

template <>
inline constexpr auto Enum<CompressionStrategy> = [](auto&& Switch) {
  return Switch  //
      .Case("none", CompressionStrategy::kNone)
      .Case("lz4", CompressionStrategy::kLz4);
};

template <>
inline constexpr auto Enum<CompressionStorageStrategy> = [](auto&& Switch) {
  return Switch  //
      .Case("none", CompressionStorageStrategy::kNone)
      .Case("slot", CompressionStorageStrategy::kSlot);
};

template <>
inline constexpr auto Enum<CheckFailAction> = [](auto&& Switch) {
  return Switch  //
      .Case("oops", CheckFailAction::kOops)
      .Case("panic", CheckFailAction::kPanic);
};

template <>
inline constexpr auto Enum<CheckerEnable> = [](auto&& Switch) {
  return Switch  //
      .Case("true", CheckerEnable::kTrue)
      .Case("false", CheckerEnable::kFalse)
      .Case("auto", CheckerEnable::kAuto);
};

#if BOOT_OPTIONS_GENERATOR || defined(__aarch64__)

template <>
inline constexpr auto Enum<Arm64PhysPsciReset> = [](auto&& Switch) {
  return Switch  //
      .Case("disabled", Arm64PhysPsciReset::kDisabled)
      .Case("shutdown", Arm64PhysPsciReset::kShutdown)
      .Case("reboot", Arm64PhysPsciReset::kReboot)
      .Case("reboot-bootloader", Arm64PhysPsciReset::kRebootBootloader)
      .Case("reboot-recovery", Arm64PhysPsciReset::kRebootRecovery);
};

template <>
inline constexpr auto Enum<Arm64AlternateVbar> = [](auto&& Switch) {
  return Switch  //
      .Case("none", Arm64AlternateVbar::kNone)
      .Case("auto", Arm64AlternateVbar::kAuto)
      .Case("arch3", Arm64AlternateVbar::kArchWorkaround3)
      .Case("arch1", Arm64AlternateVbar::kArchWorkaround1)
      .Case("psci", Arm64AlternateVbar::kPsciVersion)
      .Case("smccc10", Arm64AlternateVbar::kSmccc10);
};

#endif  // BOOT_OPTIONS_GENERATOR || defined(__x86_64__)

#if BOOT_OPTIONS_GENERATOR || defined(__x86_64__)

template <>
inline constexpr auto Enum<IntelHwpPolicy> = [](auto&& Switch) {
  return Switch  //
      .Case("bios-specified", IntelHwpPolicy::kBiosSpecified)
      .Case("performance", IntelHwpPolicy::kPerformance)
      .Case("balanced", IntelHwpPolicy::kBalanced)
      .Case("power-save", IntelHwpPolicy::kPowerSave)
      .Case("stable-performance", IntelHwpPolicy::kStablePerformance);
};

#endif  // BOOT_OPTIONS_GENERATOR || defined(__x86_64__)

// Helpers for enum overrides.

template <typename F, typename T>
class EnumEnumerator {
 public:
  EnumEnumerator(F call, T value) : call_(std::move(call)), value_(std::move(value)) {}

  constexpr EnumEnumerator& Case(std::string_view name, T value) {
    call_(name);
    return *this;
  }

 private:
  F call_;
  T value_;
};

template <typename T>
class EnumParser {
 public:
  EnumParser(std::string_view name, T* result) : name_(name), result_(result) {}
  constexpr EnumParser(const EnumParser&) noexcept = delete;
  constexpr EnumParser(EnumParser&& other) noexcept {
    result_ = other.result_;
    name_ = other.name_;
    other.result_ = nullptr;
  }
  ~EnumParser() {
    ZX_DEBUG_ASSERT_MSG(result_ == nullptr, "Failed to call EnumParser::Check() value: %.*s",
                        static_cast<int>(name_.size()), name_.data());
  }

  bool Check() {
    T* actual = result_;
    result_ = nullptr;
    if (actual) {
      printf("WARN: Ignored unknown value '%.*s' for multiple-choice option %p\n",
             static_cast<int>(name_.size()), name_.data(), this);
      printf("WARN: Valid choices are:");
      Enum<T>(EnumEnumerator{[](std::string_view name) {
                               printf(" %.*s", static_cast<int>(name.size()), name.data());
                             },
                             *actual});
      printf("\n");
    }
    return actual == nullptr;
  }

  constexpr EnumParser&& Case(std::string_view name, T value) {
    if (result_ && name == name_) {
      *result_ = value;
      result_ = nullptr;
    }
    return std::move(*this);
  }

 private:
  std::string_view name_;
  T* result_ = nullptr;
};

template <typename T>
class EnumPrinter {
 public:
  EnumPrinter(T value, FILE* out) : value_(std::move(value)), out_(out) {}

  EnumPrinter& Case(std::string_view name, T value) {
    if (value_ && *value_ == value) {
      value_.reset();
      fprintf(out_, "%.*s", static_cast<int>(name.size()), name.data());
    }
    return *this;
  }

  ~EnumPrinter() {
    if (value_) {
      fprintf(out_, "<unknown.enum.value.%#lx>", static_cast<unsigned long int>(*value_));
    }
  }

 private:
  std::optional<T> value_;
  FILE* out_ = nullptr;
};

// Deduction guides.

template <typename F, typename T>
EnumEnumerator(F, T) -> EnumEnumerator<F, T>;

template <typename T>
EnumParser(std::string_view, T*) -> EnumParser<T>;

template <typename T>
EnumPrinter(const T&, FILE*) -> EnumPrinter<T>;

#endif  // ZIRCON_KERNEL_LIB_BOOT_OPTIONS_ENUM_H_

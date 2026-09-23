// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ZXDUMP_INCLUDE_LIB_ZXDUMP_TYPES_H_
#define SRC_LIB_ZXDUMP_INCLUDE_LIB_ZXDUMP_TYPES_H_

#include <lib/elfldltl/constants.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/syscalls/debug.h>
#include <zircon/syscalls/exception.h>
#include <zircon/syscalls/object.h>
#include <zircon/types.h>

#include <array>
#include <cstdint>
#include <iostream>
#include <optional>
#include <span>
#include <string_view>
#include <type_traits>

namespace zxdump {

using ByteView = std::span<const std::byte>;

// fit::result<zxdump::Error> is used as the return type of many operations.
// It carries a zx_status_t and a string describing what operation failed.
struct Error {
  constexpr auto operator<=>(const Error&) const = default;

  std::string_view status_string() const;

  std::string_view op_;
  zx_status_t status_{};
};

// This is a shorthand for the return type of the Dump function passed to
// various <lib/zxdump/dump.>h APIs.
template <typename Dump>
using DumpResult = std::decay_t<decltype(std::declval<Dump>()(size_t{}, ByteView{}))>;

// fit::result<zxdump::DumpError<Dump>> is used as the return type of
// operations that take a Dump function.  Usually either Error::status_ will be
// ZX_OK and dump_error_ will be set, or dump_error_ will be std::nullopt and
// Error::status_ will not be ZX_OK.  For dump errors, Error::op_ will be the
// name of the <lib/zxdump/dump.h> method rather than the Zircon operation.
template <typename Dump>
struct DumpError : public Error {
  constexpr DumpError() = default;
  constexpr DumpError(const DumpError&) = default;
  constexpr DumpError(DumpError&&) noexcept = default;

  explicit constexpr DumpError(const Error& other) : Error{other} {}

  constexpr DumpError& operator=(const DumpError&) = default;
  constexpr DumpError& operator=(DumpError&&) noexcept = default;

  std::optional<std::decay_t<decltype(std::declval<DumpResult<Dump>>().error_value())>> dump_error_;
};

// This is the dump_error_ type used by fd-based Writer classes.
struct FdError {
  std::string_view error_string() const;

  // This describes the failing operation.  When the operation was I/O on a
  // file descriptor, this is usually the name of the POSIX function called.
  std::string_view op_;

  // This is a nonzero errno value when the failing operating was something
  // that set errno, such as a POSIX file descriptor I/O function.  It's zero
  // when the failure didn't have an associated errno code.
  int error_ = 0;
};

// This maps a `get_info` topic to its value type.  Though the single syscall
// interface always supports variable-sized results, some topics always return
// a single value and others can return a variable number.  Here a topic that
// is expected to return a variable size is represented as an unbounded array
// type.  The actual return value will be a span.
template <zx_object_info_topic_t Topic>
struct InfoTraits;

// zxdump::InfoTraitsType<ZX_INFO_*> is either a singleton zx_info_*_t type
// or a std::span<...> type for topics that return vectors.
template <zx_object_info_topic_t Topic>
using InfoTraitsType = typename InfoTraits<Topic>::type;

template <>
struct InfoTraits<ZX_INFO_HANDLE_BASIC> {
  static constexpr std::string_view kName = "ZX_INFO_HANDLE_BASIC";
  using type = zx_info_handle_basic_t;
};

template <>
struct InfoTraits<ZX_INFO_PROCESS> {
  static constexpr std::string_view kName = "ZX_INFO_PROCESS";
  using type = zx_info_process_t;
};

template <>
struct InfoTraits<ZX_INFO_PROCESS_THREADS> {
  static constexpr std::string_view kName = "ZX_INFO_PROCESS_THREADS";
  using type = std::span<const zx_koid_t>;
};

template <>
struct InfoTraits<ZX_INFO_VMAR> {
  static constexpr std::string_view kName = "ZX_INFO_VMAR";
  using type = zx_info_vmar_t;
};

template <>
struct InfoTraits<ZX_INFO_JOB_CHILDREN> {
  static constexpr std::string_view kName = "ZX_INFO_JOB_CHILDREN";
  using type = std::span<const zx_koid_t>;
};

template <>
struct InfoTraits<ZX_INFO_JOB_PROCESSES> {
  static constexpr std::string_view kName = "ZX_INFO_JOB_PROCESSES";
  using type = std::span<const zx_koid_t>;
};

template <>
struct InfoTraits<ZX_INFO_THREAD> {
  static constexpr std::string_view kName = "ZX_INFO_THREAD";
  using type = zx_info_thread_t;
};

template <>
struct InfoTraits<ZX_INFO_THREAD_EXCEPTION_REPORT> {
  static constexpr std::string_view kName = "ZX_INFO_THREAD_EXCEPTION_REPORT";
  using type = zx_exception_report_t;
};

template <>
struct InfoTraits<ZX_INFO_TASK_STATS> {
  static constexpr std::string_view kName = "ZX_INFO_TASK_STATS";
  using type = zx_info_task_stats_t;
};

template <>
struct InfoTraits<ZX_INFO_PROCESS_MAPS> {
  static constexpr std::string_view kName = "ZX_INFO_PROCESS_MAPS";
  using type = std::span<const zx_info_maps_t>;
};

template <>
struct InfoTraits<ZX_INFO_PROCESS_VMOS_V1> {
  static constexpr std::string_view kName = "ZX_INFO_PROCESS_VMOS_V1";
  using type = std::span<const zx_info_vmo_t>;
};

template <>
struct InfoTraits<ZX_INFO_PROCESS_VMOS> {
  static constexpr std::string_view kName = "ZX_INFO_PROCESS_VMOS";
  using type = std::span<const zx_info_vmo_t>;
};

template <>
struct InfoTraits<ZX_INFO_THREAD_STATS> {
  static constexpr std::string_view kName = "ZX_INFO_THREAD_STATS";
  using type = zx_info_thread_stats_t;
};

template <>
struct InfoTraits<ZX_INFO_CPU_STATS> {
  static constexpr std::string_view kName = "ZX_INFO_CPU_STATS";
  using type = std::span<const zx_info_cpu_stats_t>;
};

template <>
struct InfoTraits<ZX_INFO_KMEM_STATS> {
  static constexpr std::string_view kName = "ZX_INFO_KMEM_STATS";
  using type = zx_info_kmem_stats_t;
};

template <>
struct InfoTraits<ZX_INFO_RESOURCE> {
  static constexpr std::string_view kName = "ZX_INFO_RESOURCE";
  using type = zx_info_resource_t;
};

template <>
struct InfoTraits<ZX_INFO_HANDLE_COUNT> {
  static constexpr std::string_view kName = "ZX_INFO_HANDLE_COUNT";
  using type = zx_info_handle_count_t;
};

template <>
struct InfoTraits<ZX_INFO_BTI> {
  static constexpr std::string_view kName = "ZX_INFO_BTI";
  using type = zx_info_bti_t;
};

template <>
struct InfoTraits<ZX_INFO_PROCESS_HANDLE_STATS> {
  static constexpr std::string_view kName = "ZX_INFO_PROCESS_HANDLE_STATS";
  using type = zx_info_process_handle_stats_t;
};

template <>
struct InfoTraits<ZX_INFO_SOCKET> {
  static constexpr std::string_view kName = "ZX_INFO_SOCKET";
  using type = zx_info_socket_t;
};

template <>
struct InfoTraits<ZX_INFO_VMO_V1> {
  static constexpr std::string_view kName = "ZX_INFO_VMO_V1";
  using type = zx_info_vmo_t;
};

template <>
struct InfoTraits<ZX_INFO_VMO> {
  static constexpr std::string_view kName = "ZX_INFO_VMO";
  using type = zx_info_vmo_t;
};

template <>
struct InfoTraits<ZX_INFO_JOB> {
  static constexpr std::string_view kName = "ZX_INFO_JOB";
  using type = zx_info_job_t;
};

template <>
struct InfoTraits<ZX_INFO_TIMER> {
  static constexpr std::string_view kName = "ZX_INFO_TIMER";
  using type = zx_info_timer_t;
};

template <>
struct InfoTraits<ZX_INFO_STREAM> {
  static constexpr std::string_view kName = "ZX_INFO_STREAM";
  using type = zx_info_stream_t;
};

template <>
struct InfoTraits<ZX_INFO_HANDLE_TABLE> {
  static constexpr std::string_view kName = "ZX_INFO_HANDLE_TABLE";
  using type = std::span<const zx_info_handle_extended_t>;
};

template <>
struct InfoTraits<ZX_INFO_MSI> {
  static constexpr std::string_view kName = "ZX_INFO_MSI";
  using type = zx_info_msi_t;
};

template <>
struct InfoTraits<ZX_INFO_GUEST_STATS> {
  static constexpr std::string_view kName = "ZX_INFO_GUEST_STATS";
  using type = zx_info_guest_stats_t;
};

template <>
struct InfoTraits<ZX_INFO_TASK_RUNTIME> {
  static constexpr std::string_view kName = "ZX_INFO_TASK_RUNTIME";
  using type = zx_info_task_runtime_t;
};

// Similar for `get_property` properties, but these are always fixed-size.
// The generic template is defined and thus invalid properties are allowed at
// compile time, just because almost every property uses the same type.
template <uint32_t Property>
struct PropertyTraits {
  using type = uintptr_t;
};

template <uint32_t Property>
using PropertyTraitsType = typename PropertyTraits<Property>::type;

// The actual type of this property is `char[ZX_MAX_NAME_LEN]`.
// Every other property uses a normal value type, not an array.
// Since an array can't be returned by value, it's returned as std::array.
template <>
struct PropertyTraits<ZX_PROP_NAME> {
  static constexpr std::string_view kName = "ZX_PROP_NAME";
  using type = std::array<char, ZX_MAX_NAME_LEN>;
};

// Only InfoTraitsType<...> can be a span, not PropertyTraitsType<...>.

template <typename T>
inline constexpr bool kIsSpan = false;

template <typename T>
inline constexpr bool kIsSpan<std::span<T>> = true;

template <typename T>
struct RemoveSpan;

template <typename T>
struct RemoveSpan<std::span<T>> {
  using type = T;
};

template <class StateType>
struct ThreadStateTraits {
  static_assert(!std::is_same_v<StateType, StateType>,
                "invalid type specialization for zxdump::Thread::read_state<StateType>");
};

template <>
struct ThreadStateTraits<zx_x86_64_thread_state_general_regs_t> {
  static constexpr zx_thread_state_topic_t kTopic = ZX_THREAD_STATE_GENERAL_REGS;
  static constexpr elfldltl::ElfMachine kMachine = elfldltl::ElfMachine::kX86_64;
};

template <>
struct ThreadStateTraits<zx_x86_64_thread_state_fp_regs_t> {
  static constexpr zx_thread_state_topic_t kTopic = ZX_THREAD_STATE_FP_REGS;
  static constexpr elfldltl::ElfMachine kMachine = elfldltl::ElfMachine::kX86_64;
};

template <>
struct ThreadStateTraits<zx_x86_64_thread_state_vector_regs_t> {
  static constexpr zx_thread_state_topic_t kTopic = ZX_THREAD_STATE_VECTOR_REGS;
  static constexpr elfldltl::ElfMachine kMachine = elfldltl::ElfMachine::kX86_64;
};

template <>
struct ThreadStateTraits<zx_x86_64_thread_state_debug_regs_t> {
  static constexpr zx_thread_state_topic_t kTopic = ZX_THREAD_STATE_DEBUG_REGS;
  static constexpr elfldltl::ElfMachine kMachine = elfldltl::ElfMachine::kX86_64;
};

template <>
struct ThreadStateTraits<zx_arm64_thread_state_general_regs_t> {
  static constexpr zx_thread_state_topic_t kTopic = ZX_THREAD_STATE_GENERAL_REGS;
  static constexpr elfldltl::ElfMachine kMachine = elfldltl::ElfMachine::kAarch64;
};

template <>
struct ThreadStateTraits<zx_arm64_thread_state_fp_regs_t> {
  static constexpr zx_thread_state_topic_t kTopic = ZX_THREAD_STATE_FP_REGS;
  static constexpr elfldltl::ElfMachine kMachine = elfldltl::ElfMachine::kAarch64;
};

template <>
struct ThreadStateTraits<zx_arm64_thread_state_vector_regs_t> {
  static constexpr zx_thread_state_topic_t kTopic = ZX_THREAD_STATE_VECTOR_REGS;
  static constexpr elfldltl::ElfMachine kMachine = elfldltl::ElfMachine::kAarch64;
};

template <>
struct ThreadStateTraits<zx_arm64_thread_state_debug_regs_t> {
  static constexpr zx_thread_state_topic_t kTopic = ZX_THREAD_STATE_DEBUG_REGS;
  static constexpr elfldltl::ElfMachine kMachine = elfldltl::ElfMachine::kAarch64;
};

template <>
struct ThreadStateTraits<zx_riscv64_thread_state_general_regs_t> {
  static constexpr zx_thread_state_topic_t kTopic = ZX_THREAD_STATE_GENERAL_REGS;
  static constexpr elfldltl::ElfMachine kMachine = elfldltl::ElfMachine::kRiscv;
};

template <>
struct ThreadStateTraits<zx_riscv64_thread_state_fp_regs_t> {
  static constexpr zx_thread_state_topic_t kTopic = ZX_THREAD_STATE_FP_REGS;
  static constexpr elfldltl::ElfMachine kMachine = elfldltl::ElfMachine::kRiscv;
};

template <>
struct ThreadStateTraits<zx_riscv64_thread_state_vector_regs_t> {
  static constexpr zx_thread_state_topic_t kTopic = ZX_THREAD_STATE_VECTOR_REGS;
  static constexpr elfldltl::ElfMachine kMachine = elfldltl::ElfMachine::kRiscv;
};

template <>
struct ThreadStateTraits<zx_riscv64_thread_state_debug_regs_t> {
  static constexpr zx_thread_state_topic_t kTopic = ZX_THREAD_STATE_DEBUG_REGS;
  static constexpr elfldltl::ElfMachine kMachine = elfldltl::ElfMachine::kRiscv;
};

// This prints "op: status" with the status string.
std::ostream& operator<<(std::ostream& os, const Error& error);

// This does either that or `os << "op: " << error.dump_error_`.  If a
// particular error_value type of a Dump function is too generic for
// std::ostream::operator<< to be specialized as desired for it, then
// this can be explicitly specialized for the Dump type.
template <typename Dump>
inline std::ostream& operator<<(std::ostream& os, const DumpError<Dump>& error) {
  using namespace std::literals;
  if (error.status_ == ZX_OK) {
    ZX_DEBUG_ASSERT(error.dump_error_.has_value());
    return os << error.op_ << ": "sv << error.dump_error_.value();
  }
  return os << static_cast<const Error&>(error);
}

// This prints "op: Error text" (from strerror) or just "op".
std::ostream& operator<<(std::ostream& os, const FdError& fd_error);

}  // namespace zxdump

#endif  // SRC_LIB_ZXDUMP_INCLUDE_LIB_ZXDUMP_TYPES_H_

// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed
// by a BSD-style license that can be found in the LICENSE file.

#ifndef ZIRCON_KERNEL_LIB_FASTTIME_INCLUDE_LIB_FASTTIME_INTERNAL_TIME_H_
#define ZIRCON_KERNEL_LIB_FASTTIME_INCLUDE_LIB_FASTTIME_INTERNAL_TIME_H_

#include <lib/affine/ratio.h>
#include <lib/arch/intrin.h>
#include <lib/fasttime/internal/abi.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <atomic>
#include <bit>
#include <concepts>
#include <type_traits>

namespace fasttime::internal {

struct SequentialReadResult {
  zx_ticks_t raw_ticks;
  zx_ticks_t obs1;
  zx_ticks_t obs2;
};

#if defined(__ARM_ACLE)

#ifdef __aarch64__
inline uint64_t get_raw_ticks_arm_vct() { return __arm_rsr64("cntvct_el0"); }
inline uint64_t get_raw_ticks_arm_pct() { return __arm_rsr64("cntpct_el0"); }
#else
inline uint64_t get_raw_ticks_arm_vct() { return __arm_mrrc(15, 1, 14); }
inline uint64_t get_raw_ticks_arm_pct() { return __arm_mrrc(15, 0, 14); }
#endif

template <uint64_t (*Get)()>
inline uint64_t get_raw_ticks_arm_a73() {
  uint64_t ticks1 = Get();
  uint64_t ticks2 = Get();
  return (((ticks1 ^ ticks2) >> 32) & 1) ? ticks1 : ticks2;
}

inline zx_ticks_t get_raw_ticks(const TimeValues& tvalues) {
  return std::bit_cast<zx_ticks_t>(
      tvalues.use_a73_errata_mitigation
          ? (tvalues.use_pct_instead_of_vct ? get_raw_ticks_arm_a73<get_raw_ticks_arm_pct>()
                                            : get_raw_ticks_arm_a73<get_raw_ticks_arm_vct>())
          : (tvalues.use_pct_instead_of_vct ? get_raw_ticks_arm_pct() : get_raw_ticks_arm_vct()));
}

// This always returns zero in a register at runtime.  But both the compiler
// and the CPU consider that register's value to be data-dependent on the
// argument register.  So whatever load or system register read produced the
// argument value will be a data dependency of whatever uses the returned zero.
template <std::integral T>
inline uintptr_t DataDependentZero(T dep) {
  using UnsignedT = std::make_unsigned_t<T>;
  uintptr_t zero;
  // This doesn't use volatile because its whole purpose is to precisely
  // express the data dependency between its input operand and its output
  // operand, both to the compiler and to the CPU.  Using asm volatile says
  // that the asm does something whose pruning or reordering is prohibited for
  // some reason _other_ than data dependencies or side effects the compiler
  // understands.  Here we'd be failing entirely to do what we need to do if
  // the compiler doesn't understand everything that matters and will only move
  // this around in ways that preserve the data-dependency-based ordering, so
  // we wouldn't want to influence the compiler except by expressing in the
  // operands everything it needs to know.
  __asm__("eor %0, %1, %1"
          : "=r"(zero)
          : "r"(static_cast<uintptr_t>(std::bit_cast<UnsignedT>(dep))));
  return zero;
}

// This just returns the reference it's given in the first argument, but such
// that the address is computed as data-dependent on the second argument value.
// So whatever load or system register read produced that second argument will
// be a data dependency of any memory accesses done via the returned reference.
template <typename T>
inline T& DataDependentRef(T& ref, std::integral auto dep) {
  return *reinterpret_cast<T*>(reinterpret_cast<uintptr_t>(&ref) + DataDependentZero(dep));
}

// This is just a plain `ldr` instruction, in fact.  It's wrapped here in
// preparation for AArch32 where the compiler would use an `ldrexd` instruction
// instead of the `ldrd` instruction, which is already guaranteed sufficiently
// atomic in ARMv8 for an aligned 64-bit value.
inline zx_ticks_t AtomicLoadRelaxed(const std::atomic<zx_ticks_t>& value_ref) {
#ifdef __aarch64__
  return value_ref.load(std::memory_order_relaxed);
#else
#ifdef __thumb__
#define LDRD_CONSTRAINT "Q"
#else
#define LDRD_CONSTRAINT "m"
#endif
  uint64_t value;
  __asm__("ldrd %[value], %[value_ref]"
          : [value] "=r"(value)
          : [value_ref] LDRD_CONSTRAINT(value_ref));
  return value;
#endif
}

// Reads the raw ticks value in between two observations of the given atomic value.
inline SequentialReadResult read_raw_ticks_sequentially(const TimeValues& tvalues,
                                                        const std::atomic<zx_ticks_t>& value) {
  SequentialReadResult result = {0, ZX_TIME_INFINITE, ZX_TIME_INFINITE_PAST};
  // This assembly block reads the offset and ensures that this read must complete before the read
  // of the virtual ticks counter below. The method is derived from Example D12-3 in the 'Arm
  // Architecture Reference Manual for A-profile architecture' revision 'ARM DDI 0487K.a'. It works
  // by establishing a data dependency between the offset and the counter read to ensure that the
  // processor cannot reorder these instructions.

  // First, load the value into result.obs1.
  result.obs1 = AtomicLoadRelaxed(value);

  // Now produce a data-dependent zero and conditionally branch on it. We know
  // that this is an unconditional branch because the input register will
  // always be zero, but the CPU does not know that and therefore cannot
  // reorder the load to be after the test and branch.
  if (DataDependentZero(result.obs1) != 0) {
    // The branch target is just the next instruction as there aren't really
    // two paths here.  The empty asm forces the compiler to produce the
    // comparison and conditional branch anyway.
    __asm__ volatile("");
  }

  // This ISB prevents the test and branch from being reordered down.
  __isb(ARM_MB_SY);

  // This ticks must not be reordered with the steps before or after it. We
  // achieve this by using the intrinsics in get_raw_ticks that have `volatile`
  // semantics. This should ensure that things are not reordered. Furthermore,
  // the read below establishes a data dependency on result.raw_ticks.
  result.raw_ticks = get_raw_ticks(tvalues);

  // Read the offset a second time, ensuring that this read cannot begin until
  // the previous ticks counter read(s) complete. The method used comes from
  // Example D12-4 in the 'Arm Architecture Reference Manual for A-profile
  // architecture' revision 'ARM DDI 0487K.a'.
  //
  // A data-dependent zero is generated as above, but instead of using it to
  // control a branch, we use it to control an address computation so that the
  // load is transitively data-dependent on the counter read.
  result.obs2 = AtomicLoadRelaxed(DataDependentRef(value, result.raw_ticks));

  return result;
}

#elif defined(__x86_64__)

inline zx_ticks_t get_raw_ticks(const TimeValues& tvalues) { return __rdtsc(); }

// Reads the raw ticks value in between two observations of the given atomic value.
inline SequentialReadResult read_raw_ticks_sequentially(const TimeValues& tvalues,
                                                        const std::atomic<zx_ticks_t>& value) {
  SequentialReadResult result = {0, ZX_TIME_INFINITE, ZX_TIME_INFINITE_PAST};
  result.obs1 = value.load(std::memory_order_relaxed);

  // This sequential read utilizes a rdtscp instruction to ensure that all previous loads are
  // globally visible. It then emits an lfence immediately after to ensure that the rdtscp is
  // executed prior to the execution of any subsequent instruction. This method comes from the
  // "RDTSCP—Read Time-Stamp Counter and Processor ID" section of the Intel Software Developer's
  // Manual Volume 2B revision 4-557.
  unsigned int unused;
  result.raw_ticks = __rdtscp(&unused);

  // Use an lfence to ensure that the subsequent load does not start until the rdtscp is complete.
  _mm_lfence();

  result.obs2 = value.load(std::memory_order_relaxed);
  return result;
}

#elif defined(__riscv)

// Reads the raw ticks value in between two observations of the given atomic value.
inline SequentialReadResult read_raw_ticks_sequentially(const TimeValues& tvalues,
                                                        const std::atomic<zx_ticks_t>& value) {
  SequentialReadResult result = {0, ZX_TIME_INFINITE, ZX_TIME_INFINITE_PAST};
  result.obs1 = value.load(std::memory_order_relaxed);
  // TODO(https://fxbug.dev/383385811): Research is required in order to properly synchronize
  // observations of the RISC-V system timer against the instruction pipeline. Once we know the
  // methods needed to do so, apply them here.
  __asm__ volatile("rdtime %0" : "=r"(result.raw_ticks));
  result.obs2 = value.load(std::memory_order_relaxed);
  return result;
}

inline zx_ticks_t get_raw_ticks(const TimeValues& tvalues) {
  zx_ticks_t ticks;
  __asm__ volatile("rdtime %0" : "=r"(ticks));
  return ticks;
}

#else

#error Unsupported architecture

#endif

constexpr uint64_t kFasttimeVersion = 1;

enum class FasttimeVerificationMode : uint8_t {
  kNormal,
  kSkip,
};

inline bool check_fasttime_version(const TimeValues& tvalues) {
  return tvalues.version == kFasttimeVersion;
}

template <FasttimeVerificationMode kVerificationMode = FasttimeVerificationMode::kNormal>
inline zx_instant_mono_ticks_t compute_monotonic_ticks(const TimeValues& tvalues) {
  if constexpr (kVerificationMode == FasttimeVerificationMode::kNormal) {
    if (!tvalues.usermode_can_access_ticks || tvalues.version != kFasttimeVersion) {
      return ZX_TIME_INFINITE_PAST;
    }
  }
  while (true) {
    SequentialReadResult result = read_raw_ticks_sequentially(tvalues, tvalues.mono_ticks_offset);
    if (result.obs1 == result.obs2) {
      return result.raw_ticks + result.obs1;
    }
  }
}

template <FasttimeVerificationMode kVerificationMode = FasttimeVerificationMode::kNormal>
inline zx_instant_mono_t compute_monotonic_time(const TimeValues& tvalues) {
  const zx_instant_mono_ticks_t ticks = compute_monotonic_ticks<kVerificationMode>(tvalues);
  if constexpr (kVerificationMode == FasttimeVerificationMode::kNormal) {
    if (ticks == ZX_TIME_INFINITE_PAST) {
      return ticks;
    }
  }
  const affine::Ratio ticks_to_time_ratio(tvalues.ticks_to_time_numerator,
                                          tvalues.ticks_to_time_denominator);
  return ticks_to_time_ratio.Scale(ticks);
}

template <FasttimeVerificationMode kVerificationMode = FasttimeVerificationMode::kNormal>
inline zx_instant_boot_ticks_t compute_boot_ticks(const TimeValues& tvalues) {
  if constexpr (kVerificationMode == FasttimeVerificationMode::kNormal) {
    if (!tvalues.usermode_can_access_ticks || tvalues.version != kFasttimeVersion) {
      return ZX_TIME_INFINITE_PAST;
    }
  }
  return internal::get_raw_ticks(tvalues) + tvalues.boot_ticks_offset;
}

template <FasttimeVerificationMode kVerificationMode = FasttimeVerificationMode::kNormal>
inline zx_instant_boot_t compute_boot_time(const TimeValues& tvalues) {
  const zx_instant_boot_ticks_t ticks = compute_boot_ticks<kVerificationMode>(tvalues);
  if constexpr (kVerificationMode == FasttimeVerificationMode::kNormal) {
    if (ticks == ZX_TIME_INFINITE_PAST) {
      return ticks;
    }
  }
  // The scaling factor from raw ticks to boot ticks is the same as that from raw ticks to mono
  // ticks.
  const affine::Ratio ticks_to_time_ratio(tvalues.ticks_to_time_numerator,
                                          tvalues.ticks_to_time_denominator);
  return ticks_to_time_ratio.Scale(ticks);
}

}  // namespace fasttime::internal

#endif  // ZIRCON_KERNEL_LIB_FASTTIME_INCLUDE_LIB_FASTTIME_INTERNAL_TIME_H_

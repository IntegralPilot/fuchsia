// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "phys/fixed-address-boot-zbi.h"

#include <lib/memalloc/pool.h>
#include <zircon/assert.h>

#include <cstddef>
#include <cstdint>
#include <cstring>

#include <fbl/algorithm.h>
#include <ktl/byte.h>
#include <phys/address-space.h>
#include <phys/main.h>
#include <phys/stdio.h>

#include <ktl/enforce.h>

namespace {

#if defined(__x86_64__) || defined(__i386__)

// Relocated blob size must be aligned to |kRelocateAlign|.
constexpr size_t kRelocateAlign = 1;

// When a RelocatedTarget is copied forward, source and destination offsets
// must be adjusted by this.
constexpr int64_t kForwardBias = 0;

// When a RelocatedTarget is copied backwards, source and destination offsets
// must be adjusted by this.
constexpr int64_t kBackwardBias = -1;

#elif defined(__aarch64__)

// Relocated blob size must be aligned to |kRelocateAlign|.
constexpr size_t kRelocateAlign = 32;

// When a RelocatedTarget is copied forward, source and destination offsets
// must be adjusted by this.
constexpr int64_t kForwardBias = -16;

// When a RelocatedTarget is copied backwards, source and destination offsets
// must be adjusted by this.
constexpr int64_t kBackwardBias = 0;

#elif defined(__riscv)

// Relocated blob size must be aligned to |kRelocateAlign|.
constexpr size_t kRelocateAlign = 8;

// When a RelocatedTarget is copied forward, source and destination offsets
// must be adjusted by this.
constexpr int64_t kForwardBias = 0;

// When a RelocatedTarget is copied backwards, source and destination offsets
// must be adjusted by this.
constexpr int64_t kBackwardBias = 0;

#else

#error "What architecture?"

#endif

struct RelocateTarget {
  RelocateTarget() = default;

  RelocateTarget(uintptr_t destination, ktl::span<const ktl::byte> blob)
      : src(reinterpret_cast<uintptr_t>(blob.data())),
        dst(destination),
        count(fbl::round_up(blob.size(), kRelocateAlign)),
        backwards(dst > src && dst - src < count) {
    if (backwards) {
      dst += count + kBackwardBias;
      src += count + kBackwardBias;
    } else {
      dst += kForwardBias;
      src += kForwardBias;
    }
  }

  constexpr uint64_t destination() const {
    return backwards ? dst - count - kBackwardBias : dst - kForwardBias;
  }

  uint64_t src = 0;
  uint64_t dst = 0;
  uint64_t count = 0;

  // When the addresses overlap, the copying can be done backwards and so the
  // direction flag is set for REP MOVSB and the starting pointers are at the
  // last byte rather than the first. While this is a boolean flag, we can
  // use fewer ASM instruction in the inline assembly by increasinng its width.
  uint64_t backwards = 0;
};

#if __aarch64__

static_assert(offsetof(RelocateTarget, src) == offsetof(RelocateTarget, dst) - sizeof(uint64_t),
              "Must be contiguous for arm64 ldp instruction.");
static_assert(offsetof(RelocateTarget, count) ==
                  offsetof(RelocateTarget, backwards) - sizeof(uint64_t),
              "Must be contiguous for arm64 ldp instruction.");
#endif

bool RecharacterizeAllocations(uint64_t start, uint64_t size, memalloc::Type type) {
  return Allocation::GetPool().UpdateRamSubranges(type, start, size).is_ok();
}

bool RecharacterizeAllocations(ktl::span<const ktl::byte> range, memalloc::Type type) {
  return RecharacterizeAllocations(reinterpret_cast<uintptr_t>(range.data()), range.size(), type);
}

}  // namespace

void FixedAddressBootZbi::SetKernelAddresses() {
  kernel_entry_address_ = BootZbi::KernelEntryAddress();
}

fit::result<BootZbi::Error> FixedAddressBootZbi::Load(uint32_t extra_data_capacity,
                                                      ktl::optional<uint64_t> kernel_load_address,
                                                      ktl::optional<uint64_t> data_load_address) {
  if (kernel_load_address) {
    set_kernel_load_address(*kernel_load_address);
  }

  if (data_load_address) {
    data_load_address_ = data_load_address;
  }

  if (!kernel_load_address_) {
    // New-style position-independent kernel.
    return BootZbi::Load(extra_data_capacity);
  }

  // Now we know how much space the kernel image needs.
  // Reserve it at the fixed load address.
  if (!RecharacterizeAllocations(*kernel_load_address_, KernelMemorySize(),
                                 memalloc::Type::kKernel)) {
    return fit::error{BootZbi::Error{.zbi_error = "unable to reserve kernel's load image"sv}};
  }

  if (data_load_address_ &&
      !RecharacterizeAllocations(*data_load_address_, DataLoadSize() + extra_data_capacity,
                                 memalloc::Type::kDataZbi)) {
    return fit::error{BootZbi::Error{.zbi_error = "unable to reserve data ZBI's load image"sv}};
  }

  if (auto result = BootZbi::Load(extra_data_capacity, kernel_load_address_); result.is_error()) {
    return result.take_error();
  }

  // Recharacterize the staging kernel and data ZBI allocations as such. This
  // need to recharacterize the loaded images is trampoline-specific, so cleaner
  // to do that here on the outside of BootZbi.
  if (!RecharacterizeAllocations(KernelLoadAddress(), KernelMemorySize(),
                                 memalloc::Type::kFixedAddressStagingKernel)) {
    return fit::error{
        BootZbi::Error{.zbi_error = "unable to recharacterize staging trampoline kernel"sv}};
  }
  if (data_load_address_ && !RecharacterizeAllocations(
                                DataZbi().storage(), memalloc::Type::kFixedAddressStagingDataZbi)) {
    return fit::error{
        BootZbi::Error{.zbi_error = "unable to recharacterize staging trampoline data ZBI"sv}};
  }

  return fit::ok();
}

[[noreturn]] void FixedAddressBootZbi::Boot(ktl::optional<void*> argument) {
  ZX_ASSERT(!MustRelocateDataZbi());

  uintptr_t entry = static_cast<uintptr_t>(KernelEntryAddress());
  ZX_ASSERT(entry == KernelEntryAddress());

  uintptr_t zbi = static_cast<uintptr_t>(DataLoadAddress());
  ZX_ASSERT(zbi == DataLoadAddress());

  uintptr_t kernel_first = static_cast<uintptr_t>(KernelLoadAddress());
  uintptr_t kernel_last = static_cast<uintptr_t>(KernelLoadAddress() + KernelLoadSize() - 1);
  ZX_ASSERT(kernel_first == KernelLoadAddress());
  ZX_ASSERT(kernel_last == KernelLoadAddress() + KernelLoadSize() - 1);

  uintptr_t kernel_size = static_cast<uintptr_t>(KernelLoadSize());
  ZX_ASSERT(kernel_size == KernelLoadSize());

  if (kernel_load_address_) {
    uintptr_t fixed_first = static_cast<uintptr_t>(kernel_load_address_.value());
    uintptr_t fixed_last = static_cast<uintptr_t>(*kernel_load_address_ + KernelLoadSize() - 1);
    ZX_ASSERT_MSG(fixed_first == *kernel_load_address_, "0x%016" PRIx64 " != 0x%016" PRIx64 " ",
                  static_cast<uint64_t>(fixed_first), *kernel_load_address_);
    ZX_ASSERT(fixed_last == *kernel_load_address_ + KernelLoadSize() - 1);
  }

  if (!kernel_load_address_) {
    // This is a new-style position-independent kernel.  Boot it where it is.
    BootZbi::Boot(argument);
  }

  uintptr_t zbi_location =
      reinterpret_cast<uintptr_t>(argument.value_or(DataZbi().storage().data()));
  auto kernel_blob = ktl::span<const ktl::byte>(reinterpret_cast<const ktl::byte*>(KernelImage()),
                                                KernelLoadSize());
  auto zbi_blob = ktl::span<const ktl::byte>(reinterpret_cast<const ktl::byte*>(zbi_location),
                                             DataZbi().size_bytes());

  auto assert_no_overlap = [](const char* what1, ktl::span<const ktl::byte> buffer1,
                              const char* what2, ktl::span<const ktl::byte> buffer2) {
    uintptr_t start1 = reinterpret_cast<uintptr_t>(buffer1.data());
    uintptr_t end1 = start1 + buffer1.size();
    uintptr_t start2 = reinterpret_cast<uintptr_t>(buffer2.data());
    uintptr_t end2 = start2 + buffer2.size();
    ZX_ASSERT_MSG(end1 <= start2 || end2 <= start1,
                  "Overlap detected: %s @ [%#" PRIx64 ", %#" PRIx64 ") and %s @ [%#" PRIx64
                  ", %#" PRIx64 ")",
                  what1, static_cast<uint64_t>(start1), static_cast<uint64_t>(end1), what2,
                  static_cast<uint64_t>(start2), static_cast<uint64_t>(end2));
  };

  auto kernel_dst =
      ktl::span<ktl::byte>(reinterpret_cast<ktl::byte*>(*kernel_load_address_), KernelMemorySize());
  auto data_dst = ktl::span<ktl::byte>(
      reinterpret_cast<ktl::byte*>(data_load_address_.value_or(zbi_location)), zbi_blob.size());

  assert_no_overlap("kernel destination", kernel_dst, "kernel source", kernel_blob);

  if (data_load_address_) {
    assert_no_overlap("data destination", data_dst, "data source", zbi_blob);
    assert_no_overlap("data destination", data_dst, "kernel source", kernel_blob);
  }

  assert_no_overlap("kernel destination", kernel_dst, "data source", zbi_blob);
  assert_no_overlap("kernel destination", kernel_dst, "data destination", data_dst);

  // No overlaps, so memcpy suffices for placement.
  memcpy(kernel_dst.data(), kernel_blob.data(), kernel_blob.size());

  if (data_load_address_) {
    memcpy(data_dst.data(), zbi_blob.data(), zbi_blob.size());
  }

  SetKernel(reinterpret_cast<const ZbiKernelImage*>(kernel_dst.data()));
  ZbiBoot(KernelEntryAddress(), data_dst.data());
}

fit::result<FixedAddressBootZbi::Error> FixedAddressBootZbi::Init(InputZbi zbi) {
  auto res = BootZbi::Init(zbi);
  SetKernelAddresses();
  return res;
}

fit::result<FixedAddressBootZbi::Error> FixedAddressBootZbi::Init(InputZbi zbi,
                                                                  InputZbi::iterator kernel_item) {
  auto res = BootZbi::Init(zbi, kernel_item);
  SetKernelAddresses();
  return res;
}

void FixedAddressBootZbi::Log() {
  LogAddresses();
  if (kernel_load_address_) {
    LogFixedAddresses();
  }
  LogBoot(KernelEntryAddress());
}

// This output lines up with what BootZbi::LogAddresses() prints.
void FixedAddressBootZbi::LogFixedAddresses() const {
#define ADDR "0x%016" PRIx64
  const uint64_t kernel = kernel_load_address_.value();
  const uint64_t bss = kernel + KernelLoadSize();
  const uint64_t end = kernel + KernelMemorySize();
  debugf("%s: Relocated\n", ProgramName());
  debugf("%s:    Kernel @ [" ADDR ", " ADDR ")\n", ProgramName(), kernel, bss);
  debugf("%s:       BSS @ [" ADDR ", " ADDR ")\n", ProgramName(), bss, end);
  if (data_load_address_) {
    debugf("%s:       ZBI @ [" ADDR ", " ADDR ")\n", ProgramName(), *data_load_address_,
           *data_load_address_ + DataLoadSize());
  }
}

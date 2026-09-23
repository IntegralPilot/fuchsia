// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/asm.h>
#include <lib/boot-options/boot-options.h>
#include <lib/code-patching/self-test.h>
#include <lib/elfldltl/machine.h>
#include <lib/uart/all.h>
#include <stdlib.h>
#include <zircon/assert.h>

#include <phys/address-space.h>
#include <phys/boot-constants.h>
#include <phys/handoff.h>
#include <phys/zircon-abi-spec.h>

extern "C" constexpr ZirconAbiSpec kZirconAbiSpec = {
    .machine_stack =
        {
            .size_bytes = 0x1000,
            .lower_guard_size_bytes = 0x1000,
            .upper_guard_size_bytes = 0x1000,
        },
#if __has_feature(shadow_call_stack)
    .shadow_call_stack =
        {
            .size_bytes = 0x1000,
            .lower_guard_size_bytes = 0x1000,
            .upper_guard_size_bytes = 0x1000,
        },
#endif
#if __has_feature(safe_stack)
    .unsafe_stack =
        {
            .size_bytes = 0x1000,
            .lower_guard_size_bytes = 0x1000,
            .upper_guard_size_bytes = 0x1000,
        },
#endif

    .boot_constants{kBootConstants},

    .kernel_aspace_base = AddressSpace::kUpperVirtualAddressRangeStart,
    .kernel_aspace_size =
        (ktl::numeric_limits<uint64_t>::max() - AddressSpace::kUpperVirtualAddressRangeStart) + 1,
};

PhysHandoff* gPhysHandoff = nullptr;

extern "C" arch::AsmLabel __ehdr_start;  // NOLINT(bugprone-reserved-identifier)

namespace {

void CheckBootConstants() {
  // Access the field via its physical address (still identity-mapped) to check
  // that it's really the right physical address.
  uintptr_t load_bias =
      arch::kAsmLabelAddress<__ehdr_start> - kBootConstants.kernel_physical_load_address;
  const uintptr_t* physical_ptr = reinterpret_cast<const uintptr_t*>(
      reinterpret_cast<uintptr_t>(&kBootConstants.kernel_physical_load_address) - load_bias);
  ZX_ASSERT(*physical_ptr == kBootConstants.kernel_physical_load_address);
}

}  // namespace

[[clang::cfi_unchecked_callee]] void PhysbootHandoff(PhysHandoff* handoff) {
  // Check that the stack is aligned.
  uintptr_t stack_pointer = reinterpret_cast<uintptr_t>(__builtin_frame_address(0));
  ZX_ASSERT((stack_pointer & (elfldltl::AbiTraits<>::kStackAlignment<uintptr_t> - 1)) == 0);

  // Temporary hand-off pointer dereferencing checks that this is set.
  gPhysHandoff = handoff;

  __asm__ volatile(
      R"""(
      .pushsection .rodata.kBootConstants, "a", %%progbits
      .balign %cc[alignment]
      .globl kBootConstants
      .hidden kBootConstants
      .type kBootConstants, %%object
      kBootConstants:
        .space %cc[size], %cc[fill]
      .size kBootConstants, %cc[size]
      .popsection
      )"""
      :
      :
      [alignment] "i"(alignof(BootConstants)), [size] "i"(sizeof(BootConstants)), [fill] "i"(0xbb));

  uart::all::KernelDriver<uart::BasicIoProvider, uart::UnsynchronizedPolicy, uart::NullIrqProvider>(
      kBootConstants.boot_options->serial)
      .Visit([](auto& uart) {
        uart.Write("Hello world!\n");
        CodePatchingNopTest();
        uart.Write("I've been patched!\n");
        CheckBootConstants();
        uart.Write("\n" BOOT_TEST_SUCCESS_STRING "\n");
      });
  abort();
}

// This is what ZX_ASSERT calls.
void __zx_panic(const char* format, ...) { __builtin_trap(); }

// This is what libc++ headers call.
[[noreturn]] void std::__libcpp_verbose_abort(const char* format, ...) noexcept {
  __builtin_trap();
}

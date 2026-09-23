// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <stdio.h>
#include <zircon/assert.h>

#include <phys/exception.h>
#include <phys/symbolize.h>

#ifdef __aarch64__
#include <lib/arch/arm64/system.h>
#endif

#include "test-main.h"

namespace {

// These are actually defined in the __asm__ in TestMain.
extern "C" void ExceptionSite(), ExceptionResume();

const uint64_t kExceptionSite = reinterpret_cast<uintptr_t>(ExceptionSite);
const uint64_t kExceptionResume = reinterpret_cast<uintptr_t>(ExceptionResume);

#if defined(__aarch64__)
uint64_t& TestRegister(PhysExceptionState& state) { return state.regs.r[0]; }
#elif defined(__riscv)
uint64_t& TestRegister(PhysExceptionState& state) { return state.regs.a0; }
#elif defined(__x86_64__)
uint64_t& TestRegister(PhysExceptionState& state) { return state.regs.rax; }
#endif

PHYS_SINGLETHREAD uint64_t HandleExpectedException(uint64_t vector, const char* vector_name,
                                                   PhysExceptionState& state) {
  printf("%s: Handling exception %#" PRIx64 " (%s)\n", ProgramName(), vector, vector_name);

  PrintPhysException(vector, vector_name, state);

  ZX_ASSERT(state.pc() == kExceptionSite);

  uint64_t& test_value = TestRegister(state);
  ZX_ASSERT(test_value == 17);
  test_value = 23;

  printf("%s: Resume from exception at %#" PRIx64 " to PC %#" PRIx64 "...\n", ProgramName(),
         state.pc(), kExceptionResume);

  return PhysExceptionResume(state, kExceptionResume, state.sp(), state.psr());
}

}  // namespace

int TestMain(void* bootloader_data, ktl::optional<EarlyBootZbi> zbi, arch::EarlyTicks ticks) {
  MainSymbolize symbolize("phys-exception-test");
  symbolize.ContextAlways();

  printf("Hello, world.\n");

  gPhysHandledException = {.pc = reinterpret_cast<uintptr_t>(ExceptionSite),
                           .handler = HandleExpectedException};

  uint64_t interrupted_register = 17;

  printf("I'm going to crash now!  The magic number is %" PRIu64 ".\n", interrupted_register);

#if defined(__aarch64__)
  __asm__(
      R"""(
      mov x0, %[before]
.globl ExceptionSite
.hidden ExceptionSite
ExceptionSite:
      brk #0
.globl ExceptionResume
.hidden ExceptionResume
ExceptionResume:
      mov %[after], x0
      )"""
      : [after] "=r"(interrupted_register)
      : [before] "r"(interrupted_register)
      : "x0");
#elif defined(__riscv)
  __asm__(
      R"""(
      mv a0, %[before]
.globl ExceptionSite
.hidden ExceptionSite
ExceptionSite:
      unimp
.globl ExceptionResume
.hidden ExceptionResume
ExceptionResume:
      mv %[after], a0
      )"""
      : [after] "=r"(interrupted_register)
      : [before] "r"(interrupted_register)
      : "a0");
#elif defined(__x86_64__)
  __asm__(
      R"""(
.globl ExceptionSite
.hidden ExceptionSite
ExceptionSite:
      ud2
.globl ExceptionResume
.hidden ExceptionResume
ExceptionResume:
      )"""
      : "=a"(interrupted_register)
      : "a"(interrupted_register));
#else
  __builtin_trap();
#endif

  printf("I'm back now!  The magic number is %" PRIu64 ".\n", interrupted_register);

  ZX_ASSERT(interrupted_register == 23);

  return 0;
}

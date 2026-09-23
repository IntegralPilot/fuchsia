// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2008-2012 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <arch.h>
#include <ctype.h>
#include <debug.h>
#include <endian.h>
#include <lib/console.h>
#include <lib/instrumentation/asan.h>
#include <lib/page/size.h>
#include <lib/thread-stack/abi.h>
#include <lib/unittest/user_memory.h>
#include <lib/zircon-internal/macros.h>
#include <platform.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <zircon/time.h>
#include <zircon/tls.h>
#include <zircon/types.h>

#include <arch/current_thread.h>
#include <arch/ops.h>
#include <kernel/lockdep.h>
#include <kernel/thread.h>
#include <ktl/array.h>
#include <ktl/unique_ptr.h>
#include <platform/debug.h>
#include <vm/physmap.h>
#include <vm/pmm.h>

#include <ktl/enforce.h>

#if defined(__x86_64__)
#include <arch/x86/feature.h>
#endif

static int cmd_display_mem(int argc, const cmd_args* argv, uint32_t flags);
static int cmd_modify_mem(int argc, const cmd_args* argv, uint32_t flags);
static int cmd_fill_mem(int argc, const cmd_args* argv, uint32_t flags);
static int cmd_memtest(int argc, const cmd_args* argv, uint32_t flags);
static int cmd_copy_mem(int argc, const cmd_args* argv, uint32_t flags);
static int cmd_sleep(int argc, const cmd_args* argv, uint32_t flags);
static int cmd_crash(int argc, const cmd_args* argv, uint32_t flags);
static int cmd_build_instrumentation(int argc, const cmd_args* argv, uint32_t flags);
static int cmd_pop(int argc, const cmd_args* argv, uint32_t flags);

STATIC_COMMAND_START
STATIC_COMMAND_MASKED("dd", "display memory in dwords", &cmd_display_mem, CMD_AVAIL_ALWAYS)
STATIC_COMMAND_MASKED("dw", "display memory in words", &cmd_display_mem, CMD_AVAIL_ALWAYS)
STATIC_COMMAND_MASKED("dh", "display memory in halfwords", &cmd_display_mem, CMD_AVAIL_ALWAYS)
STATIC_COMMAND_MASKED("db", "display memory in bytes", &cmd_display_mem, CMD_AVAIL_ALWAYS)
STATIC_COMMAND_MASKED("mw", "modify word of memory", &cmd_modify_mem, CMD_AVAIL_ALWAYS)
STATIC_COMMAND_MASKED("mh", "modify halfword of memory", &cmd_modify_mem, CMD_AVAIL_ALWAYS)
STATIC_COMMAND_MASKED("mb", "modify byte of memory", &cmd_modify_mem, CMD_AVAIL_ALWAYS)
STATIC_COMMAND_MASKED("fw", "fill range of memory by word", &cmd_fill_mem, CMD_AVAIL_ALWAYS)
STATIC_COMMAND_MASKED("fh", "fill range of memory by halfword", &cmd_fill_mem, CMD_AVAIL_ALWAYS)
STATIC_COMMAND_MASKED("fb", "fill range of memory by byte", &cmd_fill_mem, CMD_AVAIL_ALWAYS)
STATIC_COMMAND_MASKED("mc", "copy a range of memory", &cmd_copy_mem, CMD_AVAIL_ALWAYS)
STATIC_COMMAND("mtest", "simple memory test", &cmd_memtest)
STATIC_COMMAND("crash", "intentionally crash", &cmd_crash)
STATIC_COMMAND("sleep", "sleep number of seconds", &cmd_sleep)
STATIC_COMMAND("sleepm", "sleep number of milliseconds", &cmd_sleep)
STATIC_COMMAND(
    "build_instrumentation",
    "display a non-exhaustive list of build instrumentations used to build this kernel image",
    &cmd_build_instrumentation)
STATIC_COMMAND("pop", "start drama", &cmd_pop)
STATIC_COMMAND_END(mem)

static int cmd_display_mem(int argc, const cmd_args* argv, uint32_t flags) {
  /* save the last address and len so we can continue where we left off */
  static unsigned long address;
  static size_t len;

  if (argc < 3 && len == 0) {
    printf("not enough arguments\n");
    printf("%s [-l] [-b] [-p] [address] [length]\n", argv[0].str);
    return -1;
  }

  int size;
  if (strcmp(argv[0].str, "dd") == 0) {
    size = 8;
  } else if (strcmp(argv[0].str, "dw") == 0) {
    size = 4;
  } else if (strcmp(argv[0].str, "dh") == 0) {
    size = 2;
  } else {
    size = 1;
  }

  uint byte_order = BYTE_ORDER;
  int argindex = 1;
  bool read_address = false;
  bool phys_addr = false;
  while (argc > argindex) {
    if (!strcmp(argv[argindex].str, "-l")) {
      byte_order = LITTLE_ENDIAN;
    } else if (!strcmp(argv[argindex].str, "-b")) {
      byte_order = BIG_ENDIAN;
    } else if (!strcmp(argv[argindex].str, "-p")) {
      phys_addr = true;
    } else if (!read_address) {
      address = argv[argindex].u;
      read_address = true;
    } else {
      len = argv[argindex].u;
    }

    argindex++;
  }

  if ((address & (size - 1)) != 0) {
    printf("unaligned address, cannot display\n");
    return -1;
  }

  if (phys_addr) {
    // Translate to a physmap address if present.
    void* vaddr = paddr_to_physmap(address);
    if (!vaddr) {
      printf("ERROR: failed to translate physical address\n");
      return -1;
    }

    printf("physical address %#lx translated to %p\n", address, vaddr);
    address = (unsigned long)vaddr;
  }

  /* preflight the start address to see if it's mapped */
  if (vaddr_to_paddr((void*)address) == 0) {
    printf("ERROR: address 0x%lx is unmapped\n", address);
    return -1;
  }

  unsigned long stop = address + len;
  int count = 0;

  for (; address < stop; address += size) {
    if (count == 0)
      printf("0x%08lx: ", address);
    switch (size) {
      case 8: {
        uint64_t val =
            (byte_order != BYTE_ORDER) ? SWAP_64(*(uint64_t*)address) : *(uint64_t*)address;
        printf("%016lx ", val);
        break;
      }
      case 4: {
        uint32_t val =
            (byte_order != BYTE_ORDER) ? SWAP_32(*(uint32_t*)address) : *(uint32_t*)address;
        printf("%08x ", val);
        break;
      }
      case 2: {
        uint16_t val =
            (byte_order != BYTE_ORDER) ? SWAP_16(*(uint16_t*)address) : *(uint16_t*)address;
        printf("%04hx ", val);
        break;
      }
      case 1:
        printf("%02hhx ", *(uint8_t*)address);
        break;
    }
    count += size;
    if (count == 16) {
      printf("\n");
      count = 0;
    }
  }

  if (count != 0)
    printf("\n");

  return 0;
}

static int cmd_modify_mem(int argc, const cmd_args* argv, uint32_t flags) {
  int size;

  if (argc < 3) {
    printf("not enough arguments\n");
    printf("%s <address> <val>\n", argv[0].str);
    return -1;
  }

  if (strcmp(argv[0].str, "mw") == 0) {
    size = 4;
  } else if (strcmp(argv[0].str, "mh") == 0) {
    size = 2;
  } else {
    size = 1;
  }

  unsigned long address = argv[1].u;
  unsigned long val = argv[2].u;

  if ((address & (size - 1)) != 0) {
    printf("unaligned address, cannot modify\n");
    return -1;
  }

  switch (size) {
    case 4:
      *(uint32_t*)address = (uint32_t)val;
      break;
    case 2:
      *(uint16_t*)address = (uint16_t)val;
      break;
    case 1:
      *(uint8_t*)address = (uint8_t)val;
      break;
  }

  return 0;
}

static int cmd_fill_mem(int argc, const cmd_args* argv, uint32_t flags) {
  int size;

  if (argc < 4) {
    printf("not enough arguments\n");
    printf("%s <address> <len> <val>\n", argv[0].str);
    return -1;
  }

  if (strcmp(argv[0].str, "fw") == 0) {
    size = 4;
  } else if (strcmp(argv[0].str, "fh") == 0) {
    size = 2;
  } else {
    size = 1;
  }

  unsigned long address = argv[1].u;
  unsigned long len = argv[2].u;
  unsigned long stop = address + len;
  unsigned long val = argv[3].u;

  if ((address & (size - 1)) != 0) {
    printf("unaligned address, cannot modify\n");
    return -1;
  }

  for (; address < stop; address += size) {
    switch (size) {
      case 4:
        *(uint32_t*)address = (uint32_t)val;
        break;
      case 2:
        *(uint16_t*)address = (uint16_t)val;
        break;
      case 1:
        *(uint8_t*)address = (uint8_t)val;
        break;
    }
  }

  return 0;
}

static int cmd_copy_mem(int argc, const cmd_args* argv, uint32_t flags) {
  if (argc < 4) {
    printf("not enough arguments\n");
    printf("%s <source address> <target address> <len>\n", argv[0].str);
    return -1;
  }

  uintptr_t source = argv[1].u;
  uintptr_t target = argv[2].u;
  size_t len = argv[3].u;

  memcpy((void*)target, (const void*)source, len);

  return 0;
}

static int cmd_memtest(int argc, const cmd_args* argv, uint32_t flags) {
  if (argc < 3) {
    printf("not enough arguments\n");
    printf("%s <base> <len>\n", argv[0].str);
    return -1;
  }

  uint32_t* ptr;
  size_t len;

  ptr = (uint32_t*)argv[1].u;
  len = (size_t)argv[2].u;

  size_t i;
  // write out
  printf("writing first pass...");
  for (i = 0; i < len / 4; i++) {
    ptr[i] = static_cast<uint32_t>(i);
  }
  printf("done\n");

  // verify
  printf("verifying...");
  for (i = 0; i < len / 4; i++) {
    if (ptr[i] != i)
      printf("error at %p\n", &ptr[i]);
  }
  printf("done\n");

  return 0;
}

static int cmd_sleep(int argc, const cmd_args* argv, uint32_t flags) {
  zx_duration_mono_t t = ZX_SEC(1); /* default to 1 second */

  if (argc >= 2) {
    t = ZX_MSEC(argv[1].u);
    if (!strcmp(argv[0].str, "sleep"))
      t = zx_duration_mul_int64(t, 1000);
  }

  Thread::Current::SleepRelative(t);

  return 0;
}

static int crash_thread(void*) {
  /* should crash */
  volatile uint32_t* ptr = (volatile uint32_t*)1u;
  *ptr = 1;

  return 0;
}

static int crash_deref(bool thread) {
  if (thread) {
    Thread* t = Thread::Create("crasher", &crash_thread, NULL, DEFAULT_PRIORITY);
    t->Resume();

    t->Join(NULL, ZX_TIME_INFINITE);

    return 0;
  }

  crash_thread(nullptr);

  /* if it didn't, panic the system */
  panic("crash");

  return 0;
}

// Crash by intentionally recursing to itself until the kernel
// call stack is exceeded.
static int recurse() {
  // The compiler doesn't know what function pointer this is anymore after the
  // asm, but it hasn't changed.
  decltype(recurse)* self = recurse;
  __asm__ volatile("" : "+r"(self));
  return self() + 1;
}

static void BeforeStackOverflow(const char* what) {
  const auto& stack = Thread::Current::Get()->stack();
  printf("%s from %p in [%#" PRIxPTR ", %#" PRIxPTR ") $tp %p stack_guard %#" PRIx64 "...\n", what,
         __builtin_frame_address(0), stack.base(), stack.top(),
         arch_get_current_compiler_thread_pointer(),
         *reinterpret_cast<const uint64_t*>(arch_get_current_compiler_thread_pointer() +
                                            ZX_TLS_STACK_GUARD_OFFSET));
}

static int crash_recurse() {
  BeforeStackOverflow("Recursing");

  recurse();

  printf("survived.\n");

  return 0;
}

__attribute__((noinline)) static void stomp_stack(size_t size) {
  // -Wvla prevents VLAs but not explicit alloca.
  // Neither is allowed anywhere in the kernel outside this test code.
  void* death = __builtin_alloca(size);  // OK in test-only code.
  memset(death, 0xdb, size);
  Thread::Current::SleepRelative(ZX_USEC(1));
}

static int crash_stackstomp() {
  BeforeStackOverflow("Stomping stack");

  for (size_t i = 0; i < internal::kMachineStackSize * 2; i++) {
    stomp_stack(i);
  }

  printf("survived.\n");

  return 0;
}

static int crash_shadow_call_stack_stomp() {
#if !__has_feature(shadow_call_stack)
  printf("Shadow call stack not in use.\n");
  return -1;
#else
  BeforeStackOverflow("Overflowing shadow call stack");

  // Just warp it straight to the edge so the next spill will overflow.
  const uint64_t top = Thread::Current::Get()->stack().shadow_call_top();
#if defined(__aarch64__)
  __asm__ volatile("mov x18, %0" : : "r"(top));
#elif defined(__riscv)
  __asm__ volatile("mv gp, %0" : : "r"(top));
#endif

  // Cause some calls that definitely use the shadow call stack.
  Thread::Current::SleepRelative(ZX_USEC(1));

  printf("survived.\n");

  return 0;
#endif
}

static bool has_user_code_execution_protection() {
#if defined(__x86_64__)
  return x86_feature_test(X86_FEATURE_SMEP) || g_x86_feature_has_smap;
#elif defined(__aarch64__)
  // Privilege Execute Never (PXN) is available on all aarch64 machines.
  return true;
#elif defined(__riscv)
  // Supervisor User Memory access control is always available on RISC-V.
  return true;
#else
#error "what machine?"
#endif
}

static int crash_user_execute() {
  if (!has_user_code_execution_protection()) {
    printf(
        "missing protection to avoid executing userspace code from a privileged context; will not "
        "crash.\n");
    return -1;
  }
  constexpr size_t kUserMemorySize = kPageSize;

  ktl::unique_ptr<testing::UserMemory> mem = testing::UserMemory::Create(kUserMemorySize);
  if (mem == nullptr) {
    printf("failed to allocate user memory; will not crash.\n");
    return -1;
  }

  // Define a little fragment of code that we can copy.
  const void* begin_func;
  size_t func_size;
  __asm__(
      ".pushsection .rodata.%=, \"a?\", %%progbits\n"
      "0:\n"
#if defined(__x86_64__) || defined(__aarch64__) || defined(__riscv)
      "ret\n"
#else
#error "what machine?"
#endif
      "1:\n"
      ".popsection\n"
#if defined(__aarch64__)
      "adrp %[ptr], 0b\n"
      "add %[ptr], %[ptr], #:lo12:0b\n"
      "mov %[size], #1b - 0b\n"
#elif defined(__riscv)
      "lla %[ptr], 0b\n"
      "li %[size], 1b - 0b\n"
#elif defined(__x86_64__)
      "lea 0b(%%rip), %[ptr]\n"
      "mov $1b - 0b, %[size]\n"
#else
#error "what machine?"
#endif
      : [ptr] "=r"(begin_func), [size] "=r"(func_size));

  ASSERT_MSG(func_size <= kUserMemorySize, "function does not fit in allocated user memory");

  zx_status_t status = mem->VmoWrite(begin_func, /* offset */ 0, func_size);
  if (status != ZX_OK) {
    printf("failed to copy payload (%d); will not crash.\n", status);
    return -1;
  }

  status = mem->CommitAndMap(kUserMemorySize, /* offset */ 0);
  if (status != ZX_OK) {
    printf("failed to commit memory (%d); will not crash.\n", status);
    return -1;
  }

  // Set the memory as executable. We need to also make it read-only because
  // in arm64, writable user mappings imply Privileged Execute Never (PXN).
  status = mem->MakeRX();
  if (status != ZX_OK) {
    printf("failed to make memory executable (%d); will not crash.\n", status);
    return -1;
  }

  arch_mmu_flags_t mmu_flags;
  status = mem->aspace()->arch_aspace().Query(mem->base(), nullptr, &mmu_flags);
  if (status != ZX_OK) {
    printf("failed to query mmu flags (%d); will not crash.\n", status);
    return -1;
  }

  if ((mmu_flags & ARCH_MMU_FLAG_PERM_WRITE) ||
      !(mmu_flags &
        (ARCH_MMU_FLAG_PERM_USER | ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_EXECUTE))) {
    printf("incorrect memory permissions; will not crash.\n");
    return -1;
  }

  const uint8_t* p = mem->user_in<uint8_t>().get();
  if (p == nullptr) {
    printf("failed to get pointer; will not crash.\n");
    return -1;
  }

  auto user_func = reinterpret_cast<void (*)(void)>(reinterpret_cast<uintptr_t>(p));

  arch_sync_cache_range(mem->base(), kUserMemorySize);

  printf("about to crash..\n");
  user_func();
  printf("executed userspace code from a kernel context; did not crash.\n");
  return 0;
}

// Marked with NO_ASAN because this will be called with a pointer to user memory.
NO_ASAN static uint8_t read_byte(const uint8_t* p) { return *p; }

static int crash_user_read() {
#if defined(__x86_64__)
  if (!g_x86_feature_has_smap) {
    printf("cpu does not support smap; will not crash.\n");
    return -1;
  }
#elif defined(__aarch64__)
  // TODO(https://fxbug.dev/42137335): Once we support PAN enable this for arm64.
  printf("ARM64 currently does not support PAN; will not crash.\n");
  return -1;
#endif
  // RISCV implements the sstatus.sum bit, similar to x86 SMAP

  ktl::unique_ptr<testing::UserMemory> mem = testing::UserMemory::Create(kPageSize);
  if (mem == nullptr) {
    printf("failed to allocate user memory; will not crash.\n");
    return -1;
  }
  const uint8_t* p = mem->user_in<uint8_t>().get();
  if (p == nullptr) {
    printf("failed to get pointer; will not crash.\n");
    return -1;
  }

  printf("about to crash..\n");
  uint8_t b = read_byte(p);

  printf("read %02hhx; did not crash.\n", b);
  return -1;
}

static int crash_pmm_use_after_free() {
  // We want to corrupt one of the pages on the pmm's free list.  To do so, we'll allocate a bunch
  // of pages, keep track of the address of the last page, then free them all.  The free list is
  // LIFO so by allocating and freeing a bunch of pages we'll have a pointer "to the middle" and our
  // corrupted page will be less like to be immediately allocated.

  // Allocate.
  const size_t num_pages = 10000;
  VmPageDoublyLinkedList pages;
  zx_status_t status = pmm_alloc_pages(num_pages, 0, &pages);
  if (unlikely(status != ZX_OK)) {
    printf("error: failed to allocate (%d)\n", status);
    return -1;
  }

  // Make note of address.
  vm_page_t* last_page = &pages.back();
  void* va = paddr_to_physmap(last_page->paddr());

  // We're printing a little early because once we've returned the pages to the free list, we want
  // to avoid doing anything that might cause the target page to be allocated (by this thread or
  // some other thread).
  printf("corrupting memory at address %p\n", va);

  // Free.
  pmm_free(&pages);

  // Corrupt!
  *reinterpret_cast<char*>(va) = 'X';

  printf("crash_pmm_use_after_free done\n");
  return -1;
}

static int crash_assert() {
  constexpr int kValue = 42;
  ASSERT_MSG(kValue == 0, "value %d\n", kValue);
  return -1;
}

static int crash_stack_guard() {
  printf("attempting to crash\n");
  // Attempt to crash by overwriting the compiler-inserted stack guard.
  constexpr size_t kSize = 128;
  // alloca should never be used outside of test code.
  char* p = static_cast<char*>(__builtin_alloca(kSize));
  memset(p, 0x45, 2 * kSize);
  return -1;
}

static int crash_illegal_instruction() {
  printf("attempting to crash with an illegal instruction\n");
#if defined(__riscv)
  __asm__ volatile("unimp");
#elif defined(__x86_64__)
  __asm__ volatile("ud2");
#elif defined(__aarch64__)
  __asm__ volatile("udf #0");
#else
  printf("not implemented for this architecture\n");
#endif

  return 0;
}

static int crash_break_instruction() {
  printf("attempting to crash with a break instruction\n");
#if defined(__riscv)
  __asm__ volatile("ebreak");
#elif defined(__x86_64__)
  __asm__ volatile("int3");
#elif defined(__aarch64__)
  __asm__ volatile("brk #0");
#else
  printf("not implemented for this architecture\n");
#endif

  return 0;
}

static int crash_syscall_instruction() {
  printf("attempting to crash with a syscall instruction\n");
#if defined(__riscv)
  // Actually not a good idea, this will probably call into firmware with undefined args.
  printf("not implemented for RISC-V, ecall instruction would probably be trapped by firmware\n");
#elif defined(__x86_64__)
  __asm__ volatile("syscall");
#elif defined(__aarch64__)
  __asm__ volatile("svc #0");
#else
  printf("not implemented for this architecture\n");
#endif

  return 0;
}

static int cmd_crash(int argc, const cmd_args* argv, uint32_t flags) {
  auto usage = [&]() {
    printf("usage:\n");
    printf("%s deref [thread]      : intentionally dereference an invalid pointer\n", argv[0].str);
    printf("%s stackstomp          : intentionally overrun the stack\n", argv[0].str);
    printf("%s recurse             : intentionally overrun the stack by recursing\n", argv[0].str);
    printf("%s user_read           : intentionally read user memory\n", argv[0].str);
    printf("%s user_execute        : intentionally execute user memory\n", argv[0].str);
    printf("%s pmm_use_after_free  : intentionally corrupt the pmm free list\n", argv[0].str);
    printf("%s assert              : intentionally crash by failing an assert\n", argv[0].str);
    printf("%s stack_guard         : attempt to crash by overwriting the stack guard\n",
           argv[0].str);
    printf("%s illegal_instruction : attempt to crash by running an illegal instruction\n",
           argv[0].str);
    printf("%s break_instruction   : attempt to crash by running a break instruction\n",
           argv[0].str);
    printf("%s syscall_instruction : attempt to crash by running a syscall instruction\n",
           argv[0].str);
    return ZX_ERR_INTERNAL;
  };

  if (argc < 2) {
    printf("not enough arguments\n");
    return usage();
  }
  if (!strcmp(argv[1].str, "deref")) {
    if (argc > 2 && !strcmp(argv[2].str, "thread")) {
      return crash_deref(true);
    } else {
      return crash_deref(false);
    }
  } else if (!strcmp(argv[1].str, "stackstomp")) {
    return crash_stackstomp();
  } else if (!strcmp(argv[1].str, "shadow_call_stack_stomp")) {
    return crash_shadow_call_stack_stomp();
  } else if (!strcmp(argv[1].str, "recurse")) {
    return crash_recurse();
  } else if (!strcmp(argv[1].str, "user_read")) {
    return crash_user_read();
  } else if (!strcmp(argv[1].str, "user_execute")) {
    return crash_user_execute();
  } else if (!strcmp(argv[1].str, "pmm_use_after_free")) {
    return crash_pmm_use_after_free();
  } else if (!strcmp(argv[1].str, "assert")) {
    return crash_assert();
  } else if (!strcmp(argv[1].str, "stack_guard")) {
    return crash_stack_guard();
  } else if (!strcmp(argv[1].str, "illegal_instruction")) {
    return crash_illegal_instruction();
  } else if (!strcmp(argv[1].str, "break_instruction")) {
    return crash_break_instruction();
  } else if (!strcmp(argv[1].str, "syscall_instruction")) {
    return crash_syscall_instruction();
  } else {
    printf("unknown command\n");
    return usage();
  }

  return ZX_OK;
}

static int cmd_build_instrumentation(int argc, const cmd_args* argv, uint32_t flags) {
  [[maybe_unused]] auto print_feature = [](const char* what) {
    printf("build_instrumentation: %s\n", what);
  };

#if __has_feature(address_sanitizer)
  print_feature("address_sanitizer");
#endif
#if DEBUG_ASSERT_IMPLEMENTED
  print_feature("debug_assert");
#endif
#if WITH_LOCK_DEP
  print_feature("lockdep");
#endif
#if __has_feature(safe_stack)
  print_feature("safe_stack");
#endif
#if __has_feature(shadow_call_stack)
  print_feature("shadow_call_stack");
#endif
#if __has_feature(undefined_behavior_sanitizer)
  print_feature("undefined_behavior_sanitizer");
#endif

  // missing: sancov, profile

  // This print statement is load-bearing! It is scraped in a panic test.
  // LINT.IfChange
  printf("build_instrumentation: done\n");
  // LINT.ThenChange(//src/tests/kernel_panic/kernel_panic_test.go)
  return 0;
}

static int cmd_pop(int argc, const cmd_args* argv, uint32_t flags) {
  if (argc > 1) {
    printf("aren't there already enough arguments?\n");
    return -1;
  }

  printf("drama!");
  return 0;
}

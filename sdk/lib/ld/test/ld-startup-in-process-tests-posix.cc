// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ld-startup-in-process-tests-posix.h"

#include <fcntl.h>
#include <lib/elfldltl/fd.h>
#include <lib/elfldltl/layout.h>
#include <lib/ld/abi.h>
#include <lib/ld/testing/interp.h>
#include <sys/auxv.h>
#include <sys/mman.h>
#include <unistd.h>

#include <numeric>
#include <span>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

#include "../posix.h"
#include "ld-load-tests-base.h"
#include "load-tests.h"
#include "test-chdir-guard.h"

namespace ld::testing {
namespace {

constexpr size_t kStackSize = 64 << 10;

// This is actually defined in assembly code with internal linkage.
// It simply switches to the new SP and then calls the entry point.
// When that code returns, this just restores the old SP and also returns.
extern "C" int64_t CallOnStack(uintptr_t entry, void* sp);
__asm__(
    R"""(
    .pushsection .text.CallOnStack, "ax", %progbits
    .type CallOnstack, %function
    CallOnStack:
      .cfi_startproc
    )"""
#if defined(__aarch64__)
    R"""(
      stp x29, x30, [sp, #-16]!
      .cfi_adjust_cfa_offset 16
      mov x29, sp
      .cfi_def_cfa_register x29
      mov sp, x1
      blr x0
      mov sp, x29
      .cfi_def_cfa_register sp
      ldp x29, x30, [sp], #16
      .cfi_adjust_cfa_offset -16
      ret
    )"""
#elif defined(__x86_64__)
    // Note this stores our return address below the SP and then jumps, because
    // a call would move the SP.  The posix-startup.S entry point code expects
    // the StartupStack at the SP, not a return address.  Note this saves and
    // restores %rbx so that the entry point code can clobber it.
    // TODO(https://fxbug.dev/479517175): For now, it then returns at the end,
    // popping the stack.
    R"""(
      push %rbp
      .cfi_adjust_cfa_offset 8
      mov %rsp, %rbp
      .cfi_def_cfa_register %rbp
      .cfi_offset %rbp, -8*2
      push %rbx
      .cfi_offset %rbx, -8*3
      lea 0f(%rip), %rax
      mov %rsi, %rsp
      mov %rax, -8(%rsp)
      jmp *%rdi
    0:mov %rbp, %rsp
      .cfi_def_cfa_register %rsp
      mov -8(%rsp), %rbx
      .cfi_same_value %rbx
      pop %rbp
      .cfi_same_value %rbp
      .cfi_adjust_cfa_offset -8
      ret
    )"""
#else
#error "unsupported machine"
#endif
    R"""(
      .cfi_endproc
    .size CallOnStack, . - CallOnStack
    .popsection
    )""");

}  // namespace

struct LdStartupInProcessTests::AuxvBlock {
  ld::Auxv vdso = {
      static_cast<uintptr_t>(ld::AuxvTag::kSysinfoEhdr),
      getauxval(static_cast<uintptr_t>(ld::AuxvTag::kSysinfoEhdr)),
  };
  ld::Auxv pagesz = {
      static_cast<uintptr_t>(ld::AuxvTag::kPagesz),
      static_cast<uintptr_t>(sysconf(_SC_PAGE_SIZE)),
  };
  ld::Auxv phdr = {static_cast<uintptr_t>(ld::AuxvTag::kPhdr)};
  ld::Auxv phent = {
      static_cast<uintptr_t>(ld::AuxvTag::kPhent),
      sizeof(elfldltl::Elf<>::Phdr),
  };
  ld::Auxv phnum = {static_cast<uintptr_t>(ld::AuxvTag::kPhnum)};
  ld::Auxv entry = {static_cast<uintptr_t>(ld::AuxvTag::kEntry)};
  const ld::Auxv null = {static_cast<uintptr_t>(ld::AuxvTag::kNull)};
};

void LdStartupInProcessTests::Init(std::initializer_list<std::string_view> args,
                                   std::initializer_list<std::string_view> env) {
  ASSERT_NO_FATAL_FAILURE(AllocateStack());
  ASSERT_NO_FATAL_FAILURE(PopulateStack(args, env));
}

void LdStartupInProcessTests::Load(std::string_view raw_executable_name) {
  const std::string executable_name =
      std::string(raw_executable_name) + std::string(kTestExecutableInProcessSuffix);

  ASSERT_TRUE(auxv_);  // Init must have been called already.

  // Acquire the directory where the test ELF files reside.
  ASSERT_NO_FATAL_FAILURE(LoadTestDir(executable_name));

  // Verify it contains what it should.
  ASSERT_NO_FATAL_FAILURE(CheckNeededLibs());

  auto open_file = [this](const std::string& filename, fbl::unique_fd& fd) {
    ASSERT_FALSE(fd);
    fd.reset(openat(test_dir(), filename.c_str(), O_RDONLY | O_CLOEXEC));
    ASSERT_TRUE(fd) << "cannot open " << test_dir_path() / filename.c_str() << ": "
                    << strerror(errno);
  };

  // First load the dynamic linker.  The system program loader will use the
  // PT_INTERP string to find the dynamic linker.  The string embedded at link
  // time and the test_dir() layout will use a prefix for instrumented builds.
  // So open the executable first to read its PT_INTERP rather than assuming
  // it's just ld::abi::kInterp.

  fbl::unique_fd executable_fd;
  ASSERT_NO_FATAL_FAILURE(open_file(executable_name, executable_fd));

  std::string interp;
  ASSERT_NO_FATAL_FAILURE(interp = FindInterp<elfldltl::FdFile>(executable_fd.get()));
  ASSERT_FALSE(interp.empty());

  {
    fbl::unique_fd ld_startup_fd;
    ASSERT_NO_FATAL_FAILURE(open_file(interp, ld_startup_fd));

    std::optional<LoadResult> result;
    ASSERT_NO_FATAL_FAILURE(Load(ld_startup_fd, result));

    // Stash the dynamic linker's entry point.
    entry_ = result->entry + result->loader.load_bias();

    // Save the loader object so it gets destroyed when the test fixture is
    // destroyed.  That will clean up the mappings it made.  (This doesn't do
    // anything about any mappings that were made by the loaded code at Run(),
    // but so it goes.)
    loader_ = std::move(result->loader);
  }

  // Now load the executable.
  std::optional<LoadResult> result;
  ASSERT_NO_FATAL_FAILURE(Load(std::exchange(executable_fd, {}), result));

  // Set AT_PHDR and AT_PHNUM for where the phdrs were loaded.
  std::span phdrs = result->phdrs;

  // This non-template lambda gets called with the vaddr, offset, and filesz of
  // each segment.  It's called by the generic lambda passed to VisitSegments.
  auto on_segment = [load_bias = result->loader.load_bias(), phoff = result->phoff(),
                     phdrs_size_bytes = phdrs.size_bytes(),
                     this](uintptr_t vaddr, uintptr_t offset, size_t filesz) {
    if (offset <= phoff && phoff - offset < filesz &&
        filesz - (phoff - offset) >= phdrs_size_bytes) {
      auxv_->phdr.back() = phoff - offset + vaddr + load_bias;
      return false;
    }
    return true;
  };
  result->info.VisitSegments([on_segment](const auto& segment) {
    return on_segment(segment.vaddr(), segment.offset(), segment.filesz());
  });

  ASSERT_NE(auxv_->phdr.back(), 0u);

  auxv_->phnum.back() = phdrs.size();

  // Set AT_ENTRY to the executable's entry point.
  auxv_->entry.back() = result->entry + result->loader.load_bias();

  // Save the second Loader object to keep the mappings alive.
  exec_loader_ = std::move(result->loader);
}

int64_t LdStartupInProcessTests::Run() {
  // Move into the directory where ld.so.1 and all the files are so that they
  // can be loaded by simple relative file names.  For now, the POSIX version
  // of the dynamic linker uses the plain SONAME as a relative filename.
  TestChdirGuard in_test_dir(test_dir());
  return CallOnStack(entry_, sp_);
}

LdStartupInProcessTests::~LdStartupInProcessTests() {
  if (stack_) {
    munmap(stack_, kStackSize * 2);
  }
}

void LdStartupInProcessTests::AllocateStack() {
  // Allocate a stack and a guard region below it.
  void* ptr = mmap(nullptr, kStackSize * 2, PROT_READ | PROT_WRITE, MAP_ANON | MAP_PRIVATE, -1, 0);
  ASSERT_TRUE(ptr) << "mmap: " << strerror(errno);
  stack_ = ptr;
  // Protect the guard region below the stack.
  EXPECT_EQ(mprotect(stack_, kStackSize, PROT_NONE), 0) << strerror(errno);
}

void LdStartupInProcessTests::PopulateStack(std::initializer_list<std::string_view> argv,
                                            std::initializer_list<std::string_view> envp) {
  // Figure out the total size of string data to write.
  constexpr auto string_size = [](size_t total, std::string_view str) {
    return total + str.size() + 1;
  };
  const size_t strings =
      std::accumulate(argv.begin(), argv.end(),
                      std::accumulate(envp.begin(), envp.end(), 0, string_size), string_size);

  // Compute the total number of pointers to write (after the argc word).
  size_t ptrs = argv.size() + 1 + envp.size() + 1;

  // The stack must fit all that plus the auxv block.
  ASSERT_LT(strings + 15 + ((1 + ptrs) * sizeof(uintptr_t)) + sizeof(AuxvBlock), kStackSize);

  // Start at the top of the stack, and place the strings.
  std::byte* sp = static_cast<std::byte*>(stack_) + (kStackSize * 2);
  sp -= strings;
  std::span string_space{reinterpret_cast<char*>(sp), strings};

  // Adjust down so everything will be aligned.
  const size_t strings_and_ptrs = strings + ((1 + ptrs) * sizeof(uintptr_t));
  sp -= ((strings_and_ptrs + 15) & -size_t{16}) - strings_and_ptrs;

  // Next, leave space for the auxv block, which can be filled in later.
  static_assert(sizeof(AuxvBlock) % 16 == 0);
  sp -= sizeof(AuxvBlock);
  auxv_ = new (sp) AuxvBlock;

  // Finally, the argc and pointers form what's seen right at the SP.
  sp -= (1 + ptrs) * sizeof(uintptr_t);
  ld::StartupStack* startup = new (sp) ld::StartupStack{.argc = argv.size()};
  std::span string_ptrs{startup->argv, ptrs};

  // Now copy the strings and write the pointers to them.
  for (auto list : {argv, envp}) {
    for (std::string_view str : list) {
      string_ptrs.front() = string_space.data();
      string_ptrs = string_ptrs.subspan(1);
      string_space = string_space.subspan(str.copy(string_space.data(), string_space.size()));
      string_space.front() = '\0';
      string_space = string_space.subspan(1);
    }
    string_ptrs.front() = nullptr;
    string_ptrs = string_ptrs.subspan(1);
  }
  ASSERT_TRUE(string_ptrs.empty());
  ASSERT_TRUE(string_space.empty());

  ASSERT_EQ(reinterpret_cast<uintptr_t>(sp) % 16, 0u);
  sp_ = sp;
}

// The loaded code is just writing to STDERR_FILENO in the same process.
// There's no way to install e.g. a pipe end as STDERR_FILENO for the loaded
// code without also hijacking stderr for the test harness itself, which seems
// a bit dodgy even if the original file descriptor were saved and dup2'd back
// after the test succeeds.  In the long run, most cases where the real dynamic
// linker would emit any diagnostics are when it would then crash the process,
// so those cases will only get tested via spawning a new process, not
// in-process tests.
void LdStartupInProcessTests::ExpectLog(std::string_view expected_log) {
  // No log capture, so this must be used only in tests that expect no output.
  EXPECT_EQ(expected_log, "");
}

}  // namespace ld::testing

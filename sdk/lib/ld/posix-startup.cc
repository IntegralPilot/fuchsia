// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <lib/elfldltl/posix.h>
#include <lib/elfldltl/unique-fd.h>
#include <lib/ld/memory.h>
#include <lib/trivial-allocator/new.h>
#include <lib/trivial-allocator/posix.h>
#include <sys/mman.h>
#include <zircon/compiler.h>

#include <array>
#include <cassert>
#include <cerrno>
#include <cstring>
#include <string_view>

#include "allocator.h"
#include "posix.h"
#include "startup-bootstrap.h"
#include "startup-diagnostics.h"

namespace ld {
namespace {

constexpr std::string_view kLdDebugPrefix = "LD_DEBUG=";

// This is defined in assembly, and doesn't actually use the C calling
// convention.  It calls StartLd.
extern "C" [[noreturn]] void _start();

using UniqueFdFile = elfldltl::UniqueFdFile<Diagnostics>;

using Module = abi::Abi<>::Module;

using SystemPageAllocator = trivial_allocator::PosixMmap;

constexpr auto MakeStartupSystemPageAllocator(StartupData& startup) {
  return SystemPageAllocator{startup.page_size};
}

auto MakeStartupScratchAllocator(SystemPageAllocator system) {
  return MakeScratchAllocator(std::move(system));
}

using ScratchAllocator = decltype(MakeStartupScratchAllocator(SystemPageAllocator{}));

auto MakeStartupInitialExecAllocator(SystemPageAllocator system) {
  return MakeInitialExecAllocator(std::move(system));
}

using InitialExecAllocator = decltype(MakeStartupInitialExecAllocator(SystemPageAllocator{}));

// This "loads" the executable image that's already in memory such that the
// StartupModule returned is fully populated like loading another module does.
std::pair<StartupModule*, size_t> LoadExecutable(Diagnostics& diag, StartupData& startup,
                                                 ScratchAllocator& scratch,
                                                 InitialExecAllocator& initial_exec,
                                                 uintptr_t entry, uintptr_t phdr, uintptr_t phnum) {
  if (entry == reinterpret_cast<uintptr_t>(&_start)) [[unlikely]] {
    // The dynamic linker was started directly as a standalone program,
    // rather than via PT_INTERP.
    //
    // TODO(https://fxbug.dev/479303931): The glibc dynamic linker has the
    // useful semantics of handling command line args for various behavior
    // options and for the name of an executable to load as if it had been
    // executed with a PT_INTERP pointing at this dynamic linker.
    diag.SystemError("dynamic linker executed directly, not via PT_INTERP");
    __builtin_trap();
  }

  std::span phdrs{reinterpret_cast<const Phdr*>(phdr), phnum};

  auto no_phdrs = [&diag]() { diag.FormatError("no PT_PHDR in preloaded main executable!"); };

  // The phdrs and entry point from auxv describe the main executable, which
  // was loaded into the address space before its PT_INTERP file (this
  // dynamic linker itself).
  if (phdrs.empty()) [[unlikely]] {
    no_phdrs();
  }

  // Allocate the StartupModule for the main executable.
  StartupModule* main_executable =
      StartupModule::New(diag, scratch, abi::Abi<>::kExecutableName, startup.page_size);
  fbl::AllocChecker ac;
  main_executable->NewModule(0, initial_exec, ac);
  CheckAlloc(diag, ac, "passive ABI module");
  Module& module = main_executable->decoded().module();

  // We already have the direct pointer to the phdrs in the load image, from
  // AT_PHDR even though we never see the Ehdr::phoff value.
  module.phdrs = phdrs;

  // Scan the phdrs for both the vaddr bounds that would normally be
  // determined before loading a module and for everything that would
  // normally be picked up from phdrs after loading a module.
  using PhdrPhdrObserver =
      elfldltl::PhdrSingletonObserver<elfldltl::Elf<>, elfldltl::ElfPhdrType::kPhdr>;
  std::optional<Phdr> phdr_phdr;
  auto phdr_info = DecodeModulePhdrs(
      diag, phdrs, main_executable->decoded().load_info().GetPhdrObserver(startup.page_size),
      PhdrPhdrObserver{phdr_phdr});
  if (!phdr_info) {
    return {};
  }

  main_executable->decoded().set_relro(phdr_info->relro_phdr, startup.page_size);

  // The PT_PHDR gives the link-time view of the p_vaddr of the phdrs.  Since
  // we never saw the Ehdr, we can't use Ehdr::phoff to locate it among the
  // segments, so comparing that p_vaddr to the AT_PHDR value is the only way
  // to find the load bias.
  if (!phdr_phdr) [[unlikely]] {
    no_phdrs();
    __builtin_unreachable();
  }

  SetModuleVaddrBounds(module, main_executable->load_info(), phdr - phdr_phdr->vaddr);

  // The module now has enough information to use ModuleMemory.  The common
  // code expects StartupModule::loader() to have the image and load bias,
  // so set its memory() to match.  This means that destruction would munmap
  // the executable image, but the object will never be destroyed anyway.
  main_executable->memory() = ModuleMemory{module};

  // A second phdr scan is needed to decode notes now that they can be
  // accessed in memory.
  elfldltl::DecodePhdrs(diag, phdrs, PhdrMemoryBuildIdObserver(main_executable->memory(), module));

  if (phdr_info->tls_phdr) {
    main_executable->decoded().SetTls(diag, main_executable->memory(), startup.page_size,
                                      *phdr_info->tls_phdr, 1);
  }

  std::span<const Addr> preinit_array;
  size_t needed_count = 0;
  main_executable->set_dynamic(*main_executable->decoded().DecodeDynamic(  //
      diag, main_executable->memory(), phdr_info->dyn_phdr,
      StartupModule::NeededCountObserver(needed_count),
      StartupModule::PreinitObserver(preinit_array)));
  mutable_abi.preinit_array = preinit_array;
  mutable_abi.stack_size = phdr_info->stack_size.value_or(0);

  return {main_executable, needed_count};
}

[[maybe_unused]] void ProtectData(Diagnostics& diag, const StartupBootstrap& bootstrap) {
  bootstrap.OnWritablePages([&diag](uintptr_t start, size_t size) {
    void* const ptr = reinterpret_cast<void*>(start);
    if (mprotect(ptr, size, PROT_READ) != 0) [[unlikely]] {
      diag.SystemError("cannot mprotect dynamic linker data pages", elfldltl::PosixError{errno});
    }
  });
}

}  // namespace

// This gets called from the entry-point written in assembly, which just
// passes the incoming SP value as the first argument.  It returns the main
// executable entry point address to jump to after unwinding the stack back
// to the starting state.
extern "C" uintptr_t StartLd(StartupStack& stack) {
  StartupData startup = {
      .argc = stack.argc,
      .argv = stack.argv,
      .envp = stack.envp(),
  };

  // First scan the auxv for important pointers and values.
  uintptr_t phdr = 0, phnum = 0, entry = 0;
  const void* vdso = nullptr;
  for (const auto* av = stack.GetAuxv(); av->front() != 0; ++av) {
    const auto& [tag, value] = *av;
    switch (static_cast<AuxvTag>(tag)) {
      case AuxvTag::kPagesz:
        startup.page_size = value;
        break;
      case AuxvTag::kEntry:
        entry = value;
        break;
      case AuxvTag::kSysinfoEhdr:
        vdso = reinterpret_cast<const void*>(value);
        break;
      case AuxvTag::kPhdr:
        phdr = value;
        break;
      case AuxvTag::kPhnum:
        phnum = value;
        break;
      case AuxvTag::kPhent:
        assert(value == sizeof(Phdr));
        break;
      default:
        break;
    }
  }

  // First thing, bootstrap our own dynamic linking against ourselves and the
  // vDSO.  For this, nothing should go wrong so use a diagnostics object that
  // crashes the process at the first error.  But we can still use direct
  // system calls to write error messages.
  auto bootstrap_diag = elfldltl::Diagnostics{
      DiagnosticsReport(startup),
      elfldltl::DiagnosticsPanicFlags{},
  };
  StartupBootstrap bootstrap{bootstrap_diag, vdso, startup.page_size};

  // Check for the LD_DEBUG environment variable.
  for (char** ep = startup.envp; *ep; ++ep) {
    std::string_view str = *ep;
    if (str.size() > kLdDebugPrefix.size() && str.starts_with(kLdDebugPrefix)) {
      startup.ld_debug = true;
      break;
    }
  }

  // Now that things are bootstrapped, set up the main diagnostics object.
  Diagnostics diag{startup};

  // Set up the allocators.
  auto system_page_allocator = MakeStartupSystemPageAllocator(startup);
  auto scratch = MakeStartupScratchAllocator(system_page_allocator);
  auto initial_exec = MakeStartupInitialExecAllocator(system_page_allocator);

  // "Load" the main executable.  It's usually already loaded by the system
  // program loader before the dynamic linker was loaded, so this really
  // consists of doing the bookkeeping that loading other modules does.
  auto [main_executable, needed_count] =
      LoadExecutable(diag, startup, scratch, initial_exec, entry, phdr, phnum);

  auto open_file = [&diag](const elfldltl::Soname<>& soname) -> std::optional<UniqueFdFile> {
    if (fbl::unique_fd fd{open(soname.c_str(), O_RDONLY)}) {
      return UniqueFdFile{std::move(fd), diag};
    }
    if (errno != ENOENT) {
      diag.SystemError("cannot open dependency ", soname.str(), ": ", elfldltl::PosixError{errno});
    }
    return {};
  };

  StartupModule::LinkModules(diag, scratch, initial_exec, main_executable, open_file, bootstrap,
                             needed_count, startup.page_size);

  // Bail out before relocation if there were any loading errors.
  CheckErrors(diag);

  if constexpr (kProtectData) {
    // Now that startup is completed, protect not only the RELRO, but also all
    // the data and bss.
    ProtectData(diag, bootstrap);
  }

  // Bail out before handoff if any errors have been detected.
  CheckErrors(diag);

  return entry;
}

}  // namespace ld

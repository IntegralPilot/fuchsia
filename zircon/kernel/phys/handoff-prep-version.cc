// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-options/boot-options.h>
#include <lib/elfldltl/constants.h>
#include <lib/symbolizer-markup/writer.h>

#include <ktl/ref.h>
#include <ktl/string_view.h>
#include <phys/boot-constants.h>
#include <phys/symbolize.h>

#include "handoff-prep.h"
#include "log.h"

#include <ktl/enforce.h>

namespace {

ktl::string_view SanitizeVersion(ktl::string_view version) {
  constexpr ktl::string_view kSpace = " \t\r\n";
  size_t skip = version.find_first_not_of(kSpace);
  size_t trim = version.find_last_not_of(kSpace);
  if (skip == ktl::string_view::npos || trim == ktl::string_view::npos) {
    ZX_PANIC("version.txt of %zu chars empty after trimming whitespace", version.size());
  }
  trim = version.size() - (trim + 1);
  version.remove_prefix(skip);
  version.remove_suffix(trim);
  if (BootOptions::Get()->phys_verbose) {
    if (skip + trim == 0) {
      printf("%s: zx_system_get_version_string (%zu chars): %.*s\n", ProgramName(), version.size(),
             static_cast<int>(version.size()), version.data());
    } else {
      printf("%s: zx_system_get_version_string (%zu chars trimmed from %zu): %.*s\n", ProgramName(),
             version.size(), version.size() + skip + trim, static_cast<int>(version.size()),
             version.data());
    }
  }
  return version;
}

}  // namespace

void HandoffPrep::SetVersionString(ktl::string_view version) {
  version = SanitizeVersion(version);

  // The markup writer is convenient for all the logging, in fact.
  FILE vlogf = gLog->VerboseOnlyFile();  // NOLINT(misc-non-copyable-objects)
  symbolizer_markup::Writer writer{CallableFile{&vlogf}};

  // Note where in the whole log the kernel_debug_ident log lines start.  The
  // handoff string will be copied from the saved log once it's all assembled.
  const size_t log_initial_size = gLog->size_bytes();

  // First, a summary of identifying information that's easy to read.
  writer.Literal("Zircon/")
      .Literal(elfldltl::ElfMachineFileName(elfldltl::ElfMachine::kNative))
      .Literal(" ELF (")
      .Literal(elfldltl::ElfMachineName(elfldltl::ElfMachine::kNative))
      .Literal(") build ID ")
      .HexString(kernel_.build_id()->desc)
      .Newline()
      .Literal("zx_system_get_version_string: ");
  // Note where the version string itself is going into the ident log text.
  const size_t version_pos = gLog->size_bytes() - log_initial_size;
  writer.Literal(version)
      .Newline()
      .Literal("kernel_debug_level: ")
      .DecimalDigits(abi_spec_->kernel_debug_level)
      .Newline();

  // The kernel_version_ident string is just the prefix before the symbolizer
  // markup.  The full kernel_debug_ident string is for the symbolizing filter.
  const size_t version_ident_size = gLog->size_bytes() - log_initial_size;
  writer.Reset().Newline();
  kernel_.SymbolizerContext(writer, 0);

  // The complete kernel_debug_ident text is contiguous in the log now, so
  // allocate the permanent handoff space for it.
  fbl::AllocChecker ac;
  ktl::string_view snapshot = gLog->BorrowSnapshot().substr(log_initial_size);
  New(boot_constants_->kernel_debug_ident, ac, snapshot);
  if (!ac.check()) {
    ZX_PANIC("cannot allocate %zu chars of handoff space for kernel version and debug info",
             snapshot.size());
  }

  // The two version strings are just substrings of the full debug ident text.
  boot_constants_->kernel_version_ident =  //
      boot_constants_->kernel_debug_ident.substr(0, version_ident_size);
  boot_constants_->system_version_string =
      boot_constants_->kernel_debug_ident.substr(version_pos, version.size());

  // The ELF build ID was already found in the physical kernel ELF image, just
  // translate it to its kernel virtual address.
  boot_constants_->elf_build_id = KernelImageSpan(kernel_.build_id()->desc);
}

// Copyright 2022 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-options/boot-options.h>
#include <lib/elfldltl/self.h>

#include <fbl/no_destructor.h>
#include <ktl/span.h>
#include <phys/elf-image.h>
#include <phys/stack.h>
#include <phys/symbolize.h>

#ifdef __ELF__

// These are defined by the phys.ld linker script.
extern "C" ktl::byte __executable_start[], _edata[], _end[];
extern "C" const ktl::byte __start_note_gnu_build_id[];
extern "C" const ktl::byte __stop_note_gnu_build_id[];

namespace {

void InitSelf(MainSymbolize& main) {
  using Phdr = elfldltl::Elf<>::Phdr;

  auto memory = elfldltl::Self<>::Memory(__executable_start, _end);
  auto bias = elfldltl::Self<>::LoadBias();

  Phdr load_segment = {
      .type = elfldltl::ElfPhdrType::kLoad,
      .vaddr = memory.base(),
      .filesz = _edata - __executable_start,
      .memsz = memory.image().size_bytes(),
  };
  load_segment.flags = Phdr::kRead | Phdr::kWrite | Phdr::kExecute;

  static fbl::NoDestructor<ElfImage> gSelfImage;
  gSelfImage->InitSelf(main.name(), memory, bias, load_segment,
                       {__start_note_gnu_build_id, __stop_note_gnu_build_id});
  main.set_self(gSelfImage.get());

  static constexpr Symbolize::Stack<BootStack> kBootStacks[] = {
      {boot_stack, "boot"},
      {phys_exception_stack, "exception"},
  };
  main.set_stacks(ktl::span(kBootStacks));

#if __has_feature(shadow_call_stack)
  static constexpr Symbolize::Stack<BootShadowCallStack> kBootShadowCallStacks[] = {
      {boot_shadow_call_stack, "boot"},
      {phys_exception_shadow_call_stack, "exception"},
  };
  main.set_shadow_call_stacks(ktl::span(kBootShadowCallStacks));
#endif  // __has_feature(shadow_call_stack)
}

}  // namespace

#else  // !__ELF__

namespace {
void InitSelf(MainSymbolize& main) {}
}  // namespace

#endif  // __ELF__

void MainSymbolize::set_self(const ElfImage* self) {
  ReplaceModulesStorage(ModuleList{ktl::span{&self_, 1}});
  OnLoad(*self);
  set_main_module(*self);
}

MainSymbolize::MainSymbolize(const char* name, FILE* f) : Symbolize(name, f) {
  gSymbolize = this;

  InitSelf(*this);

  if (!BootOptions::Get() || BootOptions::Get()->phys_verbose) {
    Context();
  }
}

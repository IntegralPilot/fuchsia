// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_BOOTSTRAP_H_
#define LIB_LD_BOOTSTRAP_H_

#include <lib/elfldltl/diagnostics.h>
#include <lib/elfldltl/memory.h>
#include <lib/elfldltl/note.h>
#include <lib/elfldltl/phdr.h>
#include <lib/elfldltl/self.h>
#include <lib/elfldltl/static-pie-with-vdso.h>
#include <lib/elfldltl/symbol.h>
#include <lib/ld/load.h>
#include <lib/ld/module.h>

#include <array>
#include <concepts>
#include <cstddef>
#include <cstdint>
#include <tuple>

namespace ld {

// The ld::Bootstrap constructor's page_size argument can be an integer if the
// page size is available immediately, i.e. before dynamic linking.
template <typename PageSizeT>
concept BootstrapEarlyPageSize = std::convertible_to<PageSizeT, size_t>;

// The ld::Bootstrap constructor's page_size argument can instead be callable
// as size_t() that will be called only after dynamic linking.
template <typename PageSizeT>
concept BootstrapPageSize =
    BootstrapEarlyPageSize<PageSizeT> || std::is_invocable_r_v<size_t, PageSizeT>;

// Just constructing the ld::Bootstrap object does all the bootstrapping work
// for a startup dynamic linker itself or for any static PIE.  It's expected to
// be called first thing, before any code that might rely on dynamic linking,
// including any calls into the vDSO as well as just any variables (even if
// constinit and/or const) with dynamic relocations in their initializer data.
//
// The diagnostics object will be used for assertion failures in case messages
// can be printed, but it's not really expected to return from FormatError et
// al and various kinds of failures might get crashes if FormatError ever
// returns.  It should be an object from TrapDiagnostics() or similar.
//
// This mostly initializes the Module data structures for the preloaded
// modules: this program itself, and the vDSO.  It fills out only the name,
// symbols, and vaddr bounds in each Module.  The caller supplies the two
// objects to fill.  Each must be in LinkerZeroInitialized() state as regards
// those members filled by Bootstrap, but other members may be set already.
//
// The vdso_module() describes the vDSO, which is already fully loaded
// according to its PT_LOAD segments, relocated and initialized in place as we
// find it.  The ELF image as loaded is presumed valid.
//
// The constructor does all this program's own dynamic linking for simple
// fixups and for symbolic references resolved in the vdso_module().  The
// program's own ELF image is presumed valid and its PT_LOAD segments correctly
// loaded; the diagnostics object is used for assertion failures, but not
// expected to return after errors.  The self_module() describes this program
// itself, already loaded and now fully relocated; RELRO pages remain writable.
//
// Additional observer arguments for elfldltl::DecodePhdr can be passed to the
// constructor.  These can collect e.g. PT_TLS and PT_GNU_RELRO from a static
// PIE.  The startup dynamic linker doesn't need those.

class Bootstrap {
 public:
  using Elf = elfldltl::Elf<>;
  using Ehdr = Elf::Ehdr;
  using Phdr = Elf::Phdr;
  using Dyn = Elf::Dyn;
  using Addr = Elf::Addr;
  using size_type = Elf::size_type;
  using Module = abi::Abi<>::Module;

  // For the startup dynamic linker's convenience, the correctly-bounded Dyn
  // array is returned along with the filled-out Module.
  struct Preloaded {
    Module& module;
    std::span<const Dyn> dyn;
  };
  using PreloadedList = std::array<Preloaded, 2>;

  template <BootstrapPageSize PageSizeT, class... PhdrObservers, class... DynamicObservers>
  Bootstrap(auto& diag, const void* vdso_base,
            // The page size now, or how to get it after dynamic linking.
            PageSizeT&& get_page_size,
            // The caller supplies storage for the two preloaded modules.
            Module& self_module_storage, Module& vdso_module_storage,
            // Optional additional self-phdr and self-dynamic observers.
            std::tuple<PhdrObservers...> phdr_observers = {},
            std::tuple<DynamicObservers...> dynamic_observers = {})
      : preloaded_{
            Preloaded{.module = self_module_storage},
            Preloaded{.module = vdso_module_storage},
        } {
    self_module_storage.InitLinkerZeroInitialized();
    vdso_module_storage.InitLinkerZeroInitialized();

    // If the page-size argument is an integer, the true size is already known.
    // Otherwise, it's a callback that cannot be made until after relocation.
    constexpr bool kHaveEarlyPageSize = BootstrapEarlyPageSize<PageSizeT>;
    if constexpr (kHaveEarlyPageSize) {
      page_size_ = get_page_size;
    }

    // First collect information from the vDSO.  If there is none, then there
    // will be empty symbols to link against and no references can resolve to
    // any vDSO-defined symbols.  This on1y happens on POSIX, never on Fuchsia.
    if (kAlwaysHaveVdso || vdso_base) [[likely]] {
      InitVdso(diag, vdso_base, vdso_module());
    }

    // Now collect information from this executable itself and do the linking.
    LinkSelf(diag, std::move(phdr_observers), std::move(dynamic_observers));

    if constexpr (!kHaveEarlyPageSize) {
      // Now that relocation is done, the true page size can be determined.
      page_size_ = get_page_size();
      ReifyVaddrBounds(self_module());
      ReifyVaddrBounds(vdso_module());
    }
  }

  PreloadedList& preloaded() { return preloaded_; }

  Module& self_module() { return preloaded_.front().module; }
  const Module& self_module() const { return preloaded_.front().module; }

  Module& vdso_module() { return preloaded_.back().module; }
  const Module& vdso_module() const { return preloaded_.back().module; }

  size_t page_size() const { return page_size_; }

  // Call f(start, size) for each whole-page region in self_module() containing
  // writable PT_LOAD segment data.
  void OnWritablePages(std::invocable<uintptr_t, size_t> auto&& f) const
    requires(std::is_void_v<std::invoke_result_t<decltype(f), uintptr_t, size_t>>)
  {
    const Module& self = self_module();
    uintptr_t pending_start = self.vaddr_start;
    uintptr_t pending_end = self.vaddr_start;

    auto report_pending = [&f, &pending_start, &pending_end]() {
      if (pending_end > pending_start) {
        const size_t size = pending_end - pending_start;
        f(pending_start, size);
      }
    };

    for (const auto& phdr : self.phdrs.get()) {
      if (phdr.type == elfldltl::ElfPhdrType::kLoad && (phdr.flags & Phdr::kWrite)) {
        // This is a writable segment.  Compute its page-rounded bounds.
        uintptr_t vaddr = phdr.vaddr + self.link_map.addr;
        uintptr_t start = vaddr & -page_size();
        uintptr_t end = (vaddr + phdr.memsz + page_size() - 1) & -page_size();

        // If this is contiguous with the pending segment, just extend it.
        if (start == pending_end) {
          pending_end = end;
          continue;
        }

        // Call f on the pending segment (if any).
        report_pending();

        // Collect this segment as pending for the next iteration.
        pending_start = start;
        pending_end = end;
      }
    }

    report_pending();
  }

 private:
  static void FillModule(Module& module, std::span<const Dyn> dyn, size_t vaddr_start,
                         size_t vaddr_size, size_t bias, std::span<const Phdr> phdrs) {
    module.link_map.addr = bias;
    module.link_map.ld = dyn.data();
    module.vaddr_start = vaddr_start;
    module.vaddr_end = vaddr_start + vaddr_size;
    module.phdrs = phdrs;
    module.soname = module.symbols.soname();
  }

  static std::span<const Dyn> ReadDyn(elfldltl::DirectMemory& memory, const Phdr& phdr) {
    return *memory.ReadArray<Dyn>(phdr.vaddr, phdr.memsz);
  }

  void InitVdso(auto& diag, const void* vdso_base, Module& vdso_module) {
    auto& vdso_dyn = preloaded_.back().dyn;

    // The vDSO base address comes without an immediately known upper bound on
    // the valid range of mapped image.  Create a span that's the most it could
    // possibly be and still fit it entirely into the address space while not
    // allowing .data() + .size() to wrap around.
    constexpr auto unbounded_span = [](std::byte* base) {
      constexpr uintptr_t end = std::numeric_limits<uintptr_t>::max();
      uintptr_t begin = reinterpret_cast<uintptr_t>(base);
      return std::span{base, end - begin};
    };

    // The vDSO's static vaddr_start is presumed to be zero.
    auto* vdso_bytes = static_cast<std::byte*>(const_cast<void*>(vdso_base));
    elfldltl::DirectMemory memory(unbounded_span(vdso_bytes), 0);
    const Ehdr& ehdr = *memory.ReadFromFile<Ehdr>(0);
    const std::span phdrs =
        *memory.ReadArrayFromFile<Phdr>(ehdr.phoff, elfldltl::NoArrayFromFile<Phdr>{}, ehdr.phnum);

    size_type vaddr_start, vaddr_size;
    std::optional<Phdr> dyn_phdr;
    elfldltl::DecodePhdrs(diag, phdrs,
                          elfldltl::PhdrLoadObserver<Elf>(page_size_, vaddr_start, vaddr_size),
                          elfldltl::PhdrDynamicObserver<Elf>(dyn_phdr),
                          PhdrMemoryBuildIdObserver(memory, vdso_module));

    vdso_dyn = ReadDyn(memory, *dyn_phdr);
    elfldltl::DecodeDynamic(diag, memory, vdso_dyn,
                            elfldltl::DynamicSymbolInfoObserver(vdso_module.symbols));

    size_type bias = reinterpret_cast<uintptr_t>(vdso_base) - vaddr_start;
    FillModule(vdso_module, vdso_dyn, vaddr_start, vaddr_size, bias, phdrs);
    vdso_module.link_map.name = vdso_module.soname.c_str();
  }

  template <class... PhdrObservers, class... DynamicObservers>
  void LinkSelf(auto& diag, std::tuple<PhdrObservers...> phdr_observers,
                std::tuple<DynamicObservers...> dynamic_observers) {
    auto& self_dyn = preloaded_.front().dyn;

    auto memory = elfldltl::Self<>::Memory();
    const std::span phdrs = elfldltl::Self<>::Phdrs();
    const uintptr_t bias = elfldltl::Self<>::LoadBias();
    const uintptr_t start = memory.base() + bias;

    std::optional<Phdr> dyn_phdr;
    auto decode_phdrs = [this, &diag, &phdrs, &dyn_phdr, &memory](  //
                            PhdrObservers&... phdr_observers) {
      elfldltl::DecodePhdrs(diag, phdrs, elfldltl::PhdrDynamicObserver<Elf>(dyn_phdr),
                            PhdrMemoryBuildIdObserver(memory, self_module()), phdr_observers...);
    };
    std::apply(decode_phdrs, phdr_observers);

    auto link = [this, &diag](DynamicObservers&... dynamic_observers) {
      self_module().symbols = elfldltl::LinkStaticPieWithVdso(  //
          elfldltl::Self<>(), diag, vdso_module().symbols, vdso_module().link_map.addr,
          dynamic_observers...);
    };
    std::apply(link, dynamic_observers);

    self_dyn = elfldltl::Self<>::Dynamic();
    assert(self_dyn.data() == ReadDyn(memory, *dyn_phdr).data());
    self_dyn = self_dyn.subspan(0, dyn_phdr->memsz / sizeof(Dyn));
    FillModule(self_module(), self_dyn, start, memory.image().size(), bias, phdrs);
    self_module().link_map.name = abi::Abi<>::kExecutableName.c_str();
  }

  // This is called only when the module was originally set up without knowing
  // the page size.  Page-round the bounds correctly now.
  void ReifyVaddrBounds(Module& module) const {
    module.vaddr_start = module.vaddr_start & -page_size_;
    module.vaddr_end = (module.vaddr_end + page_size_ - 1) & -page_size_;
  }

#ifdef __Fuchsia__
  static constexpr bool kAlwaysHaveVdso = true;
#else
  static constexpr bool kAlwaysHaveVdso = false;
#endif

  PreloadedList preloaded_;
  std::span<const Elf::Addr> preinit_array_;
  size_t page_size_ = 1;
};

}  // namespace ld

#endif  // LIB_LD_BOOTSTRAP_H_

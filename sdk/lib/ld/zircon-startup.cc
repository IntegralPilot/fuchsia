// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/phdr.h>
#include <lib/elfldltl/vmo.h>
#include <lib/elfldltl/zircon.h>
#include <lib/ld/fuchsia-debugdata.h>
#include <lib/ld/vmar.h>
#include <lib/llvm-profdata/llvm-profdata.h>
#include <lib/trivial-allocator/new.h>
#include <lib/trivial-allocator/zircon.h>
#include <lib/zx/channel.h>
#include <lib/zx/eventpair.h>
#include <lib/zx/vmo.h>
#include <zircon/compiler.h>
#include <zircon/syscalls.h>

#include <optional>
#include <string_view>
#include <utility>

#include "allocator.h"
#include "startup-bootstrap.h"
#include "startup-diagnostics.h"
#include "zircon.h"

namespace ld {
namespace {

using VmoFile = elfldltl::VmoFile<Diagnostics>;

// For the initial-exec allocator, always get individual pages and populate
// page tables eagerly for each page as it's mapped.  The page will always be
// written immediately after it's allocated, so faulting it in is just slower.
auto MakeStartupInitialExecAllocator(StartupData& startup) {
  using PageMemory = trivial_allocator::ZirconVmar<1, ZX_VM_MAP_RANGE>;
  return MakeInitialExecAllocator(PageMemory{startup.vmar});
}

using InitialExecAllocator =
    decltype(MakeStartupInitialExecAllocator(std::declval<StartupData&>()));

// For the scratch allocator, space will be allocated in the reservation VMAR.
// It's going to be destroyed as a whole at the end, so allocations in it can
// be completely leaked.  Use a large minimum allocation granularity (2MiB) so
// that additional mapping syscalls are rare, but let each actual page be
// faulted in as needed so no extra RAM pages are taken from the system.
auto MakeStartupScratchAllocator(const VmarReservation& reserve_vmar) {
  constexpr size_t kScratchPageSize = 2 << 20;
  return trivial_allocator::BasicLeakyAllocator{trivial_allocator::PageAllocator{
      trivial_allocator::ZirconVmar<kScratchPageSize>{reserve_vmar.vmar()}}};
}

using ScratchAllocator =
    decltype(MakeStartupScratchAllocator(std::declval<const VmarReservation&>()));

// The reservation VMAR needs to allow mappings for scratch use.
constexpr zx_vm_option_t kReserveVmarFlags = ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE;

struct LoadExecutableResult : public StartupLoadResult {
  StartupModule* module = nullptr;
  std::string_view interp;
};

LoadExecutableResult LoadExecutable(Diagnostics& diag, StartupData& startup,
                                    ScratchAllocator& scratch, InitialExecAllocator& initial_exec,
                                    zx::vmo vmo) {
  LoadExecutableResult result = {
      .module = StartupModule::New(diag, scratch, abi::Abi<>::kExecutableName, startup.vmar),
  };
  if (!vmo) [[unlikely]] {
    diag.SystemError("no executable VMO in bootstrap message");
  } else {
    elfldltl::UnownedVmoFile file{vmo.borrow(), diag};
    Elf::size_type max_tls_modid = 0;
    std::optional<Elf::Phdr> interp;
    static_cast<StartupLoadResult&>(result) = result.module->Load(
        diag, initial_exec, file, 0, max_tls_modid, elfldltl::PhdrInterpObserver<Elf>(interp));
    assert(max_tls_modid <= 1);

    // Extract the PT_INTERP string.
    if (interp) [[likely]] {
      if (auto interp_cstr = result.module->memory().ReadArray<char>(  //
              interp->vaddr, interp->filesz)) [[likely]] {
        if (interp_cstr->back() == '\0') [[likely]] {
          result.interp = {interp_cstr->data(), interp_cstr->size() - 1};
        }
      }
    }
  }
  return result;
}

[[maybe_unused]] void ProtectData(Diagnostics& diag, const StartupBootstrap& bootstrap,
                                  zx::vmar self) {
  bootstrap.OnWritablePages([&diag, self = std::move(self)](uintptr_t start, size_t size) {
    zx_status_t status = self.protect(ZX_VM_PERM_READ, start, size);
    if (status != ZX_OK) [[unlikely]] {
      diag.SystemError("cannot protect dynamic linker data pages", elfldltl::ZirconError{status});
    }
  });
}

ld::Debugdata::Deferred PublishProfdata(Diagnostics& diag, zx::unowned_vmar vmar,
                                        std::span<const std::byte> build_id) {
#if HAVE_LLVM_PROFDATA
  auto error = [&diag](zx_status_t status, auto&&... args) -> ld::Debugdata::Deferred {
    diag.SystemError(std::forward<decltype(args)>(args)..., elfldltl::ZirconError{status});
    return {};
  };

  LlvmProfdata profdata;
  profdata.Init(build_id);
  const size_t size = profdata.size_bytes();
  if (size != 0) {
    // Make a VMO and map it in to hold the profdata.
    zx::vmo vmo;
    zx_status_t status = zx::vmo::create(size, 0, &vmo);
    if (status != ZX_OK) {
      return error(status, "cannot create llvm-profdata VMO of ", size, " bytes");
    }
    uintptr_t ptr;
    status = vmar->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0, size, &ptr);
    if (status != ZX_OK) {
      return error(status, "cannot map llvm-profdata VMO of ", size, " bytes");
    }
    std::span vmo_data{reinterpret_cast<std::byte*>(ptr), size};

    // Now fill the VMO and redirect the instrumentation to update its data.
    auto live_data = profdata.WriteFixedData(vmo_data);
    profdata.CopyLiveData(live_data);
    LlvmProfdata::UseLiveData(live_data);

    // At this point the instrumentation will no longer touch the data segment.
    ld::Debugdata debugdata{LlvmProfdata::kDataSinkName, std::move(vmo)};
    zx::result deferred = std::move(debugdata).DeferredPublish();
    if (deferred.is_error()) {
      return error(status, "cannot publish ", LlvmProfdata::kDataSinkName, " to fuchsia.debugdata");
    }

    return *std::move(deferred);
  }
#endif
  return {};
}

VmarReservation CreateReservationVmar(Diagnostics& diag, zx::unowned_vmar vmar, size_t page_size) {
  zx_info_vmar_t info;
  zx_status_t status = vmar->get_info(ZX_INFO_VMAR, &info, sizeof(info), nullptr, nullptr);
  if (status != ZX_OK) [[unlikely]] {
    diag.SystemError("cannot get zx_info_vmar_t for root VMAR: ", elfldltl::ZirconError{status});
    return {};
  }

  zx_info_vmar_t bottom = VmarBottomHalf(info, page_size);

  VmarReservation reservation;
  auto result = reservation.Init(vmar->borrow(), info, bottom, kReserveVmarFlags);
  if (result.is_error()) [[unlikely]] {
    diag.SystemError("cannot reserve lower half [", bottom.base, ", ", bottom.base + bottom.len,
                     ") of root VMAR [", info.base, ", ", info.base + info.len,
                     "): ", elfldltl::ZirconError{result.status_value()});
    return {};
  }

  return reservation;
}

}  // namespace

// The _start assembly code saves the two argument registers passed by
// zx_process_start, as well as passing them through to StartLd.  StartLd
// returns this to give it the user entry point address to hand off to.  It
// unwinds the stack to starting conditions and hands off with the same two
// original argument register values, and the third argument register here.
struct StartLdResult {
  uintptr_t entry, third_argument;
};

extern "C" StartLdResult StartLd(zx_handle_t handle, void* vdso) {
  // First thing, bootstrap our own dynamic linking against ourselves and the
  // vDSO.  For this, nothing should go wrong so use a diagnostics object that
  // crashes the process at the first error.  Before linking against the vDSO
  // is completed successfully, there's no way to make a system call to get an
  // error out anyway.
  auto bootstrap_diag = elfldltl::TrapDiagnostics();
  StartupBootstrap bootstrap{bootstrap_diag, vdso,
                             []() {  // This can only be used after relocation.
                               return zx_system_get_page_size();
                             }};

  // Read the bootstrap message.
  StartupData startup = ReadBootstrap(zx::unowned_channel{handle});

  // Now that things are bootstrapped, set up the main diagnostics object.
  Diagnostics diag{startup};

  // Reserve the bottom half of the root VMAR to force all allocations and
  // loaded modules into the top half of the address space. This helps
  // facilitate large allocations to be placed into the bottom half of the
  // address space for things like sanitizers.
  //
  // The reserve_vmar object's destructor will automatically remove this
  // reservation (by destroying the child VMAR) when it goes out of scope at
  // the end of StartLd, just before returning to start the executable,
  // ensuring libc has free access to the bottom half of the address space.
  VmarReservation reserve_vmar =
      CreateReservationVmar(diag, startup.vmar.borrow(), bootstrap.page_size());
  CheckErrors(diag);

  // Start publishing profiling data in an instrumented build.  Before this,
  // the instrumentation is updating counters in the data segment.  After this,
  // it's updating a VMO mapped elsewhere.  That VMO remains mapped after
  // startup just to avoid bothering with code to unmap it since that code and
  // the rest of the return path would have to be uninstrumented.  When the
  // debugdata.vmo_token handle is closed by going out of scope at the end of
  // startup, this will signal the data receiver that the VMO's data is ready.
  // It's still possible for either the last bit of instrumented code in the
  // startup path, or just stray pointer writes in the process after startup
  // will modify it, but will be ignored or will be tolerable noise in the
  // data.  The debugdata.svc_server_end handle is returned to be passed on to
  // the user entry point as its third argument.  Eventually, the user's libc
  // will pass this to ld::Debugdata::Forward.
  ld::Debugdata::Deferred debugdata =
      PublishProfdata(diag, startup.vmar.borrow(), bootstrap.self_module().build_id);

  // Set up the allocators.  These objects hold zx::unowned_vmar copies but do
  // not own the VMAR handle.  The scratch allocator takes advantage of the
  // reserve_vmar so no additional bookkeeping is required to reclaim it all
  // when the object goes out of scope and the whole VMAR is destroyed.
  auto scratch = MakeStartupScratchAllocator(reserve_vmar);
  auto initial_exec = MakeStartupInitialExecAllocator(startup);

  // Load the main executable.
  LoadExecutableResult main =
      LoadExecutable(diag, startup, scratch, initial_exec, std::move(startup.executable_vmo));
  mutable_abi.preinit_array = main.preinit_array;
  mutable_abi.stack_size = main.stack_size.value_or(0);

  // It doesn't matter whether or how this dynamic linker binary itself is
  // instrumented.  It's only the executable's instrumentation details that
  // affect what DT_NEEDED libraries it wants to select.  This is indicated by
  // a PT_INTERP that usually selects the dynamic linker binary to use with
  // that executable, but the actual dynamic linker binary itself may or may
  // not be instrumented in similar ways as the executable and what it expects
  // of its DT_NEEDED dependencies.
  //
  // If the PT_INTERP has a prefix before a slash and the standard PT_INTERP
  // string, then the prefix is a fuchsia.ldsvc.Loader/Config argument to send.
  constexpr size_t kSuffixLen = abi::kInterp.size() + 1;
  if (main.interp.size() > kSuffixLen && main.interp.ends_with(abi::kInterp) &&
      main.interp[main.interp.size() - kSuffixLen] == '/') {
    std::string_view config = main.interp;
    config.remove_suffix(kSuffixLen);
    startup.ConfigLdsvc(diag, config);
  }

  auto get_vmo_file = [&diag,
                       &startup](const elfldltl::Soname<>& soname) -> std::optional<VmoFile> {
    if (zx::vmo vmo = startup.GetLibraryVmo(diag, soname.c_str())) {
      return VmoFile{std::move(vmo), diag};
    }
    return {};
  };

  StartupModule::LinkModules(diag, scratch, initial_exec, main.module, get_vmo_file, bootstrap,
                             main.needed_count, startup.vmar);

  // Bail out before relocation if there were any loading errors.
  CheckErrors(diag);

  if constexpr (kProtectData) {
    // Now that startup is completed, protect not only the RELRO, but also all
    // the data and bss.  Then drop that VMAR handle so the protections cannot
    // be changed again.
    ProtectData(diag, bootstrap, std::move(startup.self_vmar));
  }

  // Bail out before handoff if any errors have been detected.
  CheckErrors(diag);

  return {
      .entry = main.entry,
      // The two arguments to StartLd are the first two arguments to the user
      // entry point, and this is the third.
      .third_argument = debugdata.svc_server_end.release(),
  };
}

}  // namespace ld

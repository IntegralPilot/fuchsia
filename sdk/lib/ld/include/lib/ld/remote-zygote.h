// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_REMOTE_ZYGOTE_H_
#define LIB_LD_REMOTE_ZYGOTE_H_

#include <vector>

#include "remote-dynamic-linker.h"

namespace ld {

// ld:RemoteZygote distills the work done by a zygote-mode dynamic linking
// session into simpler state that can be used to stamp out the same set of
// fully-relocated modules at the same addresses in new processes repeatedly.
//
// ld:RemoteZygote::Linker is an alias for zygote-mode ld::RemoteDynamicLinker,
// as defined in <lib/ld/remote-dynamic-linker.h>.  That object performs the
// dynamic linking for the first process, using the VMAR provided to the
// Linker::Allocate() method to have the system choose load addresses with
// ASLR.  Linker::Commit() should be completed before the Linker is moved into
// the zygote using ld::RemoteZygote::Insert().
//
// Each ld::RemoteZygote::Insert() call consumes a Linker object to distill it
// into the zygote, and yields a ld::RemoteZygote::Domain object that gives the
// entry point address and stack size for that dynamic linking session's main
// (first) root module (usually the main executable), along with the passive
// ABI root symbol addresses for that session's namespace.
//
// Multiple remote dynamic linking sessions meant to share an address space can
// be merged into a single zygote with multiple Insert() calls.  Separate
// zygote objects can also be combined using the += or + operators.  This
// simplifes repeated loading of an identical whole constellation of dynamic
// linking namespaces sharing a process address space.
//
// Separate ld::RemoteZygote objects can also be used to load multiple sessions
// into the same process.  For example, a driver host zygote can be kept
// separate from particular driver zygotes (for individual drivers, or merged
// zygotes for multiple related drivers); thus the same driver host zygote
// could be loaded alongside disparate driver zygotes in different processes.

// ld::RemoteZygoteVmo is the type of the template parameter to
// ld::RemoteZygote that says how to "own" original file VMO handles.
//
// The ld::RemoteDynamicLinker object owns segment VMOs for relocated (and
// partial-page zeroed) segments, which are transferred into the zygote by
// Insert().  But it doesn't directly own the VMO handles for each module's
// whole original ELF file (read-only, executable handles to VMOs ultimately
// owned by the filesystem).  Those are owned indirectly by reference-counted
// ld::RemoteDecodedModule objects the ld::RemoteDynamicLinker::modules() list
// points to.
//
// The zygote needs access to these VMO handles, but it doesn't need to own
// them.  On the other hand, it doesn't need any other information from the
// ld::RemoteDecodedModule object.  So there are two options for what the
// zygote will own:
//
//  * a zx::vmo for the VMO handle
//
//  * a Linker::Module::Decoded::Ptr for the module that owns the VMO
//
// The size of the ld::RemoteZygote object is about the same either way, but a
// Decoded::Ptr references a second object (which itself also owns a read-only
// mapping of the whole ELF file into the current address space).  If there are
// other references to these ld::RemoteDecodedModule objects anyway in a cache
// of decoded modules reused for new dynamic linking sessions, then holding
// another reference in the zygote saves the overhead of duplicating the VMO
// handle.  Doing that caching is probably preferable to recreating new
// ld::RemoteDecodedModule objects (and their whole-file mappings) for the same
// underlying file VMO later.  It has nontrivial setup overhead that can be
// amortized; while the memory overhead to maintain it is fairly modest when
// the original file VMO itself is being kept alive anyway.
//
// However, if no further dynamic linking sessions will reuse the same file
// VMOs, then that modest memory overhead can be eliminated by dropping all
// references to the ld::RemoteDecodedModule after duplicating its VMO handle.
//
// ld::RemoteZygoteVmo::kDecodedPtr says to store the reference, while
// ld::RemoteZygoteVmo::kVmo says to store just the zx::vmo handle.
//
// A zygote of either type can be spliced into a zygote that uses zx::vmo.
enum class RemoteZygoteVmo : bool { kVmo = false, kDecodedPtr = true };

// This is the type returned by ld::RemoteZygote::Insert() on success.  It's
// usually used via the ld::RemoteZygote::Domain alias (see below).  Each time
// an ld::RuntimeDynamicLinker object is inserted into a zygote, Insert()
// returns this object to describe that dynamic linking domain.  This provides
// the absolute runtime addresses for the root module's entry point and for the
// root passive ABI data structures; and the stack size request from the root
// module.  These are all the details usually needed either to launch a main
// executable, or to assimilate a new linking domain via its passive ABI and/or
// via some custom entry-point protocol for the root module.
template <class Elf = elfldltl::Elf<>>
class RemoteZygoteDomain {
 public:
  using ExecInfo = RemoteDecodedModule<Elf>::ExecInfo;
  using size_type = Elf::size_type;

  // Return the absolute runtime PC of the root module's entry point.
  // For the main executable, this is the PC to pass to zx_process_start.
  size_type main_entry() const { return exec_info_.relative_entry; }

  // Return the root module (main executable)'s requested stack size, if any.
  std::optional<size_type> main_stack_size() const { return exec_info_.stack_size; }

  // Return the absolute runtime addresses of the passive ABI symbols in the
  // loaded stub dynamic linker image.
  size_type abi_vaddr() const { return abi_vaddr_; }
  size_type rdebug_vaddr() const { return rdebug_vaddr_; }

 private:
  template <RemoteZygoteVmo, class>
  friend class RemoteZygote;

  ExecInfo exec_info_;
  size_type abi_vaddr_ = 0, rdebug_vaddr_ = 0;
};

// The zygote is default-constructible and movable, but not copyable.
// Underneath it just holds a std::vector, so it's cheaper to just move it
// around or leave an unused default-constructed one around than to use
// something like std::unique_ptr<ld::RemoteZygote>.
template <RemoteZygoteVmo Vmo = RemoteZygoteVmo::kVmo, class Elf = elfldltl::Elf<>>
class RemoteZygote {
 public:
  using Linker = RemoteDynamicLinker<Elf, RemoteLoadZygote::kYes>;
  using Loader = Linker::Module::Loader;
  using LoadInfo = Linker::Module::LoadInfo;
  using ExecInfo = Linker::Module::ExecInfo;
  using size_type = Elf::size_type;

  // This is returned by Insert().
  using Domain = RemoteZygoteDomain<Elf>;

  RemoteZygote() = default;
  RemoteZygote(RemoteZygote&&) = default;
  RemoteZygote& operator=(RemoteZygote&&) = default;

  // Two zygotes can be spliced together by consuming the other one and moving
  // all its modules into *this, which is the same as if all the Insert()
  // options done on other had been done on *this instead.
  RemoteZygote& operator+=(RemoteZygote other) {
    auto first = std::make_move_iterator(other.modules_.begin());
    auto last = std::make_move_iterator(other.modules_.end());
    modules_.insert(modules_.end(), first, last);
    return *this;
  }

  // std::move(zygote1) + std::move(zygote2) + ...
  RemoteZygote operator+(RemoteZygote other) && {
    *this += std::move(other);
    return std::move(*this);
  }

  // A default-constructed zygote is empty and does not take much space.
  bool empty() const { return modules_.empty(); }

  // This does the same as `*this = {};`.
  void clear() { modules_.clear(); }

  // This consumes a Linker, distilling its dynamic linking session into the
  // zygote and a Domain object.  The Linker should already have been used to
  // Load and Commit in the initial process whose VMARs were used to do the
  // address layout for the zygote.  (The state of that process no longer
  // matters.  No handles to its VMARs are held after Commit.)  Processes
  // loaded by the new zygote will start out identical to that process and
  // share both its VMOs for read-only segments (including RELRO) and its
  // parent VMOs (and their unmodified pages) for relocated data.
  //
  // This can be used multiple times to add additional dynamic linking domains
  // from separate dynamic linking sessions.  That modifies the zygote so that
  // a process loaded from it loads both sessions' modules, including each
  // session's own stub dynamic linker providing its own passive ABI view.
  // Inserting multiple sessions into one zygote and then calling Load() is no
  // different from creating separate zygote objects for each session and
  // calling Load() on all of them.
  //
  // The only error return is for a failure to duplicate a VMO handle.  If
  // RemoteZygoteVmo::kDecodedPtr is used, then no error is possible.  The
  // returned Domain object gives the entry point and stack size request for
  // the main module, which is the first root module in the dynamic linker
  // session, such as the main executable; and the passive ABI root addresses.
  zx::result<Domain> Insert(Linker linker) {
    // Extract the main-module and passive ABI bits from the linker to return.
    zx::result<Domain> result = zx::ok(Domain{});
    result->exec_info_ = linker.main_module().decoded().exec_info();
    result->exec_info_.relative_entry += linker.main_module().load_bias();
    result->abi_vaddr_ = linker.abi_vaddr();
    result->rdebug_vaddr_ = linker.rdebug_vaddr();

    // Reduce each Linker::Module to a ZygoteModule.
    modules_.reserve(linker.modules().size());
    for (auto& module : linker.modules()) {
      if (module.preloaded()) {
        // A module that was preloaded in the linker is treated as preloaded by
        // the zygote too: there is nothing to do to load it.  This does the
        // right thing when distilling a zygote from a secondary dynamic
        // linking session: the modules from the first dynamic linking session
        // should already be in the zygote; or, equivalently, in a separate
        // zygote that will be loaded along with this one.
        continue;
      }

      // All we need from the decoded module now is its VMO handle for the
      // original file.  Either duplicate that handle now, or just hold a
      // reference to the DecodedModule that owns it.
      auto vmo = HoldVmo(module.decoded_module());
      if (vmo.is_error()) {
        return vmo.take_error();
      }
      modules_.emplace_back(std::move(module), *std::move(vmo));
    }

    return result;
  }

  // If this is a RemoteZygoteVmo::kVmo object, then another zygote object of
  // the RemoteZygoteVmo::kDecodedPtr flavor can also be spliced in.  This
  // always consumes that object and will drop all its ld::RemoteDecodedModule
  // references.  It can fail when duplicating the VMO handles owned by them.
  zx::result<> Splice(RemoteZygote<RemoteZygoteVmo::kDecodedPtr, Elf> other) {
    if constexpr (Vmo == RemoteZygoteVmo::kDecodedPtr) {
      // When the types match, splicing cannot fail so += supports it.
      *this += std::move(other);
    } else {
      // Convert DecodedPtr modules to zx::vmo modules.
      modules_.reserve(modules_.size() + other.modules_.size());
      for (auto& module : other.modules_) {
        auto vmo = HoldVmo(module.vmo_holder());
        if (vmo.is_error()) {
          return vmo.take_error();
        }
        modules_.emplace_back(std::move(module), *std::move(vmo));
      }
    }
    return zx::ok();
  }

  // Clone the prototype process into another new process.  The VMAR must be
  // some process's root VMAR or a sub-VMAR covering the same address space
  // bounds as the prototype process VMAR passed to Linker::Allocate.
  //
  // This is non-destructive to the RemoteZygote object, and no references to
  // this object are made so its lifetime has no effect on the target process.
  // There is no separate Commit phase.  When this returns false after an error
  // in loading, any sub-VMARs already created will have been destroyed.
  //
  // On success, a closed VMAR for each module has all its mappings in place;
  // they cannot be modified.  While zx_vmar_destroy cannot be used since there
  // is no handle to them, it's always possible to destroy a whole VMAR as an
  // atomic unit with a zx_vmar_unmap call on a handle to a containing VMAR.
  //
  // It's not advised to use a Diagnostics object that says to keep going for
  // errors in this call.  If the Diagnostics object does say to keep going,
  // this call will fail and return early for some errors but for others it
  // will keep going as directed and if it returned true after errors it may
  // have left partial mappings in the process.
  template <class Diagnostics>
  bool Load(Diagnostics& diag, zx::unowned_vmar vmar) const {
    // Find the base of the VMAR to calculate offsets from absolute addresses.
    zx_vaddr_t vmar_base = 0;
    if (auto result = GetVmarBase(*vmar); result.is_ok()) {
      vmar_base = *result;
    } else {
      return diag.SystemError("cannot get ZX_INFO_VMAR: ",
                              elfldltl::ZirconError{result.error_value()});
    }

    // Collect all the loaders so none gets committed before all are loaded.
    std::vector<Loader> loaders;
    loaders.reserve(modules_.size());
    for (const ZygoteModule& module : modules_) {
      if (!module.Load(diag, loaders.emplace_back(*vmar), vmar_base)) {
        return false;
      }
    }

    // Now that all modules have been loaded, commit them all.
    // Any return before this point destroys all the new VMARs.
    for (Loader& loader : loaders) {
      // There is no RELRO protection change to be done in the remote case, so
      // the Relro object returned can be safely destroyed to close the only
      // handle to each module's VMAR and prevent future mapping changes.
      constexpr typename LoadInfo::Region kNoRelro{};
      std::ignore = std::move(loader).Commit(kNoRelro);
    }

    return true;
  }

 private:
  friend RemoteZygote<RemoteZygoteVmo::kVmo, Elf>;

  using LinkerModule = Linker::Module;
  using DecodedPtr = LinkerModule::Decoded::Ptr;

  // Each module holds a reference to a zx::vmo somehow.  Either it holds a
  // zx::vmo directly or it holds a DecodedPtr whose ->vmo() can be used.
  using VmoHolder = std::conditional_t<Vmo == RemoteZygoteVmo::kVmo, zx::vmo, DecodedPtr>;

  class ZygoteModule {
   public:
    ZygoteModule() = default;

    ZygoteModule(ZygoteModule&&) = default;

    // This consumes the module by stealing its segment VMOs.
    ZygoteModule(LinkerModule&& module, VmoHolder vmo_holder)
        : vmo_holder_(std::move(vmo_holder)),
          load_bias_{module.load_bias()},
          load_info_{std::move(module.load_info())} {}

    ZygoteModule(typename RemoteZygote<RemoteZygoteVmo::kDecodedPtr, Elf>::ZygoteModule&& module,
                 VmoHolder vmo_holder)
        : vmo_holder_(std::move(vmo_holder)),
          load_bias_{module.load_bias_},
          load_info_{std::move(module.load_info_)} {}

    ZygoteModule& operator=(ZygoteModule&&) = default;

    VmoHolder& vmo_holder() { return vmo_holder_; }

    const zx::vmo& vmo() const { return GetVmo(vmo_holder_); }

    template <class Diagnostics>
    bool Load(Diagnostics& diag, Loader& loader, zx_vaddr_t vmar_base) const {
      return loader.Allocate(diag, load_info_, VmarOffset(vmar_base)) &&
             loader.Load(diag, load_info_, vmo().borrow());
    }

   private:
    friend RemoteZygote<RemoteZygoteVmo::kVmo, Elf>;

    size_t VmarOffset(zx_vaddr_t vmar_base) const {
      return Loader::VmarOffsetForLoadBias(vmar_base, load_info_, load_bias_);
    }

    VmoHolder vmo_holder_;
    size_type load_bias_;
    LoadInfo load_info_;
  };

  static const zx::vmo& GetVmo(const DecodedPtr& module) { return module->vmo(); }

  static const zx::vmo& GetVmo(const zx::vmo& vmo) { return vmo; }

  static zx::result<VmoHolder> HoldVmo(DecodedPtr module) {
    if constexpr (Vmo == RemoteZygoteVmo::kVmo) {
      // Duplicate the VMO handle owned by the RemoteDecodedModule.
      zx::vmo vmo;
      zx_status_t status = module->vmo().duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo);
      if (status != ZX_OK) {
        return zx::error_result{status};
      }
      return zx::ok(std::move(vmo));
    } else {
      // Holding the original decoded module pointer can never fail.
      return zx::ok(std::move(module));
    }
  }

  static zx::result<zx_vaddr_t> GetVmarBase(const zx::vmar& vmar) {
    zx_info_vmar_t vmar_info;
    zx_status_t status =
        vmar.get_info(ZX_INFO_VMAR, &vmar_info, sizeof(vmar_info), nullptr, nullptr);
    if (status != ZX_OK) [[unlikely]] {
      return zx::error{status};
    }
    return zx::ok(vmar_info.base);
  }

  std::vector<ZygoteModule> modules_;
};

}  // namespace ld

#endif  // LIB_LD_REMOTE_ZYGOTE_H_

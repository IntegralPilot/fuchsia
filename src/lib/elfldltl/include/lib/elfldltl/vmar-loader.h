// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_VMAR_LOADER_H_
#define SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_VMAR_LOADER_H_

#include <lib/zx/result.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>
#include <zircon/assert.h>
#include <zircon/syscalls/object.h>

#include <span>
#include <type_traits>

#include "diagnostics.h"
#include "memory.h"
#include "zircon.h"

namespace elfldltl {

/// This is the base class for LocalVmarLoader and RemoteVmarLoader, below.
/// All versions have mostly the same API.  The protected Load method here has
/// the same signature as the public Load methods in the derived classes, so
/// the one API comment on Load here applies to those public methods.
///
/// This object encapsulates the work needed to load an object into a VMAR
/// based on an elfldltl::LoadInfo struct (see load.h) previously populated
/// from program headers.  The Load method does the loading and initializes
/// the VmarLoader object's state.  If Load fails, then the object should be
/// destroyed without calling other methods.
class VmarLoader {
 public:
  // This is returned by Commit(), which completes the use of an VmarLoader.
  // It represents the capability to apply RELRO protections to a loaded image.
  // Unlike the VmarLoader object itself, its lifetime is not tied to the image
  // mappings.  After Commit(), the image mapping won't be destroyed by the
  // VmarLoader's destructor.
  class Relro {
   public:
    Relro() = default;

    // Movable, not copyable: the object represents capability ownership.
    Relro(const Relro&) = delete;
    Relro(Relro&&) = default;

    Relro& operator=(const Relro&) = delete;
    Relro& operator=(Relro&&) = default;

    // This is the only method that can be called, and it must be last.
    // It makes the RELRO region passed to MmapLoader::Commit read-only.
    template <class Diagnostics>
    [[nodiscard]] bool Commit(Diagnostics& diag) && {
      zx::vmar vmar = std::exchange(vmar_, {});
      if (size_ > 0) {
        zx_status_t status = vmar.protect(ZX_VM_PERM_READ, addr_, size_);
        if (status != ZX_OK) {
          return diag.SystemError("cannot protect PT_GNU_RELRO region: ", ZirconError{status});
        }
      }
      return true;
    }

    // Alternatively, the VMAR handle can be extracted without doing
    // protections.
    [[nodiscard]] zx::vmar TakeVmar() && { return std::move(vmar_); }

   private:
    friend VmarLoader;

    template <class Region>
    Relro(zx::vmar vmar, const Region& region, uintptr_t load_bias) : vmar_{std::move(vmar)} {
      if (!region.empty()) {
        addr_ = region.start + load_bias;
        size_ = region.size();
      }
    }

    zx_vaddr_t addr_ = 0;
    size_t size_ = 0;
    zx::vmar vmar_;
  };

  // This can be specialized by the user for particular LoadInfo types passed
  // to Load().  It's constructed by Load() for each segment using the specific
  // LoadInfo::*Segment type, not the LoadInfo::Segment std::variant type, so
  // it can have a constructor that works differently for each type.  The
  // constructor will be called with the individual segment object reference,
  // and the VMO for the whole file.  Both will be valid for the lifetime of
  // the SegmentVmo object.  The handle returned by .vmo() will only be used
  // during that lifetime.  So, a specialization might return an object that
  // owns the VMO handle it yields.
  template <class LoadInfo>
  class SegmentVmo {
   public:
    SegmentVmo() = delete;

    template <class Segment>
    SegmentVmo(const Segment& segment, zx::unowned_vmo vmo)
        : vmo_{vmo->borrow()}, offset_{segment.offset()} {}

    // This is the VMO to map the segment's contents from.
    zx::unowned_vmo vmo() const { return vmo_->borrow(); }

    // If true and the segment is writable, make a copy-on-write child VMO.
    // This is the usual behavior when using the original file VMO directly.
    // A specialization can return anything convertible to bool.
    constexpr std::true_type copy_on_write() const { return {}; }

    // This is the offset within the VMO whence the segment should be mapped
    // (or cloned if this->copy_on_write()).
    uint64_t offset() const { return offset_; }

   private:
    zx::unowned_vmo vmo_;
    uint64_t offset_;
  };

  // When default-constructed, only the zx::vmar signature of Load can be used.
  // The default-constructed object can be assigned to another that has a
  // parent VMAR handle.
  VmarLoader() = default;

  explicit VmarLoader(const zx::vmar& vmar) : vmar_(vmar.borrow()) {}

  VmarLoader(VmarLoader&& other) = default;

  VmarLoader& operator=(VmarLoader&& other) = default;

  ~VmarLoader() {
    if (load_image_vmar_) {
      load_image_vmar_.destroy();
    }
  }

  [[gnu::const]] static size_t page_size() { return zx_system_get_page_size(); }

  /// This is called implicitly by Load if not explicitly called before Load.
  /// Allocate a child VMAR from the containing VMAR for the whole load image.
  /// Only the vaddr_size() and vaddr_start() from the LoadInfo are used here.
  /// The kernel places it using ASLR within the parent VMAR provided at
  /// construction, unless the optional argument sets the offset from the base
  /// of that parent VMAR.
  template <class Diagnostics, class LoadInfo>
  [[nodiscard]] bool Allocate(Diagnostics& diag, const LoadInfo& load_info,
                              std::optional<size_t> vmar_offset = std::nullopt) {
    if (zx_status_t status =
            AllocateVmar(load_info.vaddr_size(), load_info.vaddr_start(), vmar_offset);
        status != ZX_OK) [[unlikely]] {
      constexpr std::string_view kFailed = "Failed to allocate address space: ";
      if (!vmar_offset) {
        return diag.SystemError(kFailed, ZirconError{status});
      }
      return diag.SystemError(kFailed, ZirconError{status}, " at VMAR offset ", *vmar_offset);
    }
    return true;
  }

  /// Given the base address of some parent VMAR, LoadInfo of an image to be
  /// passed to Allocate, and the desired exact load bias, this yields the
  /// value to pass for Allocate's optional argument.  For example, if another
  /// VmarLoader object has been used to load this image before, then passing
  /// this the .load_bias() value will give the Allocate argument to ensure
  /// that Load replicates the previous load layout exactly in a new process.
  template <class LoadInfo>
  static size_t VmarOffsetForLoadBias(zx_vaddr_t vmar_base, const LoadInfo& load_info,
                                      zx_vaddr_t load_bias) {
    return load_info.vaddr_start() + load_bias - vmar_base;
  }

  /// This installs a pre-allocated VMAR for the image that will be passed to
  /// Load().  The VMAR must be large enough for the .vaddr_size() of the
  /// LoadInfo passed to Load().  This takes ownership of the VMAR handle and
  /// of the VMAR itself--the VMAR will be destroyed if this object is
  /// destroyed before Commit() is called.  The load bias must match the
  /// difference between the base of the VMAR and .vaddr_start().
  void Place(zx::vmar load_image_vmar, zx_vaddr_t load_bias) {
    ZX_DEBUG_ASSERT_MSG(!load_image_vmar_, "Place or Allocate called twice");
    ZX_DEBUG_ASSERT(load_image_vmar);
    load_image_vmar_ = std::move(load_image_vmar);
    load_bias_ = load_bias;
  }

  /// After Allocate() or Place() and/or Load(), this is the bias added to the
  /// given LoadInfo::vaddr_start() to find the runtime load address.
  zx_vaddr_t load_bias() const { return load_bias_; }

  /// Commit is used to keep the mapping created by Load around even after the
  /// VmarLoader object is destroyed.  This method must be the last thing
  /// called on the object if it is used, hence it can only be called with
  /// `auto relro = std::move(loader).Commit(relro_bounds);`.  If Commit() is
  /// not called, then loading is aborted by destroying the VMAR when the
  /// VmarLoader object is destroyed.  Commit() returns the Relro object (see
  /// above) that holds the VMAR handle allowing it to change page protections
  /// of the load image.
  template <class Region>
  [[nodiscard]] Relro Commit(const Region& relro_bounds) && {
    return {std::exchange(load_image_vmar_, {}), relro_bounds, load_bias()};
  }

 protected:
  // This encapsulates the main differences between the Load methods of the
  // derived classes.  Each derived class's Load method just calls a different
  // VmarLoader::Load<Policy> instantiaton.  The PartialPagePolicy applies
  // specifically to LoadInfo::DataWithZeroFillSegment segments where the
  // segment's memsz > filesz and its vaddr + filesz is not page-aligned.
  enum class PartialPagePolicy {
    kProhibited,     // vaddr + filesz must be page-aligned.
    kCopyInProcess,  // zx_vmo_read the partial page into zero-fill memory.
    kZeroInVmo,      // ZX_VMO_OP_ZERO the remainder after a COW partial page.
  };

  // This loads the segments according to the elfldltl::LoadInfo<...>
  // instructions, mapping contents from the given VMO handle.  It's the real
  // initializer for the object, and other methods can only be used after it
  // returns success (true).  If it fails (returns false), the Diagnostics
  // object gets calls with the error details, and the VmarLoader object should
  // be destroyed promptly.
  //
  // After Load() returns, the caller must assume that the containing VMAR
  // passed to the constructor may have new mappings whether the call succeeded
  // or not. Any mappings made are cleared out by destruction of the VmarLoader
  // object unless Commit() is called, see below.
  template <PartialPagePolicy PartialPage, class Diagnostics, class LoadInfo>
  [[nodiscard]] bool Load(Diagnostics& diag, const LoadInfo& load_info, zx::unowned_vmo vmo) {
    if (!load_image_vmar_) {
      // Allocate wasn't called yet, so do it now.  If it returns true after
      // failure because Diagnostics::SystemError returned true, then still
      // bail out early but also return true.
      if (bool ok = Allocate(diag, load_info); !ok || !load_image_vmar_) {
        return ok;
      }
    }

    VmoName base_name_storage = VmarLoader::GetVmoName(vmo->borrow());
    std::string_view base_name = std::string_view(base_name_storage.data());

    auto mapper = [this, vaddr_start = load_info.vaddr_start(), vmo, &diag, base_name,
                   num_data_segments = size_t{0},
                   num_zero_segments = size_t{0}](const auto& segment) mutable {
      // Where in the VMAR the segment begins, accounting for load bias.
      const uintptr_t vmar_offset = segment.vaddr() - vaddr_start;

      // Let the Segment type choose a different VMO and offset if it wants to.
      SegmentVmo<LoadInfo> segment_vmo{segment, vmo->borrow()};
      zx::unowned_vmo map_vmo = segment_vmo.vmo();
      const auto map_cow = segment_vmo.copy_on_write();
      const uint64_t map_offset = segment_vmo.offset();

      // The read-only ConstantSegment shares little in common with the others.
      // The segment.writable() is statically true in the ConstantSegment
      // instantiation of the mapper call operator, so the rest of the code
      // will be compiled away.
      if (!segment.writable()) {
        // This is tautologically true in ConstantSegment.
        ZX_DEBUG_ASSERT(segment.filesz() == segment.memsz());

        zx_vm_option_t options = kVmCommon |  //
                                 (segment.readable() ? ZX_VM_PERM_READ : 0) |
                                 (segment.executable() ? kVmExecutable : 0);
        zx_status_t status =
            Map(vmar_offset, options, map_vmo->borrow(), map_offset, segment.filesz());
        if (status != ZX_OK) [[unlikely]] {
          diag.SystemError("cannot map read-only segment", FileAddress{segment.vaddr()}, " from ",
                           FileOffset{segment.offset()}, ": ", ZirconError{status});
          return false;
        }

        return true;
      }

      // All the writable segment types can be handled as if they were the most
      // general case: DataWithZeroFillSegment.  This comprises up to three
      // regions, depending on the segment:
      //
      //   [file pages]* [intersecting page]? [anon pages]*
      //
      // The DataSegment and ZeroFillSegment cases are statically just the
      // first or the last, with no intersecting page.  The arithmetic and if
      // branches below will compile away in those instantiations of the mapper
      // lambda to calling just MapWritable or just MapZeroFill.
      //
      // * "file pages" are present when filesz > 0, 'map_size' below.
      // * "anon pages" are present when memsz > filesz, 'zero_size' below.
      // * "intersecting page" exists when both file pages and anon pages
      //   exist, and file pages are not an exact multiple of pagesize. At most
      //   a SINGLE intersecting page exists. It is represented by 'copy_size'
      //   below.
      //
      // IMPLEMENTATION NOTE: The VmarLoader performs only two mappings.
      //    * Mapping file pages up to the last full page of file data.
      //    * Mapping zero-fill pages, including the intersecting page, to the
      //    * end of the segment.
      //
      // TODO(https://fxbug.dev/42172744): Support mapping objects into VMAR from out of
      // process.
      //
      // After the second mapping, the VmarLoader then reads in the partial
      // file data into the intersecting page.
      //
      // The alternative would be to map filesz page rounded up into memory and
      // then zero out the zero fill portion of the intersecting page. This
      // isn't preferable because we would immediately cause a page fault and
      // spend time zero'ing a page when the OS may already have copied this
      // page for us.

      // This is tautologically zero in ZeroFillSegment.
      size_t map_size = segment.filesz();
      size_t zero_size = 0;
      size_t copy_size = 0;

      // This is tautologically false in DataSegment.  In ZeroFillSegment
      // map_size is statically zero so copy_size statically stays zero too.
      if (segment.memsz() > segment.filesz()) {
        switch (PartialPage) {
          case PartialPagePolicy::kProhibited:
            // MapWritable will assert that map_size is aligned.
            zero_size = segment.memsz() - map_size;
            break;
          case PartialPagePolicy::kCopyInProcess:
            // Round down what's mapped, and copy the difference.
            copy_size = map_size & (page_size() - 1);
            map_size &= -page_size();
            // Zero the remaining whole pages.
            zero_size = segment.memsz() - map_size;
            break;
          case PartialPagePolicy::kZeroInVmo:
            // MapWritable will round up what's mapped and zero the difference.
            // Just zero the remaining whole pages.
            zero_size = segment.memsz() - ((map_size + page_size() - 1) & -page_size());
            break;
        }
      }

      // First map data from the file, if any.
      if (map_size > 0) {
        zx_status_t status = MapWritable<PartialPage == PartialPagePolicy::kZeroInVmo>(
            vmar_offset, map_vmo->borrow(), map_cow, base_name, map_offset, map_size,
            num_data_segments);
        if (status != ZX_OK) [[unlikely]] {
          diag.SystemError("cannot map writable segment", FileAddress{segment.vaddr()},
                           " from file", FileOffset{map_offset}, ": ", ZirconError{status});
          return false;
        }
      }

      // Then map zero-fill data, if any.
      if (zero_size > 0) {
        if constexpr (PartialPage == PartialPagePolicy::kZeroInVmo) {
          // MapWritable<ZeroInVmo=true> will have rounded up its true mapping
          // size and zeroed the trailing partial page.  So skip over that true
          // mapping size to the first page that's entirely zero-fill.
          map_size = (map_size + page_size() - 1) & -page_size();
        } else {
          // This should have been rounded down earlier.
          ZX_DEBUG_ASSERT((map_size & (page_size() - 1)) == 0);
        }
        const uintptr_t zero_fill_vmar_offset = vmar_offset + map_size;
        zx_status_t status =
            MapZeroFill(zero_fill_vmar_offset, base_name, zero_size, num_zero_segments);
        if (status != ZX_OK) [[unlikely]] {
          diag.SystemError("cannot map zero-fill pages", FileAddress{segment.vaddr() + map_size},
                           ": ", ZirconError{status});
          return false;
        }

        // Finally, copy the partial intersecting page, if any.
        if constexpr (PartialPage != PartialPagePolicy::kCopyInProcess) {
          ZX_DEBUG_ASSERT(copy_size == 0);
        } else if (copy_size > 0) {
          const size_t copy_offset = map_offset + map_size;
          void* const copy_data =
              reinterpret_cast<void*>(vaddr_start + zero_fill_vmar_offset + load_bias_);
          zx_status_t status = map_vmo->read(copy_data, copy_offset, copy_size);
          if (status != ZX_OK) [[unlikely]] {
            diag.SystemError("cannot read segment data from file", FileOffset{copy_offset}, ": ",
                             ZirconError{status});
            return false;
          }
        }
      }
      return true;
    };

    return load_info.VisitSegments(mapper);
  }

 private:
  using VmoName = std::array<char, ZX_MAX_NAME_LEN>;

  static VmoName GetVmoName(zx::unowned_vmo vmo);

  // Permissions on Fuchsia are trickier than Linux, so read these comments
  // carefully if you are encountering permission issues!
  //
  // * Every segment is mapped to a specific location in the child VMAR. The
  //    permission to do so is given by the ZX_VM_CAN_MAP_SPECIFIC
  //    permission on the fresh allocation.
  //
  // * ZX_VM_ALLOW_FAULTS is required by the kernel when mapping resizable
  //    or pager-backed VMOs, which we might be.

  static constexpr zx_vm_option_t kVmCommon = ZX_VM_SPECIFIC | ZX_VM_ALLOW_FAULTS;

  // Execute-only pages may or may not be available on the system.
  // The READ_IF_XOM_UNSUPPORTED gives the system license to make the
  // pages readable as well if execute-only was specified but can't
  // be honored.
  static constexpr zx_vm_option_t kVmExecutable =
      ZX_VM_PERM_EXECUTE | ZX_VM_PERM_READ_IF_XOM_UNSUPPORTED;

  // Segment types other than ConstantSegment always use R/W permissions.
  static constexpr zx_vm_option_t kMapWritable = kVmCommon | ZX_VM_PERM_READ | ZX_VM_PERM_WRITE;

  // Allocate a contiguous address space region to hold all the segments and
  // store its handle in load_image_vmar_.  The base address of the region is
  // chosen by the kernel, which can do ASLR, unless specified by the optional
  // vmar_offset argument (relative to the parent VMAR given at construction).
  // The load_image_vmar_ and load_bias_ members are updated.
  zx_status_t AllocateVmar(size_t vaddr_size, size_t vaddr_start,
                           std::optional<size_t> vmar_offset);

  zx_status_t Map(uintptr_t vmar_offset, zx_vm_option_t options, zx::unowned_vmo vmo,
                  uint64_t vmo_offset, size_t size) {
    zx_vaddr_t vaddr;
    return load_image_vmar_.map(options, vmar_offset, *vmo, vmo_offset, size, &vaddr);
  }

  template <bool ZeroPartialPage>
  zx_status_t MapWritable(uintptr_t vmar_offset, zx::unowned_vmo vmo, bool copy_vmo,
                          std::string_view base_name, uint64_t vmo_offset, size_t size,
                          size_t& num_data_segments);

  zx_status_t MapZeroFill(uintptr_t vmar_offset, std::string_view base_name, size_t size,
                          size_t& num_zero_segments);

  // The region of the address space that the module is being loaded into.  The
  // load_image_vmar_ is initialized during loading and only cleared by either
  // Commit() or destruction.
  //
  // The VMAR handle must be preserved because some Fuchsia syscalls that act
  // on VMARs, like modifying protections, cannot be applied through parent
  // regions.
  zx::vmar load_image_vmar_;

  // This is the root VMAR that the mapping is placed into.
  zx::unowned_vmar vmar_;

  zx_vaddr_t load_bias_ = 0;
};

/// elfldltl::LocalVmarLoader performs loading within the current process only.
/// See VmarLoader above for the primary API details.  LocalVmarLoader can be
/// default-constructed to place the module inside the root VMAR.  It also
/// provides the memory() method for access to the image after Load.
class LocalVmarLoader : public VmarLoader {
 public:
  explicit LocalVmarLoader(const zx::vmar& vmar = *zx::vmar::root_self()) : VmarLoader(vmar) {}

  LocalVmarLoader(LocalVmarLoader&& other) noexcept
      : VmarLoader(static_cast<VmarLoader&&>(other)),
        memory_(other.memory_.image(), other.memory_.base()) {
    other.memory().set_image({});
  }

  LocalVmarLoader& operator=(LocalVmarLoader&& other) noexcept {
    memory_.set_image(other.memory_.image());
    memory_.set_base(other.memory_.base());
    VmarLoader::operator=(static_cast<VmarLoader&&>(other));
    other.memory_.set_image({});
    return *this;
  }

  template <class Diagnostics, class LoadInfo>
  [[nodiscard]] bool Load(Diagnostics& diag, const LoadInfo& load_info, zx::unowned_vmo vmo) {
    if (!VmarLoader::Load<PartialPagePolicy::kCopyInProcess>(diag, load_info, vmo->borrow())) {
      return false;
    }
    const uintptr_t image = load_info.vaddr_start() + load_bias();
    memory_.set_image({reinterpret_cast<std::byte*>(image), load_info.vaddr_size()});
    memory_.set_base(load_info.vaddr_start());
    return true;
  }

  // This returns the DirectMemory of the mapping created by Load(). It should
  // not be used after destruction or after Commit(). If Commit() has been
  // called before destruction then the address range will continue to be
  // usable, in which case one should save memory().image() before Commit().
  DirectMemory& memory() { return memory_; }

 private:
  DirectMemory memory_;
};

/// elfldltl::RemoteVmarLoader performs loading in any process, given a VMAR
/// (that process's root VMAR or a smaller region).  See VmarLoader above for
/// the primary API details.  RemoteVmarLoader works fine for the current
/// process too, but LocalVmarLoader may optimize that case better.
class RemoteVmarLoader : public VmarLoader {
 public:
  // Can be default-constructed or constructed with a const zx::vmar& argument.
  using VmarLoader::VmarLoader;

  template <class Diagnostics, class LoadInfo>
  [[nodiscard]] bool Load(Diagnostics& diag, const LoadInfo& load_info, zx::unowned_vmo vmo) {
    return VmarLoader::Load<PartialPagePolicy::kZeroInVmo>(diag, load_info, vmo->borrow());
  }
};

/// elfldltl::AlignedRemoteVmarLoader is like elfldltl::RemoteVmarLoader but it
/// expects that any DataWithZeroFillSegment segments have been prepared to
/// have a page-aligned filesz() by zero-filling the final partial page of
/// the data portion in the VMO.
class AlignedRemoteVmarLoader : public VmarLoader {
 public:
  // Can be default-constructed or constructed with a const zx::vmar& argument.
  using VmarLoader::VmarLoader;

  template <class Diagnostics, class LoadInfo>
  [[nodiscard]] bool Load(Diagnostics& diag, const LoadInfo& load_info, zx::unowned_vmo vmo) {
    return VmarLoader::Load<PartialPagePolicy::kProhibited>(diag, load_info, vmo->borrow());
  }
};

// Both possible MapWritable instantiations are defined in vmar-loader.cc.

extern template zx_status_t VmarLoader::MapWritable<false>(  //
    uintptr_t vmar_offset, zx::unowned_vmo vmo, bool copy_vmo, std::string_view base_name,
    uint64_t vmo_offset, size_t size, size_t& num_data_segments);

extern template zx_status_t VmarLoader::MapWritable<true>(  //
    uintptr_t vmar_offset, zx::unowned_vmo vmo, bool copy_vmo, std::string_view base_name,
    uint64_t vmo_offset, size_t size, size_t& num_data_segments);

}  // namespace elfldltl

#endif  // SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_VMAR_LOADER_H_

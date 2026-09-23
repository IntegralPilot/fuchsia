// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_SEGMENT_WITH_VMO_H_
#define SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_SEGMENT_WITH_VMO_H_

#include <lib/fit/result.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <zircon/assert.h>

#include <type_traits>

#include "diagnostics.h"
#include "layout.h"
#include "load.h"
#include "mapped-vmo-file.h"
#include "vmar-loader.h"
#include "zircon.h"

namespace elfldltl {

// elfldltl::SegmentWithVmo::Copy and elfldltl::SegmentWithVmo::NoCopy can be
// used as the SegmentWrapper template template parameter in elfldltl::LoadInfo
// instantiations.  Partial specializations of elfldltl::VmarLoader::SegmentVmo
// provided below will then check segment.vmo() when mapping a segment.  If
// it's a valid handle, then the segment is mapped from that VMO at offset 0;
// otherwise it's mapped from the main file VMO at segment.offset() as usual.
//
// The Copy version has the usual behavior of treating the VMO as read-only and
// making a copy-on-write child VMO for a writable segment.  The NoCopy version
// will instead map segment.vmo() itself writable, so it can only be used once
// as that VMO's contents will be modified by the process using it.
//
// The NoCopy case allows for simple out-of-process dynamic linking where
// nothing is cached for reuse.  The Copy case allows for the "zygote" model
// where VMOs can be reused to get copy-on-write sharing of segments where
// relocation has already been done.
//
// elfldltl::SegmentWithVmo::AlignSegments adjusts any LoadInfo segments that
// have a partial page of data followed by zero-fill bytes so that all segments
// can be mapped page-aligned without additional partial-page zeroing work (as
// elfldltl::AlignedRemoteVmarLoader requires).  To do this, it installs a
// per-segment VMO that copies (via copy-on-write snapshot) the data and then
// zero-fills the partial page directly in the VMO contents.  Using this with
// an elfldltl::SegmentWithVmo::NoCopy instantiation of elfldltl::LoadInfo
// really just does the zeroing work ahead of time; elfldltl::RemoteVmarLoader
// would do the same thing when mapping.  An elfldltl::SegmentWithVmo::Copy
// instantiation can be used either for a full zygote model where the
// relocations are applied in the stored VMOs, or simply to share the zeroing
// work across multiple separate loads with their own relocations applied to
// their own copies (this may be beneficial to overall page-sharing if the
// final data page didn't need to be touched for any relocations).
//
// elfldltl::SegmentWithVmo::MakeMutable can be applied to a particular segment
// to install its own copied VMO.  This is what out-of-process dynamic linking
// must do before applying relocations that touch that segment's contents.
//
// The elfldltl::SegmentWithVmo::GetMutableMemory class is meant for use with
// the elfldltl::LoadInfoMutableMemory adapter object.

class SegmentWithVmo {
 public:
  // This becomes an additional base type along with the original Segment type.
  struct VmoHolder {
    VmoHolder() = default;

    VmoHolder(VmoHolder&&) = default;
    VmoHolder& operator=(VmoHolder&&) = default;

    explicit VmoHolder(zx::vmo vmo) : vmo_{std::move(vmo)} {}

    zx::vmo& vmo() { return vmo_; }
    const zx::vmo& vmo() const { return vmo_; }

    // Replace the VMO handle with an immutable one so no further modifications
    // can be made.  Note that writable mappings may already exist, but no new
    // writable mappings can be made with vmo() after this.
    zx::result<> MakeImmutable() {
      if (vmo_) {
        return zx::make_result(vmo_.replace(kReadonlyRights, &vmo_));
      }
      return zx::ok();
    }

   private:
    zx::vmo vmo_;
  };
  static_assert(std::is_move_constructible_v<VmoHolder>);
  static_assert(std::is_move_assignable_v<VmoHolder>);

  // The Copy and NoCopy templates are shorthands using this.
  template <class CopyT>
  struct Wrapper {
    // Segment types that have a filesz() are replaced with this subclass.
    // ZeroFillSegment never needs a VMO handle.
    template <class Segment>
    class WithVmo : public Segment, public VmoHolder {
     public:
      using Segment::Segment;

      // It's movable, though not copyable.  The base Segment type is copyable.
      WithVmo(WithVmo&&) = default;

      // It's explicitly constructible from the base Segment type.
      explicit WithVmo(const Segment& other) : Segment{other} {}

      WithVmo& operator=(WithVmo&&) = default;

      // Assigning from the base Segment type drops any previous VMO.
      WithVmo& operator=(const Segment& other) {
        Segment::operator=(other);
        vmo().reset();
      }

      // Don't merge with any other segment if there's a VMO installed.  Its
      // contents won't cover the other segment.  Both segments will get the
      // reciprocal check, so if this segment has no VMO (yet) but the other
      // does, its CanMergeWith is the one to return false.
      template <class Other>
      bool CanMergeWith(const Other& other) const {
        static_assert(std::is_move_constructible_v<WithVmo>);
        static_assert(std::is_move_assignable_v<WithVmo>);
        return !vmo();
      }

      // Similar to `CanMergeWith`, this segment can't be replaced by another
      // segment if there's a VMO installed.
      bool CanReplace() const {
        static_assert(std::is_move_constructible_v<WithVmo>);
        static_assert(std::is_move_assignable_v<WithVmo>);
        return !vmo();
      }

      // This accepts the corresponding segment type either from the base
      // (unwrapped) LoadInfo::*Segment type or from a SegmentWithVmo wrapper
      // like this one.  Both SegmentWithVmo::Copy and SegmentWithVmo::NoCopy
      // versions of the corresponding Segment subclass are accepted.  The VMOs
      // are always cloned--the only distinction of NoCopy is what a VmarLoader
      // does with these new segment VMOs (NoCopy writes them in place).
      template <class Diagnostics, class OtherSegment>
      static fit::result<bool, WithVmo> Copy(Diagnostics& diag, const OtherSegment& other) {
        // If the other segment has its own VMO, then the copied segment
        // needs a copy-on-write clone of that whole VMO.
        zx::unowned_vmo other_vmo;

        // For copy from the unadorned Segment, there's never a VMO to copy.
        if constexpr (!std::is_same_v<OtherSegment, Segment>) {
          // Copy from the corresponding Segment subclass of this wrapper is
          // the other supported option, to clone the existing VMO, if any.
          static_assert(std::is_same_v<OtherSegment, SameSegment<CopyT>> ||
                        std::is_same_v<OtherSegment, SameSegment<OtherCopyT>>);
          other_vmo = other.vmo().borrow();
        }

        // Copy the base Segment type's fields first.  It cannot fail.
        WithVmo copy{*Segment::Copy(diag, static_cast<const Segment&>(other))};

        if (*other_vmo) {
          assert(copy.filesz() == other.filesz());

          zx_status_t status =  //
              CopyVmo(other_vmo->borrow(), 0, copy.filesz(), copy.vmo());
          if (status != ZX_OK) [[unlikely]] {
            return fit::error{SystemError(diag, copy, kCopyVmoFail, status)};
          }
        }

        return fit::ok(std::move(copy));
      }

     private:
      using OtherCopyT = std::integral_constant<bool, !CopyT::value>;

      template <class SomeCopyT>
      using SameSegment = Wrapper<SomeCopyT>::template WithVmo<Segment>;
    };

    // Use the wrapper if need be, or the original type if not.
    template <class Segment>
    using Type = std::conditional_t<kSegmentHasFilesz<Segment>, WithVmo<Segment>, Segment>;

    // This implements the VmarLoader::SegmentVmo class for LoadInfo types used
    // with SegmentWithVmo::Copy and SegmentWithVmo::NoCopy.  It's the base
    // class for the partial specialization of VmarLoader::SegmentVmo below.
    class SegmentVmo {
     public:
      SegmentVmo() = delete;

      template <class Segment>
      SegmentVmo(const Segment& segment, zx::unowned_vmo vmo)
          : vmo_{vmo->borrow()}, offset_{segment.offset()} {
        if constexpr (kSegmentHasFilesz<Segment>) {
          // This is not a ZeroFillSegment, so it might have a VMO stored.
          if (segment.vmo()) {
            // This VMO at offset 0 replaces the file VMO at segment.offset().
            vmo_ = segment.vmo().borrow();
            offset_ = 0;

            // In SegmentWithVmo::NoCopy, copy_on_write() is true only when
            // using the file VMO, not the stored VMO.
            copy_on_write_ = CopyFalse{};
          }
        }
      }

      zx::unowned_vmo vmo() const { return vmo_->borrow(); }

      constexpr auto copy_on_write() const { return copy_on_write_; }

      uint64_t offset() const { return offset_; }

     private:
      // In Copy, copy_on_write() is statically true all the time.  In NoCopy,
      // it's true by default but can be set to false in the constructor.
      using CopyTrue = std::conditional_t<CopyT{}, CopyT, std::true_type>;
      using CopyFalse = std::conditional_t<CopyT{}, CopyT, std::false_type>;

      zx::unowned_vmo vmo_;
      std::conditional_t<CopyT{}, CopyT, bool> copy_on_write_{CopyTrue{}};
      uint64_t offset_;
    };
  };

  template <class Segment>
  using Copy = Wrapper<std::true_type>::template Type<Segment>;
  using CopySegmentVmo = Wrapper<std::true_type>::SegmentVmo;

  template <class Segment>
  using NoCopy = Wrapper<std::false_type>::template Type<Segment>;
  using NoCopySegmentVmo = Wrapper<std::true_type>::SegmentVmo;

  // What's not obvious is that ZX_DEFAULT_VMO_RIGHTS &~ ZX_VM_RIGHT_WRITE
  // cannot be used here.  The handle from the VMO created in MakeMutable will
  // certainly have ZX_VM_RIGHT_WRITE, but it won't necessarily have all the
  // rights in ZX_DEFAULT_VMO_RIGHTS.  The zx_handle_replace system call does
  // not have a way to mask off rights, only choose a whole new set of rights
  // that must be a subset of the existing rights--so you have to be sure
  // what's truly a subset of the existing rights.
  //
  // We shouldn't presume exactly what rights the filesystem or loader service
  // or whatnot gave us on the file VMO handle.  The per-segment VMOs are from
  // zx_vmo_create_child (in CopyVmo, below), which returns the new VMO via a
  // handle with rights that depend on the original VMO handle's rights.  So
  // even though segment.vmo() is a handle we made ourselves, we can't presume
  // exactly what rights it has.
  //
  // So to drop ZX_RIGHT_WRITE for the readonly case below, we'd need to do one
  // of two things: do a ZX_INFO_HANDLE_BASIC query to find the rights before
  // masking off ZX_RIGHT_WRITE in zx_handle_replace; or, just use a fixed set
  // of rights that's small enough to be surely a subset of the VMO rights we
  // have (as derived from the file VMO handle's rights by the behavior of
  // zx_vmo_create_child).  Since we know how these particular VMOs will need
  // to be used later, we do the latter.
  static constexpr zx_rights_t kReadonlyRights =
      ZX_RIGHTS_BASIC | ZX_RIGHTS_PROPERTY | ZX_RIGHT_MAP | ZX_RIGHT_READ;

  // This takes a LoadInfo::*Segment type that has file contents (i.e. not
  // ZeroFillSegment), and ensures that segment.vmo() is a valid segment.
  // Unless there's a VMO handle there already, it creates a new copy-on-write
  // child VMO from the original file VMO.
  template <class Diagnostics, class Segment>
  static bool MakeMutable(Diagnostics& diag, Segment& segment, zx::unowned_vmo vmo) {
    if (!segment.vmo()) {
      zx_status_t status =
          CopyVmo(vmo->borrow(), segment.offset(), segment.filesz(), segment.vmo());
      if (status != ZX_OK) [[unlikely]] {
        return SystemError(diag, segment, kCopyVmoFail, status);
      }
    }
    return true;
  }

  // elfldltl::SegmentWithVmo::AlignSegments modifies a LoadInfo that uses the
  // SegmentWithVmo wrappers.  It ensures that all segment.filesz() values are
  // page-aligned, so that elfldltl::AlignedRemoteVmarLoader can be used later.
  // To start with, each segment must have no vmo() handle installed.  If a
  // segment has a misaligned filesz(), then a new copy-on-write child of the
  // main file VMO is installed as its vmo().  In the modified VMO, the partial
  // page has been cleared to all zero bytes.  The segment.filesz() has been
  // rounded up to whole pages so it can be mapped with no additional zeroing.
  //
  // With the optional readonly flag set, make each new per-segment VMO
  // immutable by replacing the segment .vmo() handle without ZX_RIGHT_WRITE.
  template <class Diagnostics, class LoadInfo>
  [[nodiscard]] static bool AlignSegments(Diagnostics& diag, LoadInfo& info, zx::unowned_vmo vmo,
                                          LoadInfo::size_type page_size, bool readonly = false) {
    using DataWithZeroFillSegment = LoadInfo::DataWithZeroFillSegment;
    auto align_segment = [vmo, page_size, readonly, &diag](auto& segment) {
      using Segment = std::decay_t<decltype(segment)>;
      if constexpr (std::is_same_v<Segment, DataWithZeroFillSegment>) {
        const size_t zero_size = segment.MakeAligned(page_size);
        if (zero_size > 0) {
          ZX_DEBUG_ASSERT(!segment.vmo());
          if (!MakeMutable(diag, segment, vmo->borrow())) [[unlikely]] {
            return false;
          }

          const uint64_t zero_offset = segment.filesz() - zero_size;
          zx_status_t status = ZeroVmo(segment.vmo().borrow(), zero_offset, zero_size);
          if (status != ZX_OK) [[unlikely]] {
            return SystemError(diag, segment, kZeroVmoFail, status);
          }

          // When there's a partial page to zero, this will create a new VMO.
          // With the readonly flag, we want to ensure this VMO won't be
          // modified in the future, so we want to drop ZX_VM_RIGHT_WRITE
          // from our handle to it.
          if (readonly) {
            zx::result<> result = segment.MakeImmutable();
            if (result.is_error()) [[unlikely]] {
              return SystemError(diag, segment, kProtectVmoFail, result.error_value());
            }
          }
        }
      }
      return true;
    };
    return info.VisitSegments(align_segment);
  }

  // elfldltl::SegmentWithVmo::GetMutableMemory must be instantiated with a
  // LoadInfo type using SegmentWithVmo wrapper types, and created with a
  // zx::unowned_vmo for the original file contents.  It can then be used to
  // construct an elfldltl::LoadInfoMutableMemory adapter object (see details
  // in <lib/elfldltl/loadinfo-mutable-memory.h>), which provides the mutation
  // calls in the Memory API.  When mutation requests are made on that Memory
  // object, the GetMutableMemory callback will be used to map in the mutable
  // VMO for the segment contents.  The mutable VMO is created via MakeMutable
  // if needed, and then stored in the LoadInfo::Segment object for later use.
  //
  // The callable object itself is copyable and default-constructible
  // (requiring later copy-assignment to be usable).  Note that it holds (and
  // copies) the unowned handles passed in the constructor, so users are
  // responsible for ensuring those handles remain valid for the lifetime of
  // the callable object.  The optional second argument to the constructor is a
  // zx::unowned_vmar used for making mappings, default zx::vmar::root_self().
  template <class LoadInfo>
  class GetMutableMemory {
   public:
    using Result = fit::result<bool, MappedVmoFile>;
    using Segment = LoadInfo::Segment;

    GetMutableMemory() = default;

    GetMutableMemory(const GetMutableMemory&) = default;

    explicit GetMutableMemory(zx::unowned_vmo vmo, zx::unowned_vmar vmar = zx::vmar::root_self())
        : vmo_{vmo->borrow()}, vmar_{vmar->borrow()} {}

    GetMutableMemory& operator=(const GetMutableMemory&) = default;

    template <class Diagnostics>
    Result operator()(Diagnostics& diag, Segment& segment) const {
      auto get_memory = [this, &diag](auto& segment) -> Result {
        using SegmentType = std::decay_t<decltype(segment)>;
        if constexpr (kSegmentHasFilesz<SegmentType>) {
          // Make sure there's a mutable VMO available.
          if (!MakeMutable(diag, segment, vmo_->borrow())) [[unlikely]] {
            return fit::error(false);
          }
          if (!segment.vmo()) [[unlikely]] {
            // MakeMutable failed but Diagnostics said to keep going.
            // No sense also reporting the failure to map the invalid handle.
            return fit::error{true};
          }

          // Now map it in for mutation.
          MappedVmoFile memory;
          if (auto result = memory.InitMutable(segment.vmo().borrow(), segment.filesz(),
                                               segment.vaddr(), vmar_->borrow());
              result.is_error()) {
            return fit::error{SystemError(diag, segment, kMapFail, result.error_value())};
          }

          return fit::ok(std::move(memory));
        } else {
          // This should be impossible via LoadInfoMutableMemory.
          return fit::error{diag.FormatError(kMutableZeroFill)};
        }
      };
      return std::visit(get_memory, segment);
    }

   private:
    zx::unowned_vmo vmo_;
    zx::unowned_vmar vmar_;
  };

 private:
  static constexpr std::string_view kCopyVmoFail =
      "cannot create copy-on-write VMO for segment contents";
  static constexpr std::string_view kZeroVmoFail =
      "cannot zero partial page in VMO for data segment";
  static constexpr std::string_view kProtectVmoFail =
      "cannot drop ZX_RIGHT_WRITE on VMO for data segment";
  static constexpr std::string_view kColonSpace = ": ";
  static constexpr std::string_view kMapFail = "cannot map segment to apply relocations";
  static constexpr std::string_view kMutableZeroFill = "cannot make zero-fill segment mutable";

  template <class Diagnostics, class Segment>
  static constexpr bool SystemError(Diagnostics& diag, const Segment& segment,
                                    std::string_view fail, zx_status_t status) {
    return diag.SystemError(fail, FileOffset{segment.offset()}, kColonSpace, ZirconError{status});
  }

  static zx_status_t CopyVmo(zx::unowned_vmo vmo, uint64_t offset, uint64_t size,
                             zx::vmo& segment_vmo) {
    return vmo->create_child(ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE, offset, size, &segment_vmo);
  }

  static zx_status_t ZeroVmo(zx::unowned_vmo segment_vmo, uint64_t offset, uint64_t size) {
    return segment_vmo->op_range(ZX_VMO_OP_ZERO, offset, size, nullptr, 0);
  }
};

// These are the SegmentVmo types used for LoadInfo<..., SegmentWithVmo::...>.
// They use a segment.vmo() if it's present.
//
// Note that a specialization using a template template parameter matches only
// that exact parameter, not even a template alias to the same thing.  So two
// separate partial specializations are required here to match all LoadInfo
// instantiations using either SegmentWithVmo::... template.

template <ElfApi Elf, template <class> class Container, PhdrLoadPolicy Policy>
class VmarLoader::SegmentVmo<LoadInfo<Elf, Container, Policy, SegmentWithVmo::Copy>>
    : public SegmentWithVmo::CopySegmentVmo {
  using SegmentWithVmo::CopySegmentVmo::CopySegmentVmo;
};

template <ElfApi Elf, template <class> class Container, PhdrLoadPolicy Policy>
class VmarLoader::SegmentVmo<LoadInfo<Elf, Container, Policy, SegmentWithVmo::NoCopy>>
    : public SegmentWithVmo::NoCopySegmentVmo {
  using SegmentWithVmo::NoCopySegmentVmo::NoCopySegmentVmo;
};

}  // namespace elfldltl

#endif  // SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_SEGMENT_WITH_VMO_H_

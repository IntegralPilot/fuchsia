// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_INTERNAL_LOAD_SEGMENT_TYPES_H_
#define SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_INTERNAL_LOAD_SEGMENT_TYPES_H_

#include <lib/fit/result.h>

#include <cassert>
#include <string_view>
#include <type_traits>
#include <variant>

#include "../layout.h"
#include "../phdr.h"

namespace elfldltl::internal {

constexpr std::string_view kTooManyLoads = "too many PT_LOAD segments";

// The std::visit implementation is quite complicated and works in a way that
// relies on constexpr arrays of function pointers.  This is not reliably
// compiled to pure PIC as is required in some contexts.
//
// This Visit provides a limited implementation that is much simpler.  It's
// less general than std::visit in that it doesn't handle arbitrary return
// value types, nor multiple arguments.
template <typename F, typename V>
constexpr bool Visit(F&& f, V&& v) {
  constexpr auto n = std::variant_size_v<std::decay_t<V>>;
  constexpr auto seq = std::make_index_sequence<n>();
  return VisitEach(std::forward<F>(f), std::forward<V>(v), seq);
}

// Helper template for Visit, needed so it can use a fold expression across
// the variant indices.
template <typename F, typename V, size_t... I>
constexpr bool VisitEach(F&& f, V&& v, std::index_sequence<I...>) {
  static_assert(sizeof...(I) == std::variant_size_v<std::decay_t<V>>);
  bool result = false;
  auto visit_one = [&f, &result](auto&& v) {
    result = std::forward<F>(f)(v);
    return true;
  };
  // Exactly one of these visit_one calls will be evaluated.
  ((v.index() == I && visit_one(std::get<I>(std::forward<V>(v)))) || ...);
  return result;
}

// This is used to implement the LoadInfo::ConstantSegment type below.
template <typename SegmentType>
class LoadConstantSegmentType : public SegmentType {
 public:
  using typename SegmentType::size_type;

  constexpr explicit LoadConstantSegmentType(size_type offset, size_type vaddr, size_type memsz,
                                             uint32_t flags)
      : SegmentType(offset, vaddr, memsz), flags_(flags) {}

  template <class Diagnostics, class Other>
  static constexpr fit::result<bool, LoadConstantSegmentType> Copy(  //
      Diagnostics& diag, const Other& other) {
    return fit::ok(LoadConstantSegmentType{
        other.offset(),
        other.vaddr(),
        other.memsz(),
        other.flags(),
    });
  }

  // The whole segment is loaded from the file.
  constexpr size_type filesz() const { return this->memsz(); }

  constexpr uint32_t flags() const { return flags_; }

  constexpr bool readable() const { return flags_ & Flags::kRead; }

  constexpr std::false_type writable() const { return {}; }

  constexpr bool executable() const { return flags_ & Flags::kExecute; }

  constexpr bool relro() const { return flags_ & Flags::kWrite; }

 private:
  using Flags = PhdrBase::Flags;
  uint32_t flags_ = 0;
};

// This provides the several distinct LoadInfo::*Segment used below.
template <typename SizeType>
struct LoadSegmentTypes {
  using size_type = SizeType;

  // This is used for RELRO bounds.
  struct Region {
    size_type size() const { return end - start; }

    size_type empty() const { return start == end; }

    size_type start = 0, end = 0;
  };

  // Every kind of segment needs an offset and a size.
  // Only a ConstantSegment is ever executable, or not readable and writable.
  class SegmentBase {
   public:
    using size_type = LoadSegmentTypes::size_type;

    // This is a hook for a SegmentWrapper subclass to add some constraint on
    // merging (e.g. incompatible extra subclass state).  It's checked first,
    // before the segments are examined for adjacency and compatible features.
    template <class OtherSegment>
    constexpr std::true_type CanMergeWith(const OtherSegment& other) const {
      return {};
    }

    // Similar to `CanMergeWith`, this hook adds a constraint to replacing one
    // segment with another segment.
    constexpr std::true_type CanReplace() const { return {}; }

    constexpr std::true_type readable() const { return {}; }

    constexpr std::true_type writable() const { return {}; }

    constexpr std::false_type executable() const { return {}; }

    constexpr std::false_type relro() const { return {}; }

    constexpr size_type offset() const { return offset_; }

    constexpr size_type memsz() const { return memsz_; }

    constexpr explicit SegmentBase(size_type offset, size_type memsz)
        : offset_(offset), memsz_(memsz) {}

   protected:
    constexpr void set_offset(size_type offset) { offset_ = offset; }

    constexpr void set_memsz(size_type memsz) { memsz_ = memsz; }

   private:
    size_type offset_ = 0;
    size_type memsz_ = 0;
  };

  // Most generic segments need to record the vaddr separately from the offset.
  template <PhdrLoadPolicy Policy>
  class Segment : public SegmentBase {
   public:
    using typename SegmentBase::size_type;

    constexpr size_type vaddr() const { return vaddr_; }

    constexpr explicit Segment(size_type offset, size_type vaddr, size_type memsz)
        : SegmentBase(offset, memsz), vaddr_(vaddr) {}

   private:
    size_type vaddr_ = 0;
  };

  // TODO(mcgrathr): This is an optimization. GCC doesn't like the
  // specialization being here and won't allow it to be defined later either.
#ifdef __clang__
  // With constrained layout policy, the offset and vaddr don't both need to be
  // tracked.  They aren't always identical, but they always have a fixed
  // difference for the whole file.
  template <>
  class Segment<PhdrLoadPolicy::kContiguous> : public SegmentBase {
   public:
    using typename SegmentBase::size_type;

    constexpr size_type vaddr() const { return this->offset(); }

   protected:
    constexpr explicit Segment(size_type offset, size_type vaddr, size_type memsz)
        : SegmentBase(offset, memsz) {}
  };
#endif

  // A writable data segment (with no attached .bss) just identifies the pages
  // from the file to load.
  template <PhdrLoadPolicy Policy>
  struct DataSegment : public Segment<Policy> {
    using Base = Segment<Policy>;
    using Base::Base;

    constexpr explicit DataSegment(size_type offset, size_type vaddr, size_type memsz,
                                   size_type filesz)
        : Segment<Policy>(offset, vaddr, memsz) {
      assert(filesz == memsz);
    }

    template <class Diagnostics, class Other>
    static constexpr fit::result<bool, DataSegment> Copy(Diagnostics& diag, const Other& other) {
      return fit::ok(DataSegment{
          other.offset(),
          other.vaddr(),
          other.memsz(),
          other.filesz(),
      });
    }

    // The whole segment is loaded from the file.
    constexpr size_type filesz() const { return this->memsz(); }
  };

  // A writable data segment with an attached zero-fill segment is both an
  // optimization and a way to share the partial page between the two without
  // extra page-alignment waste between the file portion and the zero portion.
  template <PhdrLoadPolicy Policy>
  class DataWithZeroFillSegment : public Segment<Policy> {
   public:
    constexpr explicit DataWithZeroFillSegment(size_type offset, size_type vaddr, size_type memsz,
                                               size_type filesz)
        : Segment<Policy>(offset, vaddr, memsz), filesz_(filesz) {}

    template <class Diagnostics, class Other>
    static constexpr fit::result<bool, DataWithZeroFillSegment> Copy(  //
        Diagnostics& diag, const Other& other) {
      return fit::ok(DataWithZeroFillSegment{
          other.offset(),
          other.vaddr(),
          other.memsz(),
          other.filesz(),
      });
    }

    // Only a leading subset of the in-memory segment is loaded from the file.
    constexpr size_type filesz() const { return filesz_; }

    // Modify the segment in place so that filesz() is page-aligned.
    // Return the size of the partial page left at the end of the new size,
    // which needs to be zeroed in place before the segment is mapped.
    constexpr size_type MakeAligned(size_type page_size) {
      const size_type exact_filesz = filesz_;
      filesz_ = (filesz_ + page_size - 1) & -page_size;
      return filesz_ - exact_filesz;
    }

   private:
    size_type filesz_ = 0;
  };

  // A constant segment tracks the readable() and executable() flags.
  template <PhdrLoadPolicy Policy>
  using ConstantSegment = LoadConstantSegmentType<Segment<Policy>>;

  // A plain zero-fill segment has nothing but anonymous pages to allocate.
  // The file offset is unused, so SegmentBase::offset() is actually the vaddr.
  class ZeroFillSegment : public SegmentBase {
   public:
    using SegmentBase::SegmentBase;

    template <class Diagnostics, class Other>
    static constexpr fit::result<bool, ZeroFillSegment> Copy(Diagnostics& diag,
                                                             const Other& other) {
      return fit::ok(ZeroFillSegment{other.vaddr(), other.memsz()});
    }

    constexpr size_type vaddr() const { return this->offset(); }

    constexpr std::integral_constant<size_type, 0> filesz() const { return {}; }
  };
};

template <class LoadInfo>
struct SegmentMerger {
  using size_type = typename LoadInfo::size_type;
  using ConstantSegment = typename LoadInfo::ConstantSegment;
  using DataSegment = typename LoadInfo::DataSegment;
  using DataWithZeroFillSegment = typename LoadInfo::DataWithZeroFillSegment;
  using ZeroFillSegment = typename LoadInfo::ZeroFillSegment;
  using Segment = typename LoadInfo::Segment;

  template <class First, class Second>
  static constexpr bool Adjacent(const First& first, const Second& second) {
    // In classes where vaddr() and offset() are the same, this might be doing
    // the same check twice but that will just get CSE.
    return first.vaddr() + first.memsz() == second.vaddr() &&
           first.offset() + first.memsz() == second.offset();
  }

  // ZeroFillSegment uses vaddr() for offset() so it might not match the
  // vaddr() of the preceding real data segment the generic version checks.
  template <class First>
  static constexpr bool Adjacent(const First& first, const ZeroFillSegment& second) {
    return first.vaddr() + first.memsz() == second.vaddr();
  }

  // For each pair of segment types S1 and S2, there is a call:
  //   bool Merge(V& storage, const S1& first, const S2& second);
  // where V is std::variant<S1,S2,...> (not necessarily in that order).  The
  // first and second arguments might or might not be references (aliases) into
  // storage.  If the first and second segments are adjacent and compatible,
  // this merges them by storing a new merged range into storage and then
  // returns true.
  template <class Merged, class First, class Second, typename... Args>
  static constexpr void Emplace(Segment& storage, const First& first, const Second& second,
                                Args&&... args) {
    size_type memsz = first.memsz() + second.memsz();
    storage.template emplace<Merged>(first.offset(), first.vaddr(), memsz,
                                     std::forward<Args>(args)...);
  }

  // Helper used when details other than vaddr, offset, and memsz match.
  template <class First, typename... Args>
  static constexpr bool MergeSame(Segment& storage, const First& first, const First& second,
                                  Args&&... args) {
    if (Adjacent(first, second)) {
      Emplace<First>(storage, first, second, std::forward<Args>(args)...);
      return true;
    }
    return false;
  }

  // This is the fallback overload for mismatched segment types.
  template <class... T, class First, class Second>
  static constexpr bool Merge(std::variant<T...>& storage, const First& first,
                              const Second& second) {
    return false;
  }

  // Identical adjacent segments merge.
  static constexpr bool Merge(Segment& storage, const ConstantSegment& first,
                              const ConstantSegment& second) {
    return first.flags() == second.flags() && MergeSame(storage, first, second, first.flags());
  }

  // Identical adjacent segments merge.
  static constexpr bool Merge(Segment& storage, const DataSegment& first,
                              const DataSegment& second) {
    size_type filesz = first.filesz() + second.filesz();
    return MergeSame(storage, first, second, filesz);
  }

  // A data segment can be merged into an adjacent data + bss segment.
  static constexpr bool Merge(Segment& storage, const DataSegment& first,
                              const DataWithZeroFillSegment& second) {
    if (Adjacent(first, second)) {
      size_type filesz = first.filesz() + second.filesz();
      Emplace<DataWithZeroFillSegment>(storage, first, second, filesz);
      return true;
    }
    return false;
  }

  // A data segment can be merged with an adjacent plain zero-fill segment.
  static constexpr bool Merge(Segment& storage, const DataSegment& first,
                              const ZeroFillSegment& second) {
    if (Adjacent(first, second)) {
      Emplace<DataWithZeroFillSegment>(storage, first, second, first.filesz());
      return true;
    }
    return false;
  }

  // All those add up to maybe merging any two adjacent segments.

  template <class... T, class First>
  static constexpr bool Merge(std::variant<T...>& storage, const First& first,
                              const std::variant<T...>& second) {
    return Visit([&storage, &first](const auto& second) { return Merge(storage, first, second); },
                 second);
  }

  template <class... T, class Second>
  static constexpr bool Merge(std::variant<T...>& storage, const std::variant<T...>& first,
                              const Second& second) {
    return Visit([&storage, &second](const auto& first) { return Merge(storage, first, second); },
                 first);
  }

  template <class... T>
  static constexpr bool Merge(std::variant<T...>& first, std::variant<T...>& second) {
    auto unwrap_first = [&storage = first, &second](const auto& first) {
      auto unwrap_second = [&storage, &first](const auto& second) {
        return first.CanMergeWith(second) && second.CanMergeWith(first) &&
               Merge(storage, first, second);
      };
      return Visit(unwrap_second, second);
    };
    return Visit(unwrap_first, first);
  }

  template <class... T, class Second>
  static constexpr bool Merge(std::variant<T...>& first, const Second& second) {
    return Visit(
        [&storage = first, &second](const auto& first) { return Merge(storage, first, second); },
        first);
  }
};

}  // namespace elfldltl::internal

#endif  // SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_INTERNAL_LOAD_SEGMENT_TYPES_H_

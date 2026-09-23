// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_REMOTE_ABI_HEAP_H_
#define LIB_LD_REMOTE_ABI_HEAP_H_

#include <lib/elfldltl/abi-span.h>
#include <lib/elfldltl/mapped-vmo-file.h>
#include <lib/elfldltl/segment-with-vmo.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <zircon/syscalls.h>

#include <cstddef>
#include <memory>
#include <span>
#include <string>
#include <string_view>
#include <unordered_map>
#include <variant>

#include "remote-load-module.h"

namespace ld {

// The RemoteAbiHeap represents the data segment of the stub dynamic linker as
// instantiated in a particular dynamic linking domain in a remote address
// space.  (See <lib/ld/remote-abi-stub.h> for more context.)  This starts with
// the space for the link-time data segment layout of the stub where the named
// ABI symbols lie.  It's then extended with more anonymous space to fill in
// data structures that should be visible in the remote address space (via
// pointers in the named ABI symbol data structures).  The whole data segment
// thus extended is referred to as the "remote ABI heap".
//
// Building the remote ABI heap happens in three phases:
//
// * Layout
//
//   In the layout phase, a RemoteAbiHeapLayout object must be initialized with
//   the fixed data size gleaned from RemoteAbiStub::data_size().  Then method
//   calls Allocate<T>(count) on this object reserve space for an array of
//   T[count], represented by the returned RemoteAbiSpan<T> placeholder object.
//   The AddString(string_view) method can be called to build of a table of
//   strings (with implicit NUL terminators added), represented by the returned
//   RemoteAbiString placeholder objects.
//
// * Data
//
//   In the data phase, the actual contents of heap is filled in.  The
//   RemoteAbiHeap::Create factory method takes a RemoteLoadModule representing
//   the stub dynamic linker instance to be instantiated along with the same
//   fixed data size gleaned from RemoteAbiStub::data_size() and the
//   RemoteAbiHeapLayout object previously populated during the layout phase.
//   It modifies the stub module to replace its link-time data segment with a
//   read-only segment that's extended to cover the whole heap layout, and
//   yields a RemoteAbiHeap object representing the segment to be filled in.
//
//   During the lifetime of RemoteAbiHeap, the RemoteAbiSpan and
//   RemoteAbiString placeholders can be passed to the Local and Remote
//   methods.  Local returns a mutable std::span through which heap objects can
//   be filled in with contents.  Remote returns AbiSpan or AbiStringView
//   objects appropriate for storing elsewhere in the remote ABI heap.  (Note
//   that string contents are supplied during the layout phase and
//   Local(RemoteAbiString) doesn't really get used to fill them in, while
//   Local(RemoteAbiSpan<T>) is necessary to populate the other heap arrays.)
//
//   The Local(size_type) method is used without a placeholder object to get a
//   mutable reference through which the fixed-address ABI symbol objects in
//   the original stub data segment can be filled in.
//
// * Commit
//
//   Finally, RemoteAbiHeap::Commit() consumes the RemoteAbiHeap object and
//   reifies the data written via Local pointers into the data segment VMO in
//   the RemoteLoadModule presented to RemoteAbiHeap::Create.  Thereafter, the
//   stub module can be loaded into the remote address space like other
//   modules.  Note that the stub module's layout details are fixed by Create,
//   so remote address space layout can be done before Commit.  Loading is
//   possible before Commit, but would map in the segment VMO before all its
//   contents have been fixed.

// Forward declarations for type defined below.
class RemoteAbiHeapLayout;
template <class Elf, class AbiTraits>
class RemoteAbiHeap;

// RemoteAbiSpan<T> represents an array of T in the remote heap.  It's created
// during the Layout phase by RemoteAbiHeapLayout::Allocate.  In the Data phase
// RemoteAbiHeap::Local converts it to a std::span<T> where data gets written,
// and RemoteAbiHeap::Remote converts it to a remote AbiSpan<T> that represents
// the data in the remote address space where it will be after Commit.
template <typename T>
class RemoteAbiSpan {
 public:
  constexpr size_t size() const { return count_; }

  constexpr size_t size_bytes() const { return count_ * sizeof(T); }

 private:
  friend RemoteAbiHeapLayout;
  template <class Elf, class AbiTraits>
  friend class RemoteAbiHeap;

  size_t offset_ = 0;
  size_t count_ = 0;
};

// RemoteAbiString is to std::string_view as RemoteAbiSpan<T> is to span<T>.
// It's created during the Layout phase by RemoteAbiHeapLayout::AddString.  In
// the Data phase RemoteAbiHeap::Local converts it to a std::span<const char>
// where the string (with implicit NUL terminator) can be seen during the
// lifetime of the RemoteAbiHeap, and RemoteAbiHeap::Remote converts it to a
// remote AbiStringView that represents the strings in the remote address space
// where it will be after Commit.  (Note that a NUL terminator is guaranteed to
// be present after the bytes descirbed by the AbiStringView.)
class RemoteAbiString {
 public:
  constexpr size_t size() const { return size_; }

  constexpr bool empty() const { return size_ == 0; }

 private:
  friend RemoteAbiHeapLayout;
  template <class Elf, class AbiTraits>
  friend class RemoteAbiHeap;

  size_t offset_ = 0;
  size_t size_ = 0;
};

// RemoteAbiHeapLayout represents the layout phase of remote ABI setup.  It's a
// move-only object that contains string contents for any strings added to the
// layout via the AddString method.  After Allocate and AddString calls, it
// must passed by value (moved) in the RemoteAbiHeap::Create call (see below).
class RemoteAbiHeapLayout {
 public:
  RemoteAbiHeapLayout() = delete;

  // The layout is constructed to leave space for the stub's data segment.
  // The argument should come from RemoteAbiStub::data_size().
  explicit RemoteAbiHeapLayout(size_t data_size) : size_{data_size} {}

  RemoteAbiHeapLayout(const RemoteAbiHeapLayout&) = delete;
  RemoteAbiHeapLayout(RemoteAbiHeapLayout&&) = default;

  // Allocate heap space for an array of T.  The resulting RemoteAbiSpan<T> can
  // be used later to populate the array once the heap is realized.
  template <typename T>
  RemoteAbiSpan<T> Allocate(size_t count) {
    // RemoteAbiHeap uses `new std::byte[size_bytes_]` and then expects offsets
    // within the buffer to be usably aligned.
    if (count == 0) {
      return {};
    }
    static_assert(alignof(T) <= __STDCPP_DEFAULT_NEW_ALIGNMENT__);
    size_ = (size_ + alignof(T) - 1) & -alignof(T);
    RemoteAbiSpan<T> result;
    result.offset_ = size_;
    result.count_ = count;
    size_ += count * sizeof(T);
    return result;
  }

  // Add a string to the heap.  This builds up a table of unique strings.  The
  // resulting RemoteAbiString can be used later to refer to this string in the
  // heap using RemoteAbiHeap::Remote.
  RemoteAbiString AddString(std::string_view str) {
    RemoteAbiString result;
    if (!str.empty()) {
      auto [it, added] = strings_.emplace(str, strtab_size_);
      if (added) {
        // It wasn't already in the string table, so append it along with space
        // for its NUL terminator.  The new map entry already holds its new
        // offset in the table since that was the value of strtab_.size()
        // before appending the new string.
        strtab_size_ += str.size() + 1;
      };
      result.offset_ = it->second;
      result.size_ = str.size();
    }
    return result;
  }

 private:
  template <class Elf, class AbiTraits>
  friend class RemoteAbiHeap;

  size_t size_ = 0;
  size_t strtab_size_ = 0;
  std::unordered_map<std::string, size_t> strings_;
};

// RemoteAbiHeap represents the heap during the data phase.  Creating it
// consumes a RemoteAbiHeapLayout, and also takes the same value passed to the
// RemoteAbiHeapLayout constructor from RemoteAbiStub::data_size() and a
// mutable RemoteLoadModule describing the same stub dynamic linker ELF file
// presented to RemoteAbiStub::Init.
template <class RemoteModule, class AbiTraits>
class RemoteAbiHeap {
 public:
  using Elf = RemoteModule::Elf;
  using size_type = Elf::size_type;
  using Addr = Elf::Addr;
  using Phdr = Elf::Phdr;
  using AbiStringView = elfldltl::AbiStringView<Elf, AbiTraits>;
  template <typename T>
  using AbiPtr = elfldltl::AbiPtr<T, Elf, AbiTraits>;
  template <typename T>
  using AbiSpan = elfldltl::AbiSpan<T, std::dynamic_extent, Elf, AbiTraits>;
  using LoadInfo = RemoteModule::LoadInfo;
  using StubConstantSegment = LoadInfo::ConstantSegment;

  RemoteAbiHeap(RemoteAbiHeap&&) = default;
  RemoteAbiHeap& operator=(RemoteAbiHeap&&) = default;

  // Create the RemoteAbiHeap by modifying the RemoteLoadModule for the stub
  // dynamic linker, using the same original data segment size passed to the
  // RemoteAbiHeapLayout constructor from RemoteAbiStub::data_size().  This
  // both modifies the RemoteLoadModule's final segment and references that
  // segment's VMO, so that RemoteLoadModule must remain live for the lifetime
  // of the RemoteAbiHeap object.
  template <class Diagnostics>
  static zx::result<RemoteAbiHeap> Create(Diagnostics& diagnostics, size_type stub_data_size,
                                          RemoteModule& stub_module, RemoteAbiHeapLayout layout) {
    if (!stub_module.PrepareLoadInfo(diagnostics)) [[unlikely]] {
      return zx::error{ZX_ERR_NO_MEMORY};
    }

    // Now that everything else is allocated, allocate space for the strings.
    RemoteAbiSpan<char> strtab = layout.Allocate<char>(std::max<size_t>(layout.strtab_size_, 1));

    const size_type memsz = static_cast<size_type>(layout.size_);
    auto replace = [stub_data_size, memsz, &diagnostics,
                    &file_vmo = stub_module.decoded().vmo()](auto old_segment) {
      assert(old_segment.memsz() >= stub_data_size);
      std::ignore = stub_data_size;  // For NDEBUG, capture optimized out.
      return ReplaceSegment(diagnostics, std::move(old_segment), file_vmo, memsz);
    };
    if (auto new_segment = std::visit(replace, stub_module.load_info().RemoveLastSegment());
        new_segment.is_ok()) {
      if (!stub_module.load_info().AddSegment(diagnostics, *std::move(new_segment), false)) {
        return zx::error{ZX_ERR_NO_MEMORY};
      }
    } else {
      return new_segment.take_error();
    }
    auto& new_segment = std::get<StubConstantSegment>(stub_module.load_info().segments().back());

    // Build the RemoteAbiHeap.
    RemoteAbiHeap heap;
    heap.strtab_ = static_cast<size_type>(strtab.offset_);
    heap.size_bytes_ = memsz;

    // Map the data (or allocate a buffer for it).
    if (auto result = heap.MapData(stub_data_size, new_segment.vmo()); result.is_error()) {
      return result.take_error();
    }

    // Fill in the string table.  Each string has a NUL terminator.
    std::span<char> local_strtab = heap.Local(strtab);
    for (const auto& [str, offset] : layout.strings_) {
      std::span<char> dest = local_strtab.subspan(offset, str.size() + 1);
      memcpy(dest.data(), str.c_str(), dest.size());
    }
    // If there are no strings, the table is just one NUL.  Thus the mapping of
    // the empty string to the last char of the table always has a terminator.
    local_strtab.back() = '\0';

    return zx::ok(std::move(heap));
  }

  // The heap size is fixed when the Buffer is created.  It can be used to
  // size a mapping location whose vaddr will be passed into Remote calls.
  size_type size_bytes() const { return size_bytes_; }

  // After the stub module's load_bias() has been fixed, this yields the
  // runtime vaddr of the stub module's heap segment: the vaddr argument for
  // the Remote() methods below.
  //
  // This should only be called on a stub_module that was previously modified
  // by a successful Create() call, and has since had its load_bias() set.
  static size_type HeapVaddr(const RemoteModule& stub_module) {
    const auto& segment = stub_module.load_info().segments().back();
    const size_type segment_vaddr =
        std::visit([](const auto& segment) { return segment.vaddr(); }, segment);
    return segment_vaddr + stub_module.load_bias();
  }

  // Return a local mutable reference to a global of type T at a known offset
  // in the stub data segment.
  template <typename T>
  T& Local(size_type offset) {
    assert(offset % alignof(T) == 0);
    assert(offset < size_bytes_);
    assert(size_bytes_ - offset >= sizeof(T));
    RemoteAbiSpan<T> remote;
    remote.offset_ = offset;
    remote.count_ = 1;
    return Local(remote).front();
  }

  // The string table is already written, so each RemoteAbiString yields a
  // constant span of the image where that NUL-terminated string already lies
  // in the table.
  std::span<const char> Local(RemoteAbiString str) const {
    std::span strtab_image = image().subspan(strtab_);
    std::span<const char> strtab{
        reinterpret_cast<const char*>(strtab_image.data()),
        strtab_image.size(),
    };

    if (str.size_ == 0) {
      return strtab.last<1>();
    }
    return strtab.subspan(str.offset_, str.size_);
  }

  // The local span<T> contents in the RemoteAbiHeap can now be filled in.
  // The return span remains valid until Commit() is called (see below).
  template <typename T>
  std::span<T> Local(RemoteAbiSpan<T> span) {
    size_t bytes_count = span.count_ * sizeof(T);
    std::span bytes = image().subspan(span.offset_, bytes_count);
    return std::span{reinterpret_cast<T*>(bytes.data()), span.count_};
  }

  // Given the remote virtual address where the segment will be mapped,
  // RemoteAbiString and RemoteAbiSpan placeholders can be turned into remote
  // AbiSpan objects ready to be copied into place.

  AbiSpan<char> Remote(size_type vaddr, RemoteAbiString str) const {
    using Ptr = typename AbiSpan<char>::Ptr;
    if (str.size_ == 0) {
      // There is always a NUL at the end of the string table, which is at
      // the end of the data.  So just point there for the empty string.
      vaddr += size_bytes_ - 1;
    } else {
      vaddr += static_cast<size_type>(strtab_ + str.offset_);
    }
    return AbiSpan<char>{
        Ptr::FromAddress(vaddr),
        static_cast<size_type>(str.size_ + 1),
    };
  }

  template <typename T>
  AbiSpan<T> Remote(size_type vaddr, RemoteAbiSpan<T> span) const {
    return AbiSpan<T>{
        AbiSpan<T>::Ptr::FromAddress(vaddr + static_cast<size_type>(span.offset_)),
        static_cast<size_type>(span.count_),
    };
  }

  // Remove the original mutable segment and turn it into the larger
  // read-only segment.  It can be either a DataWithZeroFillSegment that
  // might have an existing VMO, or a ZeroFillSegment that never does.
  //
  // Private subroutine of Create; public only for unit testing.
  template <class Diagnostics, class Segment>
  static zx::result<StubConstantSegment> ReplaceSegment(Diagnostics& diagnostics,
                                                        Segment&& old_segment,
                                                        const zx::vmo& file_vmo,
                                                        size_type segment_size) {
    assert(file_vmo);

    const size_type page_size = static_cast<size_type>(RemoteModule::Loader::page_size());
    const size_type memsz = (segment_size + page_size - 1) & -page_size;
    // TODO(https://fxbug.dev/468047456): This should be const too, but in
    // C++23 mode that makes it constexpr and the lambda capture of a local
    // constexpr is superfluous and gets a warning.
    size_type filesz = old_segment.filesz();

    StubConstantSegment new_segment{
        old_segment.offset(),
        old_segment.vaddr(),
        memsz,
        Phdr::kRead,
    };

    // Replacing a ZeroFillSegment.  Create a fresh VMO of the new size.
    auto zero_fill = [memsz, &new_segment]() -> zx::result<StubConstantSegment> {
      if (zx_status_t status = zx::vmo::create(memsz, 0, &new_segment.vmo()); status != ZX_OK)
          [[unlikely]] {
        return zx::error{status};
      }
      return zx::ok(std::move(new_segment));
    };

    // Just like the ZeroFillSegment case, but then write data into it.
    auto eager_data = [filesz, zero_fill](
                          const zx::vmo& data_vmo,
                          size_type data_offset) -> zx::result<StubConstantSegment> {
      std::unique_ptr<std::byte[]> buffer{new std::byte[filesz]};
      zx_status_t status = data_vmo.read(buffer.get(), data_offset, filesz);
      if (status != ZX_OK) {
        return zx::error{status};
      }
      auto result = zero_fill();
      if (result.is_ok()) {
        status = result->vmo().write(buffer.get(), 0, filesz);
        if (status != ZX_OK) {
          return zx::error{status};
        }
      }
      return result;
    };

    // Make a COW child VMO that's larger.
    auto lazy_data = [filesz, &new_segment](
                         const zx::vmo& data_vmo,
                         size_type data_offset) -> zx::result<StubConstantSegment> {
      zx_status_t status =
          data_vmo.create_child(ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE | ZX_VMO_CHILD_RESIZABLE,
                                data_offset, filesz, &new_segment.vmo());
      if (status != ZX_OK) {
        return zx::error{status};
      }
      // If filesz is not page-aligned and SegmentWithVmo::AlignSegments (or
      // build-time equivalent) wasn't used to guarantee a zero-filled partial
      // page, then data from the file might be there.  But those nonzero .bss
      // bytes are part of the ABI symbols that will be overwritten anyway.  If
      // there are unused gaps they will only leak the contents of the original
      // stub ELF file, which are not secret anyway, and those bytes will be,
      // well, unused.  Finally, extend the VMO to include the zero-fill space.
      if (new_segment.memsz() > filesz) {
        status = new_segment.vmo().set_size(new_segment.memsz());
        if (status != ZX_OK) {
          return zx::error{status};
        }
      }
      return zx::ok(std::move(new_segment));
    };

    // Replacing a DataWithZeroFillSegment.  Create a new VMO that contains the
    // file contents.  If there's less than a page of file contents, just copy
    // it into a fresh VMO since the rest of the page will be touched to write
    // the heap anyway.  If there's a page or more, then it can be shared with
    // the file (or previously created VMO).
    auto with_data = [filesz, eager_data, lazy_data, page_size](
                         const zx::vmo& data_vmo,
                         size_type data_offset) -> zx::result<StubConstantSegment> {
      return filesz < page_size ? eager_data(data_vmo, data_offset)
                                : lazy_data(data_vmo, data_offset);
    };

    if constexpr (elfldltl::kSegmentHasFilesz<Segment>) {
      if (old_segment.filesz() > 0) {
        // If there is already a segment-specific VMO (possibly a handle
        // duplicated from the prototype stub module for example after
        // SegmentWithVmo::AlignSegments), copy from that instead of the
        // original file VMO.
        return old_segment.vmo() ? with_data(old_segment.vmo(), 0)
                                 : with_data(file_vmo, old_segment.offset());
      }
      assert(!old_segment.vmo());
    }

    return zero_fill();
  }

  // When a RemoteAbiHeap has been filled and is ready to be mapped into a
  // process, Commit() yields the VMO.  It should then be mapped at the same
  // vaddr that was used in Remote calls when filling pointers in the data.
  // This must be the last method called and must be called using std::move.
  // This invalidates all previous return values from Local.
  zx::result<> Commit() && {
    if (auto* copied = std::get_if<Copied>(&data_)) {
      // The data is in a local buffer, so it has to be written to the VMO.
      std::unique_ptr buffer = std::exchange(copied->buffer, {});
      assert(*copied->vmo);
      zx_status_t status = copied->vmo->write(buffer.get(), 0, size_bytes_);
      if (status != ZX_OK) {
        return zx::error{status};
      }
    }
    return zx::ok();
  }

 private:
  using StubSegment = typename LoadInfo::Segment;

  // Tune this to trade off vmo_write syscall vs map/unmap overhead.
  static inline const size_t kVmoWriteMapThreshold = zx_system_get_page_size();

  RemoteAbiHeap() = default;

  struct Copied {
    std::unique_ptr<std::byte[]> buffer;
    zx::unowned_vmo vmo;
  };
  using Mapped = elfldltl::MappedVmoFile;

  std::span<std::byte> image() {
    if (auto* mapped = std::get_if<Mapped>(&data_)) {
      return mapped->image();
    }
    return {std::get<Copied>(data_).buffer.get(), size_bytes_};
  }

  zx::result<> MapData(size_type stub_data_size, const zx::vmo& vmo) {
    size_type memsz = stub_data_size + size_bytes_;

    if (memsz >= kVmoWriteMapThreshold) {
      // It's a big segment, so map in the VMO to write directly.
      data_.template emplace<elfldltl::MappedVmoFile>();
      return std::get<elfldltl::MappedVmoFile>(data_).InitMutable(vmo.borrow(), memsz);
    }

    // Just fill a local buffer to be written out later.
    data_ = Copied{
        .buffer{new std::byte[size_bytes_]},
        .vmo{vmo.borrow()},
    };
    return zx::ok();
  }

  std::variant<Copied, Mapped> data_;
  size_type strtab_ = 0;
  size_type size_bytes_ = 0;
};

}  // namespace ld

#endif  // LIB_LD_REMOTE_ABI_HEAP_H_

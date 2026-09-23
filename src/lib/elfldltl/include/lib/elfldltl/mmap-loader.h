// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_MMAP_LOADER_H_
#define SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_MMAP_LOADER_H_

#include <lib/fit/result.h>
#include <sys/mman.h>
#include <unistd.h>

#include <cassert>
#include <cerrno>
#include <concepts>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <utility>

#include "diagnostics.h"
#include "memory.h"
#include "posix.h"

namespace elfldltl {

// elfldltl::MmapLoader uses something like the POSIX mmap / munmap / mprotect
// API to implement PT_LOAD processing based on elfldltl::LoadInfo.  It expects
// the POSIX semantics about mapping, overmapping, COW, and protections.
//
// It uses a dependency-injection object meeting the MmapperApi concept to do
// the actual calls.  This can be used for mocking in tests, or to implement
// some sort of proxy that maps ELF file offsets to different places in a
// larger archive (with internal page alignment as required), etc.
template <class T>
concept MmapperApi = requires {
  requires std::movable<T>;

  // Returns the page size to use for these mapping operations.
  { T::page_size() } -> std::convertible_to<size_t>;

  // This is the type passed to MMapLoader::Load and on to T::MapFromFile.
  typename T::File;
} && requires(T mmapper, size_t size, void* ptr, int prot, off_t offset, T::File fd) {
  // Methods return fit::result<int, ...> with errno codes as error_value().
  // It's fine if they also clobber errno; the MmapLoader class API makes no
  // guarantees about the errno state.

  // This creates an address space reservation (size is always a multiple of
  // what page_size() returned) within which mappings will go, or fails with an
  // errno code.  The returned pointer is the place in the local address space
  // that was reserved.  If this fails, then no other methods will be called.
  // On success, only this same object will be used for the following methods
  // and they will only be passed this whole-page address ranges inside
  { mmapper.Reserve(size) } -> std::same_as<fit::result<int, void*>>;

  // This reclaims the address space returned by a previous Reserve() call on
  // the same object; mapping calls may have been made (success or failure).
  { mmapper.Reclaim(ptr, size) };

  // This is called with a page-aligned range inside what Reserve() reserved.
  // It maps writable zero-fill pages there, or fails with an errno code.
  { mmapper.MapZeroFill(ptr, size, prot) } -> std::same_as<fit::result<int>>;

  // This is called with a page-aligned range inside what Reserve() reserved.
  // It maps pages from the file there with the given PROT_* flags.
  { mmapper.MapFromFile(ptr, size, prot, fd, offset) } -> std::same_as<fit::result<int>>;

  // This is called with a misaligned range inside what Reserve() reserved.
  // It reads contents (less than a page) from the file at the given offset.
  { mmapper.ReadFromFile(ptr, size, fd, offset) } -> std::same_as<fit::result<int>>;

  // This is called with a range of aligned, whole pages previously mapped by
  // MapZeroFill and/or MapFromFile as writable.  Change them to PROT_READ.
  { mmapper.MakeReadOnly(ptr, size) } -> std::same_as<fit::result<int>>;
};

// The default implementation just uses the POSIX <sys/mman.h> calls directly.
struct PosixMmapper {
  using File = int;

  [[gnu::const]] static size_t page_size() { return sysconf(_SC_PAGESIZE); }

  fit::result<int, void*> Reserve(size_t vaddr_size) const {
    void* ptr = mmap(nullptr, vaddr_size, PROT_NONE, MAP_ANON | MAP_PRIVATE, -1, 0);
    if (ptr == MAP_FAILED) [[unlikely]] {
      return fit::error{errno};
    }
    return fit::ok(ptr);
  }

  void Reclaim(void* ptr, size_t size) const { munmap(ptr, size); }

  fit::result<int> MapZeroFill(void* ptr, size_t size, int prot) const {
    if (mmap(ptr, size, prot, MAP_FIXED | MAP_PRIVATE | MAP_ANON, -1, 0) == MAP_FAILED)
        [[unlikely]] {
      return fit::error{errno};
    }
    return fit::ok();
  }

  fit::result<int> MapFromFile(void* ptr, size_t size, int prot, int fd, off_t offset) const {
    if (mmap(ptr, size, prot, MAP_FIXED | MAP_PRIVATE, fd, offset) == MAP_FAILED) [[unlikely]] {
      return fit::error{errno};
    }
    return fit::ok();
  }

  fit::result<int> ReadFromFile(void* ptr, size_t size, int fd, off_t offset) const {
    ssize_t n = pread(fd, ptr, size, offset);
    if (n < 0) [[unlikely]] {
      return fit::error{errno};
    }
    if (n != static_cast<ssize_t>(size)) [[unlikely]] {
      return fit::error{EIO};
    }
    return fit::ok();
  }

  fit::result<int> MakeReadOnly(void* ptr, size_t size) const {
    if (mprotect(ptr, size, PROT_READ) != 0) [[unlikely]] {
      return fit::error{errno};
    }
    return fit::ok();
  }
};
static_assert(MmapperApi<PosixMmapper>);

template <MmapperApi Mmapper = PosixMmapper>
class MmapLoader {
 public:
  // This is returned by Commit(), which completes the use of an MmapLoader.
  // It represents the capability to apply RELRO protections to a loaded image.
  // Unlike the MmapLoader object itself, its lifetime is not tied to the image
  // mappings.  After Commit(), the image mapping won't be destroyed by the
  // MmapLoader's destructor.
  //
  // Note this object holds the Mmapper object moved from the creating
  // MmapLoader, and will that same object for its MakeReadOnly call.
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
    [[nodiscard]] bool Commit(auto& diag) && {
      if (start_) {
        auto result = mapper_.MakeReadOnly(start_, size_);
        if (result.is_error()) [[unlikely]] {
          diag.SystemError("cannot protect PT_GNU_RELRO region: ",
                           PosixError{result.error_value()});
          return false;
        }
      }
      return true;
    }

   private:
    friend MmapLoader;

    Relro(Mmapper&& mapper, const auto& region, uintptr_t load_bias) : mapper_{std::move(mapper)} {
      if (!region.empty()) {
        start_ = reinterpret_cast<void*>(region.start + load_bias);
        size_ = region.size();
      }
    }

    void* start_ = nullptr;
    size_t size_ = 0;
    [[no_unique_address]] Mmapper mapper_;
  };

  MmapLoader()
    requires std::default_initializable<Mmapper>
      : MmapLoader(Mmapper{}) {}

  explicit MmapLoader(size_t page_size)
    requires std::default_initializable<Mmapper>
      : MmapLoader(Mmapper{}, page_size) {}

  explicit MmapLoader(Mmapper mapper)
      : mapper_(std::move(mapper)), page_size_(Mmapper::page_size()) {}

  explicit MmapLoader(Mmapper mapper, size_t page_size)
      : mapper_(std::move(mapper)), page_size_(page_size) {}

  MmapLoader(MmapLoader&& other) noexcept
      : memory_{std::exchange(other.memory_, {})}, page_size_(other.page_size_) {}

  MmapLoader& operator=(MmapLoader&& other) noexcept {
    memory_ = std::exchange(other.memory_, {});
    page_size_ = other.page_size_;
    return *this;
  }

  ~MmapLoader() {
    if (!image().empty()) {
      munmap(image().data(), image().size());
    }
  }

  [[gnu::const]] size_t page_size() const { return page_size_; }

  // This takes a LoadInfo object describing segments to be mapped in and an
  // opened fd from which the file contents should be mapped. It returns true
  // on success and false otherwise, in which case a diagnostic will be emitted
  // to diag.
  //
  // When Load() is called, one should assume that the address space of the
  // caller has a new mapping whether the call succeeded or failed. The mapping
  // is tied to the lifetime of the MmapLoader until Commit() is
  // called. Without committing, the destructor of the MmapLoader will destroy
  // the mapping.
  //
  // Logically, Commit() isn't sensible after Load has failed.
  template <class Diagnostics, class LoadInfo>
  [[nodiscard]] bool Load(Diagnostics& diag, const LoadInfo& load_info, const Mmapper::File& fd) {
    // Make a mapping large enough to fit all segments. This mapping will be
    // placed wherever the OS wants, achieving ASLR. We will later map the
    // segments at their specified offsets into this mapping. PROT_NONE is
    // important so that any holes in the layout of the binary will trap if
    // touched.
    fit::result<int, void*> map = mapper_.Reserve(load_info.vaddr_size());
    if (map.is_error()) [[unlikely]] {
      return diag.SystemError("couldn't mmap address range of size", load_info.vaddr_size(), ": ",
                              PosixError{map.error_value()});
    }
    memory_.set_image({static_cast<std::byte*>(*map), load_info.vaddr_size()});
    memory_.set_base(load_info.vaddr_start());

    constexpr auto prot = [](const auto& s) constexpr {
      return (s.readable() ? PROT_READ : 0) | (s.writable() ? PROT_WRITE : 0) |
             (s.executable() ? PROT_EXEC : 0);
    };

    // Load segments are divided into 2 or 3 regions depending on segment.
    // [file pages]*[intersecting page]?[anon pages]*
    //
    // * "file pages" are present when filesz > 0
    // * "anon pages" are present when memsz > filesz.
    // * "intersecting page" exists when both file pages and anon pages exist,
    //   and file pages are not an exact multiple of pagesize. At most a **single**
    //   intersecting page exists.
    //
    // **Note:**: The MmapLoader performs only two mappings.
    //    * Mapping file pages up to the last full page of file data.
    //    * Mapping anonymous pages, including the intersecting page, to the end of the segment.
    //
    // After the second mapping, the MmapLoader then reads in the partial file data into the
    // intersecting page.
    //
    // The alternative would be to map filesz page rounded up into memory and then zero out the
    // zero fill portion of the intersecting page. This isn't preferable because we would
    // immediately cause a page fault and spend time zero'ing a page when the OS may already have
    // copied this page for us.
    auto mapper = [base = reinterpret_cast<std::byte*>(*map), vaddr_start = load_info.vaddr_start(),
                   prot, &fd, &diag, this](const auto& segment) {
      std::byte* addr = base + (segment.vaddr() - vaddr_start);
      size_t map_size = segment.filesz();
      size_t zero_size = 0;
      size_t copy_size = 0;
      if (segment.memsz() > segment.filesz()) {
        copy_size = map_size & (page_size() - 1);
        map_size &= -page_size();
        zero_size = segment.memsz() - map_size;
      }

      if (map_size > 0) {
        auto result = mapper_.MapFromFile(addr, map_size, prot(segment), fd, segment.offset());
        if (result.is_error()) [[unlikely]] {
          diag.SystemError("couldn't mmap ", map_size, " bytes at offset ", segment.offset(), ": ",
                           PosixError{result.error_value()});
          return false;
        }
        addr += map_size;
      }
      if (zero_size > 0) {
        auto result = mapper_.MapZeroFill(addr, zero_size, prot(segment));
        if (result.is_error()) [[unlikely]] {
          diag.SystemError("couldn't mmap ", zero_size,
                           " anonymous bytes: ", PosixError{result.error_value()});
          return false;
        }
      }
      if (copy_size > 0) {
        auto result = mapper_.ReadFromFile(addr, copy_size, fd, segment.offset() + map_size);
        if (result.is_error()) [[unlikely]] {
          diag.SystemError("couldn't pread ", copy_size, " bytes ",
                           FileOffset{segment.offset() + map_size},
                           PosixError{result.error_value()});
          return false;
        }
      }

      return true;
    };

    return load_info.VisitSegments(mapper);
  }

  // After Load(), this is the bias added to the given LoadInfo::vaddr_start()
  // to find the runtime load address.
  uintptr_t load_bias() const {
    return reinterpret_cast<uintptr_t>(image().data()) - memory_.base();
  }

  // This returns the DirectMemory of the mapping created by Load(). It should not be used after
  // destruction or after Commit(). If Commit() has been called before destruction then the
  // address range will continue to be usable, in which case one should save the object's
  // image() before Commit().
  DirectMemory& memory() { return memory_; }

  // Commit is used to keep the mapping created by Load around even after the
  // MmapLoader object is destroyed.  It takes a RELRO region as returned by
  // LoadInfo::RelroBounds, and yields a Relro object (see above).  This method
  // is inherently the last thing called on the object if it is used.  Use like
  // `auto relro = std::move(loader).Commit(relro_bounds);`.  After any
  // relocation modifications to mapped segment memory, call
  // `std::move(relro).Commit();`.
  //
  // Note this moves the Mmapper object used at construction into the returned
  // Relro object.  That same object will be used for the MakeReadOnly call
  // made by Relro::Commit().
  template <class Region>
  [[nodiscard]] Relro Commit(const Region& relro_bounds) && {
    Relro relro{std::move(mapper_), relro_bounds, load_bias()};
    memory_.set_image({});
    return relro;
  }

 private:
  std::span<std::byte> image() const { return memory_.image(); }
  uintptr_t base() const { return memory_.base(); }

  DirectMemory memory_;
  [[no_unique_address]] Mmapper mapper_;
  size_t page_size_;
};

// Deduction guides.

template <class Mmapper>
MmapLoader(Mmapper, size_t) -> MmapLoader<Mmapper>;

template <std::convertible_to<size_t> Size>
MmapLoader(Size) -> MmapLoader<>;

template <class Mmapper>
MmapLoader(Mmapper) -> MmapLoader<Mmapper>;

}  // namespace elfldltl

#endif  // SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_MMAP_LOADER_H_

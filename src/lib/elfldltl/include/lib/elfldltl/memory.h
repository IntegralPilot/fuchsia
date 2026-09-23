// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_MEMORY_H_
#define SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_MEMORY_H_

#include <cassert>
#include <concepts>
#include <cstddef>
#include <cstdint>
#include <functional>
#include <memory>
#include <optional>
#include <span>
#include <string_view>
#include <tuple>
#include <type_traits>

namespace elfldltl {

// Various interfaces require a File or Memory type to access data structures.
//
// This header specifies the API contracts those template interfaces require,
// and provides an implementation for the simplest case.
//
// Both File and Memory types are not copied or moved, only used by reference.
// Each interface uses either the File API or the Memory API, but both APIs
// can be implemented by a single object when appropriate.
//
// The primary distinction between File and Memory is that methods for reading
// from File handle ownership since copying data into a local buffer may be
// required: the lifetime of the data read is tied to an owned object returned
// by the call (though it should be presumed that those objects are also only
// valid for the lifetime of the File object itself).  The Methods for reading
// from Memory instead simply return pointers (really references or spans) into
// mapped memory whose lifetime is that of the Memory object itself.
//
// The Memory API also has a write aspect, whereas File does only reading.  The
// Memory API is really the union of MemoryReader and MemoryWriter APIs.
//
// The methods on these objects all require an explicit template parameter for
// the datum type to be accessed in the File or Memory object.  This means
// there can't be a single C++20 concept for File or for Memory generically.
// Instead, each concept is parameterized for a particular list of types that
// its methods must work with.

template <class Memory, typename Size, typename T>
concept MemoryReaderForType =
    std::integral<Size> && requires(Memory& memory, Size address, Size count, T value) {
      // MemoryReader classes must provide these methods, which take a memory
      // address as used in the ELF metadata in this file, guaranteed to be
      // correctly aligned with respect to T.  They just return false for any
      // kind of failure (whether an invalid address range in the call, or an
      // underlying error in the backing memory), and the Memory object should
      // do its own error reporting by other means as a side effect.
      {
        // This returns a view of T[count] if that's accessible at the address.
        // The data must be permanently accessible for the lifetime of the
        // Memory object.
        memory.template ReadArray<T>(address, count)
      } -> std::same_as<std::optional<std::span<const T>>>;
      {
        // This is the same but for when the caller doesn't know the size of
        // the array.  So this returns a view of T[n] for some n > 0 that is
        // accessible, as much as is possibly accessible for valid RODATA in
        // the ELF file's memory image.  The caller will be doing random-access
        // that will only access the "actually valid" indices of the returned
        // span if the rest of the input data (e.g. relocation records) is also
        // valid.  Access past the true size of the array may return garbage
        // (unrelated parts of the ELF file's load image in memory), but
        // reading from pointers into anywhere in the span returned will at
        // least be safe to perform (for the lifetime of the Memory object).
        memory.template ReadArray<T>(address)
      } -> std::same_as<std::optional<std::span<const T>>>;
    };

// A general-purpose MemoryReader should work for all T, but templates taking a
// Memory object will constrain it to the types they actually use.
template <class Memory, typename Size, typename... Ts>
concept MemoryReader = (MemoryReaderForType<Memory, Size, Ts> && ...);

template <class Memory, typename Size, typename T>
concept MemoryWriterForType =
    std::integral<Size> && requires(Memory& memory, Size address, T value) {
      // MemoryWriter classes must provide these methods, which take a memory
      // address as used in the ELF metadata in this file, guaranteed to be
      // correctly aligned with respect to T.  They just return std::nullopt
      // for any kind of failure (whether an invalid address range in the call,
      // or an underlying error in the backing memory), and the Memory object
      // should do its own error reporting by other means as a side effect.
      {
        // This stores a T at the given address, which is in some writable
        // segment of the file previously arranged with this Memory object.  It
        // returns false if processing should fail early.  Note the explicit
        // template argument is always used to indicate the type whose
        // operator= will be called on the actual memory, so it is of the
        // explicitly intended width and can be a byte-swapping type.  The
        // value argument might be of the same type or of any type convertible
        // to it.
        memory.template Store<T>(address, value)
      } -> std::convertible_to<bool>;

      {
        // This is like Store but it adds the argument to the word already in
        // place, i.e. the in-place addend.  Note T::operator= is always
        // called, not T::operator+=.
        memory.template StoreAdd<T>(address, value)
      } -> std::convertible_to<bool>;
    };

// A general-purpose MemoryWriter should work for all T, but templates taking a
// Memory object will constrain it to the types they actually use.
template <class Memory, typename Size, typename... Ts>
concept MemoryWriter = (MemoryWriterForType<Memory, Size, Ts> && ...);

// A fully-general Memory object does all of them.
template <class Memory, typename Size, typename... Ts>
concept MemoryApi = MemoryReader<Memory, Size, Ts...> && MemoryWriter<Memory, Size, Ts...>;

// The File API methods return std::optional types, where they each can return
// std::nullopt for any kind of failure (whether an invalid address range in
// the call, or an underlying error in the backing memory); the File object
// does its own error reporting by other means as a side effect (rather than
// the caller doing anything but safely handling the failure to get that value
// and deciding whether to continue processing).
//
// Each exact value_type inside these std::optional return values is an
// implementation detail of the particular File object.  Thosed returned values
// must obey the concepts defined below, as instantiated for each datum type.

// This just unwraps some std::optional<T> type into its T, giving a constraint
// error if the parameter isn't actually some std::optional<T>.
template <class Optional>
  requires std::same_as<Optional, std::optional<typename Optional::value_type>>
using OptionalOf = Optional::value_type;

// Successfully reading a single datum of type T yields a value of an
// OwnedResult<T>.  Basically, this means it can be used as if it's a T: either
// it's a T or it's equivalent to a const T& that's valid for the lifetime of
// the File object.
template <class Result, typename T>
concept OwnedResult = std::same_as<Result, T> || std::convertible_to<const T&, Result>;

// OptionalOwnedResult<T> is std::optional of some OwnedResult<T> type.
template <class OptionalResult, typename T>
concept OptionalOwnedResult = OwnedResult<OptionalOf<OptionalResult>, T>;

// The ReadFromFile<T> reads a single datum of fixed size.  If this aspect of
// the File API is used alone then on allocation is required.  However, the
// return value is nonetheless an owning object that (presumably) must be kept
// alive as long it's used as const T&.  It also must be presumed valid only
// during the lifetime of the File object itself.  Or it might just be T--but
// if not then it may need to be copied to an explicitly typed T to be used
// longer than the File object itself.
template <class File, typename T, typename Size = size_t>
concept CanReadFromFile = requires(File& file, Size offset, Size count) {
  { file.template ReadFromFile<T>(offset) } -> OptionalOwnedResult<T>;
};

// Reading a variable-sized result from a File object may require a buffer to
// copy into.  So the method takes an Allocator object that _may or may not_ be
// called by the File object to create a buffer to copy into.  In a given call
// to read data of type T from the File, the allocator object in that call is
// specialized to T and returns an "owned array" object (or std::nullopt or
// allocation failure).  Rather than a traditional container API per se, this
// is rather a possibly-owning object that can be converted to a span<const T>.

// OwnedResultArray<T> is an object that owns some memory resource but that can
// be converted to std::span<const T>.
template <typename Result, typename T>
concept OwnedResultArray = std::convertible_to<Result, std::span<const T>>;

// OptionalOwnedResultArray<T> is std::optional of an OwnedResultArray<T> type.
template <class OptionalResult, typename T>
concept OptionalOwnedResultArray = OwnedResultArray<OptionalOf<OptionalResult>, T>;

// The allocator passed to ReadArrayFromFile<T> methods must be a callable
// object that returns std::optional of an OwnedResultArray<T> type.
template <class Allocator, typename T, typename Size = size_t>
concept ReadArrayFromFileAllocator =
    std::integral<Size> && requires(Allocator&& allocator, Size count) {
      { allocator(count) } -> OptionalOwnedResultArray<T>;
    };

// The other method required in the File API uses an allocator object.
template <class File, typename T, typename Allocator, typename Size = size_t>
concept CanReadArrayFromFile =
    ReadArrayFromFileAllocator<Allocator, T, Size> &&
    requires(File& file, Size offset, Allocator&& allocator, Size count) {
      {
        file.template ReadArrayFromFile<T>(offset, allocator, count)
      } -> OptionalOwnedResultArray<T>;
    };

// The full File API requires both methods.  A general-purpose File should work
// for all T, but templates taking a File object will constrain it to the types
// they actually use.  Multiple ReadArrayFromFile<T> for different datum types
// T calls can't use the same allocator, but users of the File API will often
// often use a single template class to supply the allocator objects for all
// their calls, so the File object can be validated for calls made with
// allocator objects instantiated using the given Allocator template.
template <class File, template <typename> class Allocator, typename... Ts>
concept FileApiFor =
    ((CanReadFromFile<File, Ts> && CanReadArrayFromFile<File, Ts, Allocator<Ts>>) && ...);

// elfldltl::DirectMemory::ReadArrayFromFile ignores its Allocator argument,
// but other implementations need one.  A few common convenience Allocator
// implementations are provided here.

// This is the stub implementation of the Allocator API that can be used with
// DirectMemory or other implementations that never call it.
template <typename T>
struct NoArrayFromFile {
  // Define a Result type that has the right API.  It will never be returned.
  struct Result {
    constexpr explicit(false) operator std::span<const T>() const { return {}; }
    constexpr explicit(false) operator std::span<T>() { return {}; }
  };

  constexpr std::optional<Result> operator()(size_t size) const { return std::nullopt; }
};

// This returns an Allocator API object File::ReadArrayFromFile that uses a
// Container API instantiation (see <lib/elfldltl/container.h>).  The explicit
// template parameter should be a specific ...::Container<T> instantiation to
// be used with ReadArrayFromFile<T> (which should already meet the Allocator
// API's return value requirement of being coercible to std::span<T> as
// contiguous containers do).  Note that this uses Container::resize and then
// overwrites the default-constructed contents, so it's not best suited for
// optimized use cases that should avoid the redundant default construction.
template <class Container, class Diagnostics>
constexpr ReadArrayFromFileAllocator<typename Container::value_type> auto ContainerArrayFromFile(
    Diagnostics& diag, std::string_view error) {
  return [&diag, error](size_t size) -> std::optional<Container> {
    std::optional<Container> result{std::in_place};
    if (!result->resize(diag, error, size)) [[unlikely]] {
      return std::nullopt;
    }
    return result;
  };
}

// This is an implementation of the Allocator API for File::ReadArrayFromFile
// that uses a fixed buffer inside the object (i.e. on the stack).  It simply
// fails if more than MaxCount elements need to be read.
template <typename T, size_t MaxCount>
class FixedArrayFromFile {
 public:
  class Result {
   public:
    // For consistency with the minimal API requirement, this is move-only.
    constexpr Result() = default;
    Result(const Result&) = delete;
    constexpr Result(Result&&) noexcept = default;

    constexpr explicit Result(size_t size) : size_(size) {
      // Note the data_ elements are left uninitialized.
      assert(size_ <= std::size(data_));
    }

    Result& operator=(const Result&) noexcept = delete;
    constexpr Result& operator=(Result&&) noexcept = default;

    constexpr operator std::span<T>() { return std::span(data_).subspan(0, size_); }

    constexpr operator std::span<const T>() const { return std::span(data_).subspan(0, size_); }

    constexpr operator bool() const { return size_ > 0; }

   private:
    std::array<T, MaxCount> data_;
    size_t size_ = 0;
  };

  std::optional<Result> operator()(size_t size) const {
    if (size > MaxCount) {
      return std::nullopt;
    }
    return Result{size};
  }
};

// This does direct memory access to an ELF load image already mapped in.
// Addresses in the ELF metadata are relative to a given base address that
// corresponds to the beginning of the image this object points to.
class DirectMemory {
 public:
  DirectMemory() = default;

  // The Memory API is always used by lvalue reference, so Memory objects don't
  // need to be either copyable or movable.  But DirectMemory is really just a
  // pointer holder, so it can be easily copied.
  DirectMemory(const DirectMemory&) = default;

  // This takes a memory image and the file-relative address it corresponds to.
  // The one-argument form can be used to use the File API before the base is
  // known.  Then set_base must be called before using the Memory API.
  explicit DirectMemory(std::span<std::byte> image, uintptr_t base = ~uintptr_t{})
      : image_(image), base_(base) {}

  DirectMemory& operator=(const DirectMemory&) = default;

  std::span<std::byte> image() const { return image_; }
  void set_image(std::span<std::byte> image) { image_ = image; }

  uintptr_t base() const { return base_; }
  void set_base(uintptr_t base) { base_ = base; }

  // This returns an address in memory for an address in the loaded ELF file.
  template <typename T>
  T* GetPointer(uintptr_t ptr) const {
    if (ptr < base_ || ptr - base_ >= image_.size() ||
        image_.size() - (ptr - base_) < PointerSize<T>()) [[unlikely]] {
      return nullptr;
    }
    return reinterpret_cast<T*>(&image_[ptr - base_]);
  }

  // Given a an address range previously handed out by GetPointer,
  // ReadArrayFromFile, or ReadArray, yield the address value that
  // must have been passed to ReadArray et al.
  template <typename T>
  std::optional<uintptr_t> GetVaddr(std::span<const T> data) const {
    std::span bytes = std::as_bytes(data);
    if (bytes.data() < image_.data() || bytes.data() > &image_.back()) [[unlikely]] {
      return std::nullopt;
    }
    const size_t data_offset = bytes.data() - image_.data();
    if (image_.size_bytes() - data_offset < bytes.size_bytes()) [[unlikely]] {
      return std::nullopt;
    }
    return base_ + data_offset;
  }

  template <typename T>
  std::optional<uintptr_t> GetVaddr(const T* ptr) const {
    return GetVaddr(std::span{ptr, 1});
  }

  // File API assumes this file's first segment has page-aligned p_offset of 0.

  template <typename T>
  std::optional<std::reference_wrapper<const T>> ReadFromFile(size_t offset) {
    if (offset >= image_.size()) [[unlikely]] {
      return std::nullopt;
    }
    auto memory = image_.subspan(offset);
    if (memory.size() < sizeof(T)) [[unlikely]] {
      return std::nullopt;
    }
    return std::cref(*reinterpret_cast<const T*>(memory.data()));
  }

  template <typename T, typename Allocator>
  std::optional<std::span<const T>> ReadArrayFromFile(size_t offset, Allocator&& allocator,
                                                      size_t count) {
    auto data = ReadAll<T>(offset);
    if (data.empty() || count > data.size()) [[unlikely]] {
      return std::nullopt;
    }
    return data.subspan(0, count);
  }

  // Memory API assumes the image represents the PT_LOAD segment layout of the
  // file by p_vaddr relative to the base address (not the raw file image by
  // p_offset).

  template <typename T>
  std::optional<std::span<const T>> ReadArray(uintptr_t ptr, size_t count) {
    if (ptr < base_) [[unlikely]] {
      return std::nullopt;
    }
    return ReadArrayFromFile<T>(ptr - base_, NoArrayFromFile<T>(), count);
  }

  template <typename T>
  std::optional<std::span<const T>> ReadArray(uintptr_t ptr) {
    if (ptr < base_) [[unlikely]] {
      return std::nullopt;
    }
    auto data = ReadAll<T>(ptr - base_);
    if (data.empty()) [[unlikely]] {
      return std::nullopt;
    }
    return data;
  }

  // Note the argument is not of type T so that T can never be deduced from the
  // argument: the caller must use Store<T> explicitly to avoid accidentally
  // using the wrong type since lots of integer types are silently coercible to
  // other ones.  (The caller doesn't need to supply the U template parameter.)
  template <typename T, typename U>
  bool Store(uintptr_t ptr, U value) {
    if (auto word = GetPointer<T>(ptr)) [[likely]] {
      *word = value;
      return true;
    }
    return false;
  }

  // Note the argument is not of type T so that T can never be deduced from the
  // argument: the caller must use Store<T> explicitly to avoid accidentally
  // using the wrong type since lots of integer types are silently coercible to
  // other ones.  (The caller doesn't need to supply the U template parameter.)
  template <typename T, typename U>
  bool StoreAdd(uintptr_t ptr, U value) {
    if (auto word = GetPointer<T>(ptr)) [[likely]] {
      *word = *word + value;  // Don't assume T::operator+= works.
      return true;
    }
    return false;
  }

 private:
  template <typename T>
  std::span<const T> ReadAll(size_t offset) {
    if (offset >= image_.size()) [[unlikely]] {
      return {};
    }
    auto memory = image_.subspan(offset);
    return {
        reinterpret_cast<const T*>(memory.data()),
        memory.size() / sizeof(T),
    };
  }

  template <typename T>
  static constexpr size_t PointerSize() {
    if constexpr (std::is_function_v<T>) {
      return 1;
    } else {
      return sizeof(T);
    }
  }

  std::span<std::byte> image_;
  uintptr_t base_ = 0;
};
}  // namespace elfldltl

#endif  // SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_MEMORY_H_

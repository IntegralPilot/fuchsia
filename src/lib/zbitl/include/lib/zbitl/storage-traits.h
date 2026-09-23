// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ZBITL_INCLUDE_LIB_ZBITL_STORAGE_TRAITS_H_
#define SRC_LIB_ZBITL_INCLUDE_LIB_ZBITL_STORAGE_TRAITS_H_

#include <lib/fit/result.h>
#include <lib/zbi-format/zbi.h>
#include <zircon/assert.h>

#include <concepts>
#include <cstdint>
#include <cstring>
#include <functional>
#include <limits>
#include <optional>
#include <ranges>
#include <span>
#include <string_view>
#include <type_traits>
#include <version>

namespace zbitl {

using ByteView = std::span<const std::byte>;

// The byte alignment that storage backends are expected to have.
constexpr size_t kStorageAlignment = __STDCPP_DEFAULT_NEW_ALIGNMENT__;

template <typename T>
concept PayloadCompatibleStorage = alignof(T) <= kStorageAlignment;

// These are types that might appropriately be used in ZBI item payloads.
template <typename T>
concept PayloadData =  //
    alignof(T) <= ZBI_ALIGNMENT && std::is_standard_layout_v<T> &&
    std::is_trivially_copyable_v<T> && std::is_trivially_destructible_v<T> &&
    std::has_unique_object_representations_v<T>;

// It is expected that `payload` is `kStorageAlignment`-aligned in the
// following AsSpan methods (see StorageTraits below), along with `T` itself.
// This ensures that `payload` is `alignof(T)`-aligned as well, which in
// particular means that it is safe to reinterpret a `U*` as a `T*`.
template <PayloadCompatibleStorage T, PayloadData U>
inline std::span<T> AsSpan(U* payload, size_t len) {
  if constexpr (sizeof(U) % sizeof(T) != 0) {
    ZX_ASSERT(len * sizeof(U) % sizeof(T) == 0);
  }
  return {reinterpret_cast<T*>(payload), (len * sizeof(U)) / sizeof(T)};
}

template <typename T, std::ranges::contiguous_range R>
inline std::span<T> AsSpan(R&& payload) {
  return AsSpan<T>(std::ranges::data(payload), std::ranges::size(payload));
}

// **NOTE:** This takes any pointer type, but the size is always in bytes.
// Consider using std::as_bytes(std::span{payload, count}) for a count of
// pointee objects instead.
inline ByteView AsBytes(const PayloadData auto* payload, size_t len) {
  return {reinterpret_cast<const std::byte*>(payload), len};
}

template <std::ranges::contiguous_range R>
  requires(PayloadData<std::ranges::range_value_t<R>>)
inline ByteView AsBytes(R&& payload) {
  return AsSpan<const std::byte>(payload);
}

inline ByteView AsBytes(std::string_view sv) { return std::as_bytes(std::span{sv}); }

// **NOTE:** Use with caution!  This takes the address of the argument passed
// by reference.  It should only be used as part of a complete expression that
// is consuming the ByteView so there can never be dangling pointers to a
// temporary object.
inline ByteView AsBytes(const PayloadData auto& payload) {
  return std::as_bytes(std::span{&payload, 1});
}

/// The zbitl::StorageTraits template must be specialized for each type used as
/// the Storage type parameter to zbitl::View (see <lib/zbitl/view.h).  The
/// requirements for specializations are described by the concepts below.
template <typename Storage>
struct StorageTraits {
  static_assert(!std::same_as<Storage, Storage>, "missing zbitl::StorageTraits specialization");
};

// Every zbitl::StorageTraits<Storage> specialization must meet at least this
// basic contract.  However the storage works, the pointers exchanged via this
// API are presumed to be `zbitl::kStorageAlignment`-aligned.
template <class Traits, typename Storage>
concept StorageTraitsBaseApi = requires {
  /// This represents an error accessing the storage, either to read a header
  /// or to access a payload.  It may be used as an error value in fit::result.
  typename Traits::error_type;
  requires std::copyable<typename Traits::error_type>;
  requires std::default_initializable<typename Traits::error_type>;
  typename fit::result<typename Traits::error_type>;

  /// This represents an item payload (does not include the header).  The
  /// corresponding zbi_header_t.length gives its size.  This type is wholly
  /// opaque to zbitl::View but must be copyable.  It might be something as
  /// simple as the offset into the whole ZBI, or for in-memory Storage types a
  /// std::span pointing to the contents.
  typename Traits::payload_type;
  requires std::copyable<typename Traits::payload_type>;
  requires std::default_initializable<typename Traits::error_type>;
} && requires(Storage& storage_ref, Traits::error_type error, Traits::payload_type payload) {
  /// This method is expected to return a type convertible to std::string_view
  /// (e.g., std::string or const char*) representing the message associated to
  /// a given error value.  The returned object is "owning" and so it is
  /// expected that the caller keep the returned object alive for as long as
  /// they use any string_view converted from it.
  { Traits::error_string(error) } -> std::convertible_to<std::string_view>;

  /// This returns the upper bound on available space where the ZBI is stored.
  /// The container must fit within this maximum.  Storage past the container's
  /// self-encoded size need not be accessible and will never be accessed.
  /// If the actual upper bound is unknown, this can safely return UINT32_MAX.
  {
    Traits::Capacity(storage_ref)
  } -> std::same_as<fit::result<typename Traits::error_type, uint32_t>>;

  /// This fetches the item payload view object, whatever that means for this
  /// Storage type.  This is not expected to read the contents, just transfer a
  /// pointer or offset around so they can be explicitly read later.
  {
    Traits::Payload(storage_ref, uint32_t /*offset*/ {}, uint32_t /*length*/ {})
  } -> std::same_as<fit::result<typename Traits::error_type, typename Traits::payload_type>>;

  // Every zbitl::StorageTraits<Storage> specialization must also have a Read
  // method.  But it can have any one or more of three Read signatures.  It
  // should define whichever is most efficient, and only needs to define more
  // than one signature if each is more efficient than just using another.
};

/// Referred to as the "buffered read".
///
/// This reads the payload indicated by a payload_type as returned by Payload
/// and feeds it to the callback in chunks sized for the convenience of the
/// storage backend.  The length is guaranteed to match that passed to Payload
/// to fetch this payload_type value.
///
/// The callback returns some type fit::result<E>.  Read returns
/// fit::result<error_type, fit::result<E>>>, yielding a storage error or the
/// result of the callback.  If a callback returns an error, its return value
/// is used immediately.  If a callback returns success, another callback may
/// be made for another chunk of the payload.  If the payload is empty
/// (`length` == 0), there will always be a single callback made with an empty
/// data argument.
template <class Traits, typename Storage>
concept StorageTraitsBufferedReadApi = requires(  //
    Storage& storage_ref, uint32_t offset, uint32_t length) {
  requires StorageTraitsBaseApi<Traits, Storage>;
  {
    Traits::Read(storage_ref, *Traits::Payload(storage_ref, offset, length), length,
                 [](ByteView contents) -> fit::result<fit::failed> { return fit::ok(); })
  } -> std::same_as<fit::result<typename Traits::error_type, fit::result<fit::failed>>>;
};

// Referred to as the "unbuffered read".
//
// A specialization provides this overload if the payload can be read directly
// into a provided buffer for zero-copy operation.
template <class Traits, typename Storage>
concept StorageTraitsUnbufferedReadApi = requires(  //
    Storage& storage_ref, uint32_t offset, void* buffer, uint32_t length) {
  requires StorageTraitsBaseApi<Traits, Storage>;
  {
    Traits::Read(storage_ref, *Traits::Payload(storage_ref, offset, length), buffer, length)
  } -> std::same_as<fit::result<typename Traits::error_type>>;
};

// Referred to as the "one-shot read".
//
// A specialization only provides this overload if the payload can be accessed
// directly in memory.  If this overload is provided, then the other overloads
// need not be provided.  The returned view is only guaranteed valid until the
// next use of the same Storage object.  So it could e.g. point into a cache
// that's repurposed by this or other calls made later using the same object.
//
// One may attempt to read the data out in any particular form, parameterized
// by `T`, provided that `alignof(T) <= kStorageAlignment`. The offset
// associated with `payload` is expected to be `alignof(T)`-aligned, though
// that is an invariant that the caller must keep track of.
//
// `LowLocality` gives whether there is an expectation that adjacent data will
// subsequently be read; if true, the amortized cost of the read might be
// determined to be too high and storage backends might decide to perform the
// read differently or not implement the method at all in this case.
template <class Traits, typename Storage, typename T, bool LowLocality>
concept StorageTraitsOneShotReadApi = requires(  //
    Storage& storage_ref, uint32_t offset, uint32_t length) {
  requires StorageTraitsBaseApi<Traits, Storage>;
  requires PayloadCompatibleStorage<T>;
  {
    Traits::template Read<T, LowLocality>(  //
        storage_ref, *Traits::Payload(storage_ref, offset, length), length)
  } -> std::same_as<fit::result<typename Traits::error_type, std::span<const T>>>;
};

template <class Traits, typename Storage>
concept StorageTraitsApi =  // Any one of the three will do.
    StorageTraitsBufferedReadApi<Traits, Storage> ||
    StorageTraitsUnbufferedReadApi<Traits, Storage> ||
    (StorageTraitsOneShotReadApi<Traits, Storage, std::byte, true> &&
     StorageTraitsOneShotReadApi<Traits, Storage, std::byte, false>);

// A specialization must meet this additional contract to support mutation.
template <class Traits, typename Storage>
concept StorageTraitsWriteApi = requires(Storage& storage_ref, uint32_t offset) {
  requires StorageTraitsApi<Traits, Storage>;

  // Referred to as the "buffered write".
  //
  // This might be called to write whole or partial headers and/or payloads,
  // but it will never be called with an offset and size that would exceed the
  // capacity previously reported by Capacity (above).  It returns success only
  // if it wrote the whole chunk specified.  If it returns an error, any subset
  // of the chunk that failed to write might be corrupted in the image and the
  // container will always revalidate everything.
  {
    Traits::Write(storage_ref, offset, ByteView{})
  } -> std::same_as<fit::result<typename Traits::error_type>>;

  /// This ensures that the capacity is at least that of the provided value
  /// (possibly larger), for specializations where such an operation is
  /// sensible.
  {
    Traits::EnsureCapacity(storage_ref, offset)
  } -> std::same_as<fit::result<typename Traits::error_type>>;
};

// A specialization that satisfies StorageTraitsWriteApi can also define
// additional Write overloads for optimization.
template <class Traits, typename Storage>
concept StorageTraitsUnbufferedWriteApi = requires(  //
    Storage& storage_ref, uint32_t offset, uint32_t length) {
  requires StorageTraitsApi<Traits, Storage>;

  // Referred to as the "unbuffered write".
  //
  // This returns a
  // pointer where the data can be mutated directly in memory.  That pointer is
  // only guaranteed valid until the next use of the same Storage object.  So
  // it could e.g. point into a cache that's repurposed by this or other calls
  // made later using the same object.
  {
    Traits::Write(storage_ref, offset, length)
  } -> std::same_as<fit::result<typename Traits::error_type, void*>>;
};

// This is a helper concept for any fit::result<Traits::error_type, ...> type.
template <typename T, class Traits>
concept StorageTraitsResultApi = requires {
  typename T::value_type;
  requires std::same_as<T, fit::result<typename Traits::error_type, typename T::value_type>>;
};

// zbitl::StorageApi is true of any type with a valid specialization.
template <typename T>
concept StorageApi = requires {
  typename StorageTraits<std::decay_t<T>>;
  requires StorageTraitsApi<StorageTraits<std::decay_t<T>>, std::decay_t<T>>;
};

// zbitl::WritableStorageApi is true of any type with a specialization that
// supports mutation.
template <typename T>
concept WritableStorageApi = requires {
  requires StorageApi<T>;
  requires StorageTraitsWriteApi<StorageTraits<std::decay_t<T>>, std::decay_t<T>>;
};

// A specialization that satisfies StorageTraitsWriteApi might also support
// creating new storage from whole cloth if that makes sense for the storage
// type somehow.
template <class Traits, typename Storage>
concept StorageTraitsCreateApi = requires(  //
    Storage& storage_ref, uint32_t capacity, uint32_t initial_zero_size) {
  // The successful return value is whatever makes sense for returning a new,
  // owning object of a type akin to Storage (possibly Storage itself, possibly
  // another type).  The new object refers to new storage of at least the given
  // capacity (in bytes) with a provided zero-fill header size.  The old
  // storage object might be used as a prototype in some sense, but the new
  // object is distinct storage.
  { Traits::Create(storage_ref, capacity, initial_zero_size) } -> StorageTraitsResultApi<Traits>;
};

template <typename Storage>
  requires StorageTraitsCreateApi<StorageTraits<Storage>, Storage>
using StorageTraitsCreateStorageType =
    std::decay_t<decltype(StorageTraits<Storage>::Create(std::declval<Storage&>(), 0, 0).value())>;

// This is a helper concept for the "slop-check" function passed to Clone.
// StorageTraits<...>::Clone implementations can use this for their template
// parameter.
template <typename T>
concept StorageTraitsCloneSlopCheckApi = requires(T slopcheck, uint32_t slop_bytes) {
  { slopcheck(slop_bytes) } -> std::convertible_to<bool>;
};

// This is a helper concept for the return value of Traits::Clone, below.
template <typename T, class Traits>
concept StorageTraitsCloneResultApi = requires(T result) {
  requires StorageTraitsResultApi<T, Traits>;

  // The fit::result::value_type must be a std::optional type.
  typename T::value_type::value_type;
  requires std::same_as<typename T::value_type,  //
                        std::optional<typename T::value_type::value_type>>;

  // The std::optional must be a std::pair<Storage, uint32_t> type.
  typename T::value_type::value_type::first_type;
  typename T::value_type::value_type::second_type;
  requires std::same_as<  //
      typename T::value_type::value_type,
      std::pair<typename T::value_type::value_type::first_type,
                typename T::value_type::value_type::second_type>>;
  requires StorageApi<typename T::value_type::value_type::first_type>;
  requires std::same_as<typename T::value_type::value_type::second_type,  //
                        uint32_t>;
};

// A specialization that satisfies StorageTraitsCreateApi might also support
// creating new storage by making a virtual copy ("clone") of existing storage.
template <class Traits, typename Storage, typename SlopCheck>
concept StorageTraitsCloneApi = requires(  //
    Storage& storage_ref, uint32_t offset, uint32_t length, uint32_t to_offset,
    SlopCheck slopcheck) {
  requires StorageTraitsCreateApi<Traits, Storage>;
  requires StorageTraitsCloneSlopCheckApi<SlopCheck>;

  // The new object is new storage that doesn't mutate the original storage,
  // whose capacity is at least `to_offset + length`, and whose contents are
  // the subrange of the original storage starting at `offset`, with zero-fill
  // from the beginning of the storage up to `to_offset` bytes.  The successful
  // return value is `std::optional<std::pair<T, uint32_t>>` where T is what a
  // successful Create call returns and the uint32_t is the actual offset into
  // the new storage, aka the "slop" (see below).  If this doesn't have
  // something more efficient to do than just allocating storage space for and
  // copying all `length` bytes of data (using Create and Write), then it can
  // just return std::nullopt.  If the method would *always* just return
  // std::nullopt then it can just be omitted entirely.  The "slop" refers to
  // some number of bytes at the beginning of the storage that will read as
  // zero before the requested range of the original storage begins.  The
  // storage backend will endeavor to make this match `to_offset`, but might
  // deliver a different result due to factors like page-rounding.  The final
  // argument is a StorageTraitsCloneApi predicate object that says whether a
  // given byte count is acceptable as slop for this clone.  If it returns
  // false, Clone *must* return std::nullopt rather than yielding storage with
  // a rejected slop byte count.
  {
    Traits::Clone(storage_ref, offset, length, to_offset, slopcheck)
  } -> StorageTraitsCloneResultApi<Traits>;
};

// zbitl::ZeroCopyStorageApi<FromStorage, ToStorage> is true if there are no
// copies into buffers in either direction to make the copy.
template <typename FromStorage, typename ToStorage = FromStorage>
concept ZeroCopyStorageApi =
    StorageApi<FromStorage> && WritableStorageApi<ToStorage> &&
    (StorageTraitsOneShotReadApi<StorageTraits<FromStorage>, FromStorage, std::byte, false> ||
     (StorageTraitsUnbufferedReadApi<StorageTraits<FromStorage>, FromStorage> &&
      StorageTraitsUnbufferedWriteApi<StorageTraits<ToStorage>, ToStorage>));

// zbitl::ReadSparseDataFromStorage<Data>(storage, offset) reads a single datum
// from any StorageApi object.  It returns either fit::result<error_type, Data>
// or fit::result<error_type, std::reference_wrapper<const Data>>.  This is
// meant to be used for things like headers that often have low locality with
// other accesses likely to be made soon.

template <PayloadData Data, StorageApi Storage>
  requires StorageTraitsOneShotReadApi<StorageTraits<Storage>, Storage, Data, true>
constexpr auto ReadSparseDataFromStorage(Storage& storage, uint32_t offset)
    -> fit::result<typename StorageTraits<Storage>::error_type,
                   std::reference_wrapper<const Data>> {
  using Traits = StorageTraits<Storage>;
  constexpr size_t kSize = sizeof(Data);
  ZX_DEBUG_ASSERT(offset % alignof(Data) == 0);
  auto payload = Traits::Payload(storage, offset, kSize);
  if (payload.is_error()) {
    return payload.take_error();
  }
  auto data = Traits::template Read<Data, true>(storage, *payload, kSize);
  if (data.is_error()) {
    return data.take_error();
  }
  ZX_DEBUG_ASSERT(data->size() == 1);  // We expect a span of one `Data`.
  return fit::ok(std::cref(data->front()));
}

template <PayloadData Data, StorageApi Storage>
  requires(!StorageTraitsOneShotReadApi<StorageTraits<Storage>, Storage, Data, true>)
constexpr auto ReadSparseDataFromStorage(Storage& storage, uint32_t offset)
    -> fit::result<typename StorageTraits<Storage>::error_type, Data> {
  using Traits = StorageTraits<Storage>;
  constexpr size_t kSize = sizeof(Data);
  ZX_DEBUG_ASSERT(offset % alignof(Data) == 0);
  auto payload = Traits::Payload(storage, offset, kSize);
  if (payload.is_error()) {
    return payload.take_error();
  }

  Data datum;
  auto fill_datum = [&storage, &payload = *payload, &datum]() {
    if constexpr (StorageTraitsUnbufferedReadApi<StorageTraits<Storage>, Storage>) {
      return Traits::Read(storage, payload, &datum, sizeof(datum));
    } else {
      size_t bytes_read = 0;
      auto result = Traits::Read(  //
          storage, payload, sizeof(datum),
          [datum_bytes = AsSpan<std::byte>(datum)](auto bytes) mutable {
            memcpy(datum_bytes.data(), bytes.data(), bytes.size());
            datum_bytes = datum_bytes.subspan(bytes.size());
          });
      if (result.is_ok()) {
        ZX_DEBUG_ASSERT(bytes_read == sizeof(Data));
      }
      return result;
    }
  };

  auto result = fill_datum();
  if (result.is_error()) {
    return result.take_error();
  }

  return fit::ok(datum);
}

// The first chunk StorageTraits<...>::Read passes to its callback must be at
// least as long as the minimum of kReadMinimum and header.length.
inline constexpr uint32_t kReadMinimum = 32;

// Specialization for std::basic_string_view<byte-size type> as Storage.  Its
// payload_type is the same type as Storage, just yielding the substring of the
// original whole-ZBI string_view.
template <typename T>
struct StorageTraits<std::basic_string_view<T>> {
  using Storage = std::basic_string_view<T>;

  static_assert(sizeof(T) == sizeof(uint8_t));

  struct error_type {};

  using payload_type = Storage;

  static constexpr std::string_view error_string(error_type error) { return "no errors possible"; }

  static fit::result<error_type, uint32_t> Capacity(Storage& zbi) {
    return fit::ok(static_cast<uint32_t>(
        std::min(zbi.size(),
                 static_cast<typename Storage::size_type>(std::numeric_limits<uint32_t>::max()))));
  }

  static fit::result<error_type, payload_type> Payload(Storage& zbi, uint32_t offset,
                                                       uint32_t length) {
    auto payload = zbi.substr(offset, length);
    ZX_DEBUG_ASSERT(payload.size() == length);
    return fit::ok(std::move(payload));
  }

  template <PayloadCompatibleStorage U, bool LowLocality>
  static fit::result<error_type, std::span<const U>> Read(Storage& zbi, payload_type payload,
                                                          uint32_t length) {
    ZX_DEBUG_ASSERT(payload.size() == length);
    return fit::ok(AsSpan<const U>(payload));
  }
};

template <typename T, size_t Extent>
struct StorageTraits<std::span<T, Extent>> {
  using Storage = std::span<T, Extent>;

  struct error_type {};

  using payload_type = Storage;

  static constexpr std::string_view error_string(error_type error) { return "no errors possible"; }

  static fit::result<error_type, uint32_t> Capacity(Storage& zbi) {
    return fit::ok(static_cast<uint32_t>(
        std::min(zbi.size_bytes(), static_cast<size_t>(std::numeric_limits<uint32_t>::max()))));
  }

  static fit::result<error_type> EnsureCapacity(Storage& zbi, uint32_t capacity_bytes)
    requires(!std::is_const_v<T>)
  {
    if (capacity_bytes > zbi.size()) {
      return fit::error{error_type{}};
    }
    return fit::ok();
  }

  static fit::result<error_type, payload_type> Payload(Storage& zbi, uint32_t offset,
                                                       uint32_t length) {
    auto payload = [&]() {
      if constexpr (std::is_const_v<T>) {
        return std::as_bytes(zbi).subspan(offset, length);
      } else {
        return std::as_writable_bytes(zbi).subspan(offset, length);
      }
    }();
    ZX_DEBUG_ASSERT(payload.size() == length);
    ZX_ASSERT_MSG(payload.size() % sizeof(T) == 0,
                  "payload size not a multiple of storage span element_type size");
    return fit::ok(payload_type{reinterpret_cast<T*>(payload.data()), payload.size() / sizeof(T)});
  }

  template <PayloadCompatibleStorage U, bool LowLocality>
  static fit::result<error_type, std::span<const U>> Read(Storage& zbi, payload_type payload,
                                                          uint32_t length) {
    ZX_DEBUG_ASSERT(std::as_bytes(payload).size() == length);
    return fit::ok(AsSpan<const U>(payload));
  }

  static fit::result<error_type> Write(Storage& zbi, uint32_t offset, ByteView data)
    requires(!std::is_const_v<T>)
  {
    if (!data.empty()) {
      memcpy(Write(zbi, offset, static_cast<uint32_t>(data.size())).value(), data.data(),
             data.size());
    }
    return fit::ok();
  }

  static fit::result<error_type, void*> Write(Storage& zbi, uint32_t offset, uint32_t length)
    requires(!std::is_const_v<T>)
  {
    // The caller is supposed to maintain these invariants.
    ZX_DEBUG_ASSERT(offset <= zbi.size_bytes());
    ZX_DEBUG_ASSERT(length <= zbi.size_bytes() - offset);
    return fit::ok(std::as_writable_bytes(zbi).data() + offset);
  }
};

}  // namespace zbitl

#endif  // SRC_LIB_ZBITL_INCLUDE_LIB_ZBITL_STORAGE_TRAITS_H_

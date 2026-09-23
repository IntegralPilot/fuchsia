// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fit/result.h>
#include <lib/stdcompat/atomic.h>
#include <lib/stdcompat/span.h>
#include <zircon/assert.h>

#include <atomic>
#include <cstdint>
#include <limits>
#include <type_traits>
#include <utility>
#include <variant>

#ifndef LIB_IOB_BLOB_ID_ALLOCATOR_H_
#define LIB_IOB_BLOB_ID_ALLOCATOR_H_

namespace iob {

// Represents a thread-safe view into an IOBuffer region of "ID allocator"
// discipline (ZX_IOB_DISCIPLINE_TYPE_ID_ALLOCATOR), used to map sized data
// blobs to sequentially-allocated numeric IDs.
//
// Suppose there are N mapped blobs. The memory is laid out as follows, with
// copies of the blobs growing down and their corresponding bookkeeping indices
// growing up:
// --------------------------------
//   next available blob ID (4 bytes)
//   blob head offset (4 bytes)
//   ----------------------------
//   blob 0 size (4 bytes)       } <-- bookkeeping index
//   blob 0 offset (4 bytes)     }
//   ...
//   blob N-1 size (4 bytes)
//   blob N-1 offset (4 bytes)
//   ----------------------------
//   zero-initialized memory      <-- remaining bytes available
//   ---------------------------- <-- blob head offset
//   blob N-1
//   ...
//   blob 0
// --------------------------------
//
// This class takes care of the atomic nuance required of accessing and updating
// such a structure.
class BlobIdAllocator {
 public:
  // The possible failure modes of Allocate().
  enum class AllocateError {
    // The header was invalid (purportedly either with indices extending past
    // the blob head offset or the blob head offset extending past the length
    // of the buffer). Suggests invalid initialization or corruption.
    kInvalidHeader,

    // The would-be bookkeeping index slot for the new allocated ID is
    // non-empty (i.e., not in the initialized state). Suggests corruption.
    kNonEmptyIndex,

    // Out Of Memory: there is insufficient memory available for the requested
    // allocation. (What is available can be queried with RemainingBytes().)
    kOutOfMemory,
  };

  // The possible failure modes of blob access, either via iteration via
  // IterableView or GetBlob().
  enum class BlobError {
    // The header was invalid (purportedly either with indices extending past
    // the blob head offset or the blob head offset extending past the length
    // of the buffer). Suggests invalid initialization or corruption.
    kInvalidHeader,

    // The requested ID has not yet been allocated.
    kUnallocatedId,

    // Suggests a lost race in which the requested ID is valid and the header
    // has been updated to reflect that, but the bookkeeping index has not yet
    // been committed. Such a case unfortunately is indistinguishable from the
    // other possibility that the index was already committed but then
    // subsequently corrupted with zeroes. Where there is confidence in the
    // first case, this call should be retried.
    kUncommittedIndex,

    // The corresponding bookkeeping index is invalid, with the blob
    // purportedly not being contained within [blob head, end of region).
    // Suggests corruption.
    kInvalidIndex,
  };

  // IterableView provides a more conventional means of iterating through the
  // blobs and IDs within a BlobIdAllocator, which cannot be done on the latter
  // as that would not be threadsafe. IterableView follows an "error-checking
  // view" pattern: the container/range/view API of begin() and end() iterators
  // is supported, but when begin() or iterator::operator++() encounters an
  // error, it simply returns end() so that loops terminate normally.
  // Thereafter, take_error() must be called to check whether the loop
  // terminated because it may have encountered an error. Once begin() has been
  // called, take_error() must be called before the IterableView is destroyed,
  // so no error goes undetected. After take_error() is called the error state
  // is consumed and take_error() cannot be called again until another begin()
  // or iterator::operator++() call has been made.
  //
  // An IterableView is intended to be constructed via
  // BlobIdAllocator::iterable().
  class IterableView {
   public:
    struct value_type {
      uint32_t id;
      cpp20::span<const std::byte> blob;
    };

    class iterator {
     public:
      // Iterator traits.
      using iterator_category = std::forward_iterator_tag;
      using reference = const IterableView::value_type&;
      using value_type = IterableView::value_type;
      using pointer = const IterableView::value_type*;
      using difference_type = size_t;

      // The default-constructed iterator is invalid for all uses except
      // equality comparison.
      iterator() = default;

      bool operator==(const iterator& other) const {
        return other.view_ == view_ && other.value_.id == value_.id;
      }

      bool operator!=(const iterator& other) const { return !(*this == other); }

      iterator& operator++() {  // prefix
        AssertNotDefaultOrEnd(__func__);
        view_->StartIteration();
        Update(value_.id + 1);
        return *this;
      }

      iterator operator++(int) {  // postfix
        iterator old = *this;
        ++*this;
        return old;
      }

      reference operator*() const {
        AssertNotDefaultOrEnd(__func__);
        return value_;
      }

      pointer operator->() const {
        AssertNotDefaultOrEnd(__func__);
        return &value_;
      }

     private:
      // This is only so that begin(), end(), and find() may use the private
      // constructor below, as well as `kEndId`.
      friend class IterableView;

      static constexpr uint32_t kEndId = -1;

      iterator(IterableView* view, uint32_t id) : view_(view) { Update(id); }

      // Checks that one is not trying to operate on a default-constructed or
      // end iterator.
      void AssertNotDefaultOrEnd(const char* func) const {
        ZX_ASSERT_MSG(
            view_, "%s on default-constructed iob::BlobIdAllocator::IterableView::iterator", func);
        ZX_ASSERT_MSG(value_.id != kEndId,
                      "%s on iob::BlobIdAllocator::IterableView::end() iterator", func);
      }

      void Update(uint32_t id) {
        if (id >= view_->allocator_->next_id()) {
          value_ = {kEndId, {}};
          return;
        }

        if (auto result = view_->allocator_->GetBlob(id); result.is_error()) {
          view_->Fail(result.error_value());
          value_ = {kEndId, {}};
        } else {
          value_ = {id, result.value()};
        }
      }

      IterableView* view_ = nullptr;
      value_type value_ = {};
    };

    ~IterableView() {
      ZX_ASSERT_MSG(!std::holds_alternative<BlobError>(error_),
                    "iob::BlobIdAllocator::IterableView destroyed after error without check");
      ZX_ASSERT_MSG(
          !std::holds_alternative<NoError>(error_),
          "iob::BlobIdAllocator::IterableView destroyed after successful iteration without check");
    }

    // After calling begin(), it's mandatory to call take_error() before
    // destroying the IterableView object. See the IterableView docstring
    // for more detail.
    iterator begin() { return find(0); }

    iterator end() { return {this, iterator::kEndId}; }

    // Returns the iterator associated with a given ID if allocated, or else
    // end(). As with end(), it's mandatory to call take_error() after calling
    // find().
    iterator find(uint32_t id) {
      StartIteration();
      return {this, id};
    }

    // Check the container for errors after using iterators. See the
    // IterableView docstring for more detail.
    fit::result<BlobError> take_error() {
      ErrorState result = error_;
      error_ = Taken{};
      if (std::holds_alternative<BlobError>(result)) {
        return fit::error{std::get<BlobError>(result)};
      }
      ZX_ASSERT_MSG(!std::holds_alternative<Taken>(result),
                    "iob::BlobIdAllocator::IterableView::take_error() was already called");
      return fit::ok();
    }

   private:
    struct Unused {};
    struct NoError {};
    struct Taken {};

    using ErrorState = std::variant<Unused, NoError, BlobError, Taken>;

    // This is only so that BlobIdAllocator::iterable() can use the private
    // constructor below.
    friend class BlobIdAllocator;
    explicit IterableView(BlobIdAllocator* allocator) : allocator_(allocator) {}

    void StartIteration() {
      ZX_ASSERT_MSG(!std::holds_alternative<BlobError>(error_),
                    "iob::BlobIdAllocator::IterableView iterators used without taking prior error");
      error_ = NoError{};
    }

    void Fail(BlobError error) {
      ZX_DEBUG_ASSERT_MSG(
          !std::holds_alternative<BlobError>(error_),
          "Fail() in error state: missing iob::BlobIdAllocator::IterableView::StartIteration() call?");
      ZX_DEBUG_ASSERT_MSG(
          !std::holds_alternative<Unused>(error_),
          "Fail() in Unused state: missing iob::BlobIdAllocator::IterableView::StartIteration() call?");
      error_ = error;
    }

    BlobIdAllocator* allocator_ = nullptr;
    ErrorState error_;
  };

  // Constructs a view into an ID allocator IOBuffer region. If the provided
  // memory does not yet reflect this layout, Init() must be called before any
  // other method.
  //
  // The provided memory must be at least 8-byte-aligned (for aligned atomic
  // access), and at least 8 bytes in size (so as to have enough room for the
  // header).
  explicit BlobIdAllocator(cpp20::span<std::byte> bytes) : bytes_(bytes) {
    ZX_ASSERT(sizeof(Header) <= bytes_.size());
    ZX_ASSERT(bytes_.size() <= std::numeric_limits<uint32_t>::max());
  }

  // Initializes the backing memory as an ID allocator region with no blobs yet
  // mapped. If the region is already known to be zero-filled, this part may be
  // skipped.
  void Init(bool zero_fill = true) {
    // Be sure to zero the region first and then store the header with release
    // semantics, so that any reader of the header will be guaranteed to see a
    // fully initialized structure.
    if (zero_fill) {
      memset(&bytes_[sizeof(Header)], 0, bytes_.size() - sizeof(Header));
    }
    header().store(
        Header{
            .next_id = 0,
            .blob_head = static_cast<uint32_t>(bytes_.size()),
        },
        std::memory_order_release);
  }

  // Provides an iterable view into the allocator, scoped to the lifetime of
  // this object.
  IterableView iterable() { return IterableView(this); }

  // The next ID to be allocated.
  uint32_t next_id() const { return self()->header().load(std::memory_order_relaxed).next_id; }

  // The remaining number of available bytes in the allocator (including those
  // that might be used for bookkeeping). fit::failed() is returned in the case
  // of an invalid header (see `AllocateError::kInvalidHeader` for more
  // detail).
  fit::result<fit::failed, size_t> RemainingBytes() const {
    return self()->header().load(std::memory_order_relaxed).RemainingBytes(bytes_.size());
  }

  // Attempts to store the provided blob and allocate its ID.
  fit::result<AllocateError, uint32_t> Allocate(cpp20::span<const std::byte> blob) {
    return Allocate(
        [](cpp20::span<const std::byte> src, cpp20::span<std::byte> dest) {
          // This check should already be made with any failure gracefully
          // handled by the time this is called.
          ZX_DEBUG_ASSERT(src.size() <= dest.size());
          memcpy(dest.data(), src.data(), src.size());
        },
        blob, blob.size());
  }

  // A variation of the allocation routine that abstracts the representation of
  // the supplied blob and the manner in which it is copied. This of particular
  // value to the use of this library in kernel, which requires care in dealing
  // with user-supplied memory.
  //
  // `CopyBlob`, which performs the copy of blob to a specified destination, is
  // a callable of input signature `(Blob src, cpp20::span<std::byte> dest)` and
  // unspecified return type.
  //
  template <typename Blob, typename CopyBlob>
  fit::result<AllocateError, uint32_t> Allocate(CopyBlob&& copy, Blob blob, size_t blob_size) {
    static_assert(std::is_invocable_v<CopyBlob, Blob, cpp20::span<std::byte>>);

    cpp20::atomic_ref<Header> ref = header();
    Header hdr = ref.load(std::memory_order_acquire);
    bool retry = true;
    while (retry) {
      size_t remaining;
      if (auto result = hdr.RemainingBytes(bytes_.size()); result.is_error()) {
        return fit::error{AllocateError::kInvalidHeader};
      } else {
        remaining = result.value();
      }

      if (remaining < sizeof(Index) || remaining - sizeof(Index) < blob_size) {
        return fit::error{AllocateError::kOutOfMemory};
      }
      Header updated = {
          .next_id = hdr.next_id + 1,
          .blob_head = hdr.blob_head - static_cast<uint32_t>(blob_size),
      };
      retry = !ref.compare_exchange_weak(hdr, updated, std::memory_order_acq_rel,
                                         std::memory_order_acquire);
    }

    // When the loop terminates `hdr` reflects the header just prior to the
    // update.
    auto [id, offset] = hdr;
    offset -= static_cast<uint32_t>(blob_size);

    // We store the blob and then store the index with release semantics to
    // ensure the following:
    // (1) The write of the index stays ordered after the previous store of the
    //     blob.
    // (2) The write of the index stays ordered before subsequent reads, which
    //     are made with release semantics.
    std::forward<CopyBlob>(copy)(blob, bytes_.subspan(offset, blob_size));

    // Before overwriting, to be safe, check that the index is in the initial
    // state (i.e., empty).
    Index empty = {0, 0};
    Index index{.size = static_cast<uint32_t>(blob_size), .offset = offset};
    if (!GetIndex(id).compare_exchange_strong(empty,                         //
                                              index,                         //
                                              std::memory_order_release)) {  //
      return fit::error{AllocateError::kNonEmptyIndex};
    }
    return fit::ok(id);
  }

  // Returns the blob corresponding to a given ID.
  fit::result<BlobError, cpp20::span<const std::byte>> GetBlob(uint32_t id) const {
    Header hdr = self()->header().load(std::memory_order_relaxed);
    if (!hdr.IsValid(bytes_.size())) [[unlikely]] {
      return fit::error{BlobError::kInvalidHeader};
    }
    if (id >= hdr.next_id) {
      return fit::error{BlobError::kUnallocatedId};
    }

    // We load the index with acquire semantics as this ensures the following:
    // (1) The read of the index stays ordered before the subsequent load of the
    //     blob.
    // (2) The read of the index stays ordered after previous updates, which
    //     were written with release semantics.
    Index index = self()->GetIndex(id).load(std::memory_order_acquire);
    if (index.size == 0 && index.offset == 0) [[unlikely]] {
      return fit::error{BlobError::kUncommittedIndex};
    }
    if (!index.IsValid(hdr.blob_head, bytes_.size())) [[unlikely]] {
      return fit::error{BlobError::kInvalidIndex};
    }
    return fit::ok(bytes_.subspan(index.offset, index.size));
  }

 private:
  struct alignas(8) Header {
    // Returns the byte offset just past the last bookkeeping index, or
    // fit::failed() if the computation would overflow a uint32_t.
    constexpr fit::result<fit::failed, uint32_t> index_end() const {
      // Guard against integer overflow: next_id * sizeof(Index) must not
      // exceed UINT32_MAX - sizeof(Header). Without this check, a corrupted
      // next_id (e.g., 0x20000000) causes the result to silently wrap around,
      // bypassing IsValid() and enabling out-of-bounds access via GetIndex().
      constexpr uint32_t kMaxNextId =
          (std::numeric_limits<uint32_t>::max() - sizeof(Header)) / sizeof(Index);
      if (next_id > kMaxNextId) [[unlikely]] {
        return fit::failed();
      }
      return fit::ok(static_cast<uint32_t>(sizeof(Header) + (next_id * sizeof(Index))));
    }

    // See AllocateError::kInvalidHeader.
    constexpr bool IsValid(size_t length) const {
      fit::result end = index_end();
      return end.is_ok() && end.value() <= blob_head && blob_head <= length;
    }

    // Returns the remaining number of available bytes in the region, given the
    // length of the region - or fit::failed() in the event of an invalid
    // header.
    constexpr fit::result<fit::failed, size_t> RemainingBytes(size_t length) const {
      if (!IsValid(length)) [[unlikely]] {
        return fit::failed();
      }
      // IsValid() already verified that index_end() is ok.
      return fit::ok(blob_head - index_end().value());
    }

    uint32_t next_id;
    uint32_t blob_head;
  };

  // TODO(https://fxbug.dev/354716628): commented to unblock clang roll.
  // TODO(https://github.com/llvm/llvm-project/pull/99570): Re-enable when libcxx underlying
  // implementation of `atomic_ref<T>` calculates `is_always_lockfree` appropriately.
  // static_assert(cpp20::atomic_ref<Header>::is_always_lock_free);

  // Represents a blob bookkeeping index.
  struct alignas(8) Index {
    // See BlobError::kInvalidIndex. The current blob head offset and length
    // of the region must be provided, and are assumed to have already been
    // validated.
    constexpr bool IsValid(uint32_t blob_head, size_t length) const {
      ZX_DEBUG_ASSERT(blob_head <= length);
      return blob_head <= offset && offset <= length && size <= length - offset;
    }

    uint32_t size;
    uint32_t offset;
  };
  // TODO(https://fxbug.dev/354716628): commented to unblock clang roll.
  // TODO(https://github.com/llvm/llvm-project/pull/99570): Re-enable when libcxx underlying
  // implementation of `atomic_ref<T>` calculates `is_always_lockfree` appropriately.
  // static_assert(cpp20::atomic_ref<Index>::is_always_lock_free);

  // TODO(https://fxbug.dev/354716628): Remove workaround for const/volatile qualified atomic_ref.
  BlobIdAllocator* self() const { return const_cast<BlobIdAllocator*>(this); }
  cpp20::atomic_ref<Header> header() {
    return cpp20::atomic_ref{*reinterpret_cast<Header*>(bytes_.data())};
  }

  cpp20::atomic_ref<const Header> header() const {
    return cpp20::atomic_ref{*reinterpret_cast<const Header*>(bytes_.data())};
  }

  cpp20::atomic_ref<Index> GetIndex(uint32_t id) {
    size_t offset = static_cast<size_t>(id + 1) * sizeof(Index);
    ZX_DEBUG_ASSERT(offset + sizeof(Index) <= bytes_.size());
    return cpp20::atomic_ref{*reinterpret_cast<Index*>(&bytes_[offset])};
  }
  cpp20::atomic_ref<const Index> GetIndex(uint32_t id) const {
    size_t offset = static_cast<size_t>(id + 1) * sizeof(Index);
    ZX_DEBUG_ASSERT(offset + sizeof(Index) <= bytes_.size());
    return cpp20::atomic_ref{*reinterpret_cast<const Index*>(&bytes_[offset])};
  }

  cpp20::span<std::byte> bytes_;
};

}  // namespace iob

#endif  // LIB_IOB_BLOB_ID_ALLOCATOR_H_

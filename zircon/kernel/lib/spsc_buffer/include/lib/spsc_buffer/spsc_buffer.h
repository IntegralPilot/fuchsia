// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_SPSC_BUFFER_INCLUDE_LIB_SPSC_BUFFER_SPSC_BUFFER_H_
#define ZIRCON_KERNEL_LIB_SPSC_BUFFER_INCLUDE_LIB_SPSC_BUFFER_SPSC_BUFFER_H_

#include <lib/zx/result.h>
#include <zircon/types.h>

#include <ktl/algorithm.h>
#include <ktl/atomic.h>
#include <ktl/bit.h>
#include <ktl/byte.h>
#include <ktl/span.h>
#include <ktl/utility.h>

// The CopyOutFunction concept sets up a concept that the Spsc::Read function uses to copy data
// out of the ring buffer.
template <typename F>
concept CopyOutFunction = requires(F func, uint32_t offset, ktl::span<ktl::byte> src) {
  { func(offset, src) } -> std::same_as<zx_status_t>;
};

// The AllocatorType concept sets up a concept that the SpscBuffer uses to allocate and free its
// underlying storage.
template <typename T, typename... Args>
concept AllocatorType = requires(T, uint32_t size, ktl::byte* ptr, Args&&... args) {
  { T::Allocate(size, args...) } -> std::same_as<ktl::byte*>;
  { T::Free(ptr) } -> std::same_as<void>;
};

// SpscBuffer implements a transactional, single-producer, single-consumer ring buffer.
//
// The caller is responsible for ensuring that there is only one reader and one writer; no internal
// synchronization is provided to enforce this constraint.
//
// The caller is also responsible for providing the underlying memory region in which to store
// data. A span pointing to this buffer must be provided to the Init method before Read/Reserve can
// be called. Note that this backing buffer must have a size that is a power of two for correct
// functionality.
template <typename Allocator, typename... Args>
  requires AllocatorType<Allocator, Args...>
class SpscBuffer {
 public:
  SpscBuffer() = default;
  ~SpscBuffer() { Allocator::Free(storage_.data()); }

  // An SpscBuffer should be initialized once and never moved or copied.
  SpscBuffer(const SpscBuffer&) = delete;
  SpscBuffer& operator=(const SpscBuffer&) = delete;
  SpscBuffer(SpscBuffer&&) = delete;
  SpscBuffer operator=(SpscBuffer&&) = delete;

  // Initializes a buffer of the given size.
  zx_status_t Init(uint32_t size, Args... args) {
    // Assert that the provided buffer meets our requirements for the storage buffer.
    // See the comments around storage_ for more information about these requirements.
    if (size > kMaxStorageSize) {
      return ZX_ERR_INVALID_ARGS;
    }
    if (!ktl::has_single_bit(size)) {
      return ZX_ERR_INVALID_ARGS;
    }

    // SpscBuffers cannot be reinitialized once their backing storage has been allocated.
    if (storage_.data() != nullptr) {
      return ZX_ERR_BAD_STATE;
    }

    ktl::byte* ptr = Allocator::Allocate(size, ktl::forward<Args>(args)...);
    if (ptr == nullptr) {
      return ZX_ERR_NO_MEMORY;
    }

    storage_ = ktl::span<ktl::byte>(ptr, size);
    return ZX_OK;
  }

  // A simple convenience type used to hold the read and write pointers as separate values.
  struct RingPointers {
    uint32_t read;
    uint32_t write;
  };

  // A Reservation is a wrapper around a slot of memory in the ring buffer. It has a predetermined
  // size that is determined by the size passed into the Reserve method below. Any attempt to write
  // more than this amount of data into the slot is a programming error and will cause an assertion
  // failure. This class provides a formal way for writers to serialize data in place in the ring
  // buffer, thus eliminating the need for a temporary serialization buffer.
  class Reservation {
   public:
    ~Reservation() {
      // Ensure that commit has been called.
      DEBUG_ASSERT(committed_);
    }

    // Reservations cannot be copied.
    Reservation(const Reservation&) = delete;
    Reservation& operator=(const Reservation&) = delete;
    // Reservations cannot be move-assigned, as several members are const.
    Reservation& operator=(Reservation&& other) = delete;
    // Reservations can be move-constructed.
    Reservation(Reservation&& other)
        : spsc_(other.spsc_),
          initial_ring_pointers_(other.initial_ring_pointers_),
          region1_(other.region1_),
          region2_(other.region2_),
          write_offset_(other.write_offset_),
          committed_(other.committed_) {
      // Writes and Commits are not allowed when `committed_` is set to true, so this effectively
      // makes `other` a no-op and means we don't have to zero out the other fields.
      other.committed_ = true;
    }

    // Writes the given data into this reservation.
    void Write(ktl::span<const ktl::byte> data) {
      // Writing to an already committed reservation is not allowed.
      DEBUG_ASSERT(!committed_);

      size_t bytes_to_copy = data.size();
      size_t region1_copy_amount = 0;

      // Check where the write_offset_ is. If it's less than the size of region 1, copy as much
      // data as we can into region 1.
      if (write_offset_ < region1_.size()) {
        const size_t space_left_in_region1 = region1_.size() - write_offset_;
        region1_copy_amount = ktl::min(bytes_to_copy, space_left_in_region1);
        ktl::copy(data.begin(), data.begin() + region1_copy_amount,
                  region1_.subspan(write_offset_, region1_copy_amount).begin());
        write_offset_ += region1_copy_amount;
        bytes_to_copy -= region1_copy_amount;
      }

      // Check if we've written all of the data we need to. If we have, return early.
      if (bytes_to_copy == 0) {
        return;
      }

      // Now, if there's still data to copy, copy it into region 2. There should always be enough
      // space in region2 to do this, unless the `Reserve` call that created this reservation did
      // not reserve enough space. If this is the case, assert since writing beyond the bounds of
      // a reservation is a programming error.
      DEBUG_ASSERT(write_offset_ >= region1_.size());
      const uint64_t region2_offset = write_offset_ - region1_.size();
      ASSERT(region2_.size() >= region2_offset);
      ASSERT((region2_.size() - region2_offset) >= bytes_to_copy);
      ktl::copy(data.begin() + region1_copy_amount, data.end(),
                region2_.subspan(region2_offset, bytes_to_copy).begin());
      write_offset_ += bytes_to_copy;
    }

    // Advances the write pointer of the associated spsc buffer. This makes the written data visible
    // to the reader, and thus can only be called once all Writes have been completed.
    void Commit() {
      // Calling Commit twice on the same reservation is not allowed.
      DEBUG_ASSERT(!committed_);
      // Writers must fill up their reservation before calling commit. This is necessary to prevent
      // Readers from exfiltrating stale data from the section that was not written to.
      DEBUG_ASSERT(write_offset_ == (region1_.size() + region2_.size()));
      spsc_->AdvanceWritePointer(initial_ring_pointers_,
                                 static_cast<uint32_t>(region1_.size() + region2_.size()));
      committed_ = true;
    }

   private:
    friend class SpscBuffer;
    // Reservations should only be constructed by SpscBuffer. Hence, the constructor is made
    // private, and the class is marked a friend of the SpscBuffer.
    Reservation(SpscBuffer* spsc, SpscBuffer::RingPointers initial_ring_pointers,
                ktl::span<ktl::byte> r1, ktl::span<ktl::byte> r2)
        : spsc_(spsc), initial_ring_pointers_(initial_ring_pointers), region1_(r1), region2_(r2) {
      // Reservations with a zero byte region1, or whose total size is greater than the max storage
      // buffer size, are not allowed.
      DEBUG_ASSERT(r1.size() > 0);
      DEBUG_ASSERT((r1.size() + r2.size()) <= kMaxStorageSize);
    }

    SpscBuffer* const spsc_ = nullptr;
    const SpscBuffer::RingPointers initial_ring_pointers_;
    const ktl::span<ktl::byte> region1_;
    const ktl::span<ktl::byte> region2_;

    uint64_t write_offset_ = 0;
    bool committed_ = false;
  };

  // Reserves a block of the given size in the buffer.
  //
  // Any data written into this block will not be visible to readers until Commit is called on the
  // Reservation.
  zx::result<Reservation> Reserve(uint32_t size) {
    // Check that we have enough space for this record.
    if (size > storage_.size()) {
      return zx::error(ZX_ERR_NO_SPACE);
    }
    const RingPointers initial_state = LoadPointers();
    const uint32_t available_space = AvailableSpace(initial_state);
    if (available_space < size) {
      return zx::error(ZX_ERR_NO_SPACE);
    }

    // Construct the reservation object.
    const uint32_t write_offset = PointerToOffset(initial_state.write);
    const uint32_t ring_break_distance = static_cast<uint32_t>(storage_.size()) - write_offset;
    const uint32_t bytes_before_break = ktl::min(size, ring_break_distance);
    const ktl::span<ktl::byte> region1 = storage_.subspan(write_offset, bytes_before_break);
    const ktl::span<ktl::byte> region2 = bytes_before_break < size
                                             ? storage_.subspan(0, size - bytes_before_break)
                                             : ktl::span<ktl::byte>{};
    return zx::ok(Reservation(this, initial_state, region1, region2));
  }

  // Copies len bytes out of the buffer and into dst.
  //
  // Takes a CopyOutFunction as the first argument, which has the following signature:
  // zx_status_t copy_fn(uint32_t offset, ktl::span<ktl::byte> src);
  // where offset is the offset into the destination buffer at which we data should be written, and
  // src is the source buffer. This copy function may be invoked multiple times during the course
  // of this function.
  //
  // Returns the number of bytes read into dst on success. If copy_fn returns an error, that error
  // will be returned to the caller of Read.
  //
  // Importantly, even if an error is returned, 'copy_fn' might have already processed a partial
  // amount of data (between 0 and 'len' bytes). However, these partially processed bytes are
  // considered *not read* by this 'Read' function. Consequently, the internal read pointer of the
  // ring buffer will *not* be advanced for these unread bytes, meaning that these same bytes will
  // remain available for reading in subsequent calls to 'Read'.
  template <CopyOutFunction CopyFunc>
  zx::result<uint32_t> Read(CopyFunc copy_fn, uint32_t len) {
    const RingPointers initial_state = LoadPointers();

    const uint32_t available_data = AvailableData(initial_state);
    if (available_data == 0) {
      return zx::ok(0);
    }
    const uint32_t amount_to_copy = ktl::min(available_data, len);

    // Copy the data before the ring break.
    const uint32_t read_offset = PointerToOffset(initial_state.read);
    const uint32_t ring_break_distance = static_cast<uint32_t>(storage_.size()) - read_offset;
    const uint32_t bytes_before_break = ktl::min(amount_to_copy, ring_break_distance);
    zx_status_t status = copy_fn(0, storage_.subspan(read_offset, bytes_before_break));
    if (status != ZX_OK) {
      return zx::error(status);
    }

    // If there's more data left to be copied after the ring break, copy it now.
    if (bytes_before_break < amount_to_copy) {
      const uint32_t bytes_after_break = amount_to_copy - bytes_before_break;
      status = copy_fn(bytes_before_break, storage_.subspan(0, bytes_after_break));
      if (status != ZX_OK) {
        return zx::error(status);
      }
    }

    // It is critically important that the advancement of the read pointer occurs _after_ the copy
    // has completed. Otherwise, the Write method may see the updated read pointer before the copy
    // completes.
    AdvanceReadPointer(initial_state, amount_to_copy);
    return zx::ok(amount_to_copy);
  }

  // Empties the contents of the buffer. This is logically a Read operation, so a Read and Drain
  // cannot be called concurrently. Additionally, the behavior of this method is non-deterministic
  // if a Write is in-progress; the Drain may empty the buffer completely, or it may empty out all
  // data up to the beginning of the in-progress write. If callers wish to use this method to
  // deterministically empty the buffer, they will need to synchronize with Writers as well as
  // Readers.
  void Drain() {
    const RingPointers initial_state = LoadPointers();
    const uint32_t available_data = AvailableData(initial_state);
    if (available_data == 0) {
      return;
    }
    AdvanceReadPointer(initial_state, available_data);
  }

 private:
  friend class SpscBufferTests;

  // Helper function that converts read and write pointers into ring buffer offsets.
  uint32_t PointerToOffset(const uint32_t pointer) const {
    // This is logically performing pointer % storage_.size(), but because storage_.size() is
    // guaranteed to be a power of two it is equivalent to this logical AND.
    return pointer & (static_cast<uint32_t>(storage_.size()) - 1);
  }

  static RingPointers SplitCombinedPointers(const uint64_t combined) {
    return {
        .read = static_cast<uint32_t>(combined >> 32),
        .write = static_cast<uint32_t>(combined),
    };
  }
  static uint64_t CombinePointers(const RingPointers& pointers) {
    uint64_t read_64 = static_cast<uint64_t>(pointers.read);
    return (read_64 << 32) | pointers.write;
  }

  // The amount of data available to read is just the write pointer minus the read pointer.
  // This works even if the write pointer has overflowed and wrapped back around to zero
  // because unsigned subtraction in C++ uses modular arithmetic.
  static uint32_t AvailableData(const RingPointers& pointers) {
    return pointers.write - pointers.read;
  }
  uint32_t AvailableSpace(const RingPointers& pointers) const {
    return static_cast<uint32_t>(storage_.size()) - AvailableData(pointers);
  }

  // Helper function that loads the current values of the read and write pointers.
  //
  // This is a load-acquire operation that synchronizes with the store-release in the
  // Advance[Read|Write]Pointer functions. By using acquire semantics, we ensure that any stores
  // that occur prior to an Advance[Read|Write]Pointer store-release operation that we synchronize
  // with are observable by this thread after the load.
  RingPointers LoadPointers() const {
    const uint64_t combined = combined_pointers_.load(ktl::memory_order_acquire);
    return SplitCombinedPointers(combined);
  }

  // The AdvanceReadPointer and AdvanceWritePointer functions add the given delta to either the read
  // or write half of the combined_pointers_.
  //
  // Because we store the pointers in a single combined atomic variable, we must update the entire
  // combined pointer. We perform this update using a compare and exchange to ensure that concurrent
  // operations to the other half of the combined pointers are preserved. We also check to ensure
  // reads do not encounter concurrent reads, and writes do not encounter concurrent writes.
  //
  // This is a store-release operation that synchronizes with the load-acquire in LoadPointer.
  // By using release semantics, we ensure that if the updated value is seen in LoadPointers,
  // all memory operations that occurred prior to this update are observable.
  void AdvanceReadPointer(const RingPointers& initial, uint32_t delta) {
    // We should never be advancing the read pointer by more than the amount of data available in
    // the buffer.
    DEBUG_ASSERT(delta <= AvailableData(initial));

    const uint32_t target_read = initial.read + delta;

    uint64_t starting_pointers = CombinePointers(initial);
    uint64_t target_pointers = CombinePointers({.read = target_read, .write = initial.write});

    while (!combined_pointers_.compare_exchange_weak(
        starting_pointers, target_pointers, ktl::memory_order_release, ktl::memory_order_relaxed)) {
      const RingPointers observed = SplitCombinedPointers(starting_pointers);
      DEBUG_ASSERT_MSG(observed.read == initial.read,
                       "potential concurrent read detected; expected read pointer %u, got %u",
                       initial.read, observed.read);
      target_pointers = CombinePointers({.read = target_read, .write = observed.write});
    }
  }
  void AdvanceWritePointer(const RingPointers& initial, uint32_t delta) {
    // We should never be advancing the write pointer by more than the amount of empty space in the
    // buffer.
    DEBUG_ASSERT(delta <= AvailableSpace(initial));

    const uint32_t target_write = initial.write + delta;

    uint64_t starting_pointers = CombinePointers(initial);
    uint64_t target_pointers = CombinePointers({.read = initial.read, .write = target_write});

    while (!combined_pointers_.compare_exchange_weak(
        starting_pointers, target_pointers, ktl::memory_order_release, ktl::memory_order_relaxed)) {
      const RingPointers observed = SplitCombinedPointers(starting_pointers);
      DEBUG_ASSERT_MSG(observed.write == initial.write,
                       "potential concurrent write detected; expected write pointer %u, got %u",
                       initial.write, observed.write);
      target_pointers = CombinePointers({.read = observed.read, .write = target_write});
    }
  }

  // The read and write pointer are stored as the upper and lower halves, respectively, of a single
  // 64-bit atomic.
  //
  // The read and write pointers:
  // * Are unsigned 32-bit integers that monotonically increase as reads and writes are performed.
  // * Wrap when they reach UINT32_MAX, not when they reach the size of storage_. The offset into
  //   storage_ remains correct because it is required to have a size that is a power of two.
  // * Can be at most storage_.size() bytes apart.
  ktl::atomic<uint64_t> combined_pointers_{0};

  // The underlying storage used to store the data.
  //
  // This buffer MUST:
  // * Have a size that is a power of two, otherwise the modular arithmetic used to translate
  //   between pointers and offsets does not work correctly when a pointer overflows.
  // * Have a size smaller than 2^32 bytes because the read and write pointer must fit into 32
  //   bits. Taken in conjunction with the previous requirement, this means that the buffer must
  //   have a size less than or equal to 2^31 bytes.
  constexpr static uint32_t kMaxStorageSize = 1u << 31;
  ktl::span<ktl::byte> storage_;
};

#endif  // ZIRCON_KERNEL_LIB_SPSC_BUFFER_INCLUDE_LIB_SPSC_BUFFER_SPSC_BUFFER_H_

// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_FBL_INCLUDE_FBL_INT_SET_H_
#define ZIRCON_KERNEL_LIB_FBL_INCLUDE_FBL_INT_SET_H_

#include <align.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>

#include <cstddef>
#include <cstdint>

#include <ktl/limits.h>

namespace fbl {

// IntSet is a bitmap-based set of integers. The main differences with other implementations
// is that is meant to be very compact. One would expect it to be a wrapper around std::bitset
// but the interface of bitset does not allow to efficiently query the bits in the way that the
// compiler could always replace loops for ctz or similar intrinsics.
//
// This template takes |max_count| which is the maximum number of elements it can contain
// which does not have to be a power-of-two. The elements are from 0 to max_count - 1, thus
// these elements are called index or indexes in the interface.
template <uint32_t max_count>
class IntSet {
 public:
  // the overhead of this set is the |free_count_|.
  static constexpr size_t kFixedOverhead = sizeof(uint32_t);

  // adds and returns the first integer not in the set or errors out if the set is full.
  zx::result<uint32_t> allocate_new() {
    // We operate care-free for all the full words.
    const size_t last_full_word = kWordCount - 1;
    for (size_t word = 0u; word != last_full_word; ++word) {
      if (word_is_full(word)) {
        continue;
      }
      auto free_bit = __builtin_ctzll(~bitmap_[word]);
      set_bit(word, free_bit);
      return zx::ok(from_word_and_bit(word, free_bit));
    }
    // For the last word we need to check, if it is less than the maximum.
    if (!word_is_full(last_full_word)) {
      auto free_bit = __builtin_ctzll(~bitmap_[last_full_word]);
      auto index = from_word_and_bit(last_full_word, free_bit);
      if (index < max_count) {
        set_bit(last_full_word, free_bit);
        return zx::ok(index);
      }
    }
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  // Removes all integers in the set, for each |fn| is called with the id being removed.
  template <typename Fn>
  size_t remove_all_fn(Fn fn) {
    size_t count = 0u;
    for (size_t word = 0u; word != kWordCount; ++word) {
      if (word_is_empty(word)) {
        continue;
      }
      do {
        auto bit = __builtin_ctzll(bitmap_[word]);
        clear_bit(word, bit);
        fn(from_word_and_bit(word, bit));
        ++count;
      } while (!word_is_empty(word));
    }
    return count;
  }

  // Adds a new |index| or does nothing if already set.
  bool add(uint32_t index) {
    if (index >= max_count) {
      return false;
    }
    const auto dr = to_word_and_bit(index);
    return set_bit(dr.word, dr.bit);
  }

  // returns true if |index| is in the set.
  bool exists(uint32_t index) const {
    if (index >= max_count) {
      return false;
    }
    const auto dr = to_word_and_bit(index);
    return bit_is_set(dr.word, dr.bit);
  }

  // Removes |index| from the set. Returns false if |index| was not in the set.
  bool remove(uint32_t index) {
    if (index >= max_count) {
      return false;
    }
    const auto dr = to_word_and_bit(index);
    return clear_bit(dr.word, dr.bit);
  }

  // Returns how many free slots there are.
  uint32_t free_count() const { return free_count_; }

 private:
  // Splits the index. Input is trusted.
  static auto to_word_and_bit(uint32_t num) {
    struct {
      uint32_t word;
      uint32_t bit;
    } result{.word = num / kModulus, .bit = num % kModulus};
    return result;
  }

  // Computes the index from the bit-position.  Inputs are trusted.
  static uint32_t from_word_and_bit(size_t word, uint32_t bit) {
    return static_cast<uint32_t>((word * kModulus) + bit);
  }

  // Sets the bit and if it was vacant adjusts the |free_count|.
  // Inputs are trusted.
  bool set_bit(size_t word, uint32_t bit) {
    auto old = bitmap_[word];
    bitmap_[word] |= 1ull << bit;
    if (old == bitmap_[word]) {
      return false;
    }
    free_count_ -= 1u;
    return true;
  }

  // Clears the bit and if it was busy adjusts the |free_count|.
  // Inputs are trusted.
  bool clear_bit(size_t word, size_t bit) {
    auto old = bitmap_[word];
    bitmap_[word] &= ~(1ull << bit);
    if (old == bitmap_[word]) {
      return false;
    }
    free_count_ += 1u;
    return true;
  }

  // Returns true if the bit is set. Inputs are trusted.
  bool bit_is_set(size_t word, size_t bit) const { return (bitmap_[word] & (1ull << bit)) != 0u; }

  bool word_is_full(size_t word) const {
    return bitmap_[word] == ktl::numeric_limits<Storage>::max();
  }

  bool word_is_empty(size_t word) const { return bitmap_[word] == 0u; }

  using Storage = uint64_t;
  static constexpr uint32_t kModulus = sizeof(Storage) * 8u;
  static constexpr uint32_t kWordCount = ROUNDUP(max_count, kModulus) / kModulus;

  Storage bitmap_[kWordCount] = {};
  uint32_t free_count_ = max_count;
};

}  // namespace fbl

#endif  // ZIRCON_KERNEL_LIB_FBL_INCLUDE_FBL_INT_SET_H_

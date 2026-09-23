// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_COMPAT_HASH_H_
#define SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_COMPAT_HASH_H_

#include <bit>
#include <cassert>
#include <cstdint>
#include <functional>
#include <iterator>
#include <ranges>
#include <span>
#include <string_view>

#include "layout.h"

namespace elfldltl {

// This handles the DT_HASH format, which is mostly obsolete but is the
// official ELF standard format.  This interface matches GnuHash (gnu-hash.h).
// See SymbolInfo (symbol.h) for details.

constexpr uint32_t CompatHashString(std::string_view name) {
  uint_fast32_t hash = 0;
  for (char c : name) {
    hash = (hash << 4) + std::bit_cast<unsigned char>(c);
    hash ^= (hash >> 24) & 0xf0;
  }
  return hash & 0x0fffffff;
}

constexpr uint32_t kCompatNoHash = ~uint32_t{};

// In DT_HASH format, there is a table mapping hash buckets to indices of the
// first symbol table entry in the bucket.  A second "chain" table maps the
// symbol table index of each symbol to the next symbol in the same bucket.
// Empty buckets and the end of a chain are identified by index 0 (STN_UNDEF),
// which is always a null entry.  The first two words of the DT_HASH data are
// the number of buckets and the number of chain entries (i.e. the number of
// symbol table entries).  Then the bucket words follow, then the chain words.

template <ElfApi Elf>
class CompatHash {
 public:
  using Word = Elf::Word;

  constexpr explicit CompatHash(std::span<const Word> table)
      : buckets_(table.subspan(2, table[0])), chain_(table.subspan(2 + table[0], table[1])) {}

  static constexpr bool Valid(std::span<const Word> table) {
    if (table.size() < 2) {
      return false;
    }
    const uint32_t nbucket = table[0], nchain = table[1];
    return table.size() - 2 > nbucket && table.size() - 2 - nbucket >= nchain;
  }

  constexpr uint32_t symtab_size() const { return static_cast<uint32_t>(chain_.size()); }

  constexpr size_t size() const { return buckets_.size(); }

  constexpr std::ranges::forward_range auto AllBuckets() const {
    return std::views::transform(
        // Each element of buckets_ is the symbol table index of the first
        // symbol in that bucket.  An empty hash bucket holds index zero.
        // Since table index zero is never a real symbol, it's used as the
        // end() BucketIterator.  So an empty bucket has begin() == end().
        buckets_, std::bind_front(&BucketIterator::MakeRange, chain_));
  }

  constexpr std::ranges::forward_range auto Bucket(uint32_t hash) const {
    uint32_t symndx = 0;
    if (!buckets_.empty()) [[likely]] {
      symndx = buckets_[hash % buckets_.size()];
    }
    return BucketIterator::MakeRange(chain_, symndx);
  }

 private:
  using ChainTable = std::span<const typename Elf::Word>;

  class BucketIterator {
   public:
    using difference_type = ptrdiff_t;  // Required by std::weakly_incrementable.
    using value_type = uint32_t;        // Required by std::indirectly_readable.

    constexpr BucketIterator() = default;
    constexpr BucketIterator(const BucketIterator&) = default;
    constexpr BucketIterator& operator=(const BucketIterator&) = default;

    constexpr bool operator==(const BucketIterator& other) const {
      assert(chain_.data() == other.chain_.data());
      return i_ == other.i_;
    }

    constexpr BucketIterator& operator++() {  // prefix
      // The chain table might encode an infinite loop here.  So cut short
      // iteration when the total number of entries has been enumerated.  In
      // corrupt data, this may not have covered all the entries because it hit a
      // loop.  In valid data, the natural end will always be reached first.
      if (++count_ > chain_.size()) [[unlikely]] {
        i_ = 0;
      } else {
        i_ = ChainIndex(chain_[i_]);
      }
      return *this;
    }

    constexpr BucketIterator operator++(int) {  // postfix
      auto old = *this;
      ++*this;
      return old;
    }

    constexpr uint32_t operator*() const { return i_; }

   private:
    friend CompatHash;

    // Index zero is never a real symbol, so it's used as the end() iterator
    // state.  If a bogus index came out of the table, reset to end() state.
    constexpr uint32_t ChainIndex(uint32_t symndx) {
      if (symndx < chain_.size()) [[likely]] {
        return symndx;
      }
      return 0;
    }

    static constexpr auto MakeRange(ChainTable chain, uint32_t symndx) {
      static_assert(std::forward_iterator<BucketIterator>);
      using Range = std::ranges::subrange<BucketIterator>;
      static_assert(std::ranges::forward_range<Range>);

      BucketIterator begin, end;
      begin.chain_ = end.chain_ = chain;
      begin.i_ = begin.ChainIndex(symndx);
      return Range{begin, end};
    }

    ChainTable chain_;
    uint32_t i_ = 0;
    uint32_t count_ = 0;
  };

  std::span<const Word> buckets_, chain_;
};

}  // namespace elfldltl

#endif  // SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_COMPAT_HASH_H_

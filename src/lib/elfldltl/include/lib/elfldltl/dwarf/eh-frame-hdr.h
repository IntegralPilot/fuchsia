// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_DWARF_EH_FRAME_HDR_H_
#define SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_DWARF_EH_FRAME_HDR_H_

#include <compare>
#include <ios>
#include <iterator>
#include <ranges>

#include "../diagnostics.h"
#include "../layout.h"
#include "../memory.h"
#include "encoding.h"

namespace elfldltl::dwarf {

// This is the fixed portion of the GNU .eh_frame_hdr format.  It identifies
// the version of the format, and then the encodings of the data that follow.
// Immediately after this header follow in sequence:
//
//  * Pointer to .eh_frame section, encoded as eh_frame_ptr says.
//
//  * Count of FDEs in the table, encoded as fde_count says.
//
//  * That many table entries, each encoded as fde_table says.
//    Each entry is a PC value followed by an FDE address, both using
//    the same encoding.  The table is sorted in ascending PC order.
//
struct EhFrameHdrEncoding {
  static constexpr uint8_t kVersion = 1;

  uint8_t version = kVersion;
  uint8_t eh_frame_ptr = EncodedPtr::kOmit;
  uint8_t fde_count = EncodedPtr::kOmit;
  uint8_t fde_table = EncodedPtr::kOmit;
};

// The .eh_frame_hdr section (PT_GNU_EH_FRAME at runtime) is an index into the
// .eh_frame section.  It provides a sorted table mapping a PC to the FDE whose
// initial_location is that PC.  Thus it's easy to do a quick binary search for
// any PC in the module's vaddr range and find the FDE that probably contains
// it.  (The last FDE in the table will be found for any PC past the last
// instruction covered by that FDE, but the FDE itself must be decoded--and its
// CIE first--before that can be determined.)

// EhFrameHdrEntry is sortable on PC.
// It can be used as `auto [pc, fde]` in for loops.
template <typename SizeType>
struct EhFrameHdrEntry {
  constexpr auto operator<=>(const EhFrameHdrEntry& other) const = default;
  constexpr auto operator<=>(SizeType other) const { return pc <=> other.pc; }

  SizeType pc = 0, fde = 0;
};

// ostream-compatible objects can format EhFrameHdrEntry with the << operator.
template <typename Ostream, typename SizeType>
constexpr decltype(auto) operator<<(Ostream&& ostream, const EhFrameHdrEntry<SizeType>& entry) {
  auto flags = ostream.flags();
  ostream << std::hex << std::showbase  // Use 0x123abc format.
          << "[PC " << entry.pc << " -> FDE " << entry.fde << "]";
  ostream.flags(flags);
  return std::forward<Ostream>(ostream);
}

// The EhFrameHdr object acts like a container of PC, FDE pairs (Entry) giving
// the vaddr of the corresponding FDE.  Its `.find` method does binary search.
// The API is otherwise also similar to `const std::map<uintNN_t, uintNN_t>`,
// except that `operator[]` is an index rather than associative (use `.find`).
//
// This object only handles the index table, not actually decoding FDEs.  Once
// an FDE is found in the table, <elfldltl/dwarf/cfi-entry.h> APIs decode it.
//
// Note that the PT_GNU_EH_FRAME p_filesz only covers .eh_frame_hdr, not
// .eh_frame.  So the eh_frame_ptr (see below) and the FDE addresses in the
// table will point outside the bounds of memory fetched (or bounded) just to
// decode .eh_frame_hdr.  They will both be in the same RODATA segment, so if
// the entire segment is made accessible then both .eh_frame_hdr and entries it
// locates can be gleaned from it can be accessed in a uniform manner.

template <ElfApi Elf = Elf<>>
class EhFrameHdr {
 public:
  // This is usually called `size_type` in toolkit code, but here that refers
  // to the size of the element count in the container-style API.
  using address_size_type = Elf::size_type;
  using Phdr = Elf::Phdr;

  class iterator {
   public:
    using value_type = EhFrameHdrEntry<address_size_type>;
    using difference_type = ptrdiff_t;  // std::weakly_incrementable requires.

    constexpr iterator() = default;
    constexpr iterator(const iterator&) = default;
    constexpr iterator& operator=(const iterator&) = default;

    constexpr bool operator==(const iterator& other) const {
      assert(hdr_ == other.hdr_);
      return pos_ == other.pos_;
    }

    constexpr std::strong_ordering operator<=>(const iterator& other) const {
      assert(hdr_ == other.hdr_);
      return pos_ <=> other.pos_;
    }

    constexpr const value_type& operator*() const { return entry_; }

    constexpr const value_type* operator->() const { return &entry_; }

    constexpr const value_type& operator[](ptrdiff_t n) const { return *(*this + n); }

    constexpr iterator& operator++() {  // prefix
      *this += 1;
      return *this;
    }

    constexpr iterator operator++(int) {  // postfix
      iterator old = *this;
      ++*this;
      return old;
    }

    constexpr iterator& operator--() {  // prefix
      *this -= 1;
      return *this;
    }

    constexpr iterator operator--(int) {  // postfix
      iterator old = *this;
      --*this;
      return old;
    }

    constexpr iterator& operator+=(ptrdiff_t n) {
      const size_t table_end_pos = hdr_->fde_table_.size_bytes();
      const ptrdiff_t distance = n * hdr_->entry_size_;
      if (distance < 0 ? static_cast<size_t>(-distance) < pos_
                       : (static_cast<size_t>(distance) > table_end_pos ||
                          table_end_pos - pos_ <= static_cast<size_t>(distance))) {
        // This either reaches the end exactly or is invalid, which we make
        // reach the end instead of aborting though it's technically undefined
        // behavior to advance an iterator past either end of its container.
        pos_ = table_end_pos;
      } else {
        pos_ += distance;
        Decode();
      }
      return *this;
    }

    constexpr iterator operator+(ptrdiff_t n) const {
      iterator it = *this;
      it += n;
      return it;
    }

    friend constexpr iterator operator+(ptrdiff_t n, iterator it) {
      it += n;
      return it;
    }

    constexpr iterator& operator-=(ptrdiff_t n) { return *this += -n; }

    constexpr iterator operator-(ptrdiff_t n) const { return *this + -n; }

    constexpr ptrdiff_t operator-(const iterator& other) const {
      assert(other.hdr_ == hdr_);
      const ptrdiff_t distance =
          (static_cast<ptrdiff_t>(pos_) - static_cast<ptrdiff_t>(other.pos_));
      return distance / hdr_->entry_size_;
    }

   private:
    friend EhFrameHdr;

    constexpr iterator(const EhFrameHdr& hdr, size_t pos) : hdr_(&hdr), pos_(pos) {
      assert(hdr_->fde_table_.size_bytes() % hdr_->entry_size_ == 0);
      if (pos_ < hdr_->fde_table_.size_bytes()) {
        Decode();
      }
    }

    constexpr void Decode() {
      assert(pos_ < hdr_->fde_table_.size_bytes());
      assert(hdr_->fde_table_.size_bytes() - pos_ >= hdr_->entry_size_);
      std::span bytes = hdr_->fde_table_.subspan(pos_, hdr_->entry_size_);
      auto pc = EncodedPtr::Read<Elf>(hdr_->encoding_.fde_table, bytes);
      assert(pc);
      assert(pc->encoded_size == hdr_->entry_size_ / 2);
      bytes = bytes.subspan(pc->encoded_size);
      auto fde = EncodedPtr::Read<Elf>(hdr_->encoding_.fde_table, bytes);
      assert(fde);
      assert(fde->encoded_size == hdr_->entry_size_ / 2);
      if (EncodedPtr::Signed(hdr_->encoding_.fde_table)) {
        // The value has already been sign-extended, so the adjustments below
        // will work the same as either signed or unsigned.  Pro forma no-op to
        // use the signed value as unsigned.
        pc->ptr = std::bit_cast<uint64_t>(pc->sptr);
      }
      switch (EncodedPtr::Modifier(hdr_->encoding_.fde_table)) {
        case EncodedPtr::kAbs:
          break;

        case EncodedPtr::kPcrel:
          // Each value is relative to its own location inside .eh_frame_hdr.
          {
            pc->ptr += hdr_->table_offset_ + pos_;
            fde->ptr += hdr_->table_offset_ + pos_ + pc->encoded_size;
          }
          [[fallthrough]];  // Now relative to .eh_frame_hdr.

        case EncodedPtr::kDatarel:
          // Each value is relative to the beginning of .eh_frame_hdr.
          pc->ptr += hdr_->vaddr_;
          fde->ptr += hdr_->vaddr_;
          break;

        default:
          // The encoding was vetted in Init.
          __builtin_trap();
      }
      entry_ = {
          .pc = static_cast<address_size_type>(pc->ptr),
          .fde = static_cast<address_size_type>(fde->ptr),
      };
    }

    const EhFrameHdr* hdr_ = nullptr;
    size_t pos_ = 0;
    value_type entry_;
  };
  static_assert(std::random_access_iterator<iterator>);

  using const_iterator = iterator;

  using value_type = iterator::value_type;

  static constexpr uint8_t kAddressSize = sizeof(address_size_type);

  // This is the whole size occupied at the PT_GNU_EH_FRAME vaddr.
  constexpr size_t size_bytes() const {
    return sizeof(encoding_) +  //
           EncodedPtr::EncodedSize(encoding_.eh_frame_ptr, kAddressSize) +
           EncodedPtr::EncodedSize(encoding_.fde_count, kAddressSize) +
           EncodedPtr::EncodedSize(encoding_.fde_table, kAddressSize) +  //
           fde_table_.size_bytes();
  }

  // The container-like methods act similarly to std::array<value_type, N>.

  constexpr size_t size() const {
    return entry_size_ == 0 ? 0 : fde_table_.size_bytes() / entry_size_;
  }

  constexpr bool empty() const { return size() == 0; }

  constexpr iterator begin() const { return {*this, 0}; }
  constexpr iterator cbegin() const { return begin(); }

  constexpr iterator end() const { return {*this, fde_table_.size_bytes()}; }
  constexpr iterator cend() const { return end(); }

  constexpr const value_type& operator[](size_t idx) { return begin()[idx]; }

  // This just does simple binary search.  Note that entries identify only the
  // FDE's starting PC and not its PC limit, so this will "find" the last FDE
  // in the table for any PC that's above all the entries.  The FDE itself must
  // be decoded to determine where its PC range ends.  (Usually the EhFrameHdr
  // for a given module will only be searched for a PC already known to fall
  // within that module's vaddr bounds, so only PCs off the end of the last FDE
  // in alignment fill or code lacking CFI would "wrongly" report that FDE.)
  constexpr iterator find(address_size_type pc) const {
    return std::lower_bound(begin(), end(), pc);
  }

  // This is the vaddr of the start of the .eh_frame table.  This is not really
  // needed since .eh_frame_hdr table entries have the vaddr of an FDE inside.
  // When the lookup table is omitted (empty() returns true), .eh_frame can be
  // searched linearly and will be terminated by an invalid CFI entry with zero
  // initial length (i.e. a lone zero uint32_t after the valid last entry).
  constexpr address_size_type eh_frame_ptr() const { return eh_frame_ptr_; }

  // Initialize directly from the PT_GNU_EH_FRAME phdr.
  template <class Diagnostics, typename... ErrorArgs>
  constexpr bool Init(Diagnostics& diag,
                      MemoryReader<address_size_type, std::byte, EhFrameHdrEncoding> auto& memory,
                      const Phdr& phdr, ErrorArgs&&... error_args) {
    return Init(diag, memory, phdr.vaddr, std::forward<ErrorArgs>(error_args)...);
  }

  // Initialize from .eh_frame_hdr data read from the vaddr via the Memory
  // object.  The return value is propagated from the Diagnostics object.
  template <class Diagnostics, typename... ErrorArgs>
  constexpr bool Init(Diagnostics& diag,
                      MemoryReader<address_size_type, std::byte, EhFrameHdrEncoding> auto& memory,
                      address_size_type vaddr, ErrorArgs&&... error_args) {
    using namespace std::string_view_literals;

    // Record the vaddr of the start of .eh_frame_hdr (the header).  The vaddr
    // local is advanced as we read, and used in error formatting so it gives
    // the precise vaddr of the problem data.
    vaddr_ = vaddr;

    auto fail = [&vaddr, &diag, &error_args...](auto... args) {
      return diag.FormatError(args..., FileAddress{vaddr}, std::forward<ErrorArgs>(error_args)...);
    };
    auto cannot_read = [&fail]() { return fail("cannot read PT_GNU_EH_FRAME header"sv); };

    if (auto read = memory.template ReadArray<EhFrameHdrEncoding>(vaddr, 1)) {
      const EhFrameHdrEncoding& hdr = read->front();
      if (hdr.version != EhFrameHdrEncoding::kVersion) [[unlikely]] {
        return fail("PT_GNU_EH_FRAME header version "sv, hdr.version, " != expected "sv,
                    EhFrameHdrEncoding::kVersion);
      }
      encoding_ = hdr;
    } else [[unlikely]] {
      return cannot_read();
    }

    if (encoding_.eh_frame_ptr == EncodedPtr::kOmit || encoding_.fde_count == EncodedPtr::kOmit ||
        encoding_.fde_table == EncodedPtr::kOmit) {
      // Empty table.
      return true;
    }

    uint8_t ptr_size = EncodedPtr::EncodedSize(encoding_.eh_frame_ptr, kAddressSize);
    uint8_t count_size = EncodedPtr::EncodedSize(encoding_.fde_count, kAddressSize);
    entry_size_ = EncodedPtr::EncodedSize(encoding_.fde_table, kAddressSize);
    if (ptr_size == EncodedPtr::kDynamicSize || count_size == EncodedPtr::kDynamicSize ||
        entry_size_ == EncodedPtr::kDynamicSize) [[unlikely]] {
      return fail("LEB128 encoded not allowed in PT_GNU_EH_FRAME"sv);
    }
    entry_size_ *= 2;  // PC + FDE each with the same encoding.

    vaddr += sizeof(EhFrameHdrEncoding);
    if (auto eh_frame_ptr = EncodedPtr::FromMemory<Elf>(encoding_.eh_frame_ptr, memory, vaddr)) {
      eh_frame_ptr_ = static_cast<address_size_type>(*eh_frame_ptr);
    } else {
      return fail("cannot decode PT_GNU_EH_FRAME .eh_frame pointer"sv);
    }
    vaddr += ptr_size;

    auto count = EncodedPtr::FromMemory<Elf>(encoding_.fde_count, memory, vaddr);
    if (!count) {
      return fail("cannot decode PT_GNU_EH_FRAME FDE count"sv);
    }
    vaddr += count_size;

    table_offset_ = sizeof(EhFrameHdrEncoding) + ptr_size + count_size;

    size_t table_size = *count * entry_size_;

    auto table = memory.template ReadArray<std::byte>(vaddr, table_size);
    if (!table) {
      return fail("cannot read PT_GNU_EH_FRAME FDE table of "sv, table_size, " bytes ("sv, *count,
                  " * "sv, static_cast<unsigned int>(entry_size_), " bytes per entry)"sv);
    }

    fde_table_ = *table;
    assert(fde_table_.size_bytes() == table_size);

    // Make sure iterator::Decode() will know what to do.
    switch (EncodedPtr::Modifier(encoding_.fde_table)) {
      case EncodedPtr::kAbs:
      case EncodedPtr::kPcrel:
      case EncodedPtr::kDatarel:
        break;
      default:
        [[unlikely]];
        return fail("PT_GNU_EH_FRAME uses unsupported relative encoding ",
                    static_cast<unsigned int>(encoding_.fde_table), " for FDE table");
    }

    return true;
  }

 private:
  std::span<const std::byte> fde_table_;
  address_size_type vaddr_ = 0;
  address_size_type eh_frame_ptr_ = 0;
  EhFrameHdrEncoding encoding_;
  uint8_t entry_size_ = 0;
  uint8_t table_offset_ = 0;
};
static_assert(std::ranges::random_access_range<EhFrameHdr<>>);

}  // namespace elfldltl::dwarf

#endif  // SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_DWARF_EH_FRAME_HDR_H_

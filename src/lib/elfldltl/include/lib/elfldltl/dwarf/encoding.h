// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_DWARF_ENCODING_H_
#define SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_DWARF_ENCODING_H_

#include <cassert>
#include <cstdint>
#include <optional>
#include <span>

#include "../layout.h"
#include "../memory.h"

namespace elfldltl::dwarf {

// ULEB128 is a byte-granularity "bignum"-style encoding used in DWARF.
// Encodings use as few bytes as needed to represent the value, 7 bits of
// value in each byte of encoding: up to 5 bytes for up to 32 bits, up to
// 10 bytes for up to 64 bits.
struct Uleb128 {
  static constexpr size_t kMaxBytes = 10;

  // Read one ULEB128 value from the byte buffer.  Returns std::nullopt if the
  // buffer is too short or if the encoding uses more bytes than should be
  // necessary for a 64-bit value.
  static std::optional<Uleb128> Read(std::span<const std::byte> bytes);

  // This is the value, zero-extended to uint64_t.
  uint64_t value = 0;

  // This is the number of bytes in the encoding: how many bytes were
  // consumed by the Read call that created this Uleb128 object.
  size_t size_bytes = 0;
};

// SLEB128 is the same encoding as ULEB128, but the value is understood to
// be sign-extended from the highest bit present in the encoded value.
struct Sleb128 {
  static constexpr size_t kMaxBytes = 10;

  // Read one SLEB128 value from the byte buffer.  Returns std::nullopt if the
  // buffer is too short or if the encoding uses more bytes than should be
  // necessary for a 64-bit value.
  static std::optional<Sleb128> Read(std::span<const std::byte> bytes);

  // This is the value, sign-extended to int64_t.
  int64_t value = 0;

  // This is the number of bytes in the encoding: how many bytes were
  // consumed by the Read call that created this Sleb128 object.
  size_t size_bytes = 0;
};

// This is the encoding byte used in the DW_OP_GNU_encoded_addr extension,
// in GNU ..eh_frame_hdr format, and in GNU de facto standard augmentation
// for .debug_frame formats.  This is a struct with non-scoped enums rather
// than using `enum class`, so that the names are scoped to the struct type
// but the values are convertible to uint8_t and implicitly usable with
// bitwise operations.
//
// The default-constructed EncodedPtr object represents an omitted value.
// This gives an integer value of zero, but takes no space to encode.
struct EncodedPtr {
  // These are the primary values that indicate basic integer encoding.
  enum PtrType : uint8_t {
    kOmit = 0xff,  // No value present.

    kPtr = 0x00,      // Address size, unsigned.
    kUleb128 = 0x01,  // ULEB128 encoded (unsigned), variable length.
    kUdata2 = 0x02,   // 16 bits unsigned.
    kUdata4 = 0x03,   // 32 bits unsigned.
    kUdata8 = 0x04,   // 64 bits unsigned.

    // This is actually a flag bit, combined with one of the unsigned
    // encodings above to yield their signed counterparts below.
    kSigned = 0x08,

    kSleb128 = 0x09,  // SLEB128 encoded (signed), variable length.
    kSdata2 = 0x0a,   // 16 bits signed.
    kSdata4 = 0x0b,   // 32 bits signed.
    kSdata8 = 0x0c,   // 64 bits signed.
  };

  // One of these can be OR'd in with one of the basic encodings above.  Note
  // that the relative encodings implicitly refer to different base addresses
  // in different contexts, e.g. kDatarel inside .eh_frame_hdr is relative to
  // the beginning of .eh_frame_hdr itself.
  enum PtrModifier : uint8_t {
    kAbs = 0x00,      // Value is absolute.
    kPcrel = 0x10,    // Value is relative to its own location.
    kTextrel = 0x20,  // Value is relative to "text" segment (contextual).
    kDatarel = 0x30,  // Value is relative to "data" segment (contextual).
    kFuncrel = 0x40,  // Value is relative to function.
    kAligned = 0x50,  // Encoded value starts at naturally aligned location.
  };

  // This can be separately OR'd to indicate that the encoded address is
  // actually the location of the value as for Encoding::kAbsptr.
  static constexpr uint8_t kIndirect = 0x80;

  // This yields just the basic encoding, regardless of indirection or
  // adjustments.  This is all that's needed to determine the encoded size.
  static constexpr PtrType Type(uint8_t encoding) {
    return encoding == kOmit ? kOmit : static_cast<PtrType>(encoding & 0x0f);
  }

  // This yields just the modifier for a relative address.  After the basic
  // value is decoded according to Type(encoding), this is what adjustment must
  // be done to the value.
  static constexpr PtrModifier Modifier(uint8_t encoding) {
    return encoding == kOmit ? kAbs : static_cast<PtrModifier>(encoding & 0x70);
  }

  // This indicates that the value is actually stored elsewhere in memory.  The
  // Type(encoding) still indicates the type of that stored pointer, as well as
  // the basic type of the encoding used to locate it.  After applying the
  // Modifier(encoding) adjustments to the encoded pointer, that pointer must
  // be dereferenced to fetch the desired value.
  static constexpr bool Indirect(uint8_t encoding) {
    return encoding != kOmit && (encoding & kIndirect);
  }

  // This indicates if the encoded value is signed, so it should be
  // sign-extended from narrower encoding to a wider integer type.
  static constexpr bool Signed(uint8_t encoding) {
    return encoding != kOmit && (encoding & kSigned);
  }

  // EncodedSize returns this value for the LEB128 types, which have a
  // variable-sized encoding.  The exact size can only be determined by
  // actually decoding the value.
  static constexpr uint8_t kDynamicSize = -1;

  // This returns the encoded size, which may depend on the contextual address
  // size.  It returns kDynamicSize for LEB128 types whose exact size cannot be
  // known without the actual data.
  static constexpr uint8_t EncodedSize(uint8_t encoding, uint8_t address_size) {
    switch (Type(encoding)) {
      case kPtr:
      case kSigned:
        return address_size;
      case kOmit:
        return 0;
      case kUdata2:
      case kSdata2:
        return 2;
      case kUdata4:
      case kSdata4:
        return 4;
      case kUdata8:
      case kSdata8:
        return 8;
      case kUleb128:
      case kSleb128:
        break;
    }
    return kDynamicSize;
  }

  // This normalizes the encoding so that it's unambiguous with respect to
  // address size.  After normalization, an encoding can be used directly
  // without keeping track of the address size that's indicated by, or implicit
  // in, the context it came from.
  template <ElfApi Elf = Elf<>>
  static constexpr uint8_t Normalize(uint8_t encoding,
                                     uint8_t address_size = sizeof(typename Elf::Addr)) {
    if ((encoding & 0x7) == 0) {
      encoding |= 3 + (address_size >> 3);
    }
    return encoding;
  }

  // Read an encoded value via the Memory object.  Both the vaddr argument and
  // the encoded addresses (in case of indirection) are in whatever address
  // space the Memory object provides.  To support the indirection case
  // properly, don't adjust the vaddr argument for use with a generic Memory
  // object.  Instead use a Memory object that takes the unadjusted address and
  // implicitly applies the runtime load bias for the module containing the
  // DWARF metadata being read; this ensures that a possible second call to the
  // Memory object will correctly handle an address read from the metadata
  // rather than the given vaddr argument.  When reading variable-sized
  // (LEB128) data, the single-argument ReadArray method of the Memory object
  // is expected to return at least as much data as the value encoding requires
  // in the single call.  Returns std::nullopt if the Memory object fails.
  // Otherwise the value is extended to 64 bits.  In the case of a signed
  // encoding, bit_cast<int64_t> should be used on the value.
  template <ElfApi Elf = Elf<>>
  static constexpr std::optional<uint64_t> FromMemory(
      uint8_t encoding,
      MemoryReader<typename Elf::size_type, std::byte, typename Elf::Addr> auto& memory,
      Elf::size_type vaddr, uint8_t address_size = sizeof(typename Elf::Addr)) {
    uint8_t size = EncodedSize(encoding, address_size);
    if (size == 0) {
      return 0;
    }

    std::optional<EncodedPtr> encoded;
    if (auto read = size == kDynamicSize  //
                        ? memory.template ReadArray<std::byte>(vaddr, size)
                        : memory.template ReadArray<std::byte>(vaddr)) {
      uint8_t read_encoding = encoding;
      if (Indirect(encoding) && Modifier(encoding) == kPcrel) {
        // Always sign-extend a relative value.
        read_encoding |= kSigned;
      }
      encoded = Read<Elf>(read_encoding, *read, address_size);
    }
    if (!encoded) {
      return std::nullopt;
    }
    switch (Modifier(encoding)) {
      case kAbs:
        break;
      case kPcrel:
        encoded->ptr = vaddr + encoded->sptr;
        break;
      default:
        return std::nullopt;
    }
    if (Indirect(encoding)) {
      if (auto read = memory.template ReadArray<typename Elf::Addr>(encoded->ptr, 1)) {
        return read->front();
      }
      return std::nullopt;
    }
    return encoded->ptr;
  }

  // Read an encoded value from the byte buffer.  This returns an
  // EncodedPtr object rather than the resolved value.  The caller is
  // responsible for applying modifiers and indirection to the value.
  template <ElfApi Elf = Elf<>>
  static constexpr std::optional<EncodedPtr> Read(
      uint8_t encoding, std::span<const std::byte> bytes,
      uint8_t address_size = sizeof(typename Elf::Addr)) {
    if (Type(encoding) == kSleb128) {
      if (auto leb = Sleb128::Read(bytes)) {
        return EncodedPtr{
            .sptr = leb->value,
            .encoding = encoding,
            .encoded_size = static_cast<uint8_t>(leb->size_bytes),
        };
        return std::nullopt;
      }
    }
    if (Type(encoding) == kUleb128) {
      if (auto leb = Uleb128::Read(bytes)) {
        return EncodedPtr{
            .ptr = leb->value,
            .encoding = encoding,
            .encoded_size = static_cast<uint8_t>(leb->size_bytes),
        };
        return std::nullopt;
      }
    }

    const uint8_t encoded_size = EncodedSize(encoding, address_size);
    if (encoded_size == 0) {
      return EncodedPtr{};
    }

    assert(encoded_size != kDynamicSize);  // LEB128 was caught above.
    if (encoded_size > bytes.size_bytes()) [[unlikely]] {
      return std::nullopt;
    }

    auto decode = [encoding, bytes](auto unsigned_value) -> EncodedPtr {
      if (Signed(encoding)) {
        typename decltype(unsigned_value)::Signed value;
        memcpy(&value, bytes.data(), sizeof(value));
        return {
            .sptr = static_cast<int64_t>(value),
            .encoding = encoding,
            .encoded_size = sizeof(value),
        };
      }
      memcpy(&unsigned_value, bytes.data(), sizeof(unsigned_value));
      return {
          .ptr = unsigned_value,
          .encoding = encoding,
          .encoded_size = sizeof(unsigned_value),
      };
    };

    switch (encoded_size) {
      case 2:
        return decode(typename Elf::Half{});
      case 4:
        return decode(typename Elf::Word{});
      case 8:
        return decode(typename Elf::Xword{});
    }

    return std::nullopt;
  }

  // The value is either signed or unsigned, as indicated by the encoding.
  // Narrower signed values have been sign-extended to int64_t.  This is
  // only the final value for encodings with no modifiers or indirection.
  union {
    uint64_t ptr = 0;
    int64_t sptr;
  };

  // This records the original encoding, including modifiers and
  // indirection.  The .ptr or .sptr value must be adjusted according to
  // any relative modifier (usually "PC-relative", meaning relative to its
  // own encoding location).  If indirection is indicated, the resulting
  // pointer must be used to fetch the actual value (of the same size).
  uint8_t encoding = kOmit;

  // This gives the total size of the encoding: how many bytes were
  // consumed by the Read call that created this EncodedPtr.
  uint8_t encoded_size = 0;
};

}  // namespace elfldltl::dwarf

#endif  // SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_DWARF_ENCODING_H_

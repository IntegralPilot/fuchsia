// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/expr/bitfield.h"

#include <cstdint>

#include "src/developer/debug/zxdb/common/int128_t.h"
#include "src/developer/debug/zxdb/expr/expr_value.h"
#include "src/developer/debug/zxdb/expr/found_member.h"
#include "src/developer/debug/zxdb/symbols/base_type.h"

namespace zxdb {

namespace {

// We use 128-bit numbers for bitfield computations so we can shift around 64 bit bitfields. This
// allows us to handle anything up to 120 bits, or 128 bits if the beginning is aligned. This
// limitation seems reasonable.

// We treat "signed int" bitfields as being signed and needing sign extensions. Whether "int"
// bitfields are signed or unsigned is actually implementation-defined in the C standard.
bool NeedsSignExtension(const fxl::RefPtr<EvalContext>& context, const Type* type, uint128_t value,
                        uint32_t bit_size) {
  fxl::RefPtr<BaseType> base_type = context->GetConcreteTypeAs<BaseType>(type);
  if (!base_type)
    return false;

  if (!BaseType::IsSigned(base_type->base_type()))
    return false;  // Unsigned type.

  // Needs sign extension when the high bit is set.
  return value & (1ull << (bit_size - 1));
}

}  // namespace

ErrOrValue ResolveBitfieldMember(const fxl::RefPtr<EvalContext>& context, const ExprValue& base,
                                 const FoundMember& found_member) {
  const DataMember* data_member = found_member.data_member();
  FX_DCHECK(data_member->is_bitfield());

  // Use the FoundMember's offset (not DataMember's) because FoundMember's takes into account base
  // classes and their offsets.
  // TODO(bug 41503) handle virtual inheritance.
  auto opt_byte_offset = found_member.GetDataMemberOffset();
  if (!opt_byte_offset) {
    return Err(
        "The debugger does not yet support bitfield access on virtually inherited base "
        "classes (bug 41503) or static members.");
  }

  // The actual size of the bitfield.
  uint32_t bit_size = data_member->bit_size();

  // The offset into the base struct, rounded to bytes.
  uint32_t byte_offset = *opt_byte_offset;

  // The bit offset in addition to byte_offset, should be in range [0, 7].
  uint32_t bit_offset = data_member->data_bit_offset();

  // DWARF 2 (byte_size, bit_offset, member_location) combination is used. Convert it to bit_offset.
  if (data_member->byte_size()) {
    bit_offset = data_member->byte_size() * 8 - data_member->bit_offset() - data_member->bit_size();
  }

  // Spill the bit_offset into byte_offset, so we could support arbitrary large bit_offset.
  byte_offset += bit_offset / 8;
  bit_offset = bit_offset % 8;

  // Round up bit_size to byte size. Note that it's possible for a bitfield of 10 bits to span
  // across 3 bytes, i.e., 1 bit + 8 bits + 1 bit.
  uint32_t byte_size = (bit_offset + bit_size + 7) / 8;

  // Check whether the bitfield can be fit in a uint128_t.
  if (byte_size > sizeof(uint128_t)) {
    // If the total coverage of this bitfield is more than out number size we can't do the
    // operations and need to rewrite this code to manually do shifts on bytes rather than using
    // numeric operations in C.
    return Err("The bitfield spans more than 128 bits which is unsupported.");
  }

  // Destination type. Here we need to save the original (possibly non-concrete type) for assigning
  // to the result type at the bottom.
  const Type* dest_type = data_member->type().Get()->As<Type>();
  if (!dest_type)
    return Err("Bitfield member has no type.");
  fxl::RefPtr<Type> concrete_dest_type = context->GetConcreteType(dest_type);

  uint128_t bits = 0;

  if (base.data().size() < byte_offset + byte_size ||
      !base.data().RangeIsValid(byte_offset, byte_size))
    return Err::OptimizedOut();

  // Copy bytes to out bitfield.
  memcpy(&bits, &base.data().bytes()[byte_offset], byte_size);

  // Shift.
  bits >>= bit_offset;

  // Mask.
  uint128_t valid_bit_mask = (static_cast<uint128_t>(1) << bit_size) - 1;
  bits &= valid_bit_mask;

  if (NeedsSignExtension(context, concrete_dest_type.get(), bits, bit_size)) {
    // Set non-masked bits to 1.
    bits |= ~valid_bit_mask;
  }

  ExprValueSource source = base.source().GetOffsetInto(byte_offset, bit_size, bit_offset);

  // Save the data back to the desired size (assume little-endian so truncation from the right is
  // correct).
  std::vector<uint8_t> new_data(concrete_dest_type->byte_size());
  memcpy(new_data.data(), &bits, new_data.size());
  return ExprValue(RefPtrTo(dest_type), std::move(new_data), source);
}

void WriteBitfieldToMemory(const fxl::RefPtr<EvalContext>& context, const ExprValueSource& dest,
                           std::vector<uint8_t> data, fit::callback<void(const Err&)> cb) {
  FX_DCHECK(dest.is_bitfield());

  // Expect bitfields to fit in our biggest int.
  if (data.size() > sizeof(uint128_t))
    return cb(Err("Writing bitfields for data > 128-bits is not supported."));

  uint128_t value = 0;
  memcpy(&value, data.data(), data.size());

  // Number of bytes affected by this bitfield.
  size_t byte_size = (dest.bit_size() + dest.bit_shift() + 7) / 8;
  if (!byte_size)
    return cb(Err("Can't write a bitfield with no data."));

  // To only write some bits we need to do a read and a write. Since these are asynchronous there
  // is a possibility of racing with the program, but there will a race if the program is running
  // even if we do the masking in the debug_agent. This implementation is simpler than passing the
  // mask to the agent, so do that.
  context->GetDataProvider()->GetMemoryAsync(
      dest.address(), byte_size,
      [context, dest, value, byte_size, cb = std::move(cb)](
          const Err& err, std::vector<uint8_t> original_data) mutable {
        if (err.has_error())
          return cb(err);
        if (original_data.size() != byte_size)  // Short read means invalid address.
          return cb(Err("Memory at address 0x%" PRIx64 " is invalid.", dest.address()));

        uint128_t original = 0;
        memcpy(&original, original_data.data(), original_data.size());

        uint128_t result = dest.SetBits(original, value);

        // Write out the new data.
        std::vector<uint8_t> new_data(byte_size);
        memcpy(new_data.data(), &result, byte_size);
        context->GetDataProvider()->WriteMemory(dest.address(), std::move(new_data), std::move(cb));
      });
}

}  // namespace zxdb

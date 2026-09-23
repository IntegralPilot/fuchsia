// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_KERNEL_LIB_ARCH_INCLUDE_LIB_ARCH_ARM64_MEMORY_H_
#define ZIRCON_KERNEL_LIB_ARCH_INCLUDE_LIB_ARCH_ARM64_MEMORY_H_

#include <lib/arch/internal/bits.h>
#include <lib/arch/sysreg.h>
#include <zircon/assert.h>
#include <zircon/compiler.h>

#include <cstdint>
#include <optional>
#include <variant>

#include <hwreg/bitfields.h>
#include <hwreg/internal.h>

namespace arch {

// The cache shareability attribute for memory regions, as defined by the
// TCR_ELx.SHn fields.
//
// [arm/v8]: D13.2.120 TCR_EL1, Translation Control Register (EL1)
// [arm/v8]: D13.2.121 TCR_EL2, Translation Control Register (EL2)
enum class ArmShareabilityAttribute {
  kNone = 0b00,
  // 0b01 is reserved.
  kOuter = 0b10,
  kInner = 0b11,
};

// The cacheability attribute of a normal memory regions, as defined by the
// TCR_ELx.IRGNn and TCR_ELx.ORGNn fields.
//
// [arm/v8]: D13.2.120 TCR_EL1, Translation Control Register (EL1)
// [arm/v8]: D13.2.121 TCR_EL2, Translation Control Register (EL2)
enum class ArmCacheabilityAttribute {
  kNonCacheable = 0b00,
  kWriteBackReadWriteAllocate = 0b01,
  kWriteThroughReadAllocate = 0b10,
  kWriteBackReadAllocate = 0b11,
};

// Types of ARM device memory.
//
// Numeric values are as expected by MAIR_ELx attribute encodings for device
// memory.
enum class ArmDeviceMemory {
  kNonGatheringNonReorderingNoEarlyAck = 0b00,  // nGnRnE
  kNonGatheringNonReorderingEarlyAck = 0b01,    // nGnRE
  kNonGatheringReorderingEarlyAck = 0b10,       // nGRE
  kGatheringReorderingEarlyAck = 0b11,          // GRE
};

// Represents a MAIR_ELx attribute value for normal memory.
//
// This representation flattens any "transience" distinctions (an
// underspecified, additional, implementation-defined attribute), and by
// default refers to the non-transient version. Ditto for certain
// feature-aware configurations.
struct ArmMairNormalAttribute {
  constexpr bool operator==(const ArmMairNormalAttribute& other) const {
    return inner == other.inner && outer == other.outer;
  }

  ArmCacheabilityAttribute inner = ArmCacheabilityAttribute::kNonCacheable;
  ArmCacheabilityAttribute outer = ArmCacheabilityAttribute::kNonCacheable;
};

// Represents a MAIR_ELx attribute value.
using ArmMairAttribute = std::variant<ArmMairNormalAttribute, ArmDeviceMemory>;

// Memory Attribute Indirection Register
//
// [arm/v8]: D13.2.95  MAIR_EL1, Memory Attribute Indirection Register, EL1
// [arm/v8]: D13.2.96  MAIR_EL2, Memory Attribute Indirection Register, EL2
struct ArmMemoryAttrIndirectionRegister
    : public SysRegDerivedBase<ArmMemoryAttrIndirectionRegister, uint64_t> {
  DEF_FIELD(63, 56, attr7);
  DEF_FIELD(55, 48, attr6);
  DEF_FIELD(47, 40, attr5);
  DEF_FIELD(39, 32, attr4);
  DEF_FIELD(31, 24, attr3);
  DEF_FIELD(23, 16, attr2);
  DEF_FIELD(15, 8, attr1);
  DEF_FIELD(7, 0, attr0);

  static constexpr unsigned int kNumAttributes = 8;

  // Get the memory attribute at the given index.
  //
  // TODO(https://fxbug.dev/42158108): Ideally hwreg would support this natively.
  constexpr uint8_t GetAttributeValue(unsigned int index) const {
    ZX_DEBUG_ASSERT(index < kNumAttributes);
    unsigned int low_bit = index * kAttributeBits;
    unsigned int high_bit = low_bit + kAttributeBits - 1;
    return static_cast<uint8_t>(internal::ExtractBits(high_bit, low_bit, reg_value()));
  }

  constexpr std::optional<ArmMairAttribute> GetAttribute(unsigned int index) const {
    return AttributeFromValue(GetAttributeValue(index));
  }

  // Set the memory attribute at the given index.
  //
  // TODO(https://fxbug.dev/42158108): Ideally hwreg would support this natively.
  constexpr ArmMemoryAttrIndirectionRegister& SetAttributeValue(unsigned int index, uint8_t value) {
    ZX_DEBUG_ASSERT(index < kNumAttributes);

    unsigned int low_bit = index * kAttributeBits;
    unsigned int high_bit = low_bit + kAttributeBits - 1;
    set_reg_value(
        internal::UpdateBits(high_bit, low_bit, reg_value(), static_cast<uint64_t>(value)));
    return *this;
  }

  constexpr ArmMemoryAttrIndirectionRegister& SetAttribute(unsigned int index,
                                                           ArmMairAttribute attr) {
    return SetAttributeValue(index, AttributeToValue(attr));
  }

  // Returns the index associated with a given, configured attribute, if
  // present.
  constexpr std::optional<unsigned int> GetIndex(uint8_t value) const {
    for (unsigned int i = 0; i < kNumAttributes; ++i) {
      if (value == GetAttributeValue(i)) {
        return i;
      }
    }
    return {};
  }

  constexpr std::optional<unsigned int> GetIndex(const ArmMairAttribute& attr) const {
    return GetIndex(AttributeToValue(attr));
  }

  // Converts structured attribute to the associated raw value.
  static constexpr uint8_t AttributeToValue(const ArmMairAttribute& attr) {
    uint8_t value = 0;
    hwreg::internal::Visit([&value](auto&& mair_attr) { value = AttributeToValue(mair_attr); },
                           attr);
    return value;
  }

  // Converts a raw attribute value to the structured version.
  //
  // Bizarrely, there are valid attribute encodings that correspond to
  // currently impossible hardware configurations (limited by what knobs are
  // available on TCR_ELx); in this case, std::nullopt is returned.
  //
  // [arm/v8]: D13.2.95  MAIR_EL1, Memory Attribute Indirection Register, EL1
  // [arm/v8]: D13.2.96  MAIR_EL2, Memory Attribute Indirection Register, EL2
  static constexpr std::optional<ArmMairAttribute> AttributeFromValue(uint8_t value) {
    using Cacheability = ArmCacheabilityAttribute;

    if ((value & 0b0000'1100) == value || (value & 0b0000'1101) == value) {
      return static_cast<ArmDeviceMemory>(value >> 2);
    }

    auto cacheability = [](uint8_t bits) -> std::optional<Cacheability> {
      if (bits == 0b0100) {
        return Cacheability::kNonCacheable;
      }
      bool write_back = (bits & 0b0100) != 0;
      bool read_allocate = (bits & 0b0010) != 0;
      bool write_allocate = (bits & 0b0001) != 0;
      if (write_back) {
        if (read_allocate && write_allocate) {
          return Cacheability::kWriteBackReadWriteAllocate;
        }
        if (read_allocate) {
          return Cacheability::kWriteBackReadAllocate;
        }
      } else if (read_allocate && !write_allocate) {
        return Cacheability::kWriteThroughReadAllocate;
      }
      return {};
    };

    uint8_t outer_attr = value >> 4;
    uint8_t inner_attr = value & 0b1111;
    if (outer_attr != 0 && inner_attr != 0) {
      std::optional inner = cacheability(inner_attr);
      std::optional outer = cacheability(outer_attr);
      if (!inner || !outer) {
        return {};
      }
      return ArmMairNormalAttribute{.inner = *inner, .outer = *outer};
    }

    if (value == 0b0100'0000) {
      return ArmMairNormalAttribute{
          .inner = Cacheability::kWriteThroughReadAllocate,
          .outer = Cacheability::kWriteThroughReadAllocate,
      };
    }
    if (value == 0b1010'0000) {
      return ArmMairNormalAttribute{
          .inner = Cacheability::kWriteBackReadWriteAllocate,
          .outer = Cacheability::kWriteBackReadWriteAllocate,
      };
    }
    return {};
  }

 private:
  static constexpr unsigned int kAttributeBits = 8;  // Width of each attribute (in bits).

  static constexpr uint8_t AttributeToValue(ArmDeviceMemory device) {
    return static_cast<uint8_t>(static_cast<unsigned int>(device) << 2);
  }

  // [arm/v8]: D13.2.95  MAIR_EL1, Memory Attribute Indirection Register, EL1
  // [arm/v8]: D13.2.96  MAIR_EL2, Memory Attribute Indirection Register, EL2
  static constexpr uint8_t AttributeToValue(const ArmMairNormalAttribute& normal) {
    auto domain_bits = [](ArmCacheabilityAttribute attr) {
      switch (attr) {
        case ArmCacheabilityAttribute::kNonCacheable:
          return 0b0100u;
        case ArmCacheabilityAttribute::kWriteBackReadWriteAllocate:
          return 0b1111u;
        case ArmCacheabilityAttribute::kWriteThroughReadAllocate:
          return 0b1010u;
        case ArmCacheabilityAttribute::kWriteBackReadAllocate:
          return 0b1110u;
      }
      __UNREACHABLE;
    };
    return static_cast<uint8_t>((domain_bits(normal.outer) << 4) | domain_bits(normal.inner));
  }
};

struct ArmMairEl1 : public arch::SysRegDerived<ArmMairEl1, ArmMemoryAttrIndirectionRegister> {};
ARCH_ARM64_SYSREG(ArmMairEl1, "mair_el1");

struct ArmMairEl2 : public arch::SysRegDerived<ArmMairEl2, ArmMemoryAttrIndirectionRegister> {};
ARCH_ARM64_SYSREG(ArmMairEl2, "mair_el2");

}  // namespace arch

#endif  // ZIRCON_KERNEL_LIB_ARCH_INCLUDE_LIB_ARCH_ARM64_MEMORY_H_

// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef HWREG_BITFIELDS_H_
#define HWREG_BITFIELDS_H_

#include <limits.h>
#include <stdint.h>
#include <zircon/assert.h>

#include <type_traits>

#include "internal.h"

// This file provides some helpers for accessing bitfields in registers.
//
// Example usage:
// ```
//
//   // Define bitfields for an "AuxControl" 32-bit register.
//   class AuxControl : public hwreg::RegisterBase<AuxControl, uint32_t> {
//   public:
//       // Returns an object representing the register's type and address.
//       static hwreg::RegisterAddr<AuxControl> Get() { return {0x64010}; }
//
//       // Bits [30:25] and [19:0] are automatically preserved across RMW cycles.
//
//       // Define a single-bit field.
//       DEF_BIT(31, enabled);
//       // Define a 5-bit field, from bits 20-24 (inclusive).
//       DEF_FIELD(24, 20, message_size);
//   };
//
//   void Example1(hwreg::RegisterIo* reg_io) {
//       // Read the register's value from MMIO.  "reg" is a snapshot of the
//       // register's value which also knows the register's address.
//       auto reg = AuxControl::Get().ReadFrom(reg_io);
//
//       // Read this register's "message_size" field.
//       uint32_t size = reg.message_size();
//
//       // Change this field's value.  This modifies the snapshot.
//       reg.set_message_size(1234);
//
//       // Write the modified register value to MMIO.
//       reg.WriteTo(reg_io);
//   }
//
//   // Fields may also be set in a fluent style
//   void Example2(hwreg::RegisterIo* reg_io) {
//       // Read the register's value from MMIO, updates the message size and
//       // enabled bit, then writes the value back to MMIO
//       AuxControl::Get().ReadFrom(reg_io).set_message_size(1234).set_enabled(1).WriteTo(reg_io);
//   }
//
//   // It is also possible to write a register without having to read it
//   // first:
//   void Example3(hwreg::RegisterIo* reg_io) {
//       // Start off with a value that is initialized to zero.
//       auto reg = AuxControl::Get().FromValue(0);
//       // Fill out fields.
//       reg.set_message_size(2345);
//       // Write the register value to MMIO.
//       reg.WriteTo(reg_io);
//   }
// ```
//
// Note that this produces efficient code with GCC and Clang, which are
// capable of optimizing away the intermediate objects.
//
// The arguments to DEF_FIELD() are organized to match up with Intel's
// documentation for their graphics hardware.  For example, if the docs
// specify a field as:
//   23:0  Data M value
// then that translates to:
//   DEF_FIELD(23, 0, data_m_value)
// To match up, we put the upper bit first and use an inclusive bit range.
//
// For fields whose value is relative to their position in the register you can
// use DEF_UNSHIFTED_FIELD(high, low, name). This ensures that when you
// read/write full values they will be applied to the field with the bits
// overlapping other fields masked off, without shifting anything.
// Enums: Enums can be used in register field definitions using the
// DEF_ENUM_FIELD macro as shown below
// ```
// enum LinkState {
//      Connected = 0,
//      Disconnected = 1,
//      Disabled = 2,
//      ...
// };
// ...
// DEF_ENUM_FIELD(LinkState, 8, 5, PLS);
// ```
//
// Register types may be templated, but in order to do so `SelfType` must be
// explicitly defined:
// ```
// template <unsigned int N>
// struct TemplatedReg : public hwreg::RegisterBase<TemplatedReg<N>, uint32_t> {
//     using SelfType = TemplatedReg<N>;
//
//     DEF_FIELD(N, 0, lower);
//     ...
// };
// ```
// The DEF_*_(FIELD|BIT) macros rely on SelfType being unambiguously defined
// within the scope at which they are used .

// The DEF_*_(BIT|FIELD) macros rely on SelfType being unambiguously defined
// within the scope at which they are used (ditto for the *conditional* flavors
// of the DEF_*_SUB(FIELD|BIT) (see next paragraph)). In a non-templated case,
// we inherit this for free from the RegisterBase base class; otherwise, it
// needs to be explicitly redeclared (as above).
//
// In contexts where the layout of the register depends on a static condition
// (e.g., relating to a template parameter), there are conditional versions of
// the DEF_FOO(...) macros, DEF_COND_FOO(..., COND), that compile away when the
// condition does not hold. So long as each layout variant is valid in its own
// right, there are no overlap constraints across different variants. Further,
// the same field name may be reused across layouts that are defined by the
// DEF_COND*_(FIELD|BIT) macros.
//

namespace hwreg {

// Tag that can be passed as the third template parameter for RegisterBase to enable
// the pretty-printing interfaces on a register.
struct EnablePrinter;

// An instance of RegisterBase represents a staging copy of a register,
// which can be written to the register itself.  It knows the register's
// address and stores a value for the register.
//
// Normal usage is to create classes that derive from RegisterBase and
// provide methods for accessing bitfields of the register.  RegisterBase
// does not provide a constructor because constructors are not inherited by
// derived classes by default, and we don't want the derived classes to
// have to declare constructors.
//
// Any bits not declared using DEF_FIELD/DEF_BIT/DEF_RSVDZ_FIELD/DEF_RSVDZ_BIT
// will be automatically preserved across RMW operations.
template <class DerivedType, class IntType, class PrinterState = void>
class RegisterBase {
  static_assert(internal::IsSupportedInt<IntType>::value, "unsupported register access width");
  static_assert(std::is_same_v<PrinterState, void> || std::is_same_v<PrinterState, EnablePrinter>,
                "unsupported printer state");

 public:
  using SelfType = DerivedType;
  using ValueType = IntType;
  using PrinterEnabled = std::is_same<PrinterState, EnablePrinter>;

  uint32_t reg_addr() const { return reg_addr_; }
  void set_reg_addr(uint32_t addr) { reg_addr_ = addr; }

  constexpr ValueType reg_value() const { return reg_value_; }
  ValueType* reg_value_ptr() { return &reg_value_; }
  const ValueType* reg_value_ptr() const { return &reg_value_; }
  constexpr SelfType& set_reg_value(IntType value) {
    reg_value_ = value;
    return *static_cast<SelfType*>(this);
  }

  template <typename T>
  SelfType& ReadFrom(T* reg_io) {
    internal::Visit([this](auto&& io) { reg_value_ = io.template Read<ValueType>(reg_addr_); },
                    *reg_io);
    return *static_cast<SelfType*>(this);
  }

  template <typename T>
  SelfType& WriteTo(T* reg_io) {
    internal::Visit(
        [this](auto&& io) {
          io.Write(static_cast<IntType>(reg_value_ & ~params_.rsvdz_mask), reg_addr_);
        },
        *reg_io);
    return *static_cast<SelfType*>(this);
  }

  // Invokes print_fn(const char* buf) once for each field, including each
  // RsvdZ field, and one extra time if there are any undefined bits set.
  // The callback argument must not be accessed after the callback
  // returns.  The callback will be called once for each field with a
  // null-terminated string describing the name and contents of the field.
  //
  // Printed fields will look like: "field_name[26:8]: 0x00123 (291)"
  // The undefined bits message will look like: "unknown set bits: 0x00301000"
  //
  // WARNING: This will substantially increase code size and stack usage at the
  // call site. https://fxbug.dev/42147424 tracks investigation to improve this.
  //
  // Example use:
  // reg.Print([](const char* arg) { printf("%s\n", arg); });
  template <typename F>
  void Print(F print_fn) {
    static_assert(PrinterEnabled::value, "Pass hwreg::EnablePrinter to RegisterBase to enable");
    internal::PrintRegister(print_fn, params_.printer.fields.data(), params_.printer.num_fields,
                            reg_value_, params_.fields_mask, sizeof(ValueType));
  }

  // Equivalent to Print([](const char* arg) { printf("%s\n", arg); });
  void Print() {
    static_assert(PrinterEnabled::value, "Pass hwreg::EnablePrinter to RegisterBase to enable");
    internal::PrintRegisterPrintf(params_.printer.fields.data(), params_.printer.num_fields,
                                  reg_value_, params_.fields_mask, sizeof(ValueType));
  }

  template <typename FieldCallback>
  constexpr void ForEachField(FieldCallback&& callback) const {
    static_assert(PrinterEnabled::value, "Pass hwreg::EnablePrinter to RegisterBase to enable");
    static_assert(std::is_invocable_v<FieldCallback, const char*, ValueType, uint32_t, uint32_t>);
    for (unsigned i = 0; i < params_.printer.num_fields; ++i) {
      ValueType mask = internal::ComputeMask<ValueType>(params_.printer.fields[i].bit_high_incl() -
                                                        params_.printer.fields[i].bit_low() + 1)
                       << params_.printer.fields[i].bit_low();
      ValueType value = (reg_value_ & mask) >> params_.printer.fields[i].bit_low();
      callback((mask & rsvdz_mask()) == mask ? nullptr : params_.printer.fields[i].name(), value,
               params_.printer.fields[i].bit_high_incl(), params_.printer.fields[i].bit_low());
    }
  }

  constexpr IntType fields_mask() const { return params_.fields_mask; }
  constexpr IntType rsvdz_mask() const { return params_.rsvdz_mask; }

 protected:
  internal::FieldParameters<PrinterEnabled::value, ValueType>& params() { return params_; }

 private:
  internal::FieldParameters<PrinterEnabled::value, ValueType> params_;
  ValueType reg_value_ = 0;
  uint32_t reg_addr_ = 0;
};

// An instance of RegisterAddr represents a typed register address: It
// knows the address of the register (within the MMIO address space) and
// the type of its contents, RegType.  RegType represents the register's
// bitfields.  RegType should be a subclass of RegisterBase.
template <class RegType>
class RegisterAddr {
 public:
  RegisterAddr(uint32_t reg_addr) : reg_addr_(reg_addr) {}

  static_assert(std::is_base_of_v<RegisterBase<RegType, typename RegType::ValueType>, RegType> ||
                    std::is_base_of_v<
                        RegisterBase<RegType, typename RegType::ValueType, EnablePrinter>, RegType>,
                "Parameter of RegisterAddr<> should derive from RegisterBase");

  // Instantiate a RegisterBase using the value of the register read from
  // MMIO.
  template <typename T>
  RegType ReadFrom(T* reg_io) const {
    RegType reg;
    reg.set_reg_addr(reg_addr_);
    reg.ReadFrom(reg_io);
    return reg;
  }

  // Instantiate a RegisterBase using the given value for the register.
  RegType FromValue(typename RegType::ValueType value) const {
    RegType reg;
    reg.set_reg_addr(reg_addr_);
    reg.set_reg_value(value);
    return reg;
  }

  uint32_t addr() const { return reg_addr_; }

 private:
  const uint32_t reg_addr_;
};

template <class IntType>
class BitfieldRef {
 public:
  constexpr BitfieldRef(IntType* value_ptr, uint32_t bit_high_incl, uint32_t bit_low)
      : value_ptr_(value_ptr),
        shift_(bit_low),
        mask_(internal::ComputeMask<IntType>(bit_high_incl - bit_low + 1)) {}

  constexpr BitfieldRef(IntType* value_ptr, uint32_t bit_high_incl, uint32_t bit_low,
                        bool unshifted)
      : value_ptr_(value_ptr),
        shift_(0),
        mask_(static_cast<IntType>(internal::ComputeMask<IntType>(bit_high_incl - bit_low + 1)
                                   << bit_low)) {}

  constexpr IntType get() const { return static_cast<IntType>((*value_ptr_ >> shift_) & mask_); }

  constexpr void set(IntType field_val) {
    static_assert(!std::is_const_v<IntType>);
    ZX_DEBUG_ASSERT((field_val & ~mask_) == 0);
    auto masked = static_cast<IntType>(*value_ptr_ & ~(mask_ << shift_));
    *value_ptr_ = static_cast<IntType>(masked | (field_val << shift_));
  }

 private:
  IntType* const value_ptr_;
  const uint32_t shift_;
  const IntType mask_;
};

//
// We take care to use `typename SelfType::ValueType` over `ValueType` below,
// since the defined contract above is that `SelfType` alone must be
// (unambiguously) defined within the applied scope. Similarly, we explicitly
// access `params()` and `reg_value_ptr()` via `this` so as to sidestep any
// possible ambiguity (e.g., in the case of a templated register type).
//

// Declares multi-bit fields in a derived class of RegisterBase<D, T>.  This
// produces functions "T NAME() const" and "D& set_NAME(T)".  Both bit indices
// are inclusive.
#define DEF_FIELD(BIT_HIGH, BIT_LOW, NAME) DEF_COND_FIELD(BIT_HIGH, BIT_LOW, NAME, true)

#define DEF_COND_FIELD(BIT_HIGH, BIT_LOW, NAME, COND)                                              \
  static_assert((BIT_HIGH) >= (BIT_LOW), "Upper bit goes before lower bit");                       \
  static_assert((BIT_HIGH) < sizeof(typename SelfType::ValueType) * CHAR_BIT,                      \
                "Upper bit is out of range");                                                      \
  /* NOLINTBEGIN(misc-non-private-member-variables-in-classes) */                                  \
  __NO_UNIQUE_ADDRESS struct {                                                                     \
    struct NAME##Marker {};                                                                        \
    __NO_UNIQUE_ADDRESS hwreg::internal::Field<SelfType, NAME##Marker, (COND)> field;              \
  } HWREG_INTERNAL_MEMBER_NAME(NAME) = {{&this->params(), #NAME, (BIT_HIGH), (BIT_LOW)}};          \
  /* NOLINTEND(misc-non-private-member-variables-in-classes) */                                    \
  template <bool Cond = (COND),                                                                    \
            std::enable_if_t<hwreg::internal::kUnexpandedPred<Cond, __LINE__>, bool> = true>       \
  typename SelfType::ValueType NAME() const {                                                      \
    return hwreg::BitfieldRef<const typename SelfType::ValueType>(this->reg_value_ptr(),           \
                                                                  (BIT_HIGH), (BIT_LOW))           \
        .get();                                                                                    \
  }                                                                                                \
  template <bool Cond = (COND),                                                                    \
            std::enable_if_t<hwreg::internal::kUnexpandedPred<Cond, __LINE__>, bool> = true>       \
  SelfType& set_##NAME(typename SelfType::ValueType val) {                                         \
    hwreg::BitfieldRef<typename SelfType::ValueType>(this->reg_value_ptr(), (BIT_HIGH), (BIT_LOW)) \
        .set(val);                                                                                 \
    return *this;                                                                                  \
  }                                                                                                \
  static_assert(true)  // eat a ;

#define DEF_UNSHIFTED_FIELD(BIT_HIGH, BIT_LOW, NAME) \
  DEF_COND_UNSHIFTED_FIELD(BIT_HIGH, BIT_LOW, NAME, true)

#define DEF_COND_UNSHIFTED_FIELD(BIT_HIGH, BIT_LOW, NAME, COND)                                    \
  static_assert((BIT_HIGH) >= (BIT_LOW), "Upper bit goes before lower bit");                       \
  static_assert((BIT_HIGH) < sizeof(typename SelfType::ValueType) * CHAR_BIT,                      \
                "Upper bit is out of range");                                                      \
  /* NOLINTBEGIN(misc-non-private-member-variables-in-classes) */                                  \
  __NO_UNIQUE_ADDRESS struct {                                                                     \
    struct NAME##Marker {};                                                                        \
    __NO_UNIQUE_ADDRESS hwreg::internal::Field<SelfType, NAME##Marker, (COND)> field;              \
  } HWREG_INTERNAL_MEMBER_NAME(NAME) = {{&this->params(), #NAME, (BIT_HIGH), (BIT_LOW)}};          \
  /* NOLINTEND(misc-non-private-member-variables-in-classes) */                                    \
  template <bool Cond = (COND),                                                                    \
            std::enable_if_t<hwreg::internal::kUnexpandedPred<Cond, __LINE__>, bool> = true>       \
  typename SelfType::ValueType NAME() const {                                                      \
    return hwreg::BitfieldRef<const typename SelfType::ValueType>(this->reg_value_ptr(),           \
                                                                  (BIT_HIGH), (BIT_LOW), true)     \
        .get();                                                                                    \
  }                                                                                                \
  template <bool Cond = (COND),                                                                    \
            std::enable_if_t<hwreg::internal::kUnexpandedPred<Cond, __LINE__>, bool> = true>       \
  SelfType& set_##NAME(typename SelfType::ValueType val) {                                         \
    hwreg::BitfieldRef<typename SelfType::ValueType>(this->reg_value_ptr(), (BIT_HIGH), (BIT_LOW), \
                                                     true)                                         \
        .set(val);                                                                                 \
    return *this;                                                                                  \
  }                                                                                                \
  static_assert(true)  // eat a ;

#define DEF_ENUM_FIELD(ENUM_TYPE, BIT_HIGH, BIT_LOW, NAME) \
  DEF_COND_ENUM_FIELD(ENUM_TYPE, BIT_HIGH, BIT_LOW, NAME, true)

#define DEF_COND_ENUM_FIELD(ENUM_TYPE, BIT_HIGH, BIT_LOW, NAME, COND)                              \
  static_assert((BIT_HIGH) >= (BIT_LOW), "Upper bit goes before lower bit");                       \
  static_assert((BIT_HIGH) < sizeof(typename SelfType::ValueType) * CHAR_BIT,                      \
                "Upper bit is out of range");                                                      \
  static_assert(std::is_enum_v<ENUM_TYPE>, "Field type is not an enum");                           \
  /* NOLINTBEGIN(misc-non-private-member-variables-in-classes) */                                  \
  __NO_UNIQUE_ADDRESS struct {                                                                     \
    struct NAME##Marker {};                                                                        \
    __NO_UNIQUE_ADDRESS hwreg::internal::Field<SelfType, NAME##Marker, (COND)> field;              \
  } HWREG_INTERNAL_MEMBER_NAME(NAME) = {{&this->params(), #NAME, (BIT_HIGH), (BIT_LOW)}};          \
  /* NOLINTEND(misc-non-private-member-variables-in-classes) */                                    \
  template <bool Cond = (COND),                                                                    \
            std::enable_if_t<hwreg::internal::kUnexpandedPred<Cond, __LINE__>, bool> = true>       \
  ENUM_TYPE NAME() const {                                                                         \
    return static_cast<ENUM_TYPE>(hwreg::BitfieldRef<const typename SelfType::ValueType>(          \
                                      this->reg_value_ptr(), (BIT_HIGH), (BIT_LOW))                \
                                      .get());                                                     \
  }                                                                                                \
  template <bool Cond = (COND),                                                                    \
            std::enable_if_t<hwreg::internal::kUnexpandedPred<Cond, __LINE__>, bool> = true>       \
  SelfType& set_##NAME(ENUM_TYPE val) {                                                            \
    hwreg::BitfieldRef<typename SelfType::ValueType>(this->reg_value_ptr(), (BIT_HIGH), (BIT_LOW)) \
        .set(static_cast<typename SelfType::ValueType>(val));                                      \
    return *this;                                                                                  \
  }                                                                                                \
  static_assert(true)  // eat a ;

// Declares single-bit fields in a derived class of RegisterBase<D, T>.  This
// produces functions "T NAME() const" and "void set_NAME(T)".
#define DEF_BIT(BIT, NAME) DEF_COND_BIT(BIT, NAME, true)
#define DEF_COND_BIT(BIT, NAME, COND) DEF_COND_FIELD(BIT, BIT, NAME, COND)

// Declares multi-bit reserved-zero fields in a derived class of RegisterBase<D, T>.
// This will ensure that on RegisterBase<T>::WriteTo(), reserved-zero bits are
// automatically zeroed.  Both bit indices are inclusive.
#define DEF_RSVDZ_FIELD(BIT_HIGH, BIT_LOW) DEF_COND_RSVDZ_FIELD(BIT_HIGH, BIT_LOW, true)

#define DEF_COND_RSVDZ_FIELD(BIT_HIGH, BIT_LOW, COND)                                     \
  static_assert((BIT_HIGH) >= (BIT_LOW), "Upper bit goes before lower bit");              \
  static_assert((BIT_HIGH) < sizeof(typename SelfType::ValueType) * CHAR_BIT,             \
                "Upper bit is out of range");                                             \
  /* NOLINTBEGIN(misc-non-private-member-variables-in-classes) */                         \
  __NO_UNIQUE_ADDRESS struct {                                                            \
    struct RsvdZMarker {};                                                                \
    __NO_UNIQUE_ADDRESS hwreg::internal::RsvdZField<SelfType, RsvdZMarker, (COND)> field; \
  } HWREG_INTERNAL_MEMBER_NAME(RsvdZ) = {                                                 \
      {&this->params(), (BIT_HIGH),                                                       \
       (BIT_LOW)}} /* NOLINTEND(misc-non-private-member-variables-in-classes) */

// Declares single-bit reserved-zero fields in a derived class of RegisterBase<D, T>.
// This will ensure that on RegisterBase<T>::WriteTo(), reserved-zero bits are
// automatically zeroed.
#define DEF_RSVDZ_BIT(BIT) DEF_COND_RSVDZ_BIT(BIT, true)
#define DEF_COND_RSVDZ_BIT(BIT, COND) DEF_COND_RSVDZ_FIELD(BIT, BIT, COND)

// Declares "decltype(FIELD) NAME() const" and "void set_NAME(decltype(FIELD))" that
// reads/modifies the declared bitrange.  Both bit indices are inclusive.
#define DEF_SUBFIELD(FIELD, BIT_HIGH, BIT_LOW, NAME)                                          \
  HWREG_INTERNAL_CHECK_SUBFIELD(FIELD, BIT_HIGH, BIT_LOW)                                     \
  constexpr typename std::remove_reference_t<decltype(FIELD)> NAME() const {                  \
    return hwreg::BitfieldRef<const typename std::remove_reference_t<decltype(FIELD)>>(       \
               &FIELD, (BIT_HIGH), (BIT_LOW))                                                 \
        .get();                                                                               \
  }                                                                                           \
  constexpr auto& set_##NAME(typename std::remove_reference_t<decltype(FIELD)> val) {         \
    hwreg::BitfieldRef<typename std::remove_reference_t<decltype(FIELD)>>(&FIELD, (BIT_HIGH), \
                                                                          (BIT_LOW))          \
        .set(val);                                                                            \
    return *this;                                                                             \
  }                                                                                           \
  static_assert(true)  // eat a ;

#define DEF_COND_SUBFIELD(FIELD, BIT_HIGH, BIT_LOW, NAME, COND)                                   \
  HWREG_INTERNAL_CHECK_SUBFIELD(FIELD, BIT_HIGH, BIT_LOW)                                         \
  template <bool Cond = (COND),                                                                   \
            std::enable_if_t<hwreg::internal::kUnexpandedPred<Cond, __LINE__>, bool> = true>      \
  constexpr std::remove_reference_t<decltype(FIELD)> NAME() const {                               \
    return hwreg::BitfieldRef<const std::remove_reference_t<decltype(FIELD)>>(&FIELD, (BIT_HIGH), \
                                                                              (BIT_LOW))          \
        .get();                                                                                   \
  }                                                                                               \
  template <bool Cond = (COND),                                                                   \
            std::enable_if_t<hwreg::internal::kUnexpandedPred<Cond, __LINE__>, bool> = true>      \
  constexpr SelfType& set_##NAME(std::remove_reference_t<decltype(FIELD)> val) {                  \
    hwreg::BitfieldRef<std::remove_reference_t<decltype(FIELD)>>(&FIELD, (BIT_HIGH), (BIT_LOW))   \
        .set(val);                                                                                \
    return *this;                                                                                 \
  }                                                                                               \
  static_assert(true)  // eat a ;

// Declares "decltype(FIELD) NAME() const" and "void set_NAME(decltype(FIELD))" that
// reads/modifies the declared bit.
#define DEF_SUBBIT(FIELD, BIT, NAME) DEF_SUBFIELD(FIELD, BIT, BIT, NAME)
#define DEF_COND_SUBBIT(FIELD, BIT, NAME, COND) DEF_COND_SUBFIELD(FIELD, BIT, BIT, NAME, COND)

// Declares "decltype(FIELD) NAME() const" and "void set_NAME(decltype(FIELD))" that
// reads/modifies the declared bitrange, without shifting the input / output value.
// Both bit indices are inclusive.
//
// For example, given the definition:
//
//   struct PackedAddress {
//     DEF_UNSHIFTED_SUBFIELD(data, 32, 1, address);
//     DEF_SUBBIT(data, 0, low_bit);
//   };
//
// will allow 32-bit addresses to be directly read/written to `address`. If
// `DEF_SUBFIELD` read/written values would need to be shifted left/right by
// 1 bit each time.
#define DEF_UNSHIFTED_SUBFIELD(FIELD, BIT_HIGH, BIT_LOW, NAME)                          \
  HWREG_INTERNAL_CHECK_SUBFIELD(FIELD, BIT_HIGH, BIT_LOW)                               \
  constexpr typename std::remove_reference_t<decltype(FIELD)> NAME() const {            \
    return hwreg::BitfieldRef<const typename std::remove_reference_t<decltype(FIELD)>>( \
               &FIELD, (BIT_HIGH), (BIT_LOW), /*unshifted=*/true)                       \
        .get();                                                                         \
  }                                                                                     \
  constexpr auto& set_##NAME(typename std::remove_reference_t<decltype(FIELD)> val) {   \
    hwreg::BitfieldRef<typename std::remove_reference_t<decltype(FIELD)>>(              \
        &FIELD, (BIT_HIGH), (BIT_LOW), /*unshifted=*/true)                              \
        .set(val);                                                                      \
    return *this;                                                                       \
  }                                                                                     \
  static_assert(true)  // eat a ;

#define DEF_COND_UNSHIFTED_SUBFIELD(FIELD, BIT_HIGH, BIT_LOW, NAME, COND)                       \
  HWREG_INTERNAL_CHECK_SUBFIELD(FIELD, BIT_HIGH, BIT_LOW)                                       \
  template <bool Cond = (COND),                                                                 \
            std::enable_if_t<hwreg::internal::kUnexpandedPred<Cond, __LINE__>, bool> = true>    \
  constexpr std::remove_reference_t<decltype(FIELD)> NAME() const {                             \
    return hwreg::BitfieldRef<const std::remove_reference_t<decltype(FIELD)>>(                  \
               &FIELD, (BIT_HIGH), (BIT_LOW), /*unshifted=*/true)                               \
        .get();                                                                                 \
  }                                                                                             \
  template <bool Cond = (COND),                                                                 \
            std::enable_if_t<hwreg::internal::kUnexpandedPred<Cond, __LINE__>, bool> = true>    \
  constexpr SelfType& set_##NAME(std::remove_reference_t<decltype(FIELD)> val) {                \
    hwreg::BitfieldRef<std::remove_reference_t<decltype(FIELD)>>(&FIELD, (BIT_HIGH), (BIT_LOW), \
                                                                 /*unshifted=*/true)            \
        .set(val);                                                                              \
    return *this;                                                                               \
  }                                                                                             \
  static_assert(true)  // eat a ;

// Declares "TYPE NAME() const" and "void set_NAME(TYPE)" that
// reads/modifies the declared bitrange.  Both bit indices are inclusive.
#define DEF_ENUM_SUBFIELD(FIELD, ENUM_TYPE, BIT_HIGH, BIT_LOW, NAME)                          \
  HWREG_INTERNAL_CHECK_SUBFIELD(FIELD, BIT_HIGH, BIT_LOW)                                     \
  static_assert(std::is_enum_v<ENUM_TYPE>, "ENUM_TYPE is not an enum");                       \
  constexpr ENUM_TYPE NAME() const {                                                          \
    return static_cast<ENUM_TYPE>(                                                            \
        hwreg::BitfieldRef<const typename std::remove_reference_t<decltype(FIELD)>>(          \
            &FIELD, (BIT_HIGH), (BIT_LOW))                                                    \
            .get());                                                                          \
  }                                                                                           \
  constexpr auto& set_##NAME(ENUM_TYPE val) {                                                 \
    hwreg::BitfieldRef<typename std::remove_reference_t<decltype(FIELD)>>(&FIELD, (BIT_HIGH), \
                                                                          (BIT_LOW))          \
        .set(static_cast<typename std::remove_reference_t<decltype(FIELD)>>(val));            \
    return *this;                                                                             \
  }                                                                                           \
  static_assert(true)  // eat a ;

#define DEF_COND_ENUM_SUBFIELD(FIELD, ENUM_TYPE, BIT_HIGH, BIT_LOW, NAME, COND)                 \
  HWREG_INTERNAL_CHECK_SUBFIELD(FIELD, BIT_HIGH, BIT_LOW)                                       \
  static_assert(std::is_enum_v<ENUM_TYPE>, "ENUM_TYPE is not an enum");                         \
  template <bool Cond = (COND),                                                                 \
            std::enable_if_t<hwreg::internal::kUnexpandedPred<Cond, __LINE__>, bool> = true>    \
  constexpr ENUM_TYPE NAME() const {                                                            \
    return static_cast<ENUM_TYPE>(                                                              \
        hwreg::BitfieldRef<const std::remove_reference_t<decltype(FIELD)>>(&FIELD, (BIT_HIGH),  \
                                                                           (BIT_LOW))           \
            .get());                                                                            \
  }                                                                                             \
  template <bool Cond = (COND),                                                                 \
            std::enable_if_t<hwreg::internal::kUnexpandedPred<Cond, __LINE__>, bool> = true>    \
  constexpr SelfType& set_##NAME(ENUM_TYPE val) {                                               \
    hwreg::BitfieldRef<std::remove_reference_t<decltype(FIELD)>>(&FIELD, (BIT_HIGH), (BIT_LOW)) \
        .set(static_cast<std::remove_reference_t<decltype(FIELD)>>(val));                       \
    return *this;                                                                               \
  }                                                                                             \
  static_assert(true)  // eat a ;

}  // namespace hwreg

#endif  // HWREG_BITFIELDS_H_

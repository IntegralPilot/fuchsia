// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Given a Writer implementing the Writer interface in writer-internal.h, provide an api
// over the writer to allow serializing fxt to the Writer.
//
// Based heavily on libTrace in zircon/system/ulib/trace to allow compatibility,
// but modified to enable passing in an arbitrary buffering system.

#ifndef SRC_PERFORMANCE_LIB_FXT_INCLUDE_LIB_FXT_ARGUMENT_H_
#define SRC_PERFORMANCE_LIB_FXT_INCLUDE_LIB_FXT_ARGUMENT_H_

#include <zircon/types.h>

#include <span>

#include "fields.h"
#include "record_types.h"
#include "string_ref.h"

namespace fxt {

static constexpr WordSize kMaxArgumentSize = WordSize(0x8000);

// Represents an FXT Argument, a typed Key Value pair.
//
// See also: https://fuchsia.dev/fuchsia-src/reference/tracing/trace-format#arguments
template <ArgumentType argument_type, RefType name_type, RefType val_type = RefType::kId>
class Argument;

template <RefType name_type>
class Argument<ArgumentType::kNull, name_type> {
 public:
  template <typename T, EnableIfConvertibleToStringRef<T, name_type> = true>
  constexpr Argument(T&& name) : Argument{StringRef<name_type>{std::forward<T>(name)}} {}

  constexpr explicit Argument(StringRef<name_type> name) : name_(name) {}

  constexpr Argument(const Argument&) = default;
  constexpr Argument& operator=(const Argument&) = default;

  constexpr WordSize PayloadSize() const {
    return WordSize::FromBytes(sizeof(ArgumentHeader)) + name_.PayloadSize();
  }
  constexpr uint64_t Header() const {
    return ArgumentFields::Type::Make(ToUnderlyingType(ArgumentType::kNull)) |
           ArgumentFields::ArgumentSize::Make(PayloadSize().SizeInWords()) |
           ArgumentFields::NameRef::Make(name_.HeaderEntry());
  }

  template <typename Reservation>
  constexpr void Write(Reservation& res) const {
    res.WriteWord(Header());
    name_.Write(res);
  }

  constexpr StringRef<name_type> name() const { return name_; }
  constexpr std::nullptr_t value() const { return nullptr; }

  constexpr bool operator==(const Argument& other) const { return name() == other.name(); }
  constexpr bool operator!=(const Argument& other) const { return !(*this == other); }

 private:
  StringRef<name_type> name_;
};

template <RefType name_type>
Argument(StringRef<name_type>) -> Argument<ArgumentType::kNull, name_type>;

template <typename T, EnableIfConvertibleToStringRef<T, RefType::kId> = true>
Argument(T&& name) -> Argument<ArgumentType::kNull, RefType::kId>;

template <typename T, EnableIfConvertibleToStringRef<T, RefType::kInline> = true>
Argument(T&& name) -> Argument<ArgumentType::kNull, RefType::kInline>;

template <RefType name_type>
class Argument<ArgumentType::kBool, name_type> {
 public:
  template <typename T, EnableIfConvertibleToStringRef<T, name_type> = true>
  constexpr Argument(T&& name, bool value)
      : Argument{StringRef<name_type>{std::forward<T>(name)}, value} {}

  constexpr Argument(StringRef<name_type> name, bool value) : name_(name), value_(value) {}

  constexpr Argument(const Argument&) = default;
  constexpr Argument& operator=(const Argument&) = default;

  constexpr WordSize PayloadSize() const {
    return WordSize::FromBytes(sizeof(ArgumentHeader)) + name_.PayloadSize();
  }
  constexpr uint64_t Header() const {
    return BoolArgumentFields::Value::Make(value_) |
           ArgumentFields::Type::Make(ToUnderlyingType(ArgumentType::kBool)) |
           ArgumentFields::ArgumentSize::Make(PayloadSize().SizeInWords()) |
           ArgumentFields::NameRef::Make(name_.HeaderEntry());
  }

  template <typename Reservation>
  constexpr void Write(Reservation& res) const {
    res.WriteWord(Header());
    name_.Write(res);
  }

  constexpr StringRef<name_type> name() const { return name_; }
  constexpr bool value() const { return value_; }

  constexpr bool operator==(const Argument& other) const {
    return name() == other.name() && value() == other.value();
  }
  constexpr bool operator!=(const Argument& other) const { return !(*this == other); }

 private:
  StringRef<name_type> name_;
  bool value_;
};

template <RefType name_type>
Argument(StringRef<name_type>, bool) -> Argument<ArgumentType::kBool, name_type>;

template <typename T, EnableIfConvertibleToStringRef<T, RefType::kId> = true>
Argument(T&& name, bool) -> Argument<ArgumentType::kBool, RefType::kId>;

template <typename T, EnableIfConvertibleToStringRef<T, RefType::kInline> = true>
Argument(T&& name, bool) -> Argument<ArgumentType::kBool, RefType::kInline>;

template <RefType name_type>
class Argument<ArgumentType::kInt32, name_type> {
 public:
  template <typename T, EnableIfConvertibleToStringRef<T, name_type> = true>
  constexpr Argument(T&& name, int32_t value)
      : Argument{StringRef<name_type>{std::forward<T>(name)}, value} {}

  constexpr Argument(StringRef<name_type> name, int32_t value) : name_(name), value_(value) {}

  constexpr Argument(const Argument&) = default;
  constexpr Argument& operator=(const Argument&) = default;

  constexpr WordSize PayloadSize() const {
    return WordSize::FromBytes(sizeof(ArgumentHeader)) + name_.PayloadSize();
  }
  constexpr uint64_t Header() const {
    return Int32ArgumentFields::Value::Make(value_) |
           ArgumentFields::Type::Make(ToUnderlyingType(ArgumentType::kInt32)) |
           ArgumentFields::ArgumentSize::Make(PayloadSize().SizeInWords()) |
           ArgumentFields::NameRef::Make(name_.HeaderEntry());
  }

  template <typename Reservation>
  constexpr void Write(Reservation& res) const {
    res.WriteWord(Header());
    name_.Write(res);
  }

  constexpr StringRef<name_type> name() const { return name_; }
  constexpr int32_t value() const { return value_; }

  constexpr bool operator==(const Argument& other) const {
    return name() == other.name() && value() == other.value();
  }
  constexpr bool operator!=(const Argument& other) const { return !(*this == other); }

 private:
  StringRef<name_type> name_;
  int32_t value_;
};

template <RefType name_type>
Argument(StringRef<name_type>, int32_t) -> Argument<ArgumentType::kInt32, name_type>;

template <typename T, EnableIfConvertibleToStringRef<T, RefType::kId> = true>
Argument(T&& name, int32_t) -> Argument<ArgumentType::kInt32, RefType::kId>;

template <typename T, EnableIfConvertibleToStringRef<T, RefType::kInline> = true>
Argument(T&& name, int32_t) -> Argument<ArgumentType::kInt32, RefType::kInline>;

template <RefType name_type>
class Argument<ArgumentType::kUint32, name_type> {
 public:
  template <typename T, EnableIfConvertibleToStringRef<T, name_type> = true>
  constexpr Argument(T&& name, uint32_t value)
      : Argument{StringRef<name_type>{std::forward<T>(name)}, value} {}

  constexpr Argument(StringRef<name_type> name, uint32_t value) : name_(name), value_(value) {}

  constexpr Argument(const Argument&) = default;
  constexpr Argument& operator=(const Argument&) = default;

  constexpr WordSize PayloadSize() const {
    return WordSize::FromBytes(sizeof(ArgumentHeader)) + name_.PayloadSize();
  }
  constexpr uint64_t Header() const {
    return Uint32ArgumentFields::Value::Make(value_) |
           ArgumentFields::Type::Make(ToUnderlyingType(ArgumentType::kUint32)) |
           ArgumentFields::ArgumentSize::Make(PayloadSize().SizeInWords()) |
           ArgumentFields::NameRef::Make(name_.HeaderEntry());
  }

  template <typename Reservation>
  constexpr void Write(Reservation& res) const {
    res.WriteWord(Header());
    name_.Write(res);
  }

  constexpr StringRef<name_type> name() const { return name_; }
  constexpr uint32_t value() const { return value_; }

  constexpr bool operator==(const Argument& other) const {
    return name() == other.name() && value() == other.value();
  }
  constexpr bool operator!=(const Argument& other) const { return !(*this == other); }

 private:
  StringRef<name_type> name_;
  uint32_t value_;
};

template <RefType name_type>
Argument(StringRef<name_type>, uint32_t) -> Argument<ArgumentType::kUint32, name_type>;

template <typename T, EnableIfConvertibleToStringRef<T, RefType::kId> = true>
Argument(T&& name, uint32_t) -> Argument<ArgumentType::kUint32, RefType::kId>;

template <typename T, EnableIfConvertibleToStringRef<T, RefType::kInline> = true>
Argument(T&& name, uint32_t) -> Argument<ArgumentType::kUint32, RefType::kInline>;

template <RefType name_type>
class Argument<ArgumentType::kInt64, name_type> {
 public:
  template <typename T, EnableIfConvertibleToStringRef<T, name_type> = true>
  constexpr Argument(T&& name, int64_t value)
      : Argument{StringRef<name_type>{std::forward<T>(name)}, value} {}

  constexpr Argument(StringRef<name_type> name, int64_t value) : name_(name), value_(value) {}

  constexpr Argument(const Argument&) = default;
  constexpr Argument& operator=(const Argument&) = default;

  constexpr WordSize PayloadSize() const {
    return WordSize::FromBytes(sizeof(ArgumentHeader)) + name_.PayloadSize() + WordSize(1);
  }
  constexpr uint64_t Header() const {
    return ArgumentFields::Type::Make(ToUnderlyingType(ArgumentType::kInt64)) |
           ArgumentFields::ArgumentSize::Make(PayloadSize().SizeInWords()) |
           ArgumentFields::NameRef::Make(name_.HeaderEntry());
  }

  template <typename Reservation>
  constexpr void Write(Reservation& res) const {
    res.WriteWord(Header());
    name_.Write(res);
    res.WriteWord(value_);
  }

  constexpr StringRef<name_type> name() const { return name_; }
  constexpr int64_t value() const { return value_; }

  constexpr bool operator==(const Argument& other) const {
    return name() == other.name() && value() == other.value();
  }
  constexpr bool operator!=(const Argument& other) const { return !(*this == other); }

 private:
  StringRef<name_type> name_;
  int64_t value_;
};

template <RefType name_type>
Argument(StringRef<name_type>, int64_t) -> Argument<ArgumentType::kInt64, name_type>;

template <typename T, EnableIfConvertibleToStringRef<T, RefType::kId> = true>
Argument(T&& name, int64_t) -> Argument<ArgumentType::kInt64, RefType::kId>;

template <typename T, EnableIfConvertibleToStringRef<T, RefType::kInline> = true>
Argument(T&& name, int64_t) -> Argument<ArgumentType::kInt64, RefType::kInline>;

template <RefType name_type>
class Argument<ArgumentType::kUint64, name_type> {
 public:
  template <typename T, EnableIfConvertibleToStringRef<T, name_type> = true>
  constexpr Argument(T&& name, uint64_t value)
      : Argument{StringRef<name_type>{std::forward<T>(name)}, value} {}

  constexpr Argument(StringRef<name_type> name, uint64_t value) : name_(name), value_(value) {}

  constexpr Argument(const Argument&) = default;
  constexpr Argument& operator=(const Argument&) = default;

  constexpr WordSize PayloadSize() const {
    return WordSize::FromBytes(sizeof(ArgumentHeader)) + name_.PayloadSize() + WordSize(1);
  }
  constexpr uint64_t Header() const {
    return ArgumentFields::Type::Make(ToUnderlyingType(ArgumentType::kUint64)) |
           ArgumentFields::ArgumentSize::Make(PayloadSize().SizeInWords()) |
           ArgumentFields::NameRef::Make(name_.HeaderEntry());
  }

  template <typename Reservation>
  constexpr void Write(Reservation& res) const {
    res.WriteWord(Header());
    name_.Write(res);
    res.WriteWord(value_);
  }

  constexpr StringRef<name_type> name() const { return name_; }
  constexpr uint64_t value() const { return value_; }

  constexpr bool operator==(const Argument& other) const {
    return name() == other.name() && value() == other.value();
  }
  constexpr bool operator!=(const Argument& other) const { return !(*this == other); }

 private:
  StringRef<name_type> name_;
  uint64_t value_;
};

template <RefType name_type>
Argument(StringRef<name_type>, uint64_t) -> Argument<ArgumentType::kUint64, name_type>;

template <typename T, EnableIfConvertibleToStringRef<T, RefType::kId> = true>
Argument(T&& name, uint64_t) -> Argument<ArgumentType::kUint64, RefType::kId>;

template <typename T, EnableIfConvertibleToStringRef<T, RefType::kInline> = true>
Argument(T&& name, uint64_t) -> Argument<ArgumentType::kUint64, RefType::kInline>;

template <RefType name_type>
class Argument<ArgumentType::kDouble, name_type> {
 public:
  template <typename T, EnableIfConvertibleToStringRef<T, name_type> = true>
  constexpr Argument(T&& name, double value)
      : Argument{StringRef<name_type>{std::forward<T>(name)}, value} {}

  constexpr Argument(StringRef<name_type> name, double value) : name_(name), value_(value) {}

  constexpr Argument(const Argument&) = default;
  constexpr Argument& operator=(const Argument&) = default;

  constexpr WordSize PayloadSize() const {
    return WordSize::FromBytes(sizeof(ArgumentHeader)) + name_.PayloadSize() + WordSize(1);
  }
  constexpr uint64_t Header() const {
    return ArgumentFields::Type::Make(ToUnderlyingType(ArgumentType::kDouble)) |
           ArgumentFields::ArgumentSize::Make(PayloadSize().SizeInWords()) |
           ArgumentFields::NameRef::Make(name_.HeaderEntry());
  }

  template <typename Reservation>
  constexpr void Write(Reservation& res) const {
    res.WriteWord(Header());
    name_.Write(res);
    res.WriteBytes(&value_, 8);
  }

  constexpr StringRef<name_type> name() const { return name_; }
  constexpr double value() const { return value_; }

  constexpr bool operator==(const Argument& other) const {
    return name() == other.name() && value() == other.value();
  }
  constexpr bool operator!=(const Argument& other) const { return !(*this == other); }

 private:
  StringRef<name_type> name_;
  double value_;
};

template <RefType name_type>
Argument(StringRef<name_type>, double) -> Argument<ArgumentType::kDouble, name_type>;

template <typename T, EnableIfConvertibleToStringRef<T, RefType::kId> = true>
Argument(T&& name, double) -> Argument<ArgumentType::kDouble, RefType::kId>;

template <typename T, EnableIfConvertibleToStringRef<T, RefType::kInline> = true>
Argument(T&& name, double) -> Argument<ArgumentType::kDouble, RefType::kInline>;

template <RefType name_type>
class Argument<ArgumentType::kPointer, name_type> {
 public:
  template <typename T, EnableIfConvertibleToStringRef<T, name_type> = true>
  constexpr Argument(T&& name, Pointer value)
      : Argument{StringRef<name_type>{std::forward<T>(name)}, value} {}

  constexpr Argument(StringRef<name_type> name, Pointer value) : name_(name), value_(value) {}

  constexpr Argument(const Argument&) = default;
  constexpr Argument& operator=(const Argument&) = default;

  constexpr WordSize PayloadSize() const {
    return WordSize::FromBytes(sizeof(ArgumentHeader)) + name_.PayloadSize() + WordSize(1);
  }
  constexpr uint64_t Header() const {
    return ArgumentFields::Type::Make(ToUnderlyingType(ArgumentType::kPointer)) |
           ArgumentFields::ArgumentSize::Make(PayloadSize().SizeInWords()) |
           ArgumentFields::NameRef::Make(name_.HeaderEntry());
  }

  template <typename Reservation>
  constexpr void Write(Reservation& res) const {
    res.WriteWord(Header());
    name_.Write(res);
    res.WriteWord(value_.ptr);
  }

  constexpr StringRef<name_type> name() const { return name_; }
  constexpr Pointer value() const { return value_; }

  constexpr bool operator==(const Argument& other) const {
    return name() == other.name() && value() == other.value();
  }
  constexpr bool operator!=(const Argument& other) const { return !(*this == other); }

 private:
  StringRef<name_type> name_;
  Pointer value_;
};

template <RefType name_type>
Argument(StringRef<name_type>, Pointer) -> Argument<ArgumentType::kPointer, name_type>;

template <typename T, EnableIfConvertibleToStringRef<T, RefType::kId> = true>
Argument(T&& name, Pointer) -> Argument<ArgumentType::kPointer, RefType::kId>;

template <typename T, EnableIfConvertibleToStringRef<T, RefType::kInline> = true>
Argument(T&& name, Pointer) -> Argument<ArgumentType::kPointer, RefType::kInline>;

template <RefType name_type>
class Argument<ArgumentType::kKoid, name_type> {
 public:
  template <typename T, EnableIfConvertibleToStringRef<T, name_type> = true>
  constexpr Argument(T&& name, Koid value) : name_{std::forward<T>(name)}, value_{value} {}

  constexpr Argument(StringRef<name_type> name, Koid value) : name_(name), value_(value) {}

  constexpr Argument(const Argument&) = default;
  constexpr Argument& operator=(const Argument&) = default;

  constexpr WordSize PayloadSize() const {
    return WordSize::FromBytes(sizeof(ArgumentHeader)) + name_.PayloadSize() + WordSize(1);
  }
  constexpr uint64_t Header() const {
    return ArgumentFields::Type::Make(ToUnderlyingType(ArgumentType::kKoid)) |
           ArgumentFields::ArgumentSize::Make(PayloadSize().SizeInWords()) |
           ArgumentFields::NameRef::Make(name_.HeaderEntry());
  }

  template <typename Reservation>
  constexpr void Write(Reservation& res) const {
    res.WriteWord(Header());
    name_.Write(res);
    res.WriteWord(value_.koid);
  }

  constexpr StringRef<name_type> name() const { return name_; }
  constexpr Koid value() const { return value_; }

  constexpr bool operator==(const Argument& other) const {
    return name() == other.name() && value() == other.value();
  }
  constexpr bool operator!=(const Argument& other) const { return !(*this == other); }

 private:
  StringRef<name_type> name_;
  Koid value_;
};

template <RefType name_type>
Argument(StringRef<name_type>, Koid) -> Argument<ArgumentType::kKoid, name_type>;

template <typename T, EnableIfConvertibleToStringRef<T, RefType::kId> = true>
Argument(T&& name, Koid) -> Argument<ArgumentType::kKoid, RefType::kId>;

template <typename T, EnableIfConvertibleToStringRef<T, RefType::kInline> = true>
Argument(T&& name, Koid) -> Argument<ArgumentType::kKoid, RefType::kInline>;

template <RefType name_type, RefType val_type>
class Argument<ArgumentType::kString, name_type, val_type> {
 public:
  template <typename T, typename U, EnableIfConvertibleToStringRef<T, name_type> = true,
            EnableIfConvertibleToStringRef<U, val_type> = true>
  constexpr Argument(T&& name, U&& value)
      : name_{std::forward<T>(name)}, value_{std::forward<U>(value)} {}

  constexpr Argument(StringRef<name_type> name, StringRef<val_type> value)
      : name_(name), value_(value) {}

  constexpr Argument(const Argument&) = default;
  constexpr Argument& operator=(const Argument&) = default;

  constexpr WordSize PayloadSize() const {
    return WordSize::FromBytes(sizeof(ArgumentHeader)) + name_.PayloadSize() + value_.PayloadSize();
  }

  constexpr uint64_t Header() const {
    return StringArgumentFields::Index::Make(value_.HeaderEntry()) |
           ArgumentFields::Type::Make(ToUnderlyingType(ArgumentType::kString)) |
           ArgumentFields::ArgumentSize::Make(PayloadSize().SizeInWords()) |
           ArgumentFields::NameRef::Make(name_.HeaderEntry());
  }

  template <typename Reservation>
  constexpr void Write(Reservation& res) const {
    res.WriteWord(Header());
    name_.Write(res);
    value_.Write(res);
  }

  constexpr StringRef<name_type> name() const { return name_; }
  constexpr StringRef<val_type> value() const { return value_; }

  constexpr bool operator==(const Argument& other) const {
    return name() == other.name() && value() == other.value();
  }
  constexpr bool operator!=(const Argument& other) const { return !(*this == other); }

 private:
  StringRef<name_type> name_;
  StringRef<val_type> value_;
};

Argument(StringRef<RefType::kInline>, StringRef<RefType::kId>)
    -> Argument<ArgumentType::kString, RefType::kInline, RefType::kId>;

Argument(StringRef<RefType::kId>, StringRef<RefType::kId>)
    -> Argument<ArgumentType::kString, RefType::kId, RefType::kId>;

Argument(StringRef<RefType::kInline>, StringRef<RefType::kInline>)
    -> Argument<ArgumentType::kString, RefType::kInline, RefType::kInline>;

Argument(StringRef<RefType::kId>, StringRef<RefType::kInline>)
    -> Argument<ArgumentType::kString, RefType::kId, RefType::kInline>;

template <typename T, typename U, EnableIfConvertibleToStringRef<T, RefType::kInline> = true,
          EnableIfConvertibleToStringRef<U, RefType::kId> = true>
Argument(T&& name, U&& value) -> Argument<ArgumentType::kString, RefType::kInline, RefType::kId>;

template <typename T, typename U, EnableIfConvertibleToStringRef<T, RefType::kId> = true,
          EnableIfConvertibleToStringRef<U, RefType::kId> = true>
Argument(T&& name, U&& value) -> Argument<ArgumentType::kString, RefType::kId, RefType::kId>;

template <typename T, typename U, EnableIfConvertibleToStringRef<T, RefType::kInline> = true,
          EnableIfConvertibleToStringRef<U, RefType::kInline> = true>
Argument(T&& name, U&& value)
    -> Argument<ArgumentType::kString, RefType::kInline, RefType::kInline>;

template <typename T, typename U, EnableIfConvertibleToStringRef<T, RefType::kId> = true,
          EnableIfConvertibleToStringRef<U, RefType::kInline> = true>
Argument(T&& name, U&& value) -> Argument<ArgumentType::kString, RefType::kId, RefType::kInline>;

template <RefType name_type>
class Argument<ArgumentType::kBlob, name_type> {
 public:
  template <typename T, EnableIfConvertibleToStringRef<T, name_type> = true>
  constexpr Argument(T&& name, std::span<const uint8_t> value)
      : Argument{StringRef<name_type>{std::forward<T>(name)}, value} {}

  constexpr Argument(StringRef<name_type> name, std::span<const uint8_t> value)
      : name_(name), value_(value) {}

  constexpr Argument(const Argument&) = default;
  constexpr Argument& operator=(const Argument&) = default;

  constexpr WordSize PayloadSize() const { return HeaderAndNameSize() + ValueSize(); }
  constexpr uint64_t Header() const {
    return BlobArgumentFields::Type::Make(ToUnderlyingType(ArgumentType::kBlob)) |
           BlobArgumentFields::ArgumentSize::Make(PayloadSize().SizeInWords()) |
           BlobArgumentFields::NameRef::Make(name_.HeaderEntry()) |
           BlobArgumentFields::ByteSize::Make(value_.size());
  }

  template <typename Reservation>
  constexpr void Write(Reservation& res) const {
    res.WriteWord(Header());
    name_.Write(res);
    // Reservation will pad to 8 byte alignment with zeroes.
    res.WriteBytes(value_.data(), std::min(value_.size(), ValueSize().SizeInBytes()));
  }

  constexpr StringRef<name_type> name() const { return name_; }
  constexpr const std::span<const uint8_t>& value() const { return value_; }

  constexpr bool operator==(const Argument& other) const {
    return name() == other.name() && value() == other.value();
  }
  constexpr bool operator!=(const Argument& other) const { return !(*this == other); }

 private:
  constexpr WordSize HeaderAndNameSize() const {
    return WordSize::FromBytes(sizeof(ArgumentHeader)) + name_.PayloadSize();
  }
  // Size of the value, possibly truncated to fit.
  constexpr WordSize ValueSize() const {
    return WordSize::FromBytes(value_.size_bytes()).Clamp(kMaxArgumentSize - HeaderAndNameSize());
  }

  StringRef<name_type> name_;
  std::span<const uint8_t> value_;
};

template <RefType name_type>
Argument(StringRef<name_type>, std::span<const uint8_t>)
    -> Argument<ArgumentType::kBlob, name_type>;

template <typename T, EnableIfConvertibleToStringRef<T, RefType::kId> = true>
Argument(T&& name, std::span<const uint8_t>) -> Argument<ArgumentType::kBlob, RefType::kId>;

template <typename T, EnableIfConvertibleToStringRef<T, RefType::kInline> = true>
Argument(T&& name, std::span<const uint8_t>) -> Argument<ArgumentType::kBlob, RefType::kInline>;

// Builds an instance of Argument from the given parameters. Each overload uses explicit
// instantiations of StringRef to avoid limitations in type deduction from convertible types. Since
// no type deduction occurs, implicit parameter conversions are considered.
constexpr Argument<ArgumentType::kNull, RefType::kId> MakeArgument(StringRef<RefType::kId> name) {
  return Argument{name};
}
constexpr Argument<ArgumentType::kNull, RefType::kInline> MakeArgument(
    StringRef<RefType::kInline> name) {
  return Argument{name};
}
constexpr Argument<ArgumentType::kBool, RefType::kId> MakeArgument(StringRef<RefType::kId> name,
                                                                   bool value) {
  return {name, value};
}
constexpr Argument<ArgumentType::kBool, RefType::kInline> MakeArgument(
    StringRef<RefType::kInline> name, bool value) {
  return {name, value};
}
constexpr Argument<ArgumentType::kInt32, RefType::kId> MakeArgument(StringRef<RefType::kId> name,
                                                                    int32_t value) {
  return {name, value};
}
constexpr Argument<ArgumentType::kInt32, RefType::kInline> MakeArgument(
    StringRef<RefType::kInline> name, int32_t value) {
  return {name, value};
}
constexpr Argument<ArgumentType::kUint32, RefType::kId> MakeArgument(StringRef<RefType::kId> name,
                                                                     uint32_t value) {
  return {name, value};
}
constexpr Argument<ArgumentType::kUint32, RefType::kInline> MakeArgument(
    StringRef<RefType::kInline> name, uint32_t value) {
  return {name, value};
}
constexpr Argument<ArgumentType::kInt64, RefType::kId> MakeArgument(StringRef<RefType::kId> name,
                                                                    int64_t value) {
  return {name, value};
}
constexpr Argument<ArgumentType::kInt64, RefType::kInline> MakeArgument(
    StringRef<RefType::kInline> name, int64_t value) {
  return {name, value};
}
constexpr Argument<ArgumentType::kUint64, RefType::kId> MakeArgument(StringRef<RefType::kId> name,
                                                                     uint64_t value) {
  return {name, value};
}
constexpr Argument<ArgumentType::kUint64, RefType::kInline> MakeArgument(
    StringRef<RefType::kInline> name, uint64_t value) {
  return {name, value};
}
constexpr Argument<ArgumentType::kDouble, RefType::kId> MakeArgument(StringRef<RefType::kId> name,
                                                                     double value) {
  return {name, value};
}
constexpr Argument<ArgumentType::kDouble, RefType::kInline> MakeArgument(
    StringRef<RefType::kInline> name, double value) {
  return {name, value};
}
constexpr Argument<ArgumentType::kPointer, RefType::kId> MakeArgument(StringRef<RefType::kId> name,
                                                                      Pointer value) {
  return {name, value};
}
constexpr Argument<ArgumentType::kPointer, RefType::kInline> MakeArgument(
    StringRef<RefType::kInline> name, Pointer value) {
  return {name, value};
}
constexpr Argument<ArgumentType::kKoid, RefType::kId> MakeArgument(StringRef<RefType::kId> name,
                                                                   Koid value) {
  return {name, value};
}
constexpr Argument<ArgumentType::kKoid, RefType::kInline> MakeArgument(
    StringRef<RefType::kInline> name, Koid value) {
  return {name, value};
}
constexpr Argument<ArgumentType::kString, RefType::kId, RefType::kId> MakeArgument(
    StringRef<RefType::kId> name, StringRef<RefType::kId> value) {
  return {name, value};
}
constexpr Argument<ArgumentType::kString, RefType::kId, RefType::kInline> MakeArgument(
    StringRef<RefType::kId> name, StringRef<RefType::kInline> value) {
  return {name, value};
}
constexpr Argument<ArgumentType::kString, RefType::kInline, RefType::kInline> MakeArgument(
    StringRef<RefType::kInline> name, StringRef<RefType::kInline> value) {
  return {name, value};
}
constexpr Argument<ArgumentType::kString, RefType::kInline, RefType::kId> MakeArgument(
    StringRef<RefType::kInline> name, StringRef<RefType::kId> value) {
  return {name, value};
}
constexpr Argument<ArgumentType::kString, RefType::kId, RefType::kInline> MakeArgument(
    StringRef<RefType::kId> name, const char* value) {
  return {name, StringRef{value}};
}
constexpr Argument<ArgumentType::kString, RefType::kInline, RefType::kInline> MakeArgument(
    StringRef<RefType::kInline> name, const char* value) {
  return {name, StringRef{value}};
}
constexpr Argument<ArgumentType::kString, RefType::kId, RefType::kInline> MakeArgument(
    StringRef<RefType::kId> name, const char* value, size_t size) {
  return {name, StringRef{value, size}};
}
constexpr Argument<ArgumentType::kString, RefType::kInline, RefType::kInline> MakeArgument(
    StringRef<RefType::kInline> name, const char* value, size_t size) {
  return {name, StringRef{value, size}};
}
constexpr Argument<ArgumentType::kBlob, RefType::kInline> MakeArgument(
    StringRef<RefType::kInline> name, std::span<const uint8_t> value) {
  return {name, value};
}
constexpr Argument<ArgumentType::kBlob, RefType::kId> MakeArgument(StringRef<RefType::kId> name,
                                                                   std::span<const uint8_t> value) {
  return {name, value};
}

}  // namespace fxt

#endif  // SRC_PERFORMANCE_LIB_FXT_INCLUDE_LIB_FXT_ARGUMENT_H_

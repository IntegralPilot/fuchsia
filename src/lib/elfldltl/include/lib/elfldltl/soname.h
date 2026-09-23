// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_SONAME_H_
#define SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_SONAME_H_

#include <cassert>
#include <cstdint>
#include <string_view>

#include "abi-ptr.h"
#include "gnu-hash.h"
#include "layout.h"

namespace elfldltl {

// This provides an optimized type for holding a DT_SONAME / DT_NEEDED string.
// It always hashes the string to make equality comparisons faster.
template <ElfApi Elf = Elf<>, AbiPtrTraitsApi<const char, Elf> AbiTraits = LocalAbiTraits>
class Soname {
 public:
  static constexpr bool kLocal = AbiPtrLocalTraitsApi<AbiTraits, const char, Elf>;

  constexpr Soname() = default;

  constexpr Soname(const Soname&) = default;

  constexpr explicit Soname(std::string_view name)
    requires kLocal
      : name_(name.data()), size_(static_cast<uint32_t>(name.size())), hash_(GnuHashString(name)) {
    assert(size_ == name.size());
  }

  constexpr Soname& operator=(const Soname&) noexcept = default;

  constexpr Soname& operator=(std::string_view name) noexcept
    requires kLocal
  {
    *this = Soname{name};
    return *this;
  }

  constexpr std::string_view str() const
    requires kLocal
  {
    return {name_.get(), size_};
  }

  // This can only be used if the std::string_view used in construction is
  // known to point to a NUL-terminated string, such as a string literal or a
  // DT_STRTAB entry.
  constexpr const char* c_str() const
    requires kLocal
  {
    assert(name_.get()[size_] == '\0');
    return name_.get();
  }

  // This is slightly different from str().copy() because it also includes the
  // '\0' terminator in the count of chars to be copied.  Hence it can return
  // up to size() + 1, not only up to size() like std::string_view::copy.
  constexpr size_t copy(char* dest, size_t count, size_t pos = 0) const
    requires kLocal
  {
    assert(name_.get()[size_] == '\0');
    size_t n = str().copy(dest, count, pos);
    if (n < count) {
      dest[n++] = '\0';
    }
    return n;
  }

  // This returns the size of a buffer sufficient for copy() not to truncate.
  constexpr size_t copy_size() const { return size_ + 1; }

  constexpr bool empty() const { return size_ == 0; }

  constexpr uint32_t size() const { return size_; }

  constexpr uint32_t hash() const { return hash_; }

  constexpr bool operator==(const Soname& other) const
    requires kLocal
  {
    return other.hash_ == hash_ && other.str() == str();
  }

  constexpr auto operator<=>(const Soname& other) const
    requires kLocal
  {
    return str() <=> other.str();
  }

  // This returns a convenient unary predicate for using things such as
  // std::ranges::find_if or std::ranges::any_of across a range of things that
  // support operator==(const Soname&).
  constexpr auto equal_to() const {
    return [self = *this](const auto& other) { return other == self; };
  }

 private:
  // This stores a pointer and 32-bit length directly rather than just using
  // std::string_view so that the whole object is still only two 64-bit words.
  // Crucially, both x86-64 and AArch64 ABIs pass and return trivial two-word
  // objects in registers but anything larger in memory, so this keeps passing
  // Soname as cheap as passing std::string_view.  This limits lengths to 4GiB,
  // which is far more than the practical limit.
  AbiPtr<const char, Elf, AbiTraits> name_;
  Elf::Word size_ = 0;
  Elf::Word hash_ = 0;

 public:
  // <lib/ld/remote-abi-transcriber.h> introspection API.  These aliases must
  // be public, but can't be defined lexically before the private: section that
  // declares the members; so this special public: section is at the end.

  using AbiLocal = Soname<Elf, LocalAbiTraits>;

  template <template <class...> class Template>
  using AbiBases = Template<>;

  template <template <auto...> class Template>
  using AbiMembers = Template<&Soname::name_, &Soname::size_, &Soname::hash_>;
};

inline namespace literals {

consteval elfldltl::Soname<> operator""_soname(const char* str, size_t len) {
  return elfldltl::Soname<>{std::string_view{str, len}};
}

}  // namespace literals
}  // namespace elfldltl

// This is the API contract for standard C++ hash-based containers.
template <class Elf, class AbiTraits>
struct std::hash<elfldltl::Soname<Elf, AbiTraits>> {
  constexpr uint32_t operator()(const elfldltl::Soname<Elf, AbiTraits>& soname) const {
    return soname.hash();
  }
};

#endif  // SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_SONAME_H_

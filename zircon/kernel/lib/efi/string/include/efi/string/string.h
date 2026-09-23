// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_KERNEL_LIB_EFI_STRING_INCLUDE_EFI_STRING_STRING_H_
#define ZIRCON_KERNEL_LIB_EFI_STRING_INCLUDE_EFI_STRING_STRING_H_

#include <string_view>
#include <tuple>

#include <fbl/vector.h>

namespace efi {

// UEFI string class.
//
// UEFI uses UCS-2 (a subset of UTF-16) as its string representation, but many libraries and
// applications use UTF-8. This class helps easily convert between the two formats to bridge
// these gaps.
//
// Additionally, std::string is not available in our EFI environment so this class helps smooth
// over some of the pain points of memory management using other storage classes.
//
// This class stores both the UTF-8 and UCS-2 representation of the string on the heap, so
// should be avoided unless you really do need to convert between both formats. There's also
// currently no support for holding one format as a view, so using this for literals will be
// inefficient as it will allocate heap memory for a duplication of the string literal.
//
// Note: this implementation currently uses true UTF-16 rather than UCS-2, which may rarely result
// in certain strings being rejected or incorrectly parsed by UEFI.
class String {
 public:
  // Users should check IsValid() after construction to ensure the conversion was successful. On
  // failure both representations will be erased (i.e. it will not have one valid format and the
  // other invalid).
  //
  // Constructors taking in string_view expect unterminated input, as is generally standard when
  // working with string_views; a null terminator will always be appended to the internal storage.
  //
  // Constructors taking in fbl::Vector<> expect terminated input and do *not* append terminators
  // internally, since in this case it's expected that the entire null-terminated string resides
  // in the allocated vector. In the spirit of fbl::Vector's care with memory allocation, we do not
  // provide a constructor to implicitly copy a vector; if needed, the caller must copy the vector
  // themselves and move in the copy.
  String() : String("") {}
  explicit String(std::string_view contents);
  explicit String(std::u16string_view contents);
  explicit String(fbl::Vector<char>&& contents);
  explicit String(fbl::Vector<char16_t>&& contents);

  // Copyable and movable.
  //
  // We do allow implicit copies of the full String object, which is a deep copy and will
  // allocate duplicates of the underlying strings.
  String(const String& other);
  String& operator=(const String& other);
  String(String&& other) = default;
  String& operator=(String&& other) = default;

  // Returns true if the string is valid in both UTF-8 and UCS-2.
  bool IsValid() const { return !utf8_.is_empty() && !ucs2_.is_empty(); }

  // Implicit conversion to string_view.
  //
  // If IsValid() is true, the returned string_view will not include a trailing terminator, but
  // the underlying storage will, making it safe to printf/strcmp/etc.
  //
  // If IsValid() is false, the returned string_view will point to null and be 0-length.
  operator std::string_view() const {
    return IsValid() ? std::string_view(utf8_.data(), utf8_.size() - 1) : std::string_view();
  }
  operator std::u16string_view() const {
    return IsValid() ? std::u16string_view(ucs2_.data(), ucs2_.size() - 1) : std::u16string_view();
  }

  const char* c_str() const;

  // Moves ovnership of UTF8 and UCS2 vectors to the caller.
  // Object is reset to empty state.
  // Use example:
  // ```
  // auto [name_utf8, name_ucs2] = variable_id.name.TakeData();
  // ```
  std::tuple<fbl::Vector<char>, fbl::Vector<char16_t>> TakeData();

  // Converts between UTF-8 and UCS-2.
  //
  // Standalone conversion functions for cases where we don't need to track both formats
  // persistently, but just need a one-time allocation and conversion.
  //
  // As with construction, the string_view input is not expected to be null-terminated, and a
  // terminator will be automatically appended in the returned storage.
  //
  // On conversion failure, the returned vector will be empty.
  static fbl::Vector<char> ToUtf8(std::u16string_view contents);
  static fbl::Vector<char16_t> ToUtf16(std::string_view contents);

 private:
  fbl::Vector<char> utf8_;
  fbl::Vector<char16_t> ucs2_;
};

// Equality comparison for C-strings, compiler won't implicitly use the string_view conversion.
inline bool operator==(const efi::String& efi_string, const char* c_string) {
  return std::string_view(efi_string).compare(c_string) == 0;
}
inline bool operator==(const efi::String& efi_string, const char16_t* c_string) {
  return std::u16string_view(efi_string).compare(c_string) == 0;
}
inline bool operator!=(const efi::String& efi_string, const char* c_string) {
  return !(efi_string == c_string);
}
inline bool operator!=(const efi::String& efi_string, const char16_t* c_string) {
  return !(efi_string == c_string);
}

// Equality comparison for two efi::Strings.
inline bool operator==(const efi::String& a, const efi::String& b) {
  // We only need to check one of the formats, the other will always be consistent.
  return std::string_view(a) == std::string_view(b);
}
inline bool operator!=(const efi::String& a, const efi::String& b) { return !(a == b); }

}  // namespace efi

#endif  // ZIRCON_KERNEL_LIB_EFI_STRING_INCLUDE_EFI_STRING_STRING_H_

// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <algorithm>

#include <efi/string/string.h>
#include <src/lib/utf_conversion/utf_conversion.h>

namespace efi {

namespace {

// Creates a duplicate fbl::Vector().
template <typename T>
fbl::Vector<T> Copy(const fbl::Vector<T>& source) {
  fbl::Vector<T> copy;
  copy.resize(source.size());
  std::copy(source.begin(), source.end(), copy.begin());
  return copy;
}

zx_status_t ConvertHelper(const char* source, size_t source_length, char16_t* dest,
                          size_t* dest_length) {
  return utf8_to_utf16(reinterpret_cast<const uint8_t*>(source), source_length,
                       reinterpret_cast<uint16_t*>(dest), dest_length);
}

zx_status_t ConvertHelper(const char16_t* source, size_t source_length, char* dest,
                          size_t* dest_length) {
  return utf16_to_utf8(reinterpret_cast<const uint16_t*>(source), source_length,
                       reinterpret_cast<uint8_t*>(dest), dest_length);
}

enum class Term { kOmit, kAdd };

// Converts between UTF-8 and UTF-16 while allocating an appropriately-sized vector.
//
// If |term| is set to kAdd, this allocates an additional terminator character when resizing the
// destination vector so the caller doesn't need to reallocate again.
template <typename From, typename To>
bool Convert(const From* source, size_t source_length, fbl::Vector<To>& dest,
             Term term = Term::kOmit) {
  // Dry-run convert to get required length for dest buffer.
  size_t dest_length = 0;
  zx_status_t res = ConvertHelper(source, source_length, nullptr, &dest_length);
  if (res != ZX_OK) {
    return false;
  }

  // Resize the dest buffer and convert again.
  if (term == Term::kOmit) {
    dest.resize(dest_length);
  } else {
    dest.resize(dest_length + 1);
    dest[dest_length] = 0;
  }
  res = ConvertHelper(source, source_length, dest.begin(), &dest_length);
  if (res != ZX_OK) {
    return false;
  }

  return true;
}

// Wraps Convert(), but on failure, resets both vectors to empty.
template <typename From, typename To>
void ConvertOrErase(fbl::Vector<From>& source, fbl::Vector<To>& dest) {
  if (!Convert(source.data(), source.size(), dest)) {
    source.reset();
    dest.reset();
  }
}

}  // namespace

String::String(std::string_view contents) {
  // Copy locally first so we can append the null terminator.
  utf8_.resize(contents.size() + 1);
  std::copy(contents.begin(), contents.end(), utf8_.begin());
  utf8_[contents.size()] = '\0';

  ConvertOrErase(utf8_, ucs2_);
}

String::String(std::u16string_view contents) {
  ucs2_.resize(contents.size() + 1);
  std::copy(contents.begin(), contents.end(), ucs2_.begin());
  ucs2_[contents.size()] = u'\0';

  ConvertOrErase(ucs2_, utf8_);
}

String::String(fbl::Vector<char>&& contents) : utf8_(std::move(contents)) {
  ConvertOrErase(utf8_, ucs2_);
}

String::String(fbl::Vector<char16_t>&& contents) : ucs2_(std::move(contents)) {
  ConvertOrErase(ucs2_, utf8_);
}

String::String(const String& other) : utf8_(Copy(other.utf8_)), ucs2_(Copy(other.ucs2_)) {}

String& String::operator=(const String& other) {
  utf8_ = Copy(other.utf8_);
  ucs2_ = Copy(other.ucs2_);
  return *this;
}

fbl::Vector<char> String::ToUtf8(std::u16string_view contents) {
  fbl::Vector<char> result;
  Convert(contents.data(), contents.size(), result, Term::kAdd);
  return result;
}

fbl::Vector<char16_t> String::ToUtf16(std::string_view contents) {
  fbl::Vector<char16_t> result;
  Convert(contents.data(), contents.size(), result, Term::kAdd);
  return result;
}

std::tuple<fbl::Vector<char>, fbl::Vector<char16_t>> String::TakeData() {
  auto ret = std::make_tuple(std::move(utf8_), std::move(ucs2_));
  return ret;
}

const char* String::c_str() const { return utf8_.data(); }

}  // namespace efi

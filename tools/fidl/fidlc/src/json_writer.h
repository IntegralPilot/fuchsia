// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef TOOLS_FIDL_FIDLC_SRC_JSON_WRITER_H_
#define TOOLS_FIDL_FIDLC_SRC_JSON_WRITER_H_

#include <lib/fit/function.h>
#include <zircon/assert.h>

#include <ostream>
#include <set>
#include <string_view>
#include <vector>

#include "utils.h"

namespace fidlc {

// Methods or functions named "Emit..." are the actual protocol to
// the JSON output.

// Other public methods take various value types and generate JSON
// output via the "Emit" routines.

// |JsonWriter| requires the derived type as a template parameter so it can
// match methods declared with parameter overrides in the derived class.
template <typename DerivedT>
class JsonWriter {
 public:
  explicit JsonWriter(std::ostream& os, int indent_level = 0)
      : os_(os), indent_level_(indent_level) {}

  ~JsonWriter() = default;

  template <typename Iterator>
  void GenerateArray(Iterator begin, Iterator end) {
    EmitArrayBegin();

    if (begin != end) {
      Indent();
      EmitNewlineWithIndent();
    }

    for (Iterator it = begin; it != end; ++it) {
      if (it != begin)
        EmitArraySeparator();
      self.Generate(*it);
    }

    if (begin != end) {
      Outdent();
      EmitNewlineWithIndent();
    }

    EmitArrayEnd();
  }

  template <typename Collection>
  void GenerateArray(const Collection& collection) {
    self.GenerateArray(collection.begin(), collection.end());
  }

  // Note that this overload will take precedence over Generate(const Base*)
  // when given a Derived* argument. To avoid that, you must either static_cast
  // to Base* or implement Generate(const Base&).
  template <typename T>
  void Generate(const T* value) {
    self.Generate(*value);
  }

  template <typename T>
  void Generate(const std::unique_ptr<T>& value) {
    self.Generate(*value);
  }

  template <typename T>
  void Generate(const std::shared_ptr<T>& value) {
    self.Generate(*value);
  }

  template <typename T>
  void Generate(const std::vector<T>& value) {
    self.GenerateArray(value);
  }

  template <typename T>
  void Generate(const std::set<T>& value) {
    self.GenerateArray(value);
  }

  void Generate(bool value) { EmitBoolean(value); }

  void Generate(std::string_view value) { EmitString(value); }

  void Generate(std::string value) { EmitString(value); }

  void Generate(uint32_t value) { EmitNumeric(value); }
  void Generate(int64_t value) { EmitNumeric(value); }
  void Generate(uint64_t value) { EmitNumeric(value); }

  void ResetIndentLevel() { indent_level_ = 0; }

  void Indent() { indent_level_++; }

  void Outdent() { indent_level_--; }

  // Similar to |this| pointer, the |self| reference is simply pre-cast to the
  // derived type. Methods called with template-typed parameters (such as
  // this->Generate(obj)) would only see the Generate() methods and acceptable
  // parameter types defined within this base class, but when called using
  // self.Generate(obj), the type of obj can be matched to all Generate()
  // methods declared by both the derived type and exposed from its base
  // class (this class).
  DerivedT& self = *static_cast<DerivedT*>(this);

 protected:
  enum class Position : uint8_t {
    kFirst,
    kSubsequent,
  };

  // ConstantStyle indicates whether the constant value to be emitted should be
  // directly placed in the JSON output, or whether is must be wrapped in a
  // string.
  enum ConstantStyle {
    kAsConstant,
    kAsString,
  };

  void GenerateEOF() { EmitNewline(); }

  void GenerateObjectPunctuation(Position position) {
    switch (position) {
      case Position::kFirst:
        Indent();
        EmitNewlineWithIndent();
        break;
      case Position::kSubsequent:
        EmitObjectSeparator();
        break;
    }
  }

  void GenerateObject(fit::closure callback) {
    int original_indent_level = indent_level_;

    EmitObjectBegin();

    callback();

    if (indent_level_ > original_indent_level) {
      Outdent();
      EmitNewlineWithIndent();
    }

    EmitObjectEnd();
  }

  template <typename Type>
  void GenerateObjectMember(std::string_view key, const Type& value,
                            Position position = Position::kSubsequent) {
    GenerateObjectPunctuation(position);
    EmitObjectKey(key);
    self.Generate(value);
  }

  void EmitBoolean(bool value, ConstantStyle style = kAsConstant) {
    if (style == kAsString) {
      os_ << "\"";
    }
    if (value) {
      os_ << "true";
    } else {
      os_ << "false";
    }
    if (style == kAsString) {
      os_ << "\"";
    }
  }

  void EmitString(std::string_view value) {
    os_ << "\"";
    for (char c : value) {
      // Escape control double quotes, backslashes, and control characters.
      // We assume the rest is valid UTF-8 and pass it through to JSON.
      switch (c) {
        case '"':
          os_ << "\\\"";
          break;
        case '\\':
          os_ << "\\\\";
          break;
        case '\n':
          os_ << "\\n";
          break;
        case '\r':
          os_ << "\\r";
          break;
        case '\t':
          os_ << "\\t";
          break;
        default:
          auto u = static_cast<unsigned char>(c);
          if (u < 0x02 || u == 0x7f) {
            // ASCII control characters. Escape it.
            char buf[7];
            snprintf(buf, sizeof buf, "\\u%04x", u);
            os_ << buf;
          } else {
            os_ << c;
          }
          break;
      }
    }
    os_ << "\"";
  }

  // Emits a string literal given its raw FIDL source.
  void EmitLiteral(std::string_view value) {
    os_ << "\"";
    for (auto it = value.begin(); it != value.end(); ++it) {
      // FIDL's string literal syntax is similar to JSON's, so we can emit most
      // characters unchanged (including escape sequences \\, \", \n, \r, \t)
      // except for Unicode escape sequences, handled below.
      if (it[0] != '\\' || it[1] != 'u') {
        os_ << *it;
        continue;
      }
      // We have a Unicode escape \u{X}. First, extract the hex string X.
      it += 2;
      ZX_ASSERT(*it == '{');
      ++it;
      auto hex_begin = it;
      while (*it != '}') {
        ++it;
      }
      std::string_view codepoint_hex = value.substr(hex_begin - value.begin(), it - hex_begin);
      // Next, decode the code point X as an integer.
      auto codepoint = DecodeUnicodeHex(codepoint_hex);
      if (codepoint <= 0xffff) {
        // This code point can be represented by a single \uNNNN in JSON.
        char buf[7];
        snprintf(buf, sizeof buf, "\\u%04x", codepoint);
        os_ << buf;
      } else {
        // This code point must be represented as a surrogate pair in JSON.
        // https://www.unicode.org/faq/utf_bom.html#utf16-4
        auto lead_offset = 0xd800 - (0x10000 >> 10);
        auto lead = lead_offset + (codepoint >> 10);
        auto trail = 0xdc00 + (codepoint & 0x3ff);
        char buf[13];
        snprintf(buf, sizeof buf, "\\u%04x\\u%04x", lead, trail);
        os_ << buf;
      }
    }
    os_ << "\"";
  }

  template <typename T>
  void EmitNumeric(T value, ConstantStyle style = kAsConstant) {
    // Unary plus promotes integers to be at least as wide as (unsigned) int.
    // This ensures we don't print (u)int8_t as an ASCII character.
    // https://en.cppreference.com/w/c/language/conversion#Integer_promotions
    auto promoted_value = +value;
    static_assert(sizeof(promoted_value) >= sizeof(int));
    switch (style) {
      case ConstantStyle::kAsConstant:
        os_ << promoted_value;
        break;
      case ConstantStyle::kAsString:
        os_ << "\"" << promoted_value << "\"";
        break;
    }
  }

  void EmitNewline() { os_ << '\n'; }

  void EmitNewlineWithIndent() {
    os_ << '\n';
    int indent_level = indent_level_;
    while (indent_level--)
      os_ << kIndent;
  }

  void EmitObjectBegin() { os_ << '{'; }

  void EmitObjectSeparator() {
    os_ << ',';
    EmitNewlineWithIndent();
  }

  void EmitObjectEnd() { os_ << '}'; }

  void EmitObjectKey(std::string_view key) {
    EmitString(key);
    os_ << ": ";
  }

  void EmitArrayBegin() { os_ << '['; }

  void EmitArraySeparator() {
    os_ << ',';
    EmitNewlineWithIndent();
  }

  void EmitArrayEnd() { os_ << ']'; }

 private:
  static constexpr const char* kIndent = "  ";
  std::ostream& os_;
  int indent_level_;
};

}  // namespace fidlc

#endif  // TOOLS_FIDL_FIDLC_SRC_JSON_WRITER_H_

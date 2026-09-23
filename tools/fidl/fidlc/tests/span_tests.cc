// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <algorithm>
#include <set>
#include <stack>

#include <gtest/gtest.h>

#include "tools/fidl/fidlc/src/raw_ast.h"
#include "tools/fidl/fidlc/src/tree_visitor.h"
#include "tools/fidl/fidlc/tests/test_library.h"

// This test provides a way to write comprehensive unit tests on the fidlc
// parser. Each test case provides a SourceElement type and a list of source
// strings, with expected source spans of that type marked with special
// characters (see kMarkerLeft and kMarkerRight). The markers can be nested and
// are expected to specify all occurrences of that type of SourceElement.

// Test cases are defined near the bottom of the file as a
// std::vector<TestCase>.

// For each test case:
// - extract_expected_span_views creates a multiset of source spans from a
//   marked source string.
// - |SourceSpanChecker| inherits from |TreeVisitor|, and it collects all the
//   actual spans of a given |SourceElement::ElementType| by walking the AST in
//   each test case.
// - then the expected values are compared against the actual values via set
//   arithmetic.

namespace fidlc {
namespace {

#define FOR_ENUM_VARIANTS(DO)   \
  DO(AliasDeclaration)          \
  DO(Attribute)                 \
  DO(AttributeArg)              \
  DO(AttributeList)             \
  DO(BinaryOperatorConstant)    \
  DO(BoolLiteral)               \
  DO(CompoundIdentifier)        \
  DO(ConstDeclaration)          \
  DO(DocCommentLiteral)         \
  DO(File)                      \
  DO(Identifier)                \
  DO(IdentifierConstant)        \
  DO(IdentifierLayoutParameter) \
  DO(InlineLayoutReference)     \
  DO(LayoutParameterList)       \
  DO(LibraryDeclaration)        \
  DO(LiteralConstant)           \
  DO(LiteralLayoutParameter)    \
  DO(Modifier)                  \
  DO(ModifierList)              \
  DO(NamedLayoutReference)      \
  DO(NumericLiteral)            \
  DO(Ordinal64)                 \
  DO(OrdinaledLayout)           \
  DO(OrdinaledLayoutMember)     \
  DO(ParameterList)             \
  DO(ProtocolCompose)           \
  DO(ProtocolDeclaration)       \
  DO(ProtocolMethod)            \
  DO(ResourceDeclaration)       \
  DO(ResourceProperty)          \
  DO(ServiceMember)             \
  DO(ServiceDeclaration)        \
  DO(StringLiteral)             \
  DO(StructLayout)              \
  DO(StructLayoutMember)        \
  DO(TypeConstraints)           \
  DO(TypeDeclaration)           \
  DO(TypeConstructor)           \
  DO(TypeLayoutParameter)       \
  DO(Using)                     \
  DO(ValueLayout)               \
  DO(ValueLayoutMember)

#define MAKE_ENUM_VARIANT(VAR) k##VAR,
enum ElementType { FOR_ENUM_VARIANTS(MAKE_ENUM_VARIANT) };

#define MAKE_ENUM_NAME(VAR) #VAR,
const char* kElementTypeNames[] = {FOR_ENUM_VARIANTS(MAKE_ENUM_NAME)};

const char* element_type_str(ElementType type) { return kElementTypeNames[type]; }

// Used to delineate spans in source code. E.g.,
// const uint32 «three» = 3;
const std::string kMarkerLeft = "«";
const std::string kMarkerRight = "»";

class SourceSpanVisitor : public TreeVisitor {
 public:
  explicit SourceSpanVisitor(ElementType test_case_type_) : test_case_type_(test_case_type_) {}

  const std::multiset<std::string>& spans() { return spans_; }

  void OnAliasDeclaration(const std::unique_ptr<RawAliasDeclaration>& element) override {
    CheckSpanOfType(ElementType::kAliasDeclaration, *element);
    TreeVisitor::OnAliasDeclaration(element);
  }
  void OnAttribute(const std::unique_ptr<RawAttribute>& element) override {
    CheckSpanOfType(ElementType::kAttribute, *element);
    TreeVisitor::OnAttribute(element);
  }
  void OnAttributeArg(const std::unique_ptr<RawAttributeArg>& element) override {
    CheckSpanOfType(ElementType::kAttributeArg, *element);
    TreeVisitor::OnAttributeArg(element);
  }
  void OnAttributeList(const std::unique_ptr<RawAttributeList>& element) override {
    CheckSpanOfType(ElementType::kAttributeList, *element);
    TreeVisitor::OnAttributeList(element);
  }
  void OnBinaryOperatorConstant(
      const std::unique_ptr<RawBinaryOperatorConstant>& element) override {
    CheckSpanOfType(ElementType::kBinaryOperatorConstant, *element);
    TreeVisitor::OnBinaryOperatorConstant(element);
  }
  void OnBoolLiteral(RawBoolLiteral& element) override {
    CheckSpanOfType(ElementType::kBoolLiteral, element);
    TreeVisitor::OnBoolLiteral(element);
  }
  void OnCompoundIdentifier(const std::unique_ptr<RawCompoundIdentifier>& element) override {
    CheckSpanOfType(ElementType::kCompoundIdentifier, *element);
    TreeVisitor::OnCompoundIdentifier(element);
  }
  void OnConstDeclaration(const std::unique_ptr<RawConstDeclaration>& element) override {
    CheckSpanOfType(ElementType::kConstDeclaration, *element);
    TreeVisitor::OnConstDeclaration(element);
  }
  void OnDocCommentLiteral(RawDocCommentLiteral& element) override {
    CheckSpanOfType(ElementType::kDocCommentLiteral, element);
    TreeVisitor::OnDocCommentLiteral(element);
  }
  void OnFile(const std::unique_ptr<File>& element) override {
    CheckSpanOfType(ElementType::kFile, *element);
    TreeVisitor::OnFile(element);
  }
  void OnIdentifier(const std::unique_ptr<RawIdentifier>& element) override {
    CheckSpanOfType(ElementType::kIdentifier, *element);
  }
  void OnIdentifierConstant(const std::unique_ptr<RawIdentifierConstant>& element) override {
    CheckSpanOfType(ElementType::kIdentifierConstant, *element);
    TreeVisitor::OnIdentifierConstant(element);
  }
  void OnIdentifierLayoutParameter(
      const std::unique_ptr<RawIdentifierLayoutParameter>& element) override {
    CheckSpanOfType(ElementType::kIdentifierLayoutParameter, *element);
    TreeVisitor::OnIdentifierLayoutParameter(element);
  }
  void OnInlineLayoutReference(const std::unique_ptr<RawInlineLayoutReference>& element) override {
    CheckSpanOfType(ElementType::kInlineLayoutReference, *element);
    TreeVisitor::OnInlineLayoutReference(element);
  }
  void OnLayout(const std::unique_ptr<RawLayout>& element) override {
    switch (element->kind) {
      case RawLayout::Kind::kBits:
      case RawLayout::Kind::kEnum:
        CheckSpanOfType(ElementType::kValueLayout, *element);
        break;
      case RawLayout::Kind::kStruct:
        CheckSpanOfType(ElementType::kStructLayout, *element);
        break;
      case RawLayout::Kind::kTable:
      case RawLayout::Kind::kOverlay:
      case RawLayout::Kind::kUnion:
        CheckSpanOfType(ElementType::kOrdinaledLayout, *element);
        break;
    }
    TreeVisitor::OnLayout(element);
  }
  void OnLayoutParameterList(const std::unique_ptr<RawLayoutParameterList>& element) override {
    CheckSpanOfType(ElementType::kLayoutParameterList, *element);
    TreeVisitor::OnLayoutParameterList(element);
  }
  void OnLibraryDeclaration(const std::unique_ptr<RawLibraryDeclaration>& element) override {
    CheckSpanOfType(ElementType::kLibraryDeclaration, *element);
    TreeVisitor::OnLibraryDeclaration(element);
  }
  void OnLiteralConstant(const std::unique_ptr<RawLiteralConstant>& element) override {
    CheckSpanOfType(ElementType::kLiteralConstant, *element);
    TreeVisitor::OnLiteralConstant(element);
  }
  void OnLiteralLayoutParameter(
      const std::unique_ptr<RawLiteralLayoutParameter>& element) override {
    CheckSpanOfType(ElementType::kLiteralLayoutParameter, *element);
    TreeVisitor::OnLiteralLayoutParameter(element);
  }
  void OnModifier(const std::unique_ptr<RawModifier>& element) override {
    CheckSpanOfType(ElementType::kModifier, *element);
    TreeVisitor::OnModifier(element);
  }
  void OnModifierList(const std::unique_ptr<RawModifierList>& element) override {
    CheckSpanOfType(ElementType::kModifierList, *element);
    TreeVisitor::OnModifierList(element);
  }
  void OnNamedLayoutReference(const std::unique_ptr<RawNamedLayoutReference>& element) override {
    CheckSpanOfType(ElementType::kNamedLayoutReference, *element);
    TreeVisitor::OnNamedLayoutReference(element);
  }
  void OnNumericLiteral(RawNumericLiteral& element) override {
    CheckSpanOfType(ElementType::kNumericLiteral, element);
    TreeVisitor::OnNumericLiteral(element);
  }
  void OnOrdinal64(RawOrdinal64& element) override {
    CheckSpanOfType(ElementType::kOrdinal64, element);
    TreeVisitor::OnOrdinal64(element);
  }
  void OnOrdinaledLayoutMember(const std::unique_ptr<RawOrdinaledLayoutMember>& element) override {
    CheckSpanOfType(ElementType::kOrdinaledLayoutMember, *element);
    TreeVisitor::OnOrdinaledLayoutMember(element);
  }
  void OnParameterList(const std::unique_ptr<RawParameterList>& element) override {
    CheckSpanOfType(ElementType::kParameterList, *element);
    TreeVisitor::OnParameterList(element);
  }
  void OnProtocolCompose(const std::unique_ptr<RawProtocolCompose>& element) override {
    CheckSpanOfType(ElementType::kProtocolCompose, *element);
    TreeVisitor::OnProtocolCompose(element);
  }
  void OnProtocolDeclaration(const std::unique_ptr<RawProtocolDeclaration>& element) override {
    CheckSpanOfType(ElementType::kProtocolDeclaration, *element);
    TreeVisitor::OnProtocolDeclaration(element);
  }
  void OnProtocolMethod(const std::unique_ptr<RawProtocolMethod>& element) override {
    CheckSpanOfType(ElementType::kProtocolMethod, *element);
    TreeVisitor::OnProtocolMethod(element);
  }
  void OnResourceDeclaration(const std::unique_ptr<RawResourceDeclaration>& element) override {
    CheckSpanOfType(ElementType::kResourceDeclaration, *element);
    TreeVisitor::OnResourceDeclaration(element);
  }
  void OnResourceProperty(const std::unique_ptr<RawResourceProperty>& element) override {
    CheckSpanOfType(ElementType::kResourceProperty, *element);
    TreeVisitor::OnResourceProperty(element);
  }
  void OnServiceDeclaration(const std::unique_ptr<RawServiceDeclaration>& element) override {
    CheckSpanOfType(ElementType::kServiceDeclaration, *element);
    TreeVisitor::OnServiceDeclaration(element);
  }
  void OnServiceMember(const std::unique_ptr<RawServiceMember>& element) override {
    CheckSpanOfType(ElementType::kServiceMember, *element);
    TreeVisitor::OnServiceMember(element);
  }
  void OnStringLiteral(RawStringLiteral& element) override {
    CheckSpanOfType(ElementType::kStringLiteral, element);
    TreeVisitor::OnStringLiteral(element);
  }
  void OnStructLayoutMember(const std::unique_ptr<RawStructLayoutMember>& element) override {
    CheckSpanOfType(ElementType::kStructLayoutMember, *element);
    TreeVisitor::OnStructLayoutMember(element);
  }
  void OnTypeConstraints(const std::unique_ptr<RawTypeConstraints>& element) override {
    CheckSpanOfType(ElementType::kTypeConstraints, *element);
    TreeVisitor::OnTypeConstraints(element);
  }
  void OnTypeConstructor(const std::unique_ptr<RawTypeConstructor>& element) override {
    CheckSpanOfType(ElementType::kTypeConstructor, *element);
    TreeVisitor::OnTypeConstructor(element);
  }
  void OnTypeDeclaration(const std::unique_ptr<RawTypeDeclaration>& element) override {
    CheckSpanOfType(ElementType::kTypeDeclaration, *element);
    TreeVisitor::OnTypeDeclaration(element);
  }
  void OnTypeLayoutParameter(const std::unique_ptr<RawTypeLayoutParameter>& element) override {
    CheckSpanOfType(ElementType::kTypeLayoutParameter, *element);
    TreeVisitor::OnTypeLayoutParameter(element);
  }
  void OnUsing(const std::unique_ptr<RawUsing>& element) override {
    CheckSpanOfType(ElementType::kUsing, *element);
    TreeVisitor::OnUsing(element);
  }
  void OnValueLayoutMember(const std::unique_ptr<RawValueLayoutMember>& element) override {
    CheckSpanOfType(ElementType::kValueLayoutMember, *element);
    TreeVisitor::OnValueLayoutMember(element);
  }

 private:
  // Called on every node of the AST that we visit. We collect spans of the |ElementType| we are
  // looking for as we traverse the tree, and store them in a multiset.
  void CheckSpanOfType(const ElementType element_type, const SourceElement& element) {
    if (element_type != test_case_type_) {
      return;
    }
    spans_.insert(std::string(element.span().data()));
  }

  ElementType test_case_type_;
  std::multiset<std::string> spans_;
};

std::string replace_markers(std::string_view source, std::string_view left_replace,
                            std::string_view right_replace) {
  std::string result(source);

  const auto replace_all = [&](std::string_view pattern, std::string_view replace_with) {
    std::string::size_type i = result.find(pattern);
    while (i != std::string::npos) {
      result.replace(i, pattern.length(), replace_with);
      i = result.find(pattern, i + replace_with.length());
    }
  };

  replace_all(kMarkerLeft, left_replace);
  replace_all(kMarkerRight, right_replace);
  return result;
}

std::string remove_markers(std::string_view source) { return replace_markers(source, "", ""); }

// Extracts marked source spans from a given source string as |string_view|s into the
// |cleaned_source|. If source spans are incorrectly marked (missing or extra markers), returns
// empty set; otherwise, returns a multiset of expected spans.
std::multiset<std::string_view> extract_expected_span_views(std::string_view marked_source,
                                                            std::string_view clean_source) {
  std::stack<size_t> stack;
  std::multiset<std::string_view> spans;
  size_t left_marker_size = kMarkerLeft.length();
  size_t right_marker_size = kMarkerRight.length();
  size_t combined_marker_size = left_marker_size + right_marker_size;

  const auto match = [&](size_t i, std::string_view marker) {
    return marker.compare(marked_source.substr(i, marker.length())) == 0;
  };

  auto offset = [&](size_t i) {
    return i - (stack.size() * left_marker_size) - (spans.size() * combined_marker_size);
  };

  for (size_t i = 0; i < marked_source.length();) {
    if (match(i, kMarkerLeft)) {
      stack.push(offset(i));
      i += left_marker_size;
    } else if (match(i, kMarkerRight)) {
      if (stack.empty()) {
        ADD_FAILURE() << "unexpected closing marker '" << kMarkerRight << "' at position " << i
                      << "  in source string";
        // Return an empty set if errors
        spans.clear();
        break;
      }

      // const std::string span = remove_markers(source.substr(stack.top(),  // index of left marker
      //                                                       i - stack.top())  // length of span
      // );
      size_t pos_in_clean_source = stack.top();
      size_t count_in_clean_source = offset(i) - pos_in_clean_source;
      stack.pop();
      auto x = clean_source.substr(pos_in_clean_source, count_in_clean_source);
      spans.insert(x);
      i += right_marker_size;
    } else {
      i += 1;
    }
  }

  if (!stack.empty()) {
    ADD_FAILURE() << "expected closing marker '" << kMarkerRight << "'";
    // Return an empty set if errors
    spans.clear();
  }

  return spans;
}

struct TestCase {
  ElementType element_type;
  std::vector<std::string> marked_sources;
};

const std::vector<TestCase> kTestCases = {
    {ElementType::kAliasDeclaration,
     {
         R"FIDL(library x; «alias Foo = uint8»;)FIDL",
         R"FIDL(library x; «alias Foo = vector<uint8>»;)FIDL",
     }},
    {ElementType::kAttribute,
     {
         R"FIDL(library x; «@foo("foo")» «@bar» const MY_BOOL bool = false;)FIDL",
         R"FIDL(library x;
          «@foo("foo")»
          «@bar»
          const MY_BOOL bool = false;
         )FIDL",
         R"FIDL(library x;
          protocol Foo {
            Bar(«@foo» struct {});
          };
         )FIDL",
     }},
    {ElementType::kAttributeArg,
     {
         R"FIDL(library x; @attr(«"foo"») const MY_BOOL bool = false;)FIDL",
         R"FIDL(library x; @attr(«a="foo"»,«b="bar"») const MY_BOOL bool = false;)FIDL",
         R"FIDL(library x;
          const MY_BOOL bool = false;
          @attr(«a=true»,«b=MY_BOOL»,«c="foo"»)
          const MY_OTHER_BOOL bool = false;
         )FIDL",
     }},
    {ElementType::kAttributeList,
     {
         R"FIDL(library x; «@foo("foo") @bar» const MY_BOOL bool = false;)FIDL",
         R"FIDL(library x;
          «@foo("foo")
          @bar»
          const MY_BOOL bool = false;
         )FIDL",
         R"FIDL(library x;
          protocol Foo {
            Bar(«@foo» struct {});
          };
         )FIDL",
     }},
    {ElementType::kBinaryOperatorConstant,
     {
         R"FIDL(library x;
          const one uint8 = 0x0001;
          const two_fifty_six uint16 = 0x0100;
          const two_fifty_seven uint16 = «one | two_fifty_six»;
         )FIDL",
         R"FIDL(library x; const two_fifty_seven uint16 = «0x0001 | 0x0100»;)FIDL",
     }},
    {ElementType::kBoolLiteral,
     {
         R"FIDL(library x; const x bool = «true»;)FIDL",
         R"FIDL(library x; @attr(«true») const x bool = «true»;)FIDL",
         R"FIDL(library x; const x bool = «false»;)FIDL",
         R"FIDL(library x; @attr(«false») const x bool = «false»;)FIDL",
     }},
    {ElementType::kCompoundIdentifier,
     {
         R"FIDL(library «foo.bar.baz»;)FIDL",
     }},
    {ElementType::kConstDeclaration,
     {
         R"FIDL(library example;
          «const C_SIMPLE uint32   = 11259375»;
          «const C_HEX_S uint32    = 0xABCDEF»;
          «const C_HEX_L uint32    = 0XABCDEF»;
          «const C_BINARY_S uint32 = 0b101010111100110111101111»;
          «const C_BINARY_L uint32 = 0B101010111100110111101111»;
      )FIDL"}},
    {ElementType::kDocCommentLiteral,
     {
         R"FIDL(library x;
          «/// Foo»
          const MY_BOOL bool = false;)FIDL",
     }},
    {ElementType::kIdentifier,
     {
         R"FIDL(library «x»;
          type «MyEnum» = strict enum {
            «A» = 1;
          };
         )FIDL",
         R"FIDL(library «x»;
          type «MyStruct» = resource struct {
            «boolval» «bool»;
            «boolval» «resource»;
            «boolval» «flexible»;
            «boolval» «struct»;
          };
         )FIDL",
         R"FIDL(library «x»;
          type «MyUnion» = flexible union {
            1: «intval» «int64»;
          };
         )FIDL",
     }},
    {ElementType::kIdentifierConstant,
     {
         R"FIDL(library x; const x bool = true; const y bool = «x»;)FIDL",
     }},
    {ElementType::kIdentifierLayoutParameter,
     {
         R"FIDL(library x; type a = bool; const b uint8 = 4; type y = array<«a»,«b»>;)FIDL",
     }},
    {ElementType::kInlineLayoutReference,
     {
         R"FIDL(library x;
          type S = «struct {
            intval int64;
            boolval bool = false;
            stringval string:MAX_STRING_SIZE;
            inner «union {
              1: floatval float64;
            }»:optional;
          }»;
         )FIDL",
         R"FIDL(library x;
          protocol P {
            M(«struct {
              intval int64;
              boolval bool = false;
              stringval string:MAX_STRING_SIZE;
              inner «union {
                1: floatval float64;
              }»:optional;
            }»);
          };
         )FIDL",
         R"FIDL(library x;
          protocol Foo {
            Bar(«@foo struct {}»);
          };
         )FIDL",
     }},
    {ElementType::kLayoutParameterList,
     {
         R"FIDL(library x; type y = array«<uint8,4>»;)FIDL",
         R"FIDL(library x; type y = vector«<array«<uint8,4>»>»;)FIDL",
     }},
    {ElementType::kLibraryDeclaration,
     {
         R"FIDL(«library x»; using y;)FIDL",
         R"FIDL(«library x.y.z»; using y;)FIDL",
     }},
    {ElementType::kLiteralConstant,
     {
         R"FIDL(library x; const x bool = «true»;)FIDL",
         R"FIDL(library x; const x uint8 = «42»;)FIDL",
         R"FIDL(library x; const x string = «"hi"»;)FIDL",
     }},
    {ElementType::kLiteralLayoutParameter,
     {
         R"FIDL(library x; type y = array<uint8,«4»>;)FIDL",
         R"FIDL(library x; type y = vector<array<uint8,«4»>>;)FIDL",
     }},
    {ElementType::kModifier,
     {
         // Layouts
         R"FIDL(library x; type MyBits = «flexible» bits { MY_VALUE = 1; };)FIDL",
         R"FIDL(library x; type MyBits = «strict» bits : uint32 { MY_VALUE = 1; };)FIDL",
         R"FIDL(library x; type MyEnum = «flexible» enum : uint32 { MY_VALUE = 1; };)FIDL",
         R"FIDL(library x; type MyEnum = «strict» enum { MY_VALUE = 1; };)FIDL",
         R"FIDL(library x; type MyStruct = «resource» struct {};)FIDL",
         R"FIDL(library x; type MyTable = «resource» table { 1: my_member bool; };)FIDL",
         R"FIDL(library x; type MyUnion = «resource» union { 1: my_member bool; };)FIDL",
         R"FIDL(library x; type MyUnion = «flexible» union { 1: my_member bool; };)FIDL",
         R"FIDL(library x; type MyUnion = «strict» union { 1: my_member bool; };)FIDL",
         R"FIDL(library x; type MyUnion = «resource» «strict» union { 1: my_member bool; };)FIDL",
         R"FIDL(library x; @attr type MyEnum = «flexible» enum : uint32 { MY_VALUE = 1; };)FIDL",
         R"FIDL(library x; @attr type MyStruct = «resource» struct {};)FIDL",
         R"FIDL(library x; @attr type MyUnion = «resource» «strict» union { 1: my_member bool; };)FIDL",
         // Note that the following 3 tests have union members named like modifiers.
         R"FIDL(library x; type MyUnion = «resource» «flexible» union { 1: my_member resource; };)FIDL",
         R"FIDL(library x; type MyUnion = «strict» «resource» union { 1: my_member flexible; };)FIDL",
         R"FIDL(library x; type MyUnion = «flexible» «resource» union { 1: my_member strict; };)FIDL",
         // Protocols
         R"FIDL(library x; «ajar» protocol MyProtocol {};)FIDL",
         R"FIDL(library x; «closed» protocol MyProtocol {};)FIDL",
         R"FIDL(library x; «open» protocol MyProtocol {};)FIDL",
         R"FIDL(library x; @attr «open» protocol MyProtocol {};)FIDL",
         // Methods
         R"FIDL(library x; «open» protocol MyProtocol { «flexible» MyMethod(); };)FIDL",
         R"FIDL(library x; «open» protocol MyProtocol { «strict» MyMethod(); };)FIDL",
         R"FIDL(library x; «open» protocol MyProtocol { @attr «strict» MyMethod(); };)FIDL",
         // With availabilities
         R"FIDL(library x; type MyUnion = «flexible(added=2)» union {};)FIDL",
         R"FIDL(library x; type MyUnion = «strict(removed=2)» «flexible(added=2)» «resource(added=3)» union {};)FIDL",
         R"FIDL(library x; «open(removed=2)» «ajar(added=2)» protocol MyProtocol { @attr «strict(added=2)» MyMethod(); };)FIDL",

// TODO(https://fxbug.dev/348403275): The ability to name methods "strict" or "flexible"
// is temporarily regressed. Re-enable after we implement explicit method kind syntax.
#if 0
         // Note that the following 3 tests have protocol methods named like modifiers.
         R"FIDL(library x; «open» protocol MyProtocol { «flexible» flexible(); strict(); };)FIDL",
         R"FIDL(library x; «open» protocol MyProtocol { «strict» strict(); flexible(); };)FIDL",
         R"FIDL(library x; «open» protocol MyProtocol { @attr «flexible» flexible(); @attr strict(); };)FIDL",
#endif
     }},
    {ElementType::kModifierList,
     {
         // Single modifier
         R"FIDL(library x; type MyUnion = «flexible» union {};)FIDL",
         R"FIDL(library x; type MyStruct = struct { anon @attr «flexible» union {}; };)FIDL",
         R"FIDL(library x; @attr «ajar» protocol MyProtocol { @attr «flexible» MyMethod(); };)FIDL",
         // Multiple modifiers
         R"FIDL(library x; type MyUnion = «flexible resource» union {};)FIDL",
         R"FIDL(library x; type MyStruct = «resource» struct { anon @attr «flexible resource» union {}; };)FIDL",
         // With availabilities
         R"FIDL(library x; type MyUnion = «flexible(added=2)» union {};)FIDL",
         R"FIDL(library x; type MyUnion = «strict(removed=2) flexible(added=2) resource(added=3)» union {};)FIDL",
         R"FIDL(library x; «open(removed=2) ajar(added=2)» protocol MyProtocol { @attr «strict(added=2)» MyMethod(); };)FIDL",
     }},
    {ElementType::kNamedLayoutReference,
     {
         R"FIDL(library x;
          type S = struct {
            intval «int64»;
            boolval «bool» = false;
            stringval «string»:MAX_STRING_SIZE;
            inner struct {
              floatval «float64»;
              uintval «uint8» = 7;
              vecval «vector»<«vector»<Foo>>;
              arrval «array»<uint8,4>;
            };
          };
         )FIDL",
     }},
    {ElementType::kNumericLiteral,
     {
         R"FIDL(library x; const x uint8 = «42»;)FIDL",
         R"FIDL(library x; @attr(«42») const x uint8 = «42»;)FIDL",
     }},
    {ElementType::kOrdinal64,
     {
         R"FIDL(library x; type U = union { «1:» one uint8; };)FIDL",
     }},
    {ElementType::kOrdinaledLayout,
     {
         R"FIDL(library x;
          type T = «resource table {
            1: intval int64;
          }»;
          type U = «flexible resource union {
            1: intval int64;
          }»:optional;
         )FIDL",
     }},
    {ElementType::kOrdinaledLayoutMember,
     {
         R"FIDL(library x;
          type T = table {
            «1: intval int64»;
            «@attr 3: floatval float64»;
            «4: stringval string:100»;
            «5: inner union {
              «1: boolval bool»;
            }:optional»;
          };
         )FIDL",
     }},
    {ElementType::kParameterList,
     {
         R"FIDL(library x; protocol X { Method«()» -> «()»; };)FIDL",
         R"FIDL(library x; protocol X { Method«(struct {})» -> «(struct {})»; };)FIDL",
         R"FIDL(library x; protocol X { Method«(struct { a int32; b bool; })» -> «(struct { c
         uint8; d bool; })»; };)FIDL",
         R"FIDL(library x; protocol X { -> Event«()»; };)FIDL",
         R"FIDL(library x; protocol X { -> Event«(struct {})»; };)FIDL",
         R"FIDL(library x; protocol X { -> Event«(struct { a int32; b bool; })»; };)FIDL",
     }},
    {ElementType::kProtocolCompose,
     {
         R"FIDL(library x; protocol X { «compose OtherProtocol»; };)FIDL",
         R"FIDL(library x; protocol X { «@attr compose OtherProtocol»; };)FIDL",
         R"FIDL(library x; protocol X {
            «/// Foo
            compose OtherProtocol»;
          };)FIDL",
     }},
    {ElementType::kProtocolDeclaration,
     {
         R"FIDL(library x; «protocol X {}»;)FIDL",
         R"FIDL(library x; «@attr protocol X { compose OtherProtocol; }»;)FIDL",
     }},
    {ElementType::kProtocolMethod,
     {
         // One-way
         R"FIDL(library x; protocol X { «Method()»; };)FIDL",
         R"FIDL(library x; protocol X { «@attr Method(struct { a int32; b bool; })»; };)FIDL",
         // Two-way
         R"FIDL(library x; protocol X { «Method(struct { a int32; }) -> ()»; };)FIDL",
         R"FIDL(library x; protocol X { «@attr Method(struct { a int32; }) -> ()»; };)FIDL",
         R"FIDL(library x; protocol X { «Method(struct { a int32; }) -> (struct { res bool; })»;
         };)FIDL",
         R"FIDL(library x; protocol X { «Method(struct { a int32; }) -> (struct { res
         bool; res2 int32; })»; };)FIDL",
         // Two-way + error
         R"FIDL(library x; protocol X { «Method(struct { a int32; }) -> () error uint32»;
         };)FIDL",
         R"FIDL(library x; protocol X { «@attr Method(struct { a int32; }) -> () error
         uint32»; };)FIDL",
         R"FIDL(library x; protocol X { «Method(struct { a int32; }) ->
         (struct { res bool; }) error uint32»; };)FIDL",
         R"FIDL(library x; protocol X {
         «Method(struct { a int32; }) -> (struct { res bool; res2 int32; }) error uint32»;
         };)FIDL",
         // Event
         R"FIDL(library x; protocol X { «-> Event()»; };)FIDL",
         R"FIDL(library x; protocol X { «-> Event(struct { res bool; })»; };)FIDL",
         R"FIDL(library x; protocol X { «@attr -> Event(struct { res bool; res2 int32; })»;
         };)FIDL",
     }},
    {ElementType::kResourceDeclaration, {R"FIDL(
     library example; «resource_definition Res : uint32 { properties { subtype Enum; };
     }»;)FIDL"}},
    {ElementType::kResourceProperty, {R"FIDL(
     library example; resource_definition Res : uint32 { properties { «subtype Enum»; };
     };)FIDL"}},
    {ElementType::kServiceDeclaration,
     {
         R"FIDL(library x; «service X {}»;)FIDL",
         R"FIDL(library x; protocol P {}; «service X { Z client_end:P; }»;)FIDL",
     }},
    {ElementType::kServiceMember,
     {
         R"FIDL(library x; protocol P {}; service X { «Z client_end:P»; };)FIDL",
         R"FIDL(library x; protocol P {}; service X { «@attr Z client_end:P»; };)FIDL",
     }},
    {ElementType::kStringLiteral,
     {
         R"FIDL(library x; const x string = «"hello"»;)FIDL",
         R"FIDL(library x; @attr(«"foo"») const x string = «"goodbye"»;)FIDL",
         R"FIDL(library x; @attr(a=«"foo"»,b=«"bar"») const MY_BOOL bool = false;)FIDL",
     }},
    {ElementType::kStructLayout,
     {
         R"FIDL(library x;
          type S = «resource struct {
            intval int64;
          }»;
         )FIDL",
     }},
    {ElementType::kStructLayoutMember,
     {
         R"FIDL(library x;
          type S = struct {
            «intval int64»;
            «boolval bool = false»;
            «@attr stringval string:100»;
            «inner struct {
              «floatval float64»;
              «uintval uint8 = 7»;
            }»;
          };
         )FIDL",
     }},
    {ElementType::kTypeConstraints,
     {
         R"FIDL(library x; type y = array<uint8,4>;)FIDL",
         R"FIDL(library x; type y = vector<vector<uint8>:«16»>:«<16,optional>»;)FIDL",
         R"FIDL(library x; type y = union { 1: foo bool; }:«optional»;)FIDL",
         R"FIDL(library x; using zx; type y = zx.Handle:«optional»;)FIDL",
         R"FIDL(library x; using zx; type y = zx.Handle:«<VMO,zx.READ,optional>»;)FIDL",
     }},
    {ElementType::kTypeConstructor,
     {
         R"FIDL(library x; const x «int32» = 1;)FIDL",
         R"FIDL(library x; const x «zx.Handle:<VMO, zx.Rights.READ, optional>» = 1;)FIDL",
         R"FIDL(library x; const x «Foo<«Bar<«zx.Handle:VMO»>:20»>:optional» = 1;)FIDL",
         R"FIDL(library x; const x «zx.Handle:VMO» = 1;)FIDL",
         R"FIDL(library x; type y = «array<uint8,4>»;)FIDL",
         R"FIDL(library x; type y = «vector<«array<Foo,4>»>»;)FIDL",
         R"FIDL(library x; type y = «string:100»;)FIDL",
         R"FIDL(library x; type y = «string:<100,optional>»;)FIDL",
         R"FIDL(library x;
          type e = «flexible enum : «uint32» {
            A = 1;
          }»;
         )FIDL",
         R"FIDL(library x;
          type S = «struct {
            intval «int64»;
            boolval «bool» = false;
            stringval «string:MAX_STRING_SIZE»;
            inner «struct {
              floatval «float64»;
              uintval «uint8» = 7;
              vecval «vector<«vector<Foo>»>»;
              arrval «array<uint8,4>»;
            }»;
          }»;
         )FIDL",
         R"FIDL(library x; protocol X { Method(«struct { a «int32»; b «bool»; }») -> («struct
         {}») error «uint32»; };)FIDL",
         R"FIDL(library x;
          resource_definition foo : «uint8» {
              properties {
                  rights «rights»;
              };
          };
         )FIDL",
         R"FIDL(library x;
          protocol Foo {
            Bar(«@foo struct {}»);
          };
         )FIDL",
     }},
    {ElementType::kTypeDeclaration,
     {
         R"FIDL(library x;
          «type E = enum : int8 {
            A = 1;
          }»;
          «type S = struct {
            intval int64;
          }»;
          «type U = union {
            1: intval int64;
          }:optional»;
         )FIDL",
     }},
    {ElementType::kTypeLayoutParameter,
     {
         R"FIDL(library x; type y = array<uint8,4>;)FIDL",
         R"FIDL(library x; type y = vector<«array<uint8,4>»>;)FIDL",
     }},
    {ElementType::kUsing,
     {
         R"FIDL(library x; «using y»;)FIDL",
         R"FIDL(library x; «using y as z»;)FIDL",
     }},
    {ElementType::kValueLayout,
     {
         R"FIDL(library x;
          type B = «bits {
            A = 1;
          }»;
          type E = «strict enum {
            A = 1;
          }»;
         )FIDL",
     }},
    {ElementType::kValueLayoutMember,
     {
         R"FIDL(library x;
          type E = enum {
            «A = 1»;
            «@attr B = 2»;
          };
         )FIDL",
         R"FIDL(library x;
          type B = bits {
            «A = 0x1»;
            «@attr B = 0x2»;
          };
         )FIDL",
     }},
};

class SpanTest : public testing::TestWithParam<TestCase> {
 public:
  void RunTest(const std::string& insert_left_padding, const std::string& insert_right_padding,
               const std::set<ElementType>& exclude) {
    const TestCase& test_case = GetParam();
    if (exclude.find(test_case.element_type) != exclude.end()) {
      return;
    }

    for (const auto& unpadded_source : test_case.marked_sources) {
      // Insert the specified left/right padding.
      std::string marked_source = replace_markers(
          unpadded_source, insert_left_padding + kMarkerLeft, kMarkerRight + insert_right_padding);
      std::string clean_source = remove_markers(marked_source);

      // Parse the source with markers removed
      TestLibrary library(clean_source);
      std::unique_ptr<File> ast;
      if (!library.Parse(&ast)) {
        FAIL() << "Failed to parse fidl:\n\n" << clean_source;
      }

      // Get the expected spans from the marked source
      std::multiset<std::string_view> span_views =
          extract_expected_span_views(marked_source, library.source_file().data());
      // Returns an empty set when there are errors
      if (span_views.empty()) {
        break;
      }

      std::multiset<std::string> expected_spans;
      for (const auto& span_view : span_views) {
        expected_spans.insert(std::string(span_view));
      }

      // Get the actual spans by walking the AST, then compare them against
      // our expectation.
      SourceSpanVisitor visitor(test_case.element_type);
      visitor.OnFile(ast);
      std::multiset<std::string> actual_spans = visitor.spans();

      // Report errors where the checker found unexpected spans (spans in actual but not
      // expected).
      std::multiset<std::string> actual_spans_minus_expected;
      std::set_difference(
          actual_spans.begin(), actual_spans.end(), expected_spans.begin(), expected_spans.end(),
          std::inserter(actual_spans_minus_expected, actual_spans_minus_expected.begin()));
      for (const auto& span : actual_spans_minus_expected) {
        ADD_FAILURE() << "unexpected occurrence of spans of type "
                      << element_type_str(test_case.element_type) << ": " << kMarkerLeft << span
                      << kMarkerRight;
      }

      // Report errors where the checker failed to find expected spans (spans in expected but not
      // actual).
      std::multiset<std::string> expected_spans_minus_actual;
      std::set_difference(
          expected_spans.begin(), expected_spans.end(), actual_spans.begin(), actual_spans.end(),
          std::inserter(expected_spans_minus_actual, expected_spans_minus_actual.begin()));
      for (const auto& span : expected_spans_minus_actual) {
        ADD_FAILURE() << "expected (but didn't find) spans of type "
                      << element_type_str(test_case.element_type) << ": " << kMarkerLeft << span
                      << kMarkerRight;
      }
    }
  }
};

TEST_P(SpanTest, GoodNoPadding) { RunTest("", "", {}); }
TEST_P(SpanTest, GoodLeftPadding) { RunTest(" ", "", {}); }
TEST_P(SpanTest, GoodRightPadding) { RunTest("", " ", {ElementType::kDocCommentLiteral}); }
TEST_P(SpanTest, GoodLeftRightPadding) { RunTest(" ", " ", {ElementType::kDocCommentLiteral}); }

INSTANTIATE_TEST_SUITE_P(SpanTests, SpanTest, testing::ValuesIn(kTestCases),
                         [](const testing::TestParamInfo<TestCase>& info) {
                           return element_type_str(info.param.element_type);
                         });

}  // namespace
}  // namespace fidlc

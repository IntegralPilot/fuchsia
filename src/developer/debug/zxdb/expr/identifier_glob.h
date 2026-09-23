// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_EXPR_IDENTIFIER_GLOB_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_EXPR_IDENTIFIER_GLOB_H_

#include <optional>

#include "src/developer/debug/zxdb/common/err.h"
#include "src/developer/debug/zxdb/expr/parsed_identifier.h"

namespace zxdb {

// Provides a simple very-restricted Glob-like syntax for matching template types.
//
// For the requirements of the pretty-printing system, we want to be able to match different
// template types but in a type-aware manner.
//
// For example, say we were to write a pretty-printer matching the glob "MyClass<*>" with a normal
// string-based matcher. It would match "MyClass<int>" as desired. But it would also match things
// like nested templates such as "MyClass<int>::Ref<Foo>" which is not desirable.
//
// So this class provides a way to match "*" for template type parameters ONLY in a manner that's
// aware of the syntax of template definitions. Since type matching doesn't need to match things
// like "all type names tarting with the letter 'a'", "*" never matches anything other than template
// parameters.
//
// Syntax
// ------
//
// There is only one special character: *
//
//  - All non-template parameters must match exactly (case sensitive).
//
//  - A '*' normally matches EXACTLY ONE template parameter.
//
//  - If the LAST template parameter in a glob is a "*", it will match ALL REMAINING template
//    parameters.
//
//  - The "*" must occur by itself as a template parameter to match. So "Foo<*>" a glob matching
//    any type, but "Foo<int*>" is a literal. This is important because "*" occurs in many type
//    definitions but never by itself in a language we support.
//
//  - It does not work recursively so while "Foo<*>" is "Foo<Bar<*>>" is currently a literal. This
//    could be changed in the future if needed.
//
//  - Global qualifications "::Foo" are ignored. Names are assumed to be globally qualified, unless
//    |match_any_namespace| is given during initialization.
//
//  - When |match_any_namespace| is given, simple names such as "Foo" will match all of
//    "SomeNamespace::Foo", "OtherNamespace::Foo", and "Foo".
//
// Scoring glob matching
// ---------------------
//
// Say we have three globs:
//   [1] MyClass<float>
//   [2] MyClass<*>
//   [3] MyClass<*, *>
//
// We have the following requirements:
//   - The type "MyClass<float>" should preferentially match [1], and secondarily match [2]
//   - The type "MyClass<int>" will match only [2].
//   - The type "MyClass<int, float>" and "MyClass<int, float, char>" will preferentially match [3]
//     and secondarily match [2].
//
// Note that the type "MyClass<>" will match none of the globs. If you wanted to match something
// with this name (which is not a valid type name in C++, but is valid in some contexts) you will
// need to supply a separate glob with an exact match.
//
// To measure match priority, Matches() computes the number of template parameters the last
// encountered wildcard (if any) matches. Better matches have lower scores.
//   - An exact string match with no wildcard will have a score of 0.
//   - A single wildcard matching a single template parameter will score 1.
//   - Two wildcards matching two template parameters will score 1.
//   - One wildcard matching two template parameters will score 2.
//   - The glob "Foo<*, Bar>" matching "Foo<int, Bar>" will score 1 (last * matched 1 param).
//
// It's possible to have multiple levels of templates, say with a glob:
//   MyClass<*, *>::Something<*>
// In this case we return the largest number of matches of the last "*" across all components. So
// in this example
//   - "MyClass<int, int>::Something<int>" will score 1.
//   - "MyClass<int, int, int>::Something<int>" will score 2.
//   - "MyClass<int, int>::Something<int, int>" will also score 2 (not clear which is better).
//
// Scoring namespace glob matching
// -------------------------------
//
// Say we have three globs:
//   [1] Foo (where match_any_namespace is true)
//   [2] MyClass::Foo
//   [3] Nested::Namespace::Foo
//   [4] MyClass::Foo<*>
//
// The requirements are now:
//   - A function named "MyClass::Foo" should preferentially match [1], and secondarily match [2].
//   - A function named "Nested::Namespace::Foo" should preferentially match [3] and secondarily
//     match [1].
//   - A function named "MyClass::Foo<int>" is the same as described above. [3] is deferentially
//     matched, [1] does not match at all.
//
// Match priority is measured similarly to template globs, the score is computed as the number of
// namespace scopes that were skipped. Again, better matches have lower scores.
class IdentifierGlob {
 public:
  // Call Init() to initialize with a parsed identifier.
  IdentifierGlob() = default;

  // Specify a pre-parsed identifier. This also allows expressing some patterns that won't parse as
  // normal identifiers (they may be expressed in DWARF).
  explicit IdentifierGlob(ParsedIdentifier input) : parsed_(std::move(input)) {}

  // An error is returned if the glob could not be parsed. It must be syntactially valid.
  Err Init(const std::string& glob);
  Err Init(const std::string& glob, bool match_any_namespace);

  // When the glob matches the given type, the match score will be returned. Lower scores are
  // better matches (see above).
  //
  // When there is no match, a nullopt will be returned.
  std::optional<int> Matches(const ParsedIdentifier& type) const;

 private:
  bool match_any_namespace_ = true;
  ParsedIdentifier parsed_;
};

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_EXPR_IDENTIFIER_GLOB_H_

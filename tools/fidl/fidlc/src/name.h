// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef TOOLS_FIDL_FIDLC_SRC_NAME_H_
#define TOOLS_FIDL_FIDLC_SRC_NAME_H_

#include <zircon/assert.h>

#include <memory>
#include <optional>
#include <string>
#include <string_view>
#include <utility>
#include <variant>
#include <vector>

#include "tools/fidl/fidlc/src/source_span.h"

namespace fidlc {

class Name;
struct Library;

// A NamingContext is a list of names, from least specific to most specific, which
// identifies the use of a layout. For example, for the FIDL:
//
// ```
// library fuchsia.bluetooth.le;
//
// protocol Peripheral {
//   StartAdvertising(table { 1: data struct {}; });
// };
// ```
//
// The context for the innermost empty struct can be built up by the calls:
//
//   auto ctx = NamingContext("Peripheral").FromRequest("StartAdvertising").EnterMember("data")
//
// `ctx` will produce a `FlattenedName` of "data", and a `Context` of
// ["Peripheral", "StartAdvertising", "data"].
class NamingContext : public std::enable_shared_from_this<NamingContext> {
 public:
  // Usage should only be through shared pointers, so that shared_from_this is always
  // valid. We use shared pointers to manage the lifetime of NamingContexts since the
  // parent pointers need to always be valid. Managing ownership with unique_ptr is tricky
  // (Push() would need to have access to a unique_ptr of this, and there would need to
  // be a place to own all the root nodes, which are not owned by an anonymous name), and
  // doing it manually is even worse.
  static std::shared_ptr<NamingContext> Create(SourceSpan decl_name) {
    return Create(decl_name, Kind::kDecl, nullptr);
  }
  static std::shared_ptr<NamingContext> Create(const Name& decl_name);

  std::shared_ptr<NamingContext> EnterRequest(SourceSpan method_name) {
    ZX_ASSERT_MSG(kind_ == Kind::kDecl, "request must follow protocol");
    return Push(method_name, Kind::kMethodRequest);
  }

  std::shared_ptr<NamingContext> EnterEvent(SourceSpan method_name) {
    ZX_ASSERT_MSG(kind_ == Kind::kDecl, "event must follow protocol");
    // an event is actually a request from the server's perspective, so we use request in the
    // naming context
    return Push(method_name, Kind::kMethodRequest);
  }

  std::shared_ptr<NamingContext> EnterResponse(SourceSpan method_name) {
    ZX_ASSERT_MSG(kind_ == Kind::kDecl, "response must follow protocol");
    return Push(method_name, Kind::kMethodResponse);
  }

  std::shared_ptr<NamingContext> EnterMember(SourceSpan member_name) {
    return Push(member_name, Kind::kLayoutMember);
  }

  SourceSpan name() const { return name_; }

  std::shared_ptr<NamingContext> parent() const {
    ZX_ASSERT_MSG(parent_ != nullptr, "traversing above root");
    return parent_;
  }

  void set_name_override(std::string value) { flattened_name_override_ = std::move(value); }

  std::string_view flattened_name() const {
    if (flattened_name_override_) {
      return flattened_name_override_.value();
    }
    return flattened_name_;
  }
  std::vector<std::string> Context() const;

  // ToName() exists to handle the case where the caller does not necessarily know what
  // kind of name (sourced or anonymous) this NamingContext corresponds to.
  // For example, this happens for layouts where the Consume* functions all take a
  // NamingContext and so the given layout may be at the top level of the library
  // (with a user-specified name) or may be nested/anonymous.
  Name ToName(Library* library, SourceSpan declaration_span);

 private:
  // Each new naming context is represented by a SourceSpan pointing to the name
  // in question (e.g. protocol/layout/member name) and a Kind. The contexts are
  // represented as linked lists with pointers back up to the parent to avoid
  // storing extraneous copies, thus the naming context for
  //
  //   type Foo = { member_a struct { ... }; member_b struct {...}; };
  //
  // Would look like
  //
  //   member_a --\
  //               ---> Foo
  //   member_b --/
  //
  // Note that there are additional constraints not captured in the type system:
  // for example, a kMethodRequest can only follow a kDecl, and a kDecl can only
  // appear as the "root" of a naming context. These are enforced somewhat loosely
  // using asserts in the class' public methods.

  enum class Kind : uint8_t {
    kDecl,
    kLayoutMember,
    kMethodRequest,
    kMethodResponse,
  };

  NamingContext(SourceSpan name, Kind kind, std::shared_ptr<NamingContext> parent)
      : name_(name),
        kind_(kind),
        parent_(std::move(parent)),
        flattened_name_(BuildFlattenedName(name_, kind_, parent_)) {}

  static std::string BuildFlattenedName(SourceSpan name, Kind kind,
                                        const std::shared_ptr<NamingContext>& parent);

  static std::shared_ptr<NamingContext> Create(SourceSpan decl_name, Kind kind,
                                               std::shared_ptr<NamingContext> parent) {
    // We need to create a shared pointer but there are only private constructors. Since
    // we don't care about an extra allocation here, we use `new` to get around this
    // (see https://abseil.io/tips/134)
    return std::shared_ptr<NamingContext>(new NamingContext(decl_name, kind, std::move(parent)));
  }

  std::shared_ptr<NamingContext> Push(SourceSpan name, Kind kind) {
    return Create(name, kind, shared_from_this());
  }

  SourceSpan name_;
  Kind kind_;
  std::shared_ptr<NamingContext> parent_;
  std::string flattened_name_;
  std::optional<std::string> flattened_name_override_;
};

// Name represents the name of a declaration. There are three kinds of names:
// sourced (the usual kind), anonymous (derived from source but chosen by the
// compiler), and intrinsic (not derived from any user source).
class Name final {
 public:
  static Name CreateSourced(const Library* library, SourceSpan span,
                            std::optional<std::string> member_name = std::nullopt) {
    return Name(library, Sourced{span}, std::move(member_name));
  }

  enum class Provenance : uint8_t {
    // The name refers to an anonymous layout, like `struct {}`.
    kAnonymousLayout,
    // The name refers to a result union generated by the compiler, e.g. the
    // response of `strict Foo(A) -> (B) error uint32` or `flexible Foo(A) -> (B)`.
    kGeneratedResultUnion,
    // The name refers to an empty success struct generated by the compiler, e.g. the
    // success variant for `strict Foo() -> () error uint32` or `flexible Foo() -> ()`.
    kGeneratedEmptySuccessStruct,
  };

  static Name CreateAnonymous(const Library* library, SourceSpan span,
                              std::shared_ptr<NamingContext> context, Provenance provenance) {
    return Name(library, Anonymous{std::move(context), provenance, span}, std::nullopt);
  }

  static Name CreateIntrinsic(const Library* library, std::string name) {
    return Name(library, Intrinsic{std::move(name)}, std::nullopt);
  }

  Name WithMemberName(std::string member_name) const {
    ZX_ASSERT_MSG(!member_name_.has_value(), "already has a member name");
    Name new_name = *this;
    new_name.member_name_ = std::move(member_name);
    return new_name;
  }

  const Library* library() const { return library_; }
  std::optional<SourceSpan> span() const;
  std::string_view decl_name() const;
  std::string full_name() const;
  const std::optional<std::string>& member_name() const { return member_name_; }

 private:
  struct Sourced {
    SourceSpan span;
  };

  struct Anonymous {
    std::shared_ptr<NamingContext> context;
    Provenance provenance;
    // The span of the object to which this anonymous name refers to (anonymous names
    // by definition do not appear in source, so the name itself has no span).
    SourceSpan span;
  };

  struct Intrinsic {
    std::string name;
  };

  using Value = std::variant<Sourced, Anonymous, Intrinsic>;

  Name(const Library* library, Value value, std::optional<std::string> member_name)
      : library_(library), value_(std::move(value)), member_name_(std::move(member_name)) {}

  const Library* library_;
  Value value_;
  std::optional<std::string> member_name_;

 public:
  bool is_sourced() const { return std::get_if<Sourced>(&value_); }
  bool is_intrinsic() const { return std::get_if<Intrinsic>(&value_); }
  const Anonymous* as_anonymous() const { return std::get_if<Anonymous>(&value_); }
};

}  // namespace fidlc

#endif  // TOOLS_FIDL_FIDLC_SRC_NAME_H_

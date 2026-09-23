// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fxt/trace_base.h>
#include <zircon/time.h>

#include <any>
#include <optional>
#include <variant>

#include <gtest/gtest.h>

namespace {

using fxt::operator""_category;
using fxt::operator""_intern;

// Reference values for variable arguments.
static const std::tuple kReferenceArgsA = std::make_tuple(FXT_MAKE_ARGUMENT("a", 10));
static const std::tuple kReferenceArgsAB =
    std::make_tuple(FXT_MAKE_ARGUMENT("a", 10), FXT_MAKE_ARGUMENT("b", 20));

// Types for captured variable arguments.
using ArgsTypeA = decltype(kReferenceArgsA);
using ArgsTypeAB = decltype(kReferenceArgsAB);
using VariantType = std::variant<std::monostate, ArgsTypeA, ArgsTypeAB>;

// Types for passthrough arguments.
struct ArgA {};
struct ArgB {};

constexpr bool kConstexprDisabled = false;
constexpr bool kConstexprEnabled = true;
constexpr bool kRuntimeEnabledAlways = true;

// Fake test host providing hooks and state to verify base tracing facilities.
struct FakeTestHost {
  static void ResetLast() {
    last_category = nullptr;
    last_label.reset();
    last_name.reset();
    last_koid = ZX_KOID_INVALID;
    last_obj_type = ZX_OBJ_TYPE_NONE;
    last_arg0.reset();
    last_arg1.reset();
    last_arg2.reset();
    last_va_args.emplace<std::monostate>();
  }

  template <typename T0, typename T1, typename T2, typename... Ts>
  static void EmitEvent(const fxt::InternedCategory& category,
                        fxt::StringRef<fxt::RefType::kId> label, T0 arg0, T1 arg1, T2 arg2,
                        const std::tuple<Ts...>& args) {
    last_category = &category;
    last_label.emplace(label);
    last_arg0 = arg0;
    last_arg1 = arg1;
    last_arg2 = arg2;
    last_va_args.emplace<const std::tuple<Ts...>>(args);
  }

  template <typename... Ts>
  static void EmitKernelObject(zx_koid_t koid, zx_obj_type_t obj_type,
                               fxt::StringRef<fxt::RefType::kInline> name,
                               const std::tuple<Ts...>& args) {
    last_koid = koid;
    last_obj_type = obj_type;
    last_name.emplace(name);
    last_va_args.emplace<const std::tuple<Ts...>>(args);
  }

  static bool CategoryEnabled(const fxt::InternedCategory& category) {
    return &category == enabled_category;
  }
  static bool RuntimeEnabled() { return runtime_enabled; }
  static uint64_t CurrentTime() { return current_time; }

  // Runtime state for category predicate and timestamp functions.
  inline static const fxt::InternedCategory* enabled_category{nullptr};
  inline static bool runtime_enabled{false};
  inline static uint64_t current_time{0};

  // Last observed inputs to emit calls. Different subsets of this state are updated by each emit
  // call.
  inline static const fxt::InternedCategory* last_category{nullptr};
  inline static std::optional<fxt::StringRef<fxt::RefType::kInline>> last_name{};
  inline static std::optional<fxt::StringRef<fxt::RefType::kId>> last_label{};
  inline static zx_koid_t last_koid{ZX_KOID_INVALID};
  inline static zx_obj_type_t last_obj_type{ZX_OBJ_TYPE_NONE};
  inline static std::any last_arg0{};
  inline static std::any last_arg1{};
  inline static std::any last_arg2{};
  inline static VariantType last_va_args{};
};

TEST(Fxt, TraceBaseScope) {
  FakeTestHost::enabled_category = nullptr;
  FakeTestHost::ResetLast();
  EXPECT_EQ(nullptr, FakeTestHost::enabled_category);
  EXPECT_EQ(nullptr, FakeTestHost::last_category);
  EXPECT_FALSE(FakeTestHost::last_label.has_value());
  EXPECT_FALSE(FakeTestHost::last_arg0.has_value());
  EXPECT_FALSE(FakeTestHost::last_arg1.has_value());
  EXPECT_FALSE(FakeTestHost::last_arg2.has_value());
  EXPECT_TRUE(std::holds_alternative<std::monostate>(FakeTestHost::last_va_args));

  // Constexpr disabled.
  {
    // Set the time before the scope begins.
    FakeTestHost::current_time = 1;

    fxt::Scope scope =
        FXT_BEGIN_SCOPE(kConstexprDisabled, kRuntimeEnabledAlways, FakeTestHost::CategoryEnabled,
                        FakeTestHost::CurrentTime, FakeTestHost::EmitEvent, "category",
                        "label"_intern, ArgA{}, ("a", 10), ("b", 20));

    // Advance the time while the scope is active.
    FakeTestHost::current_time = 2;

    // Check that no values have been emitted before the scope ends.
    EXPECT_EQ(nullptr, FakeTestHost::last_category);
    EXPECT_FALSE(FakeTestHost::last_label.has_value());
    EXPECT_FALSE(FakeTestHost::last_arg0.has_value());
    EXPECT_FALSE(FakeTestHost::last_arg1.has_value());
    EXPECT_FALSE(FakeTestHost::last_arg2.has_value());
    EXPECT_TRUE(std::holds_alternative<std::monostate>(FakeTestHost::last_va_args));
  }

  // Check that no values have been emitted after the scope ends.
  EXPECT_EQ(nullptr, FakeTestHost::last_category);
  EXPECT_FALSE(FakeTestHost::last_label.has_value());
  EXPECT_FALSE(FakeTestHost::last_arg0.has_value());
  EXPECT_FALSE(FakeTestHost::last_arg1.has_value());
  EXPECT_FALSE(FakeTestHost::last_arg2.has_value());
  EXPECT_TRUE(std::holds_alternative<std::monostate>(FakeTestHost::last_va_args));

  // Constexpr enabled, category disabled.
  {
    // Set the time before the scope begins.
    FakeTestHost::current_time = 1;

    fxt::Scope scope =
        FXT_BEGIN_SCOPE(kConstexprEnabled, kRuntimeEnabledAlways, FakeTestHost::CategoryEnabled,
                        FakeTestHost::CurrentTime, FakeTestHost::EmitEvent, "category",
                        "label"_intern, ArgA{}, ("a", 10), ("b", 20));

    // Advance the time while the scope is active.
    FakeTestHost::current_time = 2;

    // Check that no values have been emitted before the scope ends.
    EXPECT_EQ(nullptr, FakeTestHost::last_category);
    EXPECT_FALSE(FakeTestHost::last_label.has_value());
    EXPECT_FALSE(FakeTestHost::last_arg0.has_value());
    EXPECT_FALSE(FakeTestHost::last_arg1.has_value());
    EXPECT_FALSE(FakeTestHost::last_arg2.has_value());
    EXPECT_TRUE(std::holds_alternative<std::monostate>(FakeTestHost::last_va_args));
  }

  // Check that no values have been emitted after the scope ends.
  EXPECT_EQ(nullptr, FakeTestHost::last_category);
  EXPECT_FALSE(FakeTestHost::last_label.has_value());
  EXPECT_FALSE(FakeTestHost::last_arg0.has_value());
  EXPECT_FALSE(FakeTestHost::last_arg1.has_value());
  EXPECT_FALSE(FakeTestHost::last_arg2.has_value());
  EXPECT_TRUE(std::holds_alternative<std::monostate>(FakeTestHost::last_va_args));

  // Set the enabled category for the following tests.
  FakeTestHost::enabled_category = &"category"_category;

  // Constexpr enabled, category enabled.
  {
    // Set the time before the scope begins.
    FakeTestHost::current_time = 1;

    fxt::Scope scope =
        FXT_BEGIN_SCOPE(kConstexprEnabled, kRuntimeEnabledAlways, FakeTestHost::CategoryEnabled,
                        FakeTestHost::CurrentTime, FakeTestHost::EmitEvent, "category",
                        "label"_intern, ArgA{}, ("a", 10), ("b", 20));

    // Advance the time while the scope is active.
    FakeTestHost::current_time = 2;

    // Check that no values have been emitted before the scope ends.
    EXPECT_EQ(nullptr, FakeTestHost::last_category);
    EXPECT_FALSE(FakeTestHost::last_label.has_value());
    EXPECT_FALSE(FakeTestHost::last_arg0.has_value());
    EXPECT_FALSE(FakeTestHost::last_arg1.has_value());
    EXPECT_FALSE(FakeTestHost::last_arg2.has_value());
    EXPECT_TRUE(std::holds_alternative<std::monostate>(FakeTestHost::last_va_args));
  }

  // Check that the correct values have been emitted after the scope ends.
  EXPECT_EQ(&"category"_category, FakeTestHost::last_category);
  EXPECT_EQ(fxt::StringRef{"label"_intern}, FakeTestHost::last_label);
  ASSERT_TRUE(std::any_cast<uint64_t>(&FakeTestHost::last_arg0) != nullptr);
  EXPECT_EQ(1u, std::any_cast<uint64_t>(FakeTestHost::last_arg0));
  ASSERT_TRUE(std::any_cast<uint64_t>(&FakeTestHost::last_arg1) != nullptr);
  EXPECT_EQ(2u, std::any_cast<uint64_t>(FakeTestHost::last_arg1));
  EXPECT_TRUE(std::any_cast<ArgA>(&FakeTestHost::last_arg2) != nullptr);
  EXPECT_EQ(VariantType{kReferenceArgsAB}, FakeTestHost::last_va_args);

  FakeTestHost::ResetLast();
  EXPECT_EQ(nullptr, FakeTestHost::last_category);
  EXPECT_FALSE(FakeTestHost::last_label.has_value());
  EXPECT_FALSE(FakeTestHost::last_arg0.has_value());
  EXPECT_FALSE(FakeTestHost::last_arg1.has_value());
  EXPECT_FALSE(FakeTestHost::last_arg2.has_value());
  EXPECT_TRUE(std::holds_alternative<std::monostate>(FakeTestHost::last_va_args));

  // Constexpr enabled, category enabled, scope ends early.
  {
    // Set the time before the scope begins.
    FakeTestHost::current_time = 1;

    fxt::Scope scope =
        FXT_BEGIN_SCOPE(kConstexprEnabled, kRuntimeEnabledAlways, FakeTestHost::CategoryEnabled,
                        FakeTestHost::CurrentTime, FakeTestHost::EmitEvent, "category",
                        "label"_intern, ArgA{}, ("a", 10), ("b", 20));

    // Advance the time while the scope is active.
    FakeTestHost::current_time = 2;

    // Check that no values have been emitted before the scope ends.
    EXPECT_EQ(nullptr, FakeTestHost::last_category);
    EXPECT_FALSE(FakeTestHost::last_label.has_value());
    EXPECT_FALSE(FakeTestHost::last_arg0.has_value());
    EXPECT_FALSE(FakeTestHost::last_arg1.has_value());
    EXPECT_FALSE(FakeTestHost::last_arg2.has_value());
    EXPECT_TRUE(std::holds_alternative<std::monostate>(FakeTestHost::last_va_args));

    scope.End();

    // Check that the correct values have been emitted after the scope ends.
    EXPECT_EQ(&"category"_category, FakeTestHost::last_category);
    EXPECT_EQ(fxt::StringRef{"label"_intern}, FakeTestHost::last_label);
    ASSERT_TRUE(std::any_cast<uint64_t>(&FakeTestHost::last_arg0) != nullptr);
    EXPECT_EQ(1u, std::any_cast<uint64_t>(FakeTestHost::last_arg0));
    ASSERT_TRUE(std::any_cast<uint64_t>(&FakeTestHost::last_arg1) != nullptr);
    EXPECT_EQ(2u, std::any_cast<uint64_t>(FakeTestHost::last_arg1));
    EXPECT_TRUE(std::any_cast<ArgA>(&FakeTestHost::last_arg2) != nullptr);
    EXPECT_EQ(VariantType{kReferenceArgsAB}, FakeTestHost::last_va_args);

    FakeTestHost::ResetLast();
  }
  // Check that no values are emitted after scope is ended early.
  EXPECT_EQ(nullptr, FakeTestHost::last_category);
  EXPECT_FALSE(FakeTestHost::last_label.has_value());
  EXPECT_FALSE(FakeTestHost::last_arg0.has_value());
  EXPECT_FALSE(FakeTestHost::last_arg1.has_value());
  EXPECT_FALSE(FakeTestHost::last_arg1.has_value());
  EXPECT_TRUE(std::holds_alternative<std::monostate>(FakeTestHost::last_va_args));

  // Constexpr enabled, category enabled, scope ends early with arguments.
  {
    // Set the enabled category before the scope begins.
    FakeTestHost::enabled_category = &"category"_category;

    // Set the time before the scope begins.
    FakeTestHost::current_time = 1;

    fxt::Scope scope =
        FXT_BEGIN_SCOPE(kConstexprEnabled, kRuntimeEnabledAlways, FakeTestHost::CategoryEnabled,
                        FakeTestHost::CurrentTime, FakeTestHost::EmitEvent, "category",
                        "label"_intern, ArgA{}, ("a", 10));

    // Advance the time while the scope is active.
    FakeTestHost::current_time = 2;

    // Check that no values have been emitted before the scope ends.
    EXPECT_EQ(nullptr, FakeTestHost::last_category);
    EXPECT_FALSE(FakeTestHost::last_label.has_value());
    EXPECT_FALSE(FakeTestHost::last_arg0.has_value());
    EXPECT_FALSE(FakeTestHost::last_arg1.has_value());
    EXPECT_FALSE(FakeTestHost::last_arg2.has_value());
    EXPECT_TRUE(std::holds_alternative<std::monostate>(FakeTestHost::last_va_args));

    scope = FXT_END_SCOPE(("b", 20));

    // Check that the correct values have been emitted after the scope ends.
    EXPECT_EQ(&"category"_category, FakeTestHost::last_category);
    EXPECT_EQ(fxt::StringRef{"label"_intern}, FakeTestHost::last_label);
    ASSERT_TRUE(std::any_cast<uint64_t>(&FakeTestHost::last_arg0) != nullptr);
    EXPECT_EQ(1u, std::any_cast<uint64_t>(FakeTestHost::last_arg0));
    ASSERT_TRUE(std::any_cast<uint64_t>(&FakeTestHost::last_arg1) != nullptr);
    EXPECT_EQ(2u, std::any_cast<uint64_t>(FakeTestHost::last_arg1));
    EXPECT_TRUE(std::any_cast<ArgA>(&FakeTestHost::last_arg2) != nullptr);
    EXPECT_EQ(VariantType{kReferenceArgsAB}, FakeTestHost::last_va_args);
  }

  FakeTestHost::ResetLast();
  EXPECT_EQ(nullptr, FakeTestHost::last_category);
  EXPECT_FALSE(FakeTestHost::last_label.has_value());
  EXPECT_FALSE(FakeTestHost::last_arg0.has_value());
  EXPECT_FALSE(FakeTestHost::last_arg1.has_value());
  EXPECT_FALSE(FakeTestHost::last_arg2.has_value());
  EXPECT_TRUE(std::holds_alternative<std::monostate>(FakeTestHost::last_va_args));

  // Constexpr enabled, runtime disabled condition, enabled category.
  {
    // Set the enabled category and disabled runtime check before the scope begins.
    FakeTestHost::enabled_category = &"category"_category;
    FakeTestHost::runtime_enabled = false;

    // Set the time before the scope begins.
    FakeTestHost::current_time = 1;

    fxt::Scope scope = FXT_BEGIN_SCOPE(kConstexprEnabled, FakeTestHost::RuntimeEnabled(),
                                       FakeTestHost::CategoryEnabled, FakeTestHost::CurrentTime,
                                       FakeTestHost::EmitEvent, "category", "label"_intern, ArgA{},
                                       ("a", 10), ("b", 20));

    // Advance the time while the scope is active.
    FakeTestHost::current_time = 2;

    // Check that no values have been emitted before the scope ends.
    EXPECT_EQ(nullptr, FakeTestHost::last_category);
    EXPECT_FALSE(FakeTestHost::last_label.has_value());
    EXPECT_FALSE(FakeTestHost::last_arg0.has_value());
    EXPECT_FALSE(FakeTestHost::last_arg1.has_value());
    EXPECT_FALSE(FakeTestHost::last_arg2.has_value());
    EXPECT_TRUE(std::holds_alternative<std::monostate>(FakeTestHost::last_va_args));
  }

  // Check that no values have been emitted after the scope ends.
  EXPECT_EQ(nullptr, FakeTestHost::last_category);
  EXPECT_FALSE(FakeTestHost::last_label.has_value());
  EXPECT_FALSE(FakeTestHost::last_arg0.has_value());
  EXPECT_FALSE(FakeTestHost::last_arg1.has_value());
  EXPECT_FALSE(FakeTestHost::last_arg2.has_value());
  EXPECT_TRUE(std::holds_alternative<std::monostate>(FakeTestHost::last_va_args));

  // Constexpr enabled, runtime enabled condition, enabled category.
  {
    // Set the enabled category and disabled runtime check before the scope begins.
    FakeTestHost::enabled_category = &"category"_category;
    FakeTestHost::runtime_enabled = true;

    // Set the time before the scope begins.
    FakeTestHost::current_time = 1;

    fxt::Scope scope = FXT_BEGIN_SCOPE(kConstexprEnabled, FakeTestHost::RuntimeEnabled(),
                                       FakeTestHost::CategoryEnabled, FakeTestHost::CurrentTime,
                                       FakeTestHost::EmitEvent, "category", "label"_intern, ArgA{},
                                       ("a", 10), ("b", 20));

    // Advance the time while the scope is active.
    FakeTestHost::current_time = 2;

    // Check that no values have been emitted before the scope ends.
    EXPECT_EQ(nullptr, FakeTestHost::last_category);
    EXPECT_FALSE(FakeTestHost::last_label.has_value());
    EXPECT_FALSE(FakeTestHost::last_arg0.has_value());
    EXPECT_FALSE(FakeTestHost::last_arg1.has_value());
    EXPECT_FALSE(FakeTestHost::last_arg2.has_value());
    EXPECT_TRUE(std::holds_alternative<std::monostate>(FakeTestHost::last_va_args));
  }

  // Check that the correct values have been emitted after the scope ends.
  EXPECT_EQ(&"category"_category, FakeTestHost::last_category);
  EXPECT_EQ(fxt::StringRef{"label"_intern}, FakeTestHost::last_label);
  ASSERT_TRUE(std::any_cast<uint64_t>(&FakeTestHost::last_arg0) != nullptr);
  EXPECT_EQ(1u, std::any_cast<uint64_t>(FakeTestHost::last_arg0));
  ASSERT_TRUE(std::any_cast<uint64_t>(&FakeTestHost::last_arg1) != nullptr);
  EXPECT_EQ(2u, std::any_cast<uint64_t>(FakeTestHost::last_arg1));
  EXPECT_TRUE(std::any_cast<ArgA>(&FakeTestHost::last_arg2) != nullptr);
  EXPECT_EQ(VariantType{kReferenceArgsAB}, FakeTestHost::last_va_args);
}

TEST(Fxt, TraceBaseKernelObject) {
  FakeTestHost::enabled_category = nullptr;
  FakeTestHost::ResetLast();
  EXPECT_EQ(nullptr, FakeTestHost::enabled_category);
  EXPECT_FALSE(FakeTestHost::last_name.has_value());
  EXPECT_EQ(ZX_KOID_INVALID, FakeTestHost::last_koid);
  EXPECT_EQ(ZX_OBJ_TYPE_NONE, FakeTestHost::last_obj_type);
  EXPECT_TRUE(std::holds_alternative<std::monostate>(FakeTestHost::last_va_args));

  // Constexpr disabled.
  FXT_KERNEL_OBJECT(false, FakeTestHost::CategoryEnabled, FakeTestHost::EmitKernelObject,
                    "category", 10, ZX_OBJ_TYPE_PROCESS, "name", ("a", 10), ("b", 20));

  EXPECT_FALSE(FakeTestHost::last_name.has_value());
  EXPECT_EQ(ZX_KOID_INVALID, FakeTestHost::last_koid);
  EXPECT_EQ(ZX_OBJ_TYPE_NONE, FakeTestHost::last_obj_type);
  EXPECT_TRUE(std::holds_alternative<std::monostate>(FakeTestHost::last_va_args));

  // Constexpr enabled, category disabled.
  FXT_KERNEL_OBJECT(true, FakeTestHost::CategoryEnabled, FakeTestHost::EmitKernelObject, "category",
                    10, ZX_OBJ_TYPE_PROCESS, "name", ("a", 10), ("b", 20));

  EXPECT_FALSE(FakeTestHost::last_name.has_value());
  EXPECT_EQ(ZX_KOID_INVALID, FakeTestHost::last_koid);
  EXPECT_EQ(ZX_OBJ_TYPE_NONE, FakeTestHost::last_obj_type);
  EXPECT_TRUE(std::holds_alternative<std::monostate>(FakeTestHost::last_va_args));

  // Constexpr enabled, category enabled.
  FakeTestHost::enabled_category = &"category"_category;
  FXT_KERNEL_OBJECT(true, FakeTestHost::CategoryEnabled, FakeTestHost::EmitKernelObject, "category",
                    10, ZX_OBJ_TYPE_PROCESS, "name", ("a", 10), ("b", 20));

  EXPECT_EQ(fxt::StringRef{"name"}, FakeTestHost::last_name);
  EXPECT_EQ(10u, FakeTestHost::last_koid);
  EXPECT_EQ(ZX_OBJ_TYPE_PROCESS, FakeTestHost::last_obj_type);
  EXPECT_EQ(VariantType{kReferenceArgsAB}, FakeTestHost::last_va_args);

  FakeTestHost::ResetLast();
  EXPECT_FALSE(FakeTestHost::last_name.has_value());
  EXPECT_EQ(ZX_KOID_INVALID, FakeTestHost::last_koid);
  EXPECT_EQ(ZX_OBJ_TYPE_NONE, FakeTestHost::last_obj_type);
  EXPECT_TRUE(std::holds_alternative<std::monostate>(FakeTestHost::last_va_args));

  // Constexpr disabled, no category.
  FXT_KERNEL_OBJECT_ALWAYS(false, FakeTestHost::EmitKernelObject, 10, ZX_OBJ_TYPE_PROCESS, "name",
                           ("a", 10), ("b", 20));

  EXPECT_FALSE(FakeTestHost::last_name.has_value());
  EXPECT_EQ(ZX_KOID_INVALID, FakeTestHost::last_koid);
  EXPECT_EQ(ZX_OBJ_TYPE_NONE, FakeTestHost::last_obj_type);
  EXPECT_TRUE(std::holds_alternative<std::monostate>(FakeTestHost::last_va_args));

  // Constexpr enabled, no category.
  FXT_KERNEL_OBJECT_ALWAYS(true, FakeTestHost::EmitKernelObject, 10, ZX_OBJ_TYPE_PROCESS, "name",
                           ("a", 10), ("b", 20));

  EXPECT_EQ(fxt::StringRef{"name"}, FakeTestHost::last_name);
  EXPECT_EQ(10u, FakeTestHost::last_koid);
  EXPECT_EQ(ZX_OBJ_TYPE_PROCESS, FakeTestHost::last_obj_type);
  EXPECT_EQ(VariantType{kReferenceArgsAB}, FakeTestHost::last_va_args);
}

TEST(Fxt, TraceBaseEventCommon) {
  FakeTestHost::enabled_category = nullptr;
  FakeTestHost::ResetLast();
  EXPECT_EQ(nullptr, FakeTestHost::enabled_category);
  EXPECT_EQ(nullptr, FakeTestHost::last_category);
  EXPECT_FALSE(FakeTestHost::last_label.has_value());
  EXPECT_FALSE(FakeTestHost::last_arg0.has_value());
  EXPECT_FALSE(FakeTestHost::last_arg1.has_value());
  EXPECT_FALSE(FakeTestHost::last_arg2.has_value());
  EXPECT_TRUE(std::holds_alternative<std::monostate>(FakeTestHost::last_va_args));

  // Constexpr disabled.
  FakeTestHost::current_time = 1;
  FXT_EVENT_COMMON(false, FakeTestHost::CategoryEnabled, FakeTestHost::EmitEvent, "category",
                   "label"_intern, FakeTestHost::CurrentTime(), ArgA{}, ArgB{}, ("a", 10),
                   ("b", 20));

  EXPECT_EQ(nullptr, FakeTestHost::last_category);
  EXPECT_FALSE(FakeTestHost::last_label.has_value());
  EXPECT_FALSE(FakeTestHost::last_arg0.has_value());
  EXPECT_FALSE(FakeTestHost::last_arg1.has_value());
  EXPECT_FALSE(FakeTestHost::last_arg2.has_value());
  EXPECT_TRUE(std::holds_alternative<std::monostate>(FakeTestHost::last_va_args));

  // Constexpr enabled, category disabled.
  FakeTestHost::current_time = 2;
  FXT_EVENT_COMMON(true, FakeTestHost::CategoryEnabled, FakeTestHost::EmitEvent, "category",
                   "label"_intern, FakeTestHost::CurrentTime(), ArgA{}, ArgB{}, ("a", 10),
                   ("b", 20));

  EXPECT_EQ(nullptr, FakeTestHost::last_category);
  EXPECT_FALSE(FakeTestHost::last_label.has_value());
  EXPECT_FALSE(FakeTestHost::last_arg0.has_value());
  EXPECT_FALSE(FakeTestHost::last_arg1.has_value());
  EXPECT_FALSE(FakeTestHost::last_arg2.has_value());
  EXPECT_TRUE(std::holds_alternative<std::monostate>(FakeTestHost::last_va_args));

  // Constexpr enabled, category enabled.
  FakeTestHost::enabled_category = &"category"_category;
  FakeTestHost::current_time = 3;
  FXT_EVENT_COMMON(true, FakeTestHost::CategoryEnabled, FakeTestHost::EmitEvent, "category",
                   "label"_intern, FakeTestHost::CurrentTime(), ArgA{}, ArgB{}, ("a", 10),
                   ("b", 20));

  EXPECT_EQ(&"category"_category, FakeTestHost::last_category);
  EXPECT_EQ(fxt::StringRef{"label"_intern}, FakeTestHost::last_label);
  ASSERT_TRUE(std::any_cast<uint64_t>(&FakeTestHost::last_arg0) != nullptr);
  EXPECT_EQ(3u, std::any_cast<uint64_t>(FakeTestHost::last_arg0));
  EXPECT_TRUE(std::any_cast<ArgA>(&FakeTestHost::last_arg1) != nullptr);
  EXPECT_TRUE(std::any_cast<ArgB>(&FakeTestHost::last_arg2) != nullptr);
  EXPECT_EQ(VariantType{kReferenceArgsAB}, FakeTestHost::last_va_args);

  FakeTestHost::ResetLast();
  EXPECT_EQ(nullptr, FakeTestHost::last_category);
  EXPECT_FALSE(FakeTestHost::last_label.has_value());
  EXPECT_FALSE(FakeTestHost::last_arg0.has_value());
  EXPECT_FALSE(FakeTestHost::last_arg1.has_value());
  EXPECT_FALSE(FakeTestHost::last_arg2.has_value());
  EXPECT_TRUE(std::holds_alternative<std::monostate>(FakeTestHost::last_va_args));

  // Constexpr enabled, category enabled, new timestamp.
  FakeTestHost::current_time = 4;
  FXT_EVENT_COMMON(true, FakeTestHost::CategoryEnabled, FakeTestHost::EmitEvent, "category",
                   "label"_intern, FakeTestHost::CurrentTime(), ArgA{}, ArgB{}, ("a", 10),
                   ("b", 20));

  EXPECT_EQ(&"category"_category, FakeTestHost::last_category);
  EXPECT_EQ(fxt::StringRef{"label"_intern}, FakeTestHost::last_label);
  ASSERT_TRUE(std::any_cast<uint64_t>(&FakeTestHost::last_arg0) != nullptr);
  EXPECT_EQ(4u, std::any_cast<uint64_t>(FakeTestHost::last_arg0));
  EXPECT_TRUE(std::any_cast<ArgA>(&FakeTestHost::last_arg1) != nullptr);
  EXPECT_TRUE(std::any_cast<ArgB>(&FakeTestHost::last_arg2) != nullptr);
  EXPECT_EQ(VariantType{kReferenceArgsAB}, FakeTestHost::last_va_args);
}

}  // anonymous namespace

// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef TOOLS_FIDL_FIDLC_SRC_CONSTRAINTS_H_
#define TOOLS_FIDL_FIDLC_SRC_CONSTRAINTS_H_

#include <cstdint>
#include <optional>

#include "tools/fidl/fidlc/src/values.h"

namespace fidlc {
class Reporter;
}

namespace fidlc {

class TypeResolver;
struct Resource;
struct Protocol;
struct LayoutInvocation;

// Kinds of constraints that can be applied to types
enum class ConstraintKind : uint8_t {
  kHandleSubtype,
  kHandleRights,
  kSize,
  kNullability,
  kProtocol,
  kUtf8,
};

// Forward declarations.
template <ConstraintKind K>
class Constraint;
template <ConstraintKind K>
bool MergeConstraint(Reporter* reporter, const Name& layout_name, const Constraint<K>& base,
                     const Constraint<K>& resolved, Constraint<K>* out_merged);

// Base class for all ConstraintStorage overrides.
struct ConstraintStorageBase {
  // Called when a merge conflict occurs. This allows ConstraintKind specific error reporting.
  virtual bool ReportMergeFailure(Reporter* reporter, const Name& layout_name,
                                  const Constant* param) const;

  // Try to resolve a particular constant param into this constraint value.
  virtual bool ResolveConstraint(TypeResolver* resolver, Constant* param, Resource* resource) = 0;
};

// ConstraintStorage is implemented for each ConstraintKind. It holds the constraint value, default
// value, how it maps onto LayoutInvocation and the ResolveConstraint() function that knows how to
// resolve the constraint value.
// Through inheritance, the member variable for the constraint value will be visible on Type objects
// that have this constraint.
template <ConstraintKind K>
struct ConstraintStorage;

// Constraint wraps ConstraintStorage to build higher-level functionality shared by all constraints
// but depending on things defined in ConstraintStorage.
template <ConstraintKind K>
class Constraint : public ConstraintStorage<K> {
 public:
  // The type used to actually hold the value of the constraint.
  using typename ConstraintStorage<K>::ValueType;

  Constraint() = default;
  explicit Constraint(ValueType value) { *ValuePtr() = value; }

  // Try to resolve this constraint against the Constant param, and if it resolves store the
  // constant as the raw constraint.
  bool ResolveConstraint(TypeResolver* resolver, Constant* param, Resource* resource) override {
    bool resolved = ConstraintStorage<K>::ResolveConstraint(resolver, param, resource);
    if (resolved) {
      raw_constraint = param;
    }
    return resolved;
  }

  // Returns true if this has a non-default value.
  bool HasConstraint() const { return *ValuePtr() != ConstraintStorage<K>::kDefault; }

  // Fill a LayoutInvocation object with the value, and optionally raw value of this constraint.
  void PopulateLayoutInvocation(LayoutInvocation* layout_invocation);

 private:
  // Get a pointer to the address of the value of this constraint within the ConstraintStorage
  // struct.
  ValueType* ValuePtr() { return &(this->*ConstraintStorage<K>::kValuePtr); }
  const ValueType* ValuePtr() const { return &(this->*ConstraintStorage<K>::kValuePtr); }

  // The AST node where the constraint value came from. This is sometimes absent.
  const Constant* raw_constraint = nullptr;

  friend bool MergeConstraint<K>(Reporter* reporter, const Name& layout_name,
                                 const Constraint<K>& base, const Constraint<K>& resolved,
                                 Constraint<K>* out_merged);
};

template <>
struct ConstraintStorage<ConstraintKind::kHandleSubtype> : public ConstraintStorageBase {
  // The type of handle subtype constraints.
  using ValueType = HandleSubtype;

  // The default handle subtype.
  static constexpr ValueType kDefault = HandleSubtype::kHandle;

  // The handle subtype is available as |subtype| on types that have this constraint, such as
  // |HandleType|.
  ValueType subtype = kDefault;

  // Member pointer to the |subtype| handle field.
  static constexpr ValueType ConstraintStorage::*kValuePtr = &ConstraintStorage::subtype;

  // Member pointer to |LayoutInvocation.subtype_resolved|.
  static ValueType LayoutInvocation::*kLayoutInvocationValue;
  // Member pointer to |LayoutInvocation.subtype_raw|.
  static const Constant* LayoutInvocation::*kLayoutInvocationRaw;

  // Try to a |Constant| constraint param to a |HandleSubtype| and set it.
  bool ResolveConstraint(TypeResolver* resolver, Constant* param, Resource* resource) override;
};

template <>
struct ConstraintStorage<ConstraintKind::kHandleRights> : public ConstraintStorageBase {
  using ValueType = const HandleRightsValue*;
  static const ValueType kDefault;

  ValueType rights = kDefault;
  static constexpr ValueType ConstraintStorage::*kValuePtr = &ConstraintStorage::rights;
  static ValueType LayoutInvocation::*kLayoutInvocationValue;
  static const Constant* LayoutInvocation::*kLayoutInvocationRaw;

  bool ResolveConstraint(TypeResolver* resolver, Constant* param, Resource* resource) override;
};

template <>
struct ConstraintStorage<ConstraintKind::kSize> : public ConstraintStorageBase {
  using ValueType = const SizeValue*;
  static const ValueType kDefault;

  ValueType size = kDefault;
  static constexpr ValueType ConstraintStorage::*kValuePtr = &ConstraintStorage::size;
  static ValueType LayoutInvocation::*kLayoutInvocationValue;
  static const Constant* LayoutInvocation::*kLayoutInvocationRaw;

  bool ResolveConstraint(TypeResolver* resolver, Constant* param, Resource* resource) override;

  bool ReportMergeFailure(Reporter* reporter, const Name& layout_name,
                          const Constant* param) const override;
};

template <>
struct ConstraintStorage<ConstraintKind::kNullability> : public ConstraintStorageBase {
  using ValueType = Nullability;
  static constexpr ValueType kDefault = ValueType::kNonnullable;

  ValueType nullability = kDefault;
  static constexpr ValueType ConstraintStorage::*kValuePtr = &ConstraintStorage::nullability;
  static ValueType LayoutInvocation::*kLayoutInvocationValue;
  static const Constant* LayoutInvocation::*kLayoutInvocationRaw;

  bool ResolveConstraint(TypeResolver* resolver, Constant* param, Resource* resource) override;

  bool ReportMergeFailure(Reporter* reporter, const Name& layout_name,
                          const Constant* param) const override;
};

template <>
struct ConstraintStorage<ConstraintKind::kProtocol> : public ConstraintStorageBase {
  using ValueType = const Protocol*;
  static constexpr ValueType kDefault = nullptr;

  ValueType protocol_decl = kDefault;
  static constexpr ValueType ConstraintStorage::*kValuePtr = &ConstraintStorage::protocol_decl;
  static ValueType LayoutInvocation::*kLayoutInvocationValue;
  static const Constant* LayoutInvocation::*kLayoutInvocationRaw;

  bool ResolveConstraint(TypeResolver* resolver, Constant* param, Resource* resource) override;
};

template <>
struct ConstraintStorage<ConstraintKind::kUtf8> : public ConstraintStorageBase {
  using ValueType = bool;
  static constexpr ValueType kDefault = false;

  ValueType utf8 = kDefault;
  static constexpr ValueType ConstraintStorage::*kValuePtr = &ConstraintStorage::utf8;
  static ValueType LayoutInvocation::*kLayoutInvocationValue;
  static const Constant* LayoutInvocation::*kLayoutInvocationRaw;

  bool ResolveConstraint(TypeResolver* resolver, Constant* param, Resource* resource) override;

  bool ReportMergeFailure(Reporter* reporter, const Name& layout_name,
                          const Constant* param) const override;
};

// Merge two |Constraint| objects of the same |ConstraintKind|.
// It's an error for them to both be set.
template <ConstraintKind K>
bool MergeConstraint(Reporter* reporter, const Name& layout_name, const Constraint<K>& base,
                     const Constraint<K>& resolved, Constraint<K>* out_merged) {
  if (base.HasConstraint() && resolved.HasConstraint()) {
    return resolved.ReportMergeFailure(reporter, layout_name, resolved.raw_constraint);
  }
  if (resolved.HasConstraint()) {
    *out_merged->ValuePtr() = *resolved.ValuePtr();
    out_merged->raw_constraint = resolved.raw_constraint;
  } else {
    *out_merged->ValuePtr() = *base.ValuePtr();
    out_merged->raw_constraint = base.raw_constraint;
  }
  return true;
}

// Base type for the |Constraints| struct.
struct ConstraintsBase {
  virtual bool OnUnexpectedConstraint(TypeResolver* resolver, Reporter* reporter,
                                      std::optional<SourceSpan> params_span,
                                      const Name& layout_name, Resource* resource,
                                      size_t num_constraints,
                                      const std::vector<std::unique_ptr<Constant>>& params,
                                      size_t param_index) const;
};

// |Constraints| holds multiple kinds of constraints (indicated by template parameters) and provides
// functions that operate over them. |Type|s can inherit from |Constraints| directly or inherit from
// a subclass that customizes error reporting.
template <ConstraintKind... Ks>
struct Constraints : public Constraint<Ks>..., public ConstraintsBase {
  Constraints() = default;
  Constraints(const Constraints&) = default;
  Constraints(Constraints&&) noexcept = default;

  // Construct a constraints instance with all constraint values supplied.
  explicit Constraints(typename Constraint<Ks>::ValueType... arg) : Constraint<Ks>(arg)... {}

  // Construct a constraint instance with values from another constraint instance that has a subset
  // of constraints.
  template <ConstraintKind... OKs>
  explicit Constraints(const Constraints<OKs...>& other) : Constraint<OKs>(other)... {
    static_assert(CanConstrainAll<OKs...>());
  }

  // There is a specialization below for |Constraints<>|.
  static_assert(sizeof...(Ks) > 0);

  // Check if this a particular constraint |K| has a non-default value.
  template <ConstraintKind K>
  bool HasConstraint() const {
    return Constraint<K>::HasConstraint();
  }

  // Resolve the constraints in the params, then optionally populate the layout invocation and merge
  // the resolved constraints with the ones in this object.
  template <typename M>
  bool ResolveAndMergeConstraints(TypeResolver* resolver, Reporter* reporter,
                                  std::optional<SourceSpan> params_span, const Name& layout_name,
                                  Resource* resource,
                                  const std::vector<std::unique_ptr<Constant>>& params,
                                  M* out_merged,
                                  LayoutInvocation* layout_invocation = nullptr) const {
    static_assert(std::is_base_of_v<std::remove_reference_t<decltype(*this)>, M>);
    M resolved;
    if (!resolved.ResolveConstraints(resolver, reporter, params_span, layout_name, resource,
                                     params)) {
      return false;
    }
    if (layout_invocation) {
      resolved.PopulateLayoutInvocation(layout_invocation);
    }
    if (out_merged) {
      return M::MergeConstraints(reporter, layout_name, *this, resolved, out_merged);
    }
    return true;
  }

 private:
  // Resolve all of the constraints against the params supplied into this object.
  bool ResolveConstraints(TypeResolver* resolver, Reporter* reporter,
                          std::optional<SourceSpan> params_span, const Name& layout_name,
                          Resource* resource,
                          const std::vector<std::unique_ptr<Constant>>& params) {
    size_t num_params = params.size();
    constexpr size_t num_constraints = sizeof...(Ks);

    size_t constraint_index = 0;

    // For each param supplied...
    for (size_t param_index = 0; param_index < num_params; param_index++) {
      // Walk through the next constraint to see if one can resolve.
      while (constraint_index < num_constraints &&
             !ResolveOneConstraint(resolver, layout_name, resource, constraint_index,
                                   params[param_index].get())) {
        constraint_index++;
      }
      if (constraint_index == num_constraints) {
        // Ran out of constraint kinds trying to match this item.
        return OnUnexpectedConstraint(resolver, reporter, params_span, layout_name, resource,
                                      num_constraints, params, param_index);
      }
      constraint_index++;
    }
    return true;
  }

  // Merge resolved constraints onto base constraints.
  // This is a recursive template that's applied to each constraint in order.
  template <size_t I = 0>
  static bool MergeConstraints(Reporter* reporter, const Name& layout_name, const Constraints& base,
                               const Constraints& resolved, Constraints* out_merged) {
    if constexpr (I < sizeof...(Ks)) {
      constexpr auto K = std::get<I>(std::make_tuple(Ks...));
      if (!MergeConstraint<K>(reporter, layout_name, base, resolved, out_merged)) {
        return false;
      }
      return MergeConstraints<I + 1>(reporter, layout_name, base, resolved, out_merged);
    } else {
      return true;
    }
  }

  // Populate a LayoutInvocation based on these constraints.
  // This is a recursive template that's applied to each constraint in order.
  template <size_t I = 0>
  bool PopulateLayoutInvocation(LayoutInvocation* layout_invocation) {
    if constexpr (I < sizeof...(Ks)) {
      constexpr auto K = std::get<I>(std::make_tuple(Ks...));
      using C = Constraint<K>;
      C::PopulateLayoutInvocation(layout_invocation);
      return PopulateLayoutInvocation<I + 1>(layout_invocation);
    } else {
      return true;
    }
  }

  // Call |ResolveConstraint| for the constraint Ks[constraint_index].
  template <size_t I = 0>
  bool ResolveOneConstraint(TypeResolver* resolver, const Name& layout_name, Resource* resource,
                            size_t constraint_index, Constant* param) {
    static_assert(I < sizeof...(Ks));
    if (I == constraint_index) {
      constexpr auto K = std::get<I>(std::make_tuple(Ks...));
      using C = Constraint<K>;
      return C::ResolveConstraint(resolver, param, resource);
    }

    if constexpr ((I + 1) < sizeof...(Ks)) {
      return ResolveOneConstraint<I + 1>(resolver, layout_name, resource, constraint_index, param);
    } else {
      ZX_PANIC("Constraint %zu not found.", constraint_index);
    }
  }

  // Check if a particular ConstraintKind is included in this type.
  // This uses pretty straightforward template recursion to walk through each of the template
  // arguments.
  template <ConstraintKind K, size_t I = 0>
  static constexpr bool CanConstrain() {
    if constexpr (I < sizeof...(Ks)) {
      constexpr ConstraintKind ki = std::get<I>(std::make_tuple(Ks...));
      if constexpr (K == ki) {
        return true;
      } else {
        return CanConstrain<K, I + 1>();
      }
    } else {
      return false;
    }
  }

  // Check if all of the supplied ConstraintKinds are included in this type.
  template <ConstraintKind... OKs>
  static constexpr bool CanConstrainAll() {
    return CanConstrainAllImpl<0, OKs...>();
  }

  // Recursive helper for CanConstrainAll.
  // This uses pretty straightforward template recursion to walk through each of the template
  // arguments.
  template <size_t I, ConstraintKind... OKs>
  static constexpr bool CanConstrainAllImpl() {
    if constexpr (I < sizeof...(OKs)) {
      constexpr ConstraintKind oki = std::get<I>(std::make_tuple(OKs...));
      if constexpr (!CanConstrain<oki>()) {
        return false;
      } else {
        return CanConstrainAllImpl<I + 1, OKs...>();
      }
    } else {
      return true;
    }
  }
};

// Constraints specialization for when there are no constraints.
template <>
struct Constraints<> : ConstraintsBase {
  Constraints() = default;
  Constraints(const Constraints&) = default;
  Constraints(Constraints&&) noexcept = default;

  bool ResolveAndMergeConstraints(TypeResolver* resolver, Reporter* reporter,
                                  std::optional<SourceSpan> params_span, const Name& layout_name,
                                  Resource* resource,
                                  const std::vector<std::unique_ptr<Constant>>& params,
                                  nullptr_t out_merged,
                                  LayoutInvocation* layout_invocation = nullptr) const {
    if (!params.empty()) {
      return OnUnexpectedConstraint(resolver, reporter, params_span, layout_name, resource, 0,
                                    params, 0);
    }
    return true;
  }
};

}  // namespace fidlc

#endif  // TOOLS_FIDL_FIDLC_SRC_CONSTRAINTS_H_

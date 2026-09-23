// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/expr/resolve_function.h"

#include "src/developer/debug/shared/logging/logging.h"
#include "src/developer/debug/zxdb/common/err.h"
#include "src/developer/debug/zxdb/common/err_or.h"
#include "src/developer/debug/zxdb/common/join_callbacks.h"
#include "src/developer/debug/zxdb/expr/cast.h"
#include "src/developer/debug/zxdb/expr/eval_context.h"
#include "src/developer/debug/zxdb/expr/find_name.h"
#include "src/developer/debug/zxdb/expr/found_name.h"
#include "src/developer/debug/zxdb/symbols/base_type.h"
#include "src/developer/debug/zxdb/symbols/dwarf_tag.h"
#include "src/developer/debug/zxdb/symbols/function.h"
#include "src/developer/debug/zxdb/symbols/function_call_info.h"
#include "src/developer/debug/zxdb/symbols/input_location.h"
#include "src/developer/debug/zxdb/symbols/process_symbols.h"
#include "src/developer/debug/zxdb/symbols/target_symbols.h"
#include "src/developer/debug/zxdb/symbols/type.h"
#include "src/developer/debug/zxdb/symbols/variable.h"
#include "src/lib/fxl/memory/ref_ptr.h"

namespace zxdb {

namespace {
bool TypeIsPointerOrArray(const Type* type) {
  return DwarfTagIsPointerOrReference(type->tag()) || !!type->As<ArrayType>();
}

bool ImplicitTypeCastIsSupported(const Type* desired, const Type* given) {
  // Only cast to a pointer if the given type is another pointer or an array (implying a pointer to
  // the first element of the array). We will not implicitly convert an integer to a pointer (a user
  // can manually cast it themselves if that's what they really want). CastExprValue below will do
  // more robust checking of the pointed to types that we do not need to replicate here, but will
  // allow any random integer to be cast to a pointer, which we don't want unless the user casts it
  // themselves.
  if (DwarfTagIsPointerOrReference(desired->tag())) {
    if (!TypeIsPointerOrArray(given)) {
      return false;
    }
  }

  return true;
}

// Validate the given arguments against the types the function expects. We do some simple coercion
// and casting to get the right types from what the user gave us. If all of the attempts fail the
// callback will be issued reentrantly with an appropriate error. If the argument requires a full
// cast that cannot be done synchronously, then an asynchronous cast will be kicked off. Preserving
// the order of the arguments is important.
void CollectArguments(const fxl::RefPtr<EvalContext>& eval_context, std::string_view fn_name,
                      const std::vector<LazySymbol>& expected_types,
                      const std::vector<ExprValue>& given_args,
                      fit::callback<void(ErrOr<std::vector<ExprValue>>)> cb) {
  if (expected_types.size() != given_args.size()) {
    return cb(
        Err("Function expects %zu arguments, got %zu", expected_types.size(), given_args.size()));
  }

  std::vector<ExprValue> results;
  results.resize(given_args.size());
  std::map<size_t, ExprValue> arg_map;

  // This will collect the casted to types from below.
  auto joiner = fxl::MakeRefCounted<JoinCallbacks<ErrOrValue>>();

  for (size_t i = 0; i < expected_types.size(); i++) {
    const Variable* var = expected_types[i].Get()->As<Variable>();

    // This is the type the function expects. The corresponding ExprValue in |given_args| should
    // either match this type or be castable to this type.
    fxl::RefPtr<Type> desired_type = eval_context->GetConcreteType(var->type());

    if (!desired_type) {
      joiner->Abandon();
      return cb(Err("Could not determine type of %s", var->GetAssignedName().c_str()));
    }

    if (desired_type->As<BaseType>()) {
      if (desired_type->As<BaseType>()->base_type() == BaseType::kBaseTypeFloat) {
        joiner->Abandon();
        return cb(Err("Floating point arguments are not yet supported!"));
      }
    }

    // Make sure the given parameter is supported.
    if (given_args[i].type()->As<BaseType>() &&
        given_args[i].type()->As<BaseType>()->base_type() == BaseType::kBaseTypeFloat) {
      joiner->Abandon();
      return cb(Err("Floating point arguments are not supported yet."));
    }

    // This is a bit tricky. The debugger can be helpful by forcing the given argument into the type
    // the function expects, but if we are _too_ helpful, then we could end up calling a function
    // with an argument that it was not expecting and produce a bogus result, or cause the program
    // to crash. CastExprValue is powerful and will be more lenient than we can allow, so we have
    // some restrictions to apply in ImplicitTypeCastIsSupported. For passing arrays as pointers, we
    // can construct a new ExprValue with the address of the first element of the array.
    // CastExprValue will refuse to do this, so it is done manually here.
    if (DwarfTagIsPointerOrReference(desired_type->tag()) &&
        given_args[i].type()->As<ArrayType>() &&
        given_args[i].source().type() == ExprValueSource::Type::kMemory) {
      arg_map[i] =
          ExprValue(given_args[i].source().address(), desired_type, given_args[i].source());
    } else if (ImplicitTypeCastIsSupported(desired_type.get(), given_args[i].type())) {
      CastExprValue(eval_context, CastType::kImplicit, given_args[i], desired_type,
                    given_args[i].source(),
                    [&arg_map, i, cb = joiner->AddCallback()](ErrOrValue result) mutable {
                      if (result.ok())
                        arg_map[i] = result.take_value();
                      cb(result);
                    });
    } else {
      joiner->Abandon();
      return cb(Err("Refusing to implicitly cast from %s to %s, try casting explicitly.",
                    given_args[i].type()->GetFullName().c_str(),
                    desired_type->GetFullName().c_str()));
    }
  }

  joiner->Ready(
      [results, arg_map, cb = std::move(cb)](const std::vector<ErrOrValue>& cast_results) mutable {
        // If any of the casts failed, we cannot continue.
        if (auto err_it = std::find_if(cast_results.begin(), cast_results.end(),
                                       [](const ErrOrValue& result) { return result.has_error(); });
            err_it != cast_results.end()) {
          return cb(err_it->err());
        }

        for (auto& [i, expr_value] : arg_map) {
          results[i] = std::move(expr_value);
        }

        cb(results);
      });

  // return results;
}
}  // namespace

void ResolveFunction(const fxl::RefPtr<EvalContext>& eval_context, const ParsedIdentifier& fn_name,
                     const std::vector<ExprValue>& params,
                     fit::callback<void(ErrOr<FunctionCallInfo>)> cb) {
  // FindName should be able to find the identifier in any context, regardless of where the current
  // execution is. This includes any namespaces that the function might live in.
  std::vector<FoundName> found_names;

  FindNameOptions opts(FindNameOptions::kNoKinds);
  opts.find_functions = true;

  eval_context->FindName(opts, fn_name, &found_names);
  if (found_names.size() > 1) {
    // TODO(https://fxbug.dev/42132103): Handle overloads.
    return cb(Err(fn_name.GetFullName() +
                  " resolved to multiple locations. Overloaded functions are not supported yet."));
  }

  auto found = found_names[0];

  if (!found.function()) {
    // Print a more helpful error message if something was found but it wasn't a
    // function.
    if (found.is_found()) {
      return cb(Err(std::string("Identifier was not function: ") + found.GetName().GetFullName() +
                    " " + FoundName::KindToString(found.kind())));
    }

    return cb(Err(fn_name.GetFullName() + " was not found in this context."));
  }

  auto fn_ref = found.function();

  // None of the locations actually resolved to the function or the function was inlined, return an
  // error.
  if (!fn_ref) {
    return cb(Err("Error casting symbol " + fn_name.GetDebugName() + " to function."));
  } else if (fn_ref->is_inline()) {
    return cb(
        Err(fn_ref->GetFullName() + " has been inlined, and cannot be called from the debugger."));
  }

  FunctionCallInfo call_info;
  call_info.fn = fn_ref;

  CollectArguments(
      eval_context, fn_ref->GetAssignedName(), fn_ref->parameters(), params,
      [call_info, cb = std::move(cb)](ErrOr<std::vector<ExprValue>> err_or_args) mutable {
        if (err_or_args.has_error()) {
          return cb(err_or_args.err());
        }

        call_info.parameters = err_or_args.take_value();

        cb(call_info);
      });
}

}  // namespace zxdb

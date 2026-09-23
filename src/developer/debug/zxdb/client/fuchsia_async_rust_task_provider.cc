// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/client/fuchsia_async_rust_task_provider.h"

#include <algorithm>
#include <string_view>

#include "src/developer/debug/shared/message_loop.h"
#include "src/developer/debug/shared/string_util.h"
#include "src/developer/debug/zxdb/client/frame.h"
#include "src/developer/debug/zxdb/client/session.h"
#include "src/developer/debug/zxdb/common/file_util.h"
#include "src/developer/debug/zxdb/common/join_callbacks.h"
#include "src/developer/debug/zxdb/common/string_util.h"
#include "src/developer/debug/zxdb/expr/cast.h"
#include "src/developer/debug/zxdb/expr/expr.h"
#include "src/developer/debug/zxdb/expr/found_member.h"
#include "src/developer/debug/zxdb/expr/resolve_collection.h"
#include "src/developer/debug/zxdb/expr/resolve_ptr_ref.h"
#include "src/developer/debug/zxdb/expr/resolve_variant.h"
#include "src/developer/debug/zxdb/symbols/collection.h"
#include "src/developer/debug/zxdb/symbols/data_member.h"
#include "src/developer/debug/zxdb/symbols/function.h"
#include "src/developer/debug/zxdb/symbols/identifier.h"
#include "src/developer/debug/zxdb/symbols/modified_type.h"
#include "src/developer/debug/zxdb/symbols/template_parameter.h"
#include "src/lib/fxl/memory/ref_ptr.h"
#include "src/lib/fxl/strings/split_string.h"

namespace zxdb {

namespace {

// Concrete implementation of AsyncTask for fuchsia-async Rust.
class FuchsiaAsyncRustTask : public AsyncTask {
 public:
  FuchsiaAsyncRustTask(Session* session, uint64_t id, Location location, Identifier identifier,
                       std::string state, AsyncTask::Type type)
      : AsyncTask(session),
        id_(id),
        location_(std::move(location)),
        identifier_(std::move(identifier)),
        state_(std::move(state)),
        type_(type) {}

  uint64_t GetId() const override { return id_; }
  AsyncTask::Type GetType() const override { return type_; }
  const Location& GetLocation() const override { return location_; }
  const Identifier& GetIdentifier() const override { return identifier_; }
  std::string GetState() const override { return state_; }
  const std::vector<NamedValue>& GetValues() const override { return values_; }
  std::vector<AsyncTask::Ref> GetChildren() const override {
    std::vector<AsyncTask::Ref> ret;
    ret.reserve(children_.size());
    for (const auto& child : children_) {
      ret.push_back(*child);
    }
    return ret;
  }

  void AddChild(std::unique_ptr<AsyncTask> child) { children_.push_back(std::move(child)); }
  void AddNamedValue(std::optional<std::string> name, ExprValue value) {
    values_.push_back({.name = std::move(name), .value = std::move(value)});
  }

 private:
  uint64_t id_;
  Location location_;
  Identifier identifier_;
  std::string state_;
  AsyncTask::Type type_;
  std::vector<NamedValue> values_;
  std::vector<std::unique_ptr<AsyncTask>> children_;
};

std::string_view StripTemplate(std::string_view type_name) {
  return type_name.substr(0, type_name.find('<'));
}

bool IsAsyncFunctionOrBlock(Type* type) {
  if (type->GetIdentifier().components().empty())
    return false;
  return debug::StringStartsWith(type->GetIdentifier().components().back().name(), "{async_");
}

Err MakeError(std::string msg, const Err& err = Err()) {
  if (err.has_error())
    msg += ": " + err.msg();
  return Err(msg);
}

Identifier MakeIdentifier(std::string_view qualified_name) {
  Identifier identifier;
  auto components =
      fxl::SplitStringCopy(qualified_name, "::", fxl::WhiteSpaceHandling::kTrimWhitespace,
                           fxl::SplitResult::kSplitWantNonEmpty);

  for (const auto& component : components) {
    identifier.AppendComponent(IdentifierComponent(component));
  }

  return identifier;
}

// Forward declarations for recursive fetching
void FetchFuture(Session* session, const fxl::RefPtr<EvalContext>& context, const ExprValue& future,
                 const std::string& awaiter_file_name,
                 fit::callback<void(std::unique_ptr<AsyncTask>)> cb);

// We don't have the facilities here to access the pretty-printing system, which is how we would
// typically fetch the string value. Since we can't do that, we manually extract the data and length
// and fetch the memory manually.
void FormatRustString(const fxl::RefPtr<EvalContext>& context, const ExprValue& value,
                      fit::callback<void(std::string)> cb) {
  // A Rust String is { vec: { buf: { ptr: { pointer: <addr> }, ... }, len: <len> } }
  ErrOrValue len_val = ResolveNonstaticMember(context, value, {"vec", "len"});
  ErrOrValue ptr_val =
      ResolveNonstaticMember(context, value, {"vec", "buf", "inner", "ptr", "pointer", "pointer"});

  if (len_val.has_error() || ptr_val.has_error()) {
    cb("<error>");
    return;
  }

  uint64_t len = 0;
  uint64_t addr = 0;
  len_val.value().PromoteTo64(&len);
  ptr_val.value().PromoteTo64(&addr);

  if (len == 0) {
    cb("");
    return;
  }

  context->GetDataProvider()->GetMemoryAsync(
      addr, static_cast<uint32_t>(len),
      [len, cb = std::move(cb)](const Err& err, std::vector<uint8_t> data) mutable {
        if (err.has_error()) {
          cb("<error>");
          return;
        }

        // We should always get back the exact size we requested even if the implementation read
        // more than we asked for.
        FX_DCHECK(data.size() == len);
        cb(std::string(reinterpret_cast<const char*>(data.data()), len));
      });
}

void FetchAsyncFunctionOrBlock(Session* session, const fxl::RefPtr<EvalContext>& context,
                               const ExprValue& future,
                               fit::callback<void(std::unique_ptr<AsyncTask>)> cb) {
  fxl::RefPtr<DataMember> member;
  ErrOrValue variant_val = ResolveSingleVariantValue(context, future, &member);
  if (variant_val.has_error()) {
    cb(nullptr);
    return;
  }

  ExprValue value = variant_val.take_value();
  Identifier ident = value.type()->GetIdentifier();
  std::string state = ident.components()[ident.components().size() - 1].name();

  // Trim off the async state from the end of the type identifier.
  ident.components().resize(ident.components().size() - 2);

  // Don't display suspend states.
  if (debug::StringStartsWith(state, "Suspend")) {
    state = "";
  }

  Location loc;
  if (member->decl_line().is_valid()) {
    // Create a location with the file line.
    loc = Location(0, member->decl_line(), 0, SymbolContext::ForRelativeAddresses(), {});
  }

  auto task = std::make_unique<FuchsiaAsyncRustTask>(session, 0, loc, ident, state,
                                                     AsyncTask::Type::kFunction);

  std::optional<ExprValue> awaitee;
  std::map<std::string, ExprValue> values;
  if (const Collection* coll = value.type()->As<Collection>()) {
    for (const auto& lazy_member : coll->data_members()) {
      const DataMember* member = lazy_member.Get()->As<DataMember>();
      if (!member || member->artificial() || member->is_external())
        continue;

      std::string name = member->GetAssignedName();
      ErrOrValue val = ResolveNonstaticMember(context, value, FoundMember(coll, member));
      if (val.has_error()) {
        continue;
      } else if (name == "__awaitee") {
        awaitee = val.take_value();
      } else {
        // For some reason Rust could repeat the same field twice.
        values.try_emplace(name, val.take_value());
      }
    }
  }

  for (auto [name, val] : values) {
    task->AddNamedValue(name, std::move(val));
  }

  if (awaitee) {
    std::string filename;
    if (loc.file_line().is_valid()) {
      filename = ExtractLastFileComponent(loc.file_line().file());
    }
    FetchFuture(
        session, context, *awaitee, filename,
        [task = std::move(task), cb = std::move(cb)](std::unique_ptr<AsyncTask> child) mutable {
          if (child)
            task->AddChild(std::move(child));
          cb(std::move(task));
        });
  } else {
    cb(std::move(task));
  }
}

void FetchSelectJoin(Session* session, const fxl::RefPtr<EvalContext>& context,
                     const ExprValue& future, const std::string& name,
                     fit::callback<void(std::unique_ptr<AsyncTask>)> cb) {
  ErrOrValue err_or_f = ResolveNonstaticMember(context, future, {"f"});
  if (err_or_f.has_error())
    return cb(nullptr);

  ExprValue f = err_or_f.take_value();
  const Collection* f_coll = f.type()->As<Collection>();
  if (!f_coll)
    return cb(nullptr);

  auto task = std::make_unique<FuchsiaAsyncRustTask>(session, 0, Location(), MakeIdentifier(name),
                                                     "", AsyncTask::Type::kOther);
  auto joiner = fxl::MakeRefCounted<JoinCallbacks<std::unique_ptr<AsyncTask>>>();

  for (const auto& lazy_member : f_coll->data_members()) {
    const DataMember* member = lazy_member.Get()->As<DataMember>();
    if (!member || member->artificial() || member->is_external())
      continue;
    ErrOrValue member_val = ResolveNonstaticMember(context, f, FoundMember(f_coll, member));
    if (member_val.ok()) {
      FetchFuture(session, context, member_val.take_value(), "", joiner->AddCallback());
    }
  }

  joiner->Ready([task = std::move(task),
                 cb = std::move(cb)](std::vector<std::unique_ptr<AsyncTask>> children) mutable {
    for (auto& child : children) {
      if (child)
        task->AddChild(std::move(child));
    }
    cb(std::move(task));
  });
}

void FetchMember(Session* session, const fxl::RefPtr<EvalContext>& context, const ExprValue& value,
                 const std::vector<std::string>& names,
                 fit::callback<void(std::unique_ptr<AsyncTask>)> cb) {
  ErrOrValue val(value);
  for (const auto& name : names) {
    val = ResolveNonstaticMember(context, val.value(), {name});
    if (val.has_error())
      return cb(nullptr);
  }
  FetchFuture(session, context, val.value(), "", std::move(cb));
}

void FetchPin(Session* session, const fxl::RefPtr<EvalContext>& context, const ExprValue& pin,
              fit::callback<void(std::unique_ptr<AsyncTask>)> cb) {
  ErrOrValue pointer = ResolveNonstaticMember(context, pin, {"__pointer"});
  if (pointer.has_error())
    pointer = ResolveNonstaticMember(context, pin, {"pointer"});
  if (pointer.ok())
    return FetchFuture(session, context, pointer.value(), "", std::move(cb));
}

void FetchTask(Session* session, const fxl::RefPtr<EvalContext>& context, const ExprValue& task,
               fit::callback<void(std::unique_ptr<AsyncTask>)> cb) {
  ErrOrValue task_handle_opt = ResolveNonstaticMember(context, task, {"__0", "task"});
  if (task_handle_opt.ok()) {
    ErrOrValue task_handle = ResolveSingleVariantValue(context, task_handle_opt.value());
    if (task_handle.ok() && task_handle.value().type()->GetAssignedName() == "Some") {
      // Some(AtomicFutureHandle) -> AtomicFutureHandle -> NonNull -> pointer
      ErrOrValue ptr =
          ResolveNonstaticMember(context, task_handle.value(), {"__0", "__0", "pointer"});
      uint64_t task_id = 0;
      if (ptr.ok() && ptr.value().PromoteTo64(&task_id).ok()) {
        auto task = std::make_unique<FuchsiaAsyncRustTask>(
            session, task_id, Location(), MakeIdentifier("fuchsia_async::Task"),
            "id = " + to_hex_string(task_id), AsyncTask::Type::kTask);
        debug::MessageLoop::Current()->PostTask(
            FROM_HERE,
            [task = std::move(task), cb = std::move(cb)]() mutable { cb(std::move(task)); });
        return;
      }
    }
  }

  FetchMember(session, context, task, {"__0"}, std::move(cb));
}

void FetchJoinHandle(Session* session, const fxl::RefPtr<EvalContext>& context,
                     const ExprValue& join_handle,
                     fit::callback<void(std::unique_ptr<AsyncTask>)> cb) {
  ErrOrValue task_handle_opt = ResolveNonstaticMember(context, join_handle, {"task"});
  if (task_handle_opt.ok()) {
    ErrOrValue task_handle = ResolveSingleVariantValue(context, task_handle_opt.value());
    if (task_handle.ok() && task_handle.value().type()->GetAssignedName() == "Some") {
      ErrOrValue ptr =
          ResolveNonstaticMember(context, task_handle.value(), {"__0", "__0", "pointer"});
      uint64_t task_id = 0;
      if (ptr.ok() && ptr.value().PromoteTo64(&task_id).ok()) {
        auto task = std::make_unique<FuchsiaAsyncRustTask>(
            session, task_id, Location(), MakeIdentifier("fuchsia_async::JoinHandle"),
            "id = " + to_hex_string(task_id), AsyncTask::Type::kFuture);
        debug::MessageLoop::Current()->PostTask(
            FROM_HERE,
            [task = std::move(task), cb = std::move(cb)]() mutable { cb(std::move(task)); });
        return;
      }
    }
  }
  FetchMember(session, context, join_handle, {"task"}, std::move(cb));
}

void FetchScopeJoin(Session* session, const fxl::RefPtr<EvalContext>& context,
                    const ExprValue& scope_join,
                    fit::callback<void(std::unique_ptr<AsyncTask>)> cb) {
  ErrOrValue arc_inner_ptr =
      ResolveNonstaticMember(context, scope_join, {"scope", "inner", "inner", "ptr", "pointer"});
  uint64_t addr = 0;
  if (arc_inner_ptr.ok()) {
    arc_inner_ptr.value().PromoteTo64(&addr);
  }
  auto task = std::make_unique<FuchsiaAsyncRustTask>(session, 0, Location(),
                                                     MakeIdentifier("fuchsia_async::scope::Join"),
                                                     to_hex_string(addr), AsyncTask::Type::kOther);
  debug::MessageLoop::Current()->PostTask(
      FROM_HERE, [task = std::move(task), cb = std::move(cb)]() mutable { cb(std::move(task)); });
}

void FetchFuse(Session* session, const fxl::RefPtr<EvalContext>& context, const ExprValue& fuse,
               fit::callback<void(std::unique_ptr<AsyncTask>)> cb) {
  ErrOrValue inner = ResolveNonstaticMember(context, fuse, {"inner"});
  if (inner.ok()) {
    ErrOrValue some = ResolveSingleVariantValue(context, inner.value());
    if (some.ok() && some.value().type()->GetAssignedName() != "None") {
      FetchMember(session, context, some.value(), {"__0"}, std::move(cb));
      return;
    }
  }

  cb(nullptr);
}

void FetchMap(Session* session, const fxl::RefPtr<EvalContext>& context, const ExprValue& map,
              fit::callback<void(std::unique_ptr<AsyncTask>)> cb) {
  ErrOrValue val = ResolveSingleVariantValue(context, map);
  if (val.has_error()) {
    cb(nullptr);
    return;
  }
  val = ResolveNonstaticMember(context, val.value(), {"future"});
  if (val.has_error()) {
    cb(nullptr);
    return;
  }

  FetchFuture(session, context, val.value(), "", std::move(cb));
}

void FetchMapDebug(Session* session, const fxl::RefPtr<EvalContext>& context,
                   const ExprValue& debug_map, fit::callback<void(std::unique_ptr<AsyncTask>)> cb) {
  ErrOrValue map = ResolveNonstaticMember(context, debug_map, {"inner"});
  if (map.has_error()) {
    cb(nullptr);
    return;
  }

  FetchMap(session, context, map.value(), std::move(cb));
}

void FetchMaybeDone(Session* session, const fxl::RefPtr<EvalContext>& context,
                    const ExprValue& maybe_done,
                    fit::callback<void(std::unique_ptr<AsyncTask>)> cb) {
  ErrOrValue some = ResolveSingleVariantValue(context, maybe_done);
  if (some.ok() && some.value().type()->GetAssignedName() == "Future") {
    FetchMember(session, context, some.value(), {"__0"}, std::move(cb));
  } else {
    cb(nullptr);
  }
}

void FetchTraceFuture(Session* session, const fxl::RefPtr<EvalContext>& context,
                      const ExprValue& trace_future,
                      fit::callback<void(std::unique_ptr<AsyncTask>)> cb) {
  ErrOrValue future = ResolveNonstaticMember(context, trace_future, {"future"});
  if (future.has_error()) {
    cb(nullptr);
    return;
  }

  ErrOrValue trace_category = ResolveNonstaticMember(context, trace_future, {"category"});
  if (trace_category.has_error()) {
    cb(nullptr);
    return;
  }

  ErrOrValue trace_name = ResolveNonstaticMember(context, trace_future, {"name"});
  if (trace_name.has_error()) {
    cb(nullptr);
    return;
  }

  auto fut = std::make_unique<FuchsiaAsyncRustTask>(session, 0, Location(),
                                                    MakeIdentifier("fuchsia_trace::TraceFuture"),
                                                    "", AsyncTask::Type::kFuture);

  fut->AddNamedValue(std::nullopt, trace_category.take_value());
  fut->AddNamedValue(std::nullopt, trace_name.take_value());

  FetchFuture(
      session, context, future.take_value(), "",
      [task = std::move(fut), cb = std::move(cb)](std::unique_ptr<AsyncTask> child) mutable {
        task->AddChild(std::move(child));
        cb(std::move(task));
      });
}

void FetchVfsRequestListener(Session* session, const fxl::RefPtr<EvalContext>& context,
                             const ExprValue& request_listener,
                             fit::callback<void(std::unique_ptr<AsyncTask>)> cb) {
  ErrOrValue state = ResolveNonstaticMember(context, request_listener, {"state"});
  if (state.has_error()) {
    cb(nullptr);
    return;
  }

  ErrOrValue variant = ResolveSingleVariantValue(context, state.value());
  if (variant.has_error()) {
    cb(nullptr);
    return;
  }

  std::string variant_name = variant.value().type()->GetAssignedName();

  auto fut = std::make_unique<FuchsiaAsyncRustTask>(
      session, 0, Location(), MakeIdentifier("vfs::request_handler::RequestListener"), variant_name,
      AsyncTask::Type::kFuture);

  if (variant_name == "PollStream") {
    ErrOrValue stream = ResolveNonstaticMember(context, request_listener, {"stream"});
    if (stream.has_error()) {
      cb(nullptr);
      return;
    }

    fut->AddNamedValue(stream.value().type()->GetFullName(), stream.take_value());
    cb(std::move(fut));
  } else if (variant_name == "RequestFuture" || variant_name == "CloseFuture") {
    ErrOrValue future = ResolveNonstaticMember(context, variant.value(), {"__0"});
    if (future.has_error()) {
      cb(nullptr);
      return;
    }
    FetchFuture(
        session, context, future.take_value(), "",
        [fut = std::move(fut), cb = std::move(cb)](std::unique_ptr<AsyncTask> child) mutable {
          fut->AddChild(std::move(child));
          cb(std::move(fut));
        });
  }
}

void FetchFuture(Session* session, const fxl::RefPtr<EvalContext>& context, const ExprValue& future,
                 const std::string& awaiter_file_name,
                 fit::callback<void(std::unique_ptr<AsyncTask>)> cb) {
  std::string_view type = StripTemplate(future.type()->GetFullName());

  // Resolve pointers first. A pointer could be either non-dyn or dyn, raw or boxed.
  //
  // A non-dyn pointer (raw or boxed) is a ModifiedType.
  // A dyn raw pointer is a Collection and has a name "*mut dyn ..." or "*mut (dyn ... + ...)".
  // A dyn boxed pointer has the same layout but with a name "alloc::boxed::Box<(dyn ... + ...)>".
  if (future.type()->As<ModifiedType>() || debug::StringStartsWith(type, "*mut ") ||
      type == "alloc::boxed::Box") {
    ResolvePointer(context, future, [session, context, cb = std::move(cb)](ErrOrValue val) mutable {
      if (val.has_error()) {
        cb(nullptr);
      } else {
        FetchFuture(session, context, val.value(), "", std::move(cb));
      }
    });
    return;
  }

  if (IsAsyncFunctionOrBlock(future.type())) {
    FetchAsyncFunctionOrBlock(session, context, future, std::move(cb));
    return;
  }

  if (type == "core::pin::Pin") {
    FetchPin(session, context, future, std::move(cb));
    return;
  }
  if (type == "core::mem::maybe_dangling::MaybeDangling" ||
      type == "futures_util::future::future::WrappedFuture") {
    FetchMember(session, context, future, {"__0"}, std::move(cb));
    return;
  }
  if (type == "fuchsia_async::runtime::fuchsia::task::Task") {
    FetchTask(session, context, future, std::move(cb));
    return;
  }
  if (type == "fuchsia_async::runtime::fuchsia::task::JoinHandle") {
    FetchJoinHandle(session, context, future, std::move(cb));
    return;
  }
  if (type == "futures_util::future::future::fuse::Fuse") {
    FetchFuse(session, context, future, std::move(cb));
    return;
  }
  if (type == "futures_util::future::maybe_done::MaybeDone") {
    FetchMaybeDone(session, context, future, std::move(cb));
    return;
  }
  if (type == "futures_util::future::future::Then") {
    FetchMember(session, context, future, {"inner", "__0", "f"}, std::move(cb));
    return;
  }
  if (type == "futures_util::future::future::Map") {  // only appears in debug mode.
    FetchMapDebug(session, context, future, std::move(cb));
    return;
  }
  if (type == "futures_util::future::future::map::Map") {
    FetchMap(session, context, future, std::move(cb));
    return;
  }
  if (type == "futures_util::future::future::remote_handle::Remote") {
    FetchMember(session, context, future, {"future", "future", "__0"}, std::move(cb));
    return;
  }
  if (type == "vfs::execution_scope::TaskRunner") {
    FetchMember(session, context, future, {"task"}, std::move(cb));
    return;
  }
  if (type == "starnix_core::task::kernel_threads::WrappedFuture") {
    FetchMember(session, context, future, {"__0"}, std::move(cb));
    return;
  }
  if (type == "fuchsia_async::runtime::fuchsia::executor::scope::Join") {
    FetchScopeJoin(session, context, future, std::move(cb));
    return;
  }
  if (type == "fxfs::future_with_guard::FutureWithGuard") {
    FetchMember(session, context, future, {"future"}, std::move(cb));
    return;
  }
  if (type == "fuchsia_trace::TraceFuture") {
    FetchTraceFuture(session, context, future, std::move(cb));
    return;
  }
  if (type == "vfs::request_handler::RequestListener") {
    FetchVfsRequestListener(session, context, future, std::move(cb));
    return;
  }

  // NOTE: `select!` and `join!` macro expand to PollFn. It'll be useful if we could describe it.
  // However, PollFn could encode an arbitrary function so there's a chance we're doing very wrong.
  // To be more accurate, we also check the filename of the awaiter. `select!` will be expanded
  // from select_mod.rs, and `join!` will be expanded from `join_mod.rs`.
  if (type == "futures_util::future::poll_fn::PollFn") {
    if (awaiter_file_name == "select_mod.rs") {
      FetchSelectJoin(session, context, future, "select!", std::move(cb));
      return;
    }
    if (awaiter_file_name == "join_mod.rs") {
      FetchSelectJoin(session, context, future, "join!", std::move(cb));
      return;
    }
  }

  // Generic task object.
  auto task = std::make_unique<FuchsiaAsyncRustTask>(session, 0, Location(), MakeIdentifier(type),
                                                     "", AsyncTask::Type::kOther);
  task->AddNamedValue(std::nullopt, future);
  debug::MessageLoop::Current()->PostTask(
      FROM_HERE, [task = std::move(task), cb = std::move(cb)]() mutable { cb(std::move(task)); });
}

template <typename Val>
void IterateHashMap(const fxl::RefPtr<EvalContext>& context, const ExprValue& hashmap,
                    fit::function<void(const ExprValue&, fit::callback<void(Val)>)> each_cb,
                    fit::callback<void(ErrOr<std::vector<Val>>)> done_cb) {
  if (StripTemplate(hashmap.type()->GetFullName()) != "hashbrown::map::HashMap") {
    return done_cb(MakeError("Expect a HashMap, got " + hashmap.type()->GetFullName()));
  }

  ErrOrValue raw_table = ResolveNonstaticMember(context, hashmap, {"table"});
  if (raw_table.has_error())
    return done_cb(MakeError("Invalid HashMap (1)", raw_table.err()));
  const Collection* raw_table_coll = raw_table.value().type()->As<Collection>();
  if (!raw_table_coll || raw_table_coll->template_params().empty())
    return done_cb(MakeError("Invalid HashMap (2)"));
  fxl::RefPtr<Type> tuple_type;
  if (auto param = raw_table_coll->template_params()[0].Get()->As<TemplateParameter>())
    tuple_type = RefPtrTo(param->type().Get()->As<Type>());
  if (!tuple_type)
    return done_cb(MakeError("Invalid HashMap (3)"));

  ErrOrValue bucket_mask_res =
      ResolveNonstaticMember(context, raw_table.value(), {"table", "bucket_mask"});
  if (bucket_mask_res.has_error())
    return done_cb(MakeError("Invalid HashMap (4)", bucket_mask_res.err()));
  uint64_t bucket_mask = 0;
  bucket_mask_res.value().PromoteTo64(&bucket_mask);

  ErrOrValue ctrl_res =
      ResolveNonstaticMember(context, raw_table.value(), {"table", "ctrl", "pointer"});
  if (ctrl_res.has_error())
    return done_cb(MakeError("Invalid HashMap (6)", ctrl_res.err()));
  uint64_t ctrl = 0;
  ctrl_res.value().PromoteTo64(&ctrl);
  if (!ctrl) {
    // If ctrl is null, the map is empty.
    return done_cb(std::vector<Val>{});
  }

  uint64_t capacity = bucket_mask + 1;
  uint64_t total_buckets_size = tuple_type->byte_size() * capacity;
  context->GetDataProvider()->GetMemoryAsync(
      uint64_t(ctrl - total_buckets_size), uint32_t(total_buckets_size + capacity),
      [=, done_cb = std::move(done_cb), each_cb = std::move(each_cb)](
          const Err& err, std::vector<uint8_t> data) mutable {
        if (err.has_error()) {
          return done_cb(MakeError("Invalid HashMap (8)", err));
        }

        auto joiner = fxl::MakeRefCounted<JoinCallbacks<Val>>();
        for (size_t idx = 0; idx < capacity; idx++) {
          if ((data[total_buckets_size + idx] & 0x80))
            continue;
          uint8_t* slot = &data[total_buckets_size - (idx + 1) * tuple_type->byte_size()];
          ExprValue tuple(tuple_type, {slot, slot + tuple_type->byte_size()},
                          ExprValueSource(ctrl - (idx + 1) * tuple_type->byte_size()));
          each_cb(tuple, joiner->AddCallback());
        }
        joiner->Ready([done_cb = std::move(done_cb)](std::vector<Val> val) mutable {
          done_cb(std::move(val));
        });
      });
}

void FetchScopeTasks(Session* session, const fxl::RefPtr<EvalContext>& context,
                     const ExprValue& scope_state,
                     fit::callback<void(std::vector<std::unique_ptr<AsyncTask>>)> cb);

void FetchTaskFromHashSet(Session* session, const fxl::RefPtr<EvalContext>& context,
                          const ExprValue& tuple,
                          fit::callback<void(std::unique_ptr<AsyncTask>)> cb) {
  // Some(AtomicFutureHandle) -> AtomicFutureHandle -> NonNull
  ErrOrValue meta_ptr = ResolveNonstaticMember(context, tuple, {"__0", "__0", "pointer"});
  if (meta_ptr.has_error()) {
    cb(nullptr);
    return;
  }

  ResolvePointer(
      context, meta_ptr.value(),
      [session, context, meta_ptr, cb = std::move(cb)](ErrOrValue meta) mutable {
        if (meta.has_error()) {
          cb(nullptr);
          return;
        }
        uint64_t task_id = 0;
        if (auto err = meta_ptr.value().PromoteTo64(&task_id); err.has_error()) {
          cb(nullptr);
          return;
        }

        ErrOrValue state = ResolveNonstaticMember(context, meta.value(), {"state"});
        if (state.has_error()) {
          cb(nullptr);
          return;
        }

        uint64_t state_value;
        if (auto err = state.value().PromoteTo64(&state_value); err.has_error()) {
          cb(nullptr);
          return;
        }

        // If bit 61 is set then the future is DONE.
        if (state_value & (1ull << 61)) {
          auto task = std::make_unique<FuchsiaAsyncRustTask>(
              session, task_id, Location(), Identifier(), "Finished", AsyncTask::Type::kTask);
          debug::MessageLoop::Current()->PostTask(
              FROM_HERE,
              [task = std::move(task), cb = std::move(cb)]() mutable { cb(std::move(task)); });
          return;
        }

        // Read the drop vtable ptr.
        ErrOrValue vtable_ptr = ResolveNonstaticMember(context, meta.value(), {"vtable"});
        if (vtable_ptr.has_error()) {
          cb(nullptr);
          return;
        }

        ResolvePointer(
            context, vtable_ptr.value(),
            [session, context, meta_ptr, task_id, cb = std::move(cb)](ErrOrValue vtable) mutable {
              if (vtable.has_error()) {
                cb(nullptr);
                return;
              }

              ErrOrValue drop_fn = ResolveNonstaticMember(context, vtable.value(), {"drop"});
              if (drop_fn.has_error()) {
                cb(nullptr);
                return;
              }

              uint64_t drop_fn_value;
              if (auto err = drop_fn.value().PromoteTo64(&drop_fn_value); err.has_error()) {
                cb(nullptr);
                return;
              }

              Location loc = context->GetLocationForAddress(drop_fn_value);
              if (!loc.symbol()) {
                cb(nullptr);
                return;
              }

              const Function* func = loc.symbol().Get()->As<Function>();
              if (!func || func->template_params().empty()) {
                cb(nullptr);
                return;
              }

              auto derived = fxl::MakeRefCounted<ModifiedType>(DwarfTag::kPointerType,
                                                               func->parent().GetCached());

              CastExprValue(
                  context, CastType::kRust, meta_ptr.value(), derived, ExprValueSource(),
                  [session, context, task_id,
                   cb = std::move(cb)](ErrOrValue atomic_future_ptr) mutable {
                    if (atomic_future_ptr.has_error()) {
                      cb(nullptr);
                      return;
                    }

                    ResolvePointer(
                        context, atomic_future_ptr.value(),
                        [session, context, task_id,
                         cb = std::move(cb)](ErrOrValue atomic_future) mutable {
                          if (atomic_future.has_error()) {
                            cb(nullptr);
                            return;
                          }
                          ErrOrValue future = ResolveNonstaticMember(context, atomic_future.value(),
                                                                     {"future", "future", "value"});
                          if (future.has_error()) {
                            cb(nullptr);
                            return;
                          }

                          FetchFuture(
                              session, context, future.value(), "",
                              [session, task_id,
                               cb = std::move(cb)](std::unique_ptr<AsyncTask> task) mutable {
                                if (task) {
                                  auto wrapper = std::make_unique<FuchsiaAsyncRustTask>(
                                      session, task_id, task->GetLocation(), Identifier("Task"),
                                      "id = " + to_hex_string(task_id), AsyncTask::Type::kTask);
                                  wrapper->AddChild(std::move(task));

                                  debug::MessageLoop::Current()->PostTask(
                                      FROM_HERE,
                                      [wrapper = std::move(wrapper), cb = std::move(cb)]() mutable {
                                        cb(std::move(wrapper));
                                      });
                                } else {
                                  cb(nullptr);
                                }
                              });
                        });
                  });
            });
      });
}

void ResolveScopeHandle(const fxl::RefPtr<EvalContext>& context, const ExprValue& handle,
                        fit::callback<void(ErrOrValue)> cb) {
  // ScopeHandle has "inner" (Arc<ScopeInner>).
  ErrOrValue arc_ptr = ResolveNonstaticMember(context, handle, {"inner", "ptr", "pointer"});
  if (arc_ptr.has_error())
    arc_ptr = ResolveNonstaticMember(context, handle, {"inner", "inner", "ptr", "pointer"});

  if (arc_ptr.has_error())
    return cb(arc_ptr.err());

  ResolvePointer(
      context, arc_ptr.value(), [context, cb = std::move(cb)](ErrOrValue arc_inner) mutable {
        if (arc_inner.has_error())
          return cb(arc_inner.err());

        // ArcInner<T> has a field "data" of type T (ScopeInner).
        ErrOrValue scope_inner = ResolveNonstaticMember(context, arc_inner.value(), {"data"});
        if (scope_inner.has_error())
          scope_inner = arc_inner;

        // Condition<T> has a field "__0" (Arc<Mutex<Inner<T>>>).
        ErrOrValue cond = ResolveNonstaticMember(context, scope_inner.value(), {"state"});
        if (cond.has_error())
          return cb(cond.err());

        ErrOrValue cond_arc_ptr =
            ResolveNonstaticMember(context, cond.value(), {"__0", "ptr", "pointer"});
        if (cond_arc_ptr.has_error())
          cond_arc_ptr =
              ResolveNonstaticMember(context, cond.value(), {"__0", "inner", "ptr", "pointer"});

        if (cond_arc_ptr.has_error())
          return cb(cond_arc_ptr.err());

        ResolvePointer(context, cond_arc_ptr.value(),
                       [context, cb = std::move(cb)](ErrOrValue cond_arc_inner) mutable {
                         if (cond_arc_inner.has_error())
                           return cb(cond_arc_inner.err());

                         // ArcInner<Mutex<Inner<ScopeState>>>
                         // data is Mutex<Inner<ScopeState>>
                         // Mutex.data is Inner<ScopeState>
                         // Inner.data is ScopeState
                         // We try various paths to be robust to UnsafeCell wrappers etc.
                         ErrOrValue child_state = ResolveNonstaticMember(
                             context, cond_arc_inner.value(), {"data", "data", "value", "data"});
                         if (child_state.has_error())
                           child_state = ResolveNonstaticMember(context, cond_arc_inner.value(),
                                                                {"data", "data", "data"});
                         if (child_state.has_error())
                           child_state = ResolveNonstaticMember(context, cond_arc_inner.value(),
                                                                {"data", "data"});
                         if (child_state.has_error())
                           child_state =
                               ResolveNonstaticMember(context, cond_arc_inner.value(), {"data"});

                         cb(std::move(child_state));
                       });
      });
}

void FetchScopeTasks(Session* session, const fxl::RefPtr<EvalContext>& context,
                     const ExprValue& scope_state,
                     fit::callback<void(std::vector<std::unique_ptr<AsyncTask>>)> cb) {
  // Try to find the tasks HashSet. It might be in "all_tasks" or "all_tasks.base".
  ErrOrValue tasks_member = ResolveNonstaticMember(context, scope_state, {"all_tasks"});
  if (tasks_member.ok()) {
    ErrOrValue base = ResolveNonstaticMember(context, tasks_member.value(), {"base"});
    if (base.ok())
      tasks_member = base;
  }

  auto joiner = fxl::MakeRefCounted<JoinCallbacks<std::vector<std::unique_ptr<AsyncTask>>>>();

  auto tasks_cb = joiner->AddCallback();
  if (tasks_member.has_error()) {
    tasks_cb({});
  } else {
    // A HashSet usually wraps a HashMap in a field called "map".
    ErrOrValue map = ResolveNonstaticMember(context, tasks_member.value(), {"map"});
    ExprValue tasks_map = map.ok() ? map.value() : tasks_member.value();

    IterateHashMap<std::unique_ptr<AsyncTask>>(
        context, tasks_map,
        [session, context](const ExprValue& tuple,
                           fit::callback<void(std::unique_ptr<AsyncTask>)> cb) {
          FetchTaskFromHashSet(session, context, tuple, std::move(cb));
        },
        [tasks_cb =
             std::move(tasks_cb)](ErrOr<std::vector<std::unique_ptr<AsyncTask>>> result) mutable {
          std::vector<std::unique_ptr<AsyncTask>> tasks;
          if (result.ok()) {
            for (auto& t : result.value()) {
              if (t)
                tasks.push_back(std::move(t));
            }
          }
          tasks_cb(std::move(tasks));
        });
  }

  auto children_cb = joiner->AddCallback();
  // Try to find the children HashSet. It might be in "children" or "children.base".
  ErrOrValue children_member = ResolveNonstaticMember(context, scope_state, {"children"});
  if (children_member.ok()) {
    ErrOrValue base = ResolveNonstaticMember(context, children_member.value(), {"base"});
    if (base.ok())
      children_member = base;
  }

  if (children_member.has_error()) {
    children_cb({});
  } else {
    ErrOrValue map = ResolveNonstaticMember(context, children_member.value(), {"map"});
    ExprValue children_map = map.ok() ? map.value() : children_member.value();

    IterateHashMap<std::vector<std::unique_ptr<AsyncTask>>>(
        context, children_map,
        [session, context](const ExprValue& tuple,
                           fit::callback<void(std::vector<std::unique_ptr<AsyncTask>>)> cb) {
          // HashSet entry for WeakScopeHandle. Try to find the pointer.
          ErrOrValue weak_handle = ResolveNonstaticMember(context, tuple, {"__0"});
          if (weak_handle.has_error())
            weak_handle = ErrOrValue(tuple);

          // WeakScopeHandle has "inner" (Weak<ScopeInner>).
          // Weak has "ptr" (NonNull). NonNull has "pointer".
          ErrOrValue arc_inner_ptr =
              ResolveNonstaticMember(context, weak_handle.value(), {"inner", "ptr", "pointer"});
          if (arc_inner_ptr.has_error())
            arc_inner_ptr =
                ResolveNonstaticMember(context, weak_handle.value(), {"inner", "pointer"});

          if (arc_inner_ptr.has_error()) {
            cb({});
            return;
          }

          ResolvePointer(
              context, arc_inner_ptr.value(),
              [session, context, arc_inner_ptr, cb = std::move(cb)](ErrOrValue arc_inner) mutable {
                if (arc_inner.has_error()) {
                  cb({});
                  return;
                }

                // ArcInner<T> has a field "data" of type T.
                ErrOrValue scope_inner =
                    ResolveNonstaticMember(context, arc_inner.value(), {"data"});
                if (scope_inner.has_error()) {
                  cb({});
                  return;
                }

                ErrOrValue scope_name_opt =
                    ResolveNonstaticMember(context, scope_inner.value(), {"name"});
                auto on_name_ready = [session, context, scope_inner, arc_inner_ptr,
                                      cb = std::move(cb)](std::string scope_name) mutable {
                  ErrOrValue mutex_ptr = ResolveNonstaticMember(context, scope_inner.value(),
                                                                {"state", "__0", "ptr", "pointer"});
                  if (mutex_ptr.has_error()) {
                    cb({});
                    return;
                  }

                  ResolvePointer(
                      context, mutex_ptr.value(),
                      [session, context, scope_name, arc_inner_ptr,
                       cb = std::move(cb)](ErrOrValue mutex) mutable {
                        if (mutex.has_error()) {
                          cb({});
                          return;
                        }

                        ErrOrValue child_state = ResolveNonstaticMember(
                            context, mutex.value(), {"data", "data", "value", "data"});
                        if (child_state.has_error()) {
                          cb({});
                          return;
                        }

                        FetchScopeTasks(
                            session, context, child_state.value(),
                            [session, scope_name, arc_inner_ptr, cb = std::move(cb)](
                                std::vector<std::unique_ptr<AsyncTask>> tasks) mutable {
                              auto scope = std::make_unique<FuchsiaAsyncRustTask>(
                                  session, 0, Location(), Identifier("Scope"), scope_name,
                                  AsyncTask::Type::kScope);
                              scope->AddNamedValue(std::nullopt, arc_inner_ptr.take_value());
                              for (auto& t : tasks) {
                                scope->AddChild(std::move(t));
                              }
                              std::vector<std::unique_ptr<AsyncTask>> res;
                              res.push_back(std::move(scope));

                              debug::MessageLoop::Current()->PostTask(
                                  FROM_HERE, [res = std::move(res), cb = std::move(cb)]() mutable {
                                    cb(std::move(res));
                                  });
                            });
                      });
                };

                if (scope_name_opt.ok()) {
                  FormatRustString(context, scope_name_opt.value(), std::move(on_name_ready));
                } else {
                  on_name_ready("<unknown>");
                }
              });
        },
        [children_cb = std::move(children_cb)](
            ErrOr<std::vector<std::vector<std::unique_ptr<AsyncTask>>>> result) mutable {
          std::vector<std::unique_ptr<AsyncTask>> all_children_tasks;
          if (result.ok()) {
            for (auto& tasks : result.value()) {
              for (auto& t : tasks) {
                if (t)
                  all_children_tasks.push_back(std::move(t));
              }
            }
          }
          children_cb(std::move(all_children_tasks));
        });
  }

  joiner->Ready(
      [cb = std::move(cb)](std::vector<std::vector<std::unique_ptr<AsyncTask>>> results) mutable {
        std::vector<std::unique_ptr<AsyncTask>> final_tasks;
        for (auto& tasks : results) {
          for (auto& t : tasks) {
            final_tasks.push_back(std::move(t));
          }
        }
        std::ranges::sort(final_tasks, [](const auto& lhs, const auto& rhs) {
          return lhs->GetLocation().address() < rhs->GetLocation().address();
        });
        cb(std::move(final_tasks));
      });
}

std::optional<std::string> GetScopeExpressionForExecutorFrame(
    std::string_view executor_frame_name) {
  // This keeps the mapping of function signatures to expression strings that we expect to be able
  // to find in order to get at the executor's root scope, which is where all subsequent scopes,
  // tasks, and futures will be posted. The root scope is not actually anything the user cares
  // about, and has precisely the same lifetime as the executor itself.
  //
  // The key is a vector of strings, the first of which will be treated as a prefix match. Any
  // subsequent strings will be checked for containment only. All elements of the vector must
  static const std::vector<std::pair<std::vector<std::string_view>, std::string_view>>
      kExecutorFrameNameToScopeExpr = {
          // Single threaded executor, found on the main thread.
          {{"fuchsia_async::runtime::fuchsia::executor::local::LocalExecutor::run"},
           "self.ehandle.root_scope"},
          // Multithreaded executor, found on the main thread.
          {{"fuchsia_async::runtime::fuchsia::executor::send::SendExecutor::run"},
           "self.root_scope"},
          // Multithreaded executor, found on a worker thread.
          {{"fuchsia_async::runtime::fuchsia::executor::send", "create_worker_threads::{closure"},
           "root_scope"},
      };

  for (const auto& [func_parts, expr] : kExecutorFrameNameToScopeExpr) {
    size_t i = 0;
    for (; i < func_parts.size(); i++) {
      if ((i == 0 && !debug::StringStartsWith(executor_frame_name, func_parts[i])) ||
          (i > 0 && !debug::StringContains(executor_frame_name, func_parts[i]))) {
        break;
      }
    }

    // Return the first entry that matched all elements.
    if (i == func_parts.size()) {
      return std::string(expr);
    }
  }

  return std::nullopt;
}

}  // namespace

FuchsiaAsyncRustTaskProvider::FuchsiaAsyncRustTaskProvider() = default;
FuchsiaAsyncRustTaskProvider::~FuchsiaAsyncRustTaskProvider() = default;

bool FuchsiaAsyncRustTaskProvider::CanHandle(Frame* frame) const {
  if (!frame->GetLocation().symbol().is_valid())
    return false;

  std::string func_name(StripTemplate(frame->GetLocation().symbol().Get()->GetFullName()));
  return GetScopeExpressionForExecutorFrame(func_name) != std::nullopt;
}

void FuchsiaAsyncRustTaskProvider::GetTasks(
    Frame* frame, fit::callback<void(const Err&, std::vector<std::unique_ptr<AsyncTask>>)> cb) {
  FX_DCHECK(frame);

  std::string func_name(StripTemplate(frame->GetLocation().symbol().Get()->GetFullName()));
  auto maybe_expr = GetScopeExpressionForExecutorFrame(func_name);
  if (maybe_expr == std::nullopt) {
    debug::MessageLoop::Current()->PostTask(FROM_HERE, [cb = std::move(cb)]() mutable {
      cb(Err("No matching function signature to find async executor."), {});
    });
    return;
  }

  auto context = frame->GetEvalContext();
  EvalExpression(
      *maybe_expr, context, false,
      [session = frame->session(), context, cb = std::move(cb)](ErrOrValue value) mutable {
        if (value.has_error()) {
          cb(value.err(), {});
        } else {
          ResolveScopeHandle(
              context, value.value(),
              [session, context, cb = std::move(cb)](ErrOrValue state) mutable {
                if (state.has_error()) {
                  cb(state.err(), {});
                } else {
                  FetchScopeTasks(
                      session, context, state.value(),
                      [cb = std::move(cb)](std::vector<std::unique_ptr<AsyncTask>> tasks) mutable {
                        cb(Err(), std::move(tasks));
                      });
                }
              });
        }
      });
}

}  // namespace zxdb

// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "provider_impl.h"

#include <fidl/fuchsia.tracing.provider/cpp/wire.h>
#include <fidl/fuchsia.tracing/cpp/wire.h>
#include <lib/async/cpp/task.h>
#include <lib/zx/process.h>
#include <stdio.h>
#include <zircon/assert.h>
#include <zircon/status.h>

#include <utility>

#include "export.h"
#include "lib/trace-engine/handler.h"
#include "lib/trace-engine/types.h"
#include "lib/trace-provider/provider.h"
#include "session.h"
#include "utils.h"

namespace {
trace_buffering_mode_t FidlBufferingModeToTraceEngineBufferingMode(
    fuchsia_tracing::wire::BufferingMode buffering_mode) {
  switch (buffering_mode) {
    case fuchsia_tracing::wire::BufferingMode::kOneshot:
      return TRACE_BUFFERING_MODE_ONESHOT;
    case fuchsia_tracing::wire::BufferingMode::kCircular:
      return TRACE_BUFFERING_MODE_CIRCULAR;
    case fuchsia_tracing::wire::BufferingMode::kStreaming:
      return TRACE_BUFFERING_MODE_STREAMING;
  }
}

trace_start_mode_t FidlBufferingDispositionToTraceEngineStartMode(
    fuchsia_tracing::BufferDisposition buffer_disposition) {
  switch (buffer_disposition) {
    case fuchsia_tracing::wire::BufferDisposition::kClearEntire:
      return TRACE_START_CLEAR_ENTIRE_BUFFER;
    case fuchsia_tracing::wire::BufferDisposition::kClearNondurable:
      return TRACE_START_CLEAR_NONDURABLE_BUFFER;
    case fuchsia_tracing::wire::BufferDisposition::kRetain:
      return TRACE_START_RETAIN_BUFFER;
  }
}

#if FUCHSIA_API_LEVEL_AT_LEAST(31)
std::vector<std::string> CloneCategories(
    const fuchsia_tracing_provider::wire::ProviderConfigV2& config) {
  std::vector<std::string> categories;
  if (config.has_categories()) {
    categories.reserve(config.categories().size());
    for (const auto& category : config.categories()) {
      categories.emplace_back(category.data(), category.size());
    }
  }
  return categories;
}
#else
std::vector<std::string> CloneCategories(
    const fuchsia_tracing_provider::wire::ProviderConfig& config) {
  std::vector<std::string> categories;
  categories.reserve(config.categories.size());
  for (const auto& category : config.categories) {
    categories.emplace_back(category.data(), category.size());
  }
  return categories;
}
#endif
}  // namespace

namespace trace {
namespace internal {

#if FUCHSIA_API_LEVEL_AT_LEAST(31)
TraceProviderImpl::TraceProviderImpl(
    std::string name, async_dispatcher_t* dispatcher,
    fidl::ServerEnd<fuchsia_tracing_provider::ProviderV2> server_end)
    : name_(std::move(name)), dispatcher_(dispatcher) {
  std::scoped_lock lock{mutex_};
  binding_ = fidl::BindServer(dispatcher_, std::move(server_end), this,
                              [](TraceProviderImpl* impl, fidl::UnbindInfo info,
                                 fidl::ServerEnd<fuchsia_tracing_provider::ProviderV2> server_end) {
                                // We don't own impl, so don't attempt to clean it up.
                              });
}

TraceProviderImpl::~TraceProviderImpl() {
  // Our clean up step involves a bit of a dance:
  //
  // 1) We may be destroyed on/by the async loop, but trace-engine may trigger and then wait for
  //    clean up operations on the loop. So we need to release the loop by posting a continuation
  //    task between each step to allow for the cleanup to occur.
  // 2) After each step completes, trace-engine calls a callback on the registered handler
  //    `Session session_`. So the Session needs to stay alive until after we've received the
  //    TraceTerminated callback.
  //
  // Therefore, we allow the session to outlive us and task the async loop with stopping, then
  // terminating, releasing the async loop between each stop so that trace engine can process the
  // intermediate update.
  //
  // Finally, we hand ownership of the session to an empty callback so that it can be destroyed
  // cleanly.
  std::scoped_lock lock{mutex_};
  if (binding_) {
    binding_->Unbind();
  }
  if (session_) {
    async::PostTask(dispatcher_, [session = std::move(session_)]() mutable {
      session->StopEngine([session = std::move(session)]() mutable {
        session->TerminateEngine([session = std::move(session)]() {});
      });
    });
  }
}
#else
TraceProviderImpl::TraceProviderImpl(std::string name, async_dispatcher_t* dispatcher,
                                     fidl::ServerEnd<fuchsia_tracing_provider::Provider> server_end)
    : name_(std::move(name)), dispatcher_(dispatcher) {
  binding_ = fidl::BindServer(dispatcher_, std::move(server_end), this,
                              [](TraceProviderImpl* impl, fidl::UnbindInfo info,
                                 fidl::ServerEnd<fuchsia_tracing_provider::Provider> server_end) {
                                Session::TerminateEngine();
                              });
}
TraceProviderImpl::~TraceProviderImpl() {
  if (binding_) {
    binding_->Unbind();
  }
  Session::TerminateEngine();
}
#endif

#if FUCHSIA_API_LEVEL_AT_LEAST(31)
void TraceProviderImpl::Initialize(
    fuchsia_tracing_provider::wire::ProviderV2InitializeRequest* request,
    InitializeCompleter::Sync& completer) {
  std::scoped_lock lock{mutex_};
  if (session_) {
    fprintf(stderr, "TraceProvider: Initialize failed, session already exists.\n");
    completer.Close(ZX_ERR_BAD_STATE);
    return;
  }

  if (!binding_) {
    fprintf(stderr, "TraceProvider: Initialize failed, binding not yet completed.\n");
    completer.Close(ZX_ERR_BAD_STATE);
    return;
  }

  fuchsia_tracing_provider::wire::ProviderConfigV2& config = request->config;
  trace_buffering_mode_t buffering_mode = TRACE_BUFFERING_MODE_ONESHOT;
  if (config.has_buffering_mode()) {
    buffering_mode = FidlBufferingModeToTraceEngineBufferingMode(config.buffering_mode());
  }

  zx::vmo buffer;
  if (config.has_buffer()) {
    buffer = std::move(config.buffer());
  }

  std::vector<std::string> categories = CloneCategories(config);
  session_ = Session::InitializeEngine(dispatcher_, buffering_mode, std::move(buffer), categories,
                                       *binding_);
  provider_config_ = {
      .buffering_mode = buffering_mode,
      .categories = std::move(categories),
  };
}
#else
void TraceProviderImpl::Initialize(
    fuchsia_tracing_provider::wire::ProviderInitializeRequest* request,
    InitializeCompleter::Sync& completer) {
  std::scoped_lock lock{mutex_};
  fuchsia_tracing_provider::wire::ProviderConfig& config = request->config;
  Session::InitializeEngine(
      dispatcher_, FidlBufferingModeToTraceEngineBufferingMode(config.buffering_mode),
      std::move(config.buffer), std::move(config.fifo), CloneCategories(config));
  provider_config_ = {
      .buffering_mode = FidlBufferingModeToTraceEngineBufferingMode(config.buffering_mode),
      .categories = CloneCategories(config),
  };
}
#endif

#if FUCHSIA_API_LEVEL_AT_LEAST(31)
void TraceProviderImpl::Start(fuchsia_tracing_provider::wire::ProviderV2StartRequest* request,
                              StartCompleter::Sync& completer) {
  std::scoped_lock lock{mutex_};
  const fuchsia_tracing_provider::wire::StartOptions& options = request->options;
  if (session_) {
    session_->StartEngine(
        FidlBufferingDispositionToTraceEngineStartMode(options.buffer_disposition),
        [cb = completer.ToAsync()]() mutable { cb.Reply(); });
  } else {
    fprintf(stderr, "TraceProvider: Start failed, session doesn't exist.\n");
    completer.Reply();
  }
}
#else
void TraceProviderImpl::Start(fuchsia_tracing_provider::wire::ProviderStartRequest* request,
                              StartCompleter::Sync& completer) {
  const fuchsia_tracing_provider::wire::StartOptions& options = request->options;
  Session::StartEngine(FidlBufferingDispositionToTraceEngineStartMode(options.buffer_disposition));
}
#endif

#if FUCHSIA_API_LEVEL_AT_LEAST(31)
void TraceProviderImpl::Stop(StopCompleter::Sync& completer) {
  std::scoped_lock lock{mutex_};
  if (session_) {
    session_->StopEngine([cb = completer.ToAsync()]() mutable { cb.Reply(); });
  } else {
    completer.Reply();
  }
}
#else
void TraceProviderImpl::Stop(StopCompleter::Sync& completer) { Session::StopEngine(); }
#endif

void TraceProviderImpl::Terminate(TerminateCompleter::Sync& completer) {
#if FUCHSIA_API_LEVEL_AT_LEAST(31)
  std::unique_ptr<Session> session;
  {
    std::scoped_lock lock{mutex_};
    if (!session_) {
      completer.Reply();
      return;
    }
    session = std::move(session_);
  }

  // Calling TerminateEngine will call Session::TraceTerminated, which then calls this callback.
  // Passing Session into the callback extends its lifetime until we no longer need it.
  session->TerminateEngine(
      [cb = completer.ToAsync(), session = std::move(session)]() mutable { cb.Reply(); });
#else
  Session::TerminateEngine();
#endif
}

void TraceProviderImpl::GetKnownCategories(GetKnownCategoriesCompleter::Sync& completer) {
  // TODO(https://fxbug.dev/42068744): Return the trace categories that were registered with the
  // category string literal.
  std::vector<trace::KnownCategory> known_categories;
  {
    std::scoped_lock lock{mutex_};
    if (get_known_categories_callback_ == nullptr) {
      completer.Reply({});
      return;
    }
    known_categories = get_known_categories_callback_();
  }

  std::vector<fuchsia_tracing::wire::KnownCategory> known_categories_fidl;
  known_categories_fidl.reserve(known_categories.size());

  for (const auto& known_category : known_categories) {
    known_categories_fidl.emplace_back(fidl::StringView::FromExternal(known_category.name),
                                       fidl::StringView::FromExternal(known_category.description));
  }
  completer.Reply(
      fidl::VectorView<fuchsia_tracing::wire::KnownCategory>::FromExternal(known_categories_fidl));
}

void TraceProviderImpl::SetGetKnownCategoriesCallback(GetKnownCategoriesCallback callback) {
  std::scoped_lock lock{mutex_};
  get_known_categories_callback_ = std::move(callback);
}

#if FUCHSIA_API_LEVEL_AT_LEAST(31)
void TraceProviderImpl::NotifyBufferSaved(
    fuchsia_tracing_provider::wire::ProviderV2NotifyBufferSavedRequest* request,
    NotifyBufferSavedCompleter::Sync& completer) {
  Session::MarkBufferSaved(request->wrapped_count, request->durable_data_end);
}

void TraceProviderImpl::Flush(FlushCompleter::Sync& completer) {
  zx_status_t status = trace_engine_flush_buffer();
  if (status == ZX_ERR_BAD_STATE) {
    // This happens when tracing has stopped. Ignore it.
  } else if (status != ZX_OK) {
    fprintf(stderr, "Session: FlushBuffer failed: status=%d\n", status);
  }
}

void TraceProviderImpl::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_tracing_provider::ProviderV2> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  completer.Close(ZX_ERR_NOT_SUPPORTED);
}
#endif

ProviderConfig TraceProviderImpl::GetProviderConfig() const {
  std::scoped_lock lock{mutex_};
  return provider_config_;
}

}  // namespace internal

ProviderConfig TraceProvider::GetProviderConfig() const {
  ZX_DEBUG_ASSERT(provider_);
  const auto* provider_impl = reinterpret_cast<internal::TraceProviderImpl*>(provider_);
  return provider_impl->GetProviderConfig();
}

void TraceProvider::SetGetKnownCategoriesCallback(GetKnownCategoriesCallback callback) {
  ZX_DEBUG_ASSERT(provider_);
  auto* provider_impl = reinterpret_cast<internal::TraceProviderImpl*>(provider_);
  provider_impl->SetGetKnownCategoriesCallback(std::move(callback));
}
}  // namespace trace

EXPORT trace_provider_t* trace_provider_create_with_name(zx_handle_t to_service_h,
                                                         async_dispatcher_t* dispatcher,
                                                         const char* name) {
  std::string provider_name =
      name == nullptr ? trace::internal::GetProcessName().value_or("") : name;

  const fidl::ClientEnd<fuchsia_tracing_provider::Registry> to_service{zx::channel{to_service_h}};

  ZX_DEBUG_ASSERT(to_service.is_valid());
  ZX_DEBUG_ASSERT(dispatcher);

  // Create the channel to which we will bind the trace provider.
#if FUCHSIA_API_LEVEL_AT_LEAST(31)
  auto [client_end, server_end] = fidl::Endpoints<fuchsia_tracing_provider::ProviderV2>::Create();
  const fidl::Status result = fidl::WireCall(to_service)
                                  ->RegisterV2(std::move(client_end), trace::internal::GetPid(),
                                               fidl::StringView::FromExternal(provider_name));
#else
  auto [client_end, server_end] = fidl::Endpoints<fuchsia_tracing_provider::Provider>::Create();

  // Register the trace provider.
  const fidl::Status result =
      fidl::WireCall(to_service)
          ->RegisterProvider(std::move(client_end), trace::internal::GetPid(),
                             fidl::StringView::FromExternal(provider_name));
#endif
  if (!result.ok()) {
    // On products where trace_manager is not included, it is expected that we fail to register a
    // provider with ZX_ERR_PEER_CLOSED
    if (result.error().status() != ZX_ERR_PEER_CLOSED) {
      fprintf(stderr, "TraceProvider: RegisterProvider failed: result=%s\n",
              result.FormatDescription().c_str());
    }
    return nullptr;
  }
  // Note: |to_service| can be closed now. Let it close as a consequence
  // of going out of scope.

  return new trace::internal::TraceProviderImpl(std::move(provider_name), dispatcher,
                                                std::move(server_end));
}

EXPORT trace_provider_t* trace_provider_create(zx_handle_t to_service,
                                               async_dispatcher_t* dispatcher) {
  return trace_provider_create_with_name(to_service, dispatcher, nullptr);
}

EXPORT trace_provider_t* trace_provider_create_synchronously(zx_handle_t to_service_h,
                                                             async_dispatcher_t* dispatcher,
                                                             const char* name,
                                                             bool* out_already_started) {
  std::string provider_name =
      name == nullptr ? trace::internal::GetProcessName().value_or("") : name;

  const fidl::ClientEnd<fuchsia_tracing_provider::Registry> to_service{zx::channel{to_service_h}};

  ZX_DEBUG_ASSERT(to_service.is_valid());
  ZX_DEBUG_ASSERT(dispatcher);

  // Create the channel to which we will bind the trace provider.
#if FUCHSIA_API_LEVEL_AT_LEAST(31)
  zx::result endpoints = fidl::CreateEndpoints<fuchsia_tracing_provider::ProviderV2>();
  if (endpoints.is_error()) {
    fprintf(stderr, "TraceProvider: channel create failed: status=%d(%s)\n",
            endpoints.status_value(), endpoints.status_string());
    return nullptr;
  }

  const fidl::WireResult result =
      fidl::WireCall(to_service)
          ->RegisterV2Synchronously(std::move(endpoints->client), trace::internal::GetPid(),
                                    fidl::StringView::FromExternal(provider_name));
  if (!result.ok()) {
    // On products where trace_manager is not included, it is expected that we fail to register a
    // provider with ZX_ERR_PEER_CLOSED
    if (result.error().status() != ZX_ERR_PEER_CLOSED) {
      fprintf(stderr, "TraceProvider: RegisterV2Synchronously failed: result=%s\n",
              result.FormatDescription().c_str());
    }
    return nullptr;
  }

  if (result.value().is_error()) {
    zx_status_t status = result.value().error_value();
    fprintf(stderr, "TraceProvider: registry failed: status=%d(%s)\n", status,
            zx_status_get_string(status));
    return nullptr;
  }

  if (out_already_started) {
    *out_already_started = result.value().value()->started;
  }
#else
  zx::result endpoints = fidl::CreateEndpoints<fuchsia_tracing_provider::Provider>();
  if (endpoints.is_error()) {
    fprintf(stderr, "TraceProvider: channel create failed: status=%d(%s)\n",
            endpoints.status_value(), endpoints.status_string());
    return nullptr;
  }

  // Register the trace provider.
  const fidl::WireResult result =
      fidl::WireCall(to_service)
          ->RegisterProviderSynchronously(std::move(endpoints->client), trace::internal::GetPid(),
                                          fidl::StringView::FromExternal(provider_name));
  if (!result.ok()) {
    // On products where trace_manager is not included, it is expected that we fail to register a
    // provider with ZX_ERR_PEER_CLOSED
    if (result.error().status() != ZX_ERR_PEER_CLOSED) {
      fprintf(stderr, "TraceProvider: RegisterProviderSynchronously failed: result=%s\n",
              result.FormatDescription().c_str());
    }
    return nullptr;
  }
  const fidl::WireResponse response = result.value();
  if (const zx_status_t status = response.s; status != ZX_OK) {
    fprintf(stderr, "TraceProvider: registry failed: status=%d(%s)\n", status,
            zx_status_get_string(status));
    return nullptr;
  }
  // Note: |to_service| can be closed now. Let it close as a consequence
  // of going out of scope.

  if (out_already_started) {
    *out_already_started = response.started;
  }
#endif

  return new trace::internal::TraceProviderImpl(std::move(provider_name), dispatcher,
                                                std::move(endpoints->server));
}

EXPORT void trace_provider_destroy(trace_provider_t* provider) {
  ZX_DEBUG_ASSERT(provider);

  // The provider's dispatcher may be running on a different thread. This happens when, e.g., the
  // dispatcher is running in a background thread and we are called in the foreground thread.
  // async::WaitBase, which we use, requires all calls be made on the dispatcher thread. Thus we
  // can't delete |provider| here. Instead we schedule it to be deleted on the dispatcher's thread.
  //
  // There are two cases to be handled:
  // 1) The dispatcher's thread is our thread.
  // 2) The dispatcher's thread is a different thread.
  // In both cases there's an additional wrinkle:
  // a) The task we post is run.
  // b) The task we post is not run.
  // In cases (1a,2a) we're ok: The provider is deleted. The provider isn't destroyed immediately
  // but that's ok, it will be shortly.
  // In cases (1b,2b) we're also ok. The only time this happens is if the loop is shutdown before
  // our task is run. This is ok because when this happens our WaitBase method cannot be running.
  //
  // While one might want to check whether we're running in a different thread from the dispatcher
  // with dispatcher == async_get_default_dispatcher(), we don't do this as we don't assume the
  // default dispatcher has been set.

  auto raw_provider_impl = static_cast<trace::internal::TraceProviderImpl*>(provider);
  std::unique_ptr<trace::internal::TraceProviderImpl> provider_impl(raw_provider_impl);
  async::PostTask(raw_provider_impl->dispatcher(), [provider_impl = std::move(provider_impl)]() {
    // The provider will be deleted when the closure is deleted.
  });
}

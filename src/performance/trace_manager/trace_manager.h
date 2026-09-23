// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_TRACE_MANAGER_TRACE_MANAGER_H_
#define SRC_PERFORMANCE_TRACE_MANAGER_TRACE_MANAGER_H_

#include <fidl/fuchsia.tracing.controller/cpp/fidl.h>
#include <fidl/fuchsia.tracing.provider/cpp/fidl.h>
#include <fidl/fuchsia.tracing/cpp/fidl.h>
#include <lib/async/cpp/executor.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/zx/socket.h>

#include <list>
#include <queue>
#include <variant>

#include "src/lib/fxl/memory/weak_ptr.h"
#include "src/performance/trace_manager/config.h"
#include "src/performance/trace_manager/provider_connection.h"
#include "src/performance/trace_manager/trace_provider_bundle.h"
#include "src/performance/trace_manager/trace_session.h"

namespace tracing {

class TraceManager;

class TraceController : public fidl::Server<fuchsia_tracing_controller::Session> {
  friend TraceManager;

 public:
  TraceController(std::unique_ptr<TraceSession> session,
                  fidl::ServerEnd<fuchsia_tracing_controller::Session> server_end,
                  async_dispatcher_t* dispatcher, fit::closure on_disconnect);
  ~TraceController() override;

  void TerminateTracing(fit::closure cb);

  void OnAlert(const std::string& alert_name);

  uint64_t Id() { return id_; }

  // For testing.
  TraceSession* session() const { return session_.get(); }

 private:
  // |fuchsia_tracing_controller::Session| implementation.
  void StartTracing(StartTracingRequest& request, StartTracingCompleter::Sync& completer) override;
  void StopTracing(StopTracingRequest& request, StopTracingCompleter::Sync& completer) override;
  void WatchAlert(WatchAlertCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_tracing_controller::Session> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  void SendSessionStateEvent(fuchsia_tracing_controller::SessionState state);
  static fuchsia_tracing_controller::SessionState TranslateSessionState(TraceSession::State state);

  void FlushBuffers(FlushBuffersCompleter::Sync& completer) override;

  // We only set this to false when aborting.
  bool write_results_on_terminate_ = true;

  uint64_t id_;
  std::unique_ptr<TraceSession> session_;
  fidl::ServerBinding<fuchsia_tracing_controller::Session> binding_;
  std::queue<std::string> alerts_;
  std::queue<WatchAlertCompleter::Async> watch_alert_completers_;
};

class TraceManager : public fidl::Server<fuchsia_tracing_controller::Provisioner>,
                     public fidl::Server<fuchsia_tracing_provider::Registry> {
  friend TraceController;

 public:
  TraceManager(Config config, async::Executor& executor);
  ~TraceManager() override;

  // For testing.
  TraceSession* session() const;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_tracing_provider::Registry> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  void OnControllerDisconnect();

  fidl::ProtocolHandler<fuchsia_tracing_provider::Registry> GetRegistryHandler() {
    return provider_registry_bindings_.CreateHandler(this, executor_.dispatcher(),
                                                     fidl::kIgnoreBindingClosure);
  }

  fidl::ProtocolHandler<fuchsia_tracing_controller::Provisioner> GetProvisionerHandler() {
    return provisioner_bindings_.CreateHandler(this, executor_.dispatcher(),
                                               fidl::kIgnoreBindingClosure);
  }

 private:
  // |fuchsia_tracing_controller::Provisioner| implementation.
  void InitializeTracing(InitializeTracingRequest& request,
                         InitializeTracingCompleter::Sync& completer) override;
  void GetProviders(GetProvidersCompleter::Sync& completer) override;
  void GetKnownCategories(GetKnownCategoriesCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_tracing_controller::Provisioner> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  // Deprecated, but we need to support the old apis until all supported api levels drop support for
  // the api.
  void RegisterProvider(RegisterProviderRequest& request,
                        RegisterProviderCompleter::Sync& completer) override;
  // Deprecated, but we need to support the old apis until all supported api levels drop support for
  // the api.
  void RegisterProviderSynchronously(
      RegisterProviderSynchronouslyRequest& request,
      RegisterProviderSynchronouslyCompleter::Sync& completer) override;

  void RegisterV2(RegisterV2Request& request, RegisterV2Completer::Sync& completer) override;
  void RegisterV2Synchronously(RegisterV2SynchronouslyRequest& request,
                               RegisterV2SynchronouslyCompleter::Sync& completer) override;

  void RegisterProviderWorker(fidl::ClientEnd<fuchsia_tracing_provider::Provider> provider,
                              uint64_t pid, const std::string& name);
  void RegisterProviderV2Worker(fidl::ClientEnd<fuchsia_tracing_provider::ProviderV2> provider,
                                uint64_t pid, const std::string& name);

  void CloseSession();

  fidl::ServerBindingGroup<fuchsia_tracing_provider::Registry> provider_registry_bindings_;
  fidl::ServerBindingGroup<fuchsia_tracing_controller::Provisioner> provisioner_bindings_;

  const Config config_;

  uint32_t next_provider_id_ = 1u;
  std::list<std::variant<TraceProviderBundle, ProviderConnection>> providers_;
  async::Executor& executor_;
  std::optional<TraceController> trace_controller_{std::nullopt};
  fxl::WeakPtrFactory<TraceManager> weak_ptr_factory_{this};

  TraceManager(const TraceManager&) = delete;
  TraceManager(TraceManager&&) = delete;
  TraceManager& operator=(const TraceManager&) = delete;
  TraceManager& operator=(TraceManager&&) = delete;
};

}  // namespace tracing

#endif  // SRC_PERFORMANCE_TRACE_MANAGER_TRACE_MANAGER_H_

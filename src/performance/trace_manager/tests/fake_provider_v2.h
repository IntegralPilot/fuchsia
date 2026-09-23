// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_TRACE_MANAGER_TESTS_FAKE_PROVIDER_V2_H_
#define SRC_PERFORMANCE_TRACE_MANAGER_TESTS_FAKE_PROVIDER_V2_H_

#include <fidl/fuchsia.tracing.provider/cpp/fidl.h>
#include <fidl/fuchsia.tracing/cpp/fidl.h>
#include <lib/trace-engine/buffer_internal.h>
#include <lib/zx/vmo.h>

#include <gtest/gtest.h>

#include "src/performance/trace_manager/tests/fake_provider.h"

namespace tracing {
namespace test {

class FakeProviderV2 : public fidl::Server<fuchsia_tracing_provider::ProviderV2> {
 public:
  using State = FakeProvider::State;

  FakeProviderV2(zx_koid_t pid, const std::string& name);

  std::string PrettyName() const;

  // |fidl::Server<fuchsia_tracing_provider::ProviderV2>| implementation.
  void Initialize(InitializeRequest& request, InitializeCompleter::Sync& completer) override;
  void Start(StartRequest& request, StartCompleter::Sync& completer) override;
  void Stop(StopCompleter::Sync& completer) override;
  void Terminate(TerminateCompleter::Sync& completer) override;
  void NotifyBufferSaved(NotifyBufferSavedRequest& request,
                         NotifyBufferSavedCompleter::Sync& completer) override;
  void GetKnownCategories(GetKnownCategoriesCompleter::Sync& completer) override;
  void Flush(FlushCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_tracing_provider::ProviderV2> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  // Helpers to provide discrete advancement of provider state.
  void MarkStarted();
  void MarkStopped();
  void MarkTerminated();

  // Helper to move the provider to a start where it does not respond to fidl calls
  void MarkUnresponsive();
  void MarkResponsive();

  zx_koid_t pid() const { return pid_; }
  const std::string& name() const { return name_; }
  State state() const { return state_; }

  int initialize_count() const { return initialize_count_; }
  int start_count() const { return start_count_; }
  int stop_count() const { return stop_count_; }
  int terminate_count() const { return terminate_count_; }

  void SetKnownCategories(std::vector<fuchsia_tracing::KnownCategory> known_categories) {
    known_categories_ = std::move(known_categories);
  }

 private:
  void InitializeBuffer();
  void ComputeBufferSizes();
  void ResetBufferPointers();
  void InitBufferHeader();
  void UpdateBufferHeaderAfterStopped();
  void WriteInitRecord();
  void WriteRecordToBuffer(const uint8_t* data, size_t size);
  void WriteZeroLengthRecord(size_t offset);
  void WriteBytes(const uint8_t* data, size_t offset, size_t size);

  const zx_koid_t pid_;
  const std::string name_;

  State state_ = State::kReady;

  fuchsia_tracing::BufferingMode buffering_mode_;
  zx::vmo buffer_vmo_;
  std::vector<std::string> enabled_categories_;

  size_t total_buffer_size_ = 0;
  size_t durable_buffer_size_ = 0;
  size_t rolling_buffer_size_ = 0;
  size_t buffer_next_ = 0;

  int initialize_count_ = 0;
  int start_count_ = 0;
  int stop_count_ = 0;
  int terminate_count_ = 0;

  std::optional<StartCompleter::Async> start_completer_;
  std::optional<StopCompleter::Async> stop_completer_;
  std::optional<TerminateCompleter::Async> terminate_completer_;

  std::vector<fuchsia_tracing::KnownCategory> known_categories_;
  bool responsive_ = true;
  std::vector<GetKnownCategoriesCompleter::Async> pending_cat_completers_;
};

struct FakeProviderV2Binding {
  explicit FakeProviderV2Binding(std::unique_ptr<FakeProviderV2> p) : provider(std::move(p)) {}

  fidl::ClientEnd<fuchsia_tracing_provider::ProviderV2> NewBinding(async_dispatcher_t* dispatcher);

  std::unique_ptr<FakeProviderV2> provider;
  std::optional<fidl::ServerBindingRef<fuchsia_tracing_provider::ProviderV2>> binding;
};

}  // namespace test
}  // namespace tracing

#endif  // SRC_PERFORMANCE_TRACE_MANAGER_TESTS_FAKE_PROVIDER_V2_H_

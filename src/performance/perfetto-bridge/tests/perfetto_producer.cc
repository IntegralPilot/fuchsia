// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// A Basic Component that produces perfetto tracing data
#include <fidl/fuchsia.tracing.perfetto/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/async/dispatcher.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/fdio/fd.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/vmar.h>

#include <condition_variable>

#include <perfetto/base/task_runner.h>
#include <perfetto/ext/ipc/client.h>
#include <perfetto/ext/tracing/core/producer.h>
#include <perfetto/ext/tracing/core/trace_writer.h>
#include <perfetto/ext/tracing/ipc/producer_ipc_client.h>
#include <perfetto/tracing/backend_type.h>
#include <perfetto/tracing/core/data_source_descriptor.h>
#include <perfetto/tracing/core/trace_config.h>
#include <perfetto/tracing/platform.h>
#include <perfetto/tracing/tracing.h>
#include <perfetto/tracing/track_event.h>

#include "shared_vmo.h"

#include <protos/perfetto/trace/track_event/process_descriptor.gen.h>

class FuchsiaProducer : public perfetto::Producer {
 public:
  explicit FuchsiaProducer(int perfetto_service_fd) {
    perfetto::base::ScopedSocketHandle scoped_handle(perfetto_service_fd);
    perfetto::ipc::Client::ConnArgs conn_args(std::move(scoped_handle));

    // Here we have a somewhat convoluted approach to sharing memory. The goal is to share a buffer
    // between us, the producer that we can write to it, and perfetto-bridge, the client that can
    // read from it.
    //
    // The primary limitation is that Perfetto expects to share a buffer by sending the fd over a
    // socket, posix style. This doesn't work in Fuchsia because handle sharing must be done over
    // FIDL. Fuchsia isn't currently an officially supported platform of Perfetto so porting
    // Perfetto to use FIDL when running on Fuchsia is, in the mean time, somewhat infeasible. So
    // instead we use the following workaround:
    //
    // 1) We instruct the Perfetto server on how to create the buffer we want via the following
    //    callback
    // 2) Perfetto server runs the callback and hands perfetto-bridge the fd via callback.
    //    2a) Note that perfetto-bridge runs in the same thread as perfetto-server so this hand off
    //        can work
    // 3) Perfetto-bridge gets and duplicates the underlying VMO handle for the FD.
    // 4) Perfetto-bridge calls our "ProvideBuffer" FIDL call to give us the duplicate handle to the
    //    buffer
    conn_args.receive_shmem_fd_cb_fuchsia = []() {
      zx::vmo vmo;
      zx_status_t result = zx::vmo::create(8 * zx_system_get_page_size(), 0, &vmo);
      if (result != ZX_OK) {
        FX_PLOGS(ERROR, result) << "Failed to create shmem vmo!";
        return -1;
      }

      int shmem_fd;
      result = fdio_fd_create(vmo.release(), &shmem_fd);
      if (result != ZX_OK) {
        FX_PLOGS(ERROR, result) << "Failed to create fd for vmo!";
        return -1;
      }
      return shmem_fd;
    };

    FX_LOGS(INFO) << "Connecting to Perfetto";
    perfetto_service_ = perfetto::ProducerIPCClient::Connect(
        std::move(conn_args), this, "perfetto_producer", task_runner_.get());
  }

  void OnConnect() override {
    FX_LOGS(INFO) << "OnConnect";
    std::lock_guard<std::mutex> l(lock_);
    state_ = ProducerState::Connected;
    state_cv_.notify_all();

    perfetto::DataSourceDescriptor new_registration;
    new_registration.set_name("track_event");
    new_registration.set_will_notify_on_start(true);
    new_registration.set_will_notify_on_stop(true);
    new_registration.set_handles_incremental_state_clear(false);
    new_registration.set_track_event_descriptor_raw("test");
    perfetto_service_->RegisterDataSource(new_registration);
  }

  void OnDisconnect() override {
    std::lock_guard<std::mutex> l(lock_);
    state_ = ProducerState::Disconnected;
    state_cv_.notify_all();
  }

  void OnTracingSetup() override {
    FX_LOGS(INFO) << "OnTracingSetup";
    std::unique_lock<std::mutex> l(lock_);
    state_cv_.wait(l, [this]() { return state_ == ProducerState::Connected; });
    state_ = ProducerState::TracingSetUp;
    state_cv_.notify_all();
  }

  void OnStartupTracingSetup() override {}

  void SetupDataSource(perfetto::DataSourceInstanceID id,
                       const perfetto::DataSourceConfig& config) override {}

  void StartDataSource(perfetto::DataSourceInstanceID id,
                       const perfetto::DataSourceConfig& config) override {
    FX_LOGS(INFO) << "Start Data Source";
    std::unique_lock<std::mutex> l(lock_);
    arbiter_cv_.wait(l, [this]() { return shmem_arbiter_ != nullptr; });
    FX_LOGS(INFO) << "Creating Writers";

    writer_ =
        shmem_arbiter_->CreateTraceWriter(static_cast<perfetto::BufferID>(config.target_buffer()),
                                          perfetto::BufferExhaustedPolicy::kDrop);
    perfetto_service_->RegisterTraceWriter(writer_->writer_id(), config.target_buffer());
    perfetto_service_->NotifyDataSourceStarted(id);

    // For testing perfetto-bridge, emit just over 30000 bytes of trace data to ensure we can see it
    // from the test client.
    FX_LOGS(INFO) << "Writing Events";
    for (unsigned i = 0; i < 1000; i++) {
      auto message = writer_->NewTracePacket();
      auto track_event = message->set_track_event();
      track_event->set_name("Track Event");
      track_event->add_categories("test");
      track_event->set_counter_value(0xFFFFFFFF);
    }
    FX_LOGS(INFO) << "/Writing Events";

    writer_->Flush();
  }

  void StopDataSource(perfetto::DataSourceInstanceID id) override {
    perfetto_service_->UnregisterTraceWriter(writer_->writer_id());
    writer_.reset();
    perfetto_service_->NotifyDataSourceStopped(id);
  }

  void Flush(perfetto::FlushRequestID id, const perfetto::DataSourceInstanceID* data_source_ids,
             size_t num_data_sources, perfetto::FlushFlags) override {
    writer_->Flush([&]() { perfetto_service_->NotifyFlushComplete(id); });
  }

  void ClearIncrementalState(const perfetto::DataSourceInstanceID* data_source_ids,
                             size_t num_data_sources) override {}

  zx::result<> ConfigureSharedMemory(fidl::ClientEnd<fuchsia_io::File> buffer) {
    std::unique_lock<std::mutex> l(lock_);
    state_cv_.wait(l, [this]() { return state_ == ProducerState::TracingSetUp; });

    fidl::SyncClient<fuchsia_io::File> client(std::move(buffer));
    fidl::Result<fuchsia_io::File::GetBackingMemory> vmo_result = client->GetBackingMemory({
        fuchsia_io::VmoFlags::kRead | fuchsia_io::VmoFlags::kWrite,
    });

    if (vmo_result.is_error()) {
      return vmo_result.error_value().is_domain_error()
                 ? zx::error<zx_status_t>(vmo_result.error_value().domain_error())
                 : zx::error(ZX_ERR_BAD_HANDLE);
    }

    shared_memory_ = SharedVmo::AdoptVmo(std::move(vmo_result->vmo()));
    if (shared_memory_ == nullptr) {
      return zx::error(ZX_ERR_BAD_HANDLE);
    }

    shmem_arbiter_ = perfetto::SharedMemoryArbiter::CreateInstance(
        shared_memory_.get(), zx_system_get_page_size(),
        perfetto::SharedMemoryABI::ShmemMode::kDefault, perfetto_service_.get(),
        task_runner_.get());
    if (shmem_arbiter_ == nullptr) {
      return zx::error(ZX_ERR_NO_MEMORY);
    }
    FX_LOGS(INFO) << "Arbiter is Setup";
    arbiter_cv_.notify_all();
    return zx::ok();
  }

  ~FuchsiaProducer() override { shmem_arbiter_->TryShutdown(); }

 private:
  std::unique_ptr<perfetto::TracingService::ProducerEndpoint> perfetto_service_;

  std::unique_ptr<perfetto::TraceWriter> writer_;
  std::unique_ptr<perfetto::SharedMemory> shared_memory_;
  std::unique_ptr<perfetto::base::TaskRunner> task_runner_ =
      perfetto::Platform::GetDefaultPlatform()->CreateTaskRunner(
          {.name_for_debugging = "FuchsiaProducer"});

  // The ordering of when perfetto-bridge will send us the SharedMemory and when Perfetto itself is
  // actually ready for use to register is indeterminate. We need to ensure that calls happen in the
  // following order:
  // 1) We are notified that we are connected
  // 2) We are notified that tracing is set up
  // 3) We receive the shared memory from perfetto-bridge
  // 4) We register our shared memory arbiter with the Perfetto service
  // 5) We begin writing to the shared memory
  std::mutex lock_;
  // Writers can't start writing until the shared memory set up. Writers will wait on this cv to be
  // notified when SharedMemoryArbiter is ready for them.
  std::condition_variable arbiter_cv_;
  std::unique_ptr<perfetto::SharedMemoryArbiter> shmem_arbiter_;

  enum class ProducerState {
    Connected,
    TracingSetUp,
    Disconnected,
  };
  // The Perfetto Server needs to have tracing fully set up before we set up the shared memory and
  // register it with the perfetto service. The Perfetto service will send us "OnTracingSetUp" when
  // it's ready.
  std::condition_variable state_cv_;
  ProducerState state_ = ProducerState::Disconnected;
};

class PerfettoTraceProvider : public fidl::Server<fuchsia_tracing_perfetto::BufferReceiver> {
 public:
  explicit PerfettoTraceProvider(std::unique_ptr<FuchsiaProducer> producer)
      : producer_(std::move(producer)) {}

  static zx::result<fidl::ServerBindingRef<fuchsia_tracing_perfetto::BufferReceiver>> Serve(
      async_dispatcher_t* dispatcher) {
    // 1) Create sockets to communicate to the remote perfetto instance. We're going to give one end
    //    to perfetto-bridge so it will connect us to the system perfetto.
    zx::socket local_perfetto_socket, remote_perfetto_socket;
    zx_status_t status = zx::socket::create(0, &local_perfetto_socket, &remote_perfetto_socket);
    if (status != ZX_OK) {
      FX_PLOGS(ERROR, status) << "Failed to create a socket pair for perfetto";
      return zx::error(status);
    }

    // 2) Implement buffer receiver and create a client end
    zx::result endpoints = fidl::CreateEndpoints<fuchsia_tracing_perfetto::BufferReceiver>();
    if (endpoints.is_error()) {
      FX_PLOGS(ERROR, endpoints.error_value())
          << "Failed to create endpoint pair for perfetto server";
      return endpoints.take_error();
    }

    int perfetto_service_fd;
    const zx_status_t fd_status =
        fdio_fd_create(local_perfetto_socket.release(), &perfetto_service_fd);
    if (fd_status != ZX_OK) {
      FX_PLOGS(ERROR, fd_status) << "Failed to create an fd for perfetto";
      return zx::error(status);
    }

    std::unique_ptr<PerfettoTraceProvider> impl = std::make_unique<PerfettoTraceProvider>(
        std::make_unique<FuchsiaProducer>(perfetto_service_fd));

    const fidl::ServerBindingRef binding_ref =
        fidl::BindServer(dispatcher, std::move(std::move(endpoints->server)), std::move(impl));
    auto trace_buffer_receiver =
        fuchsia_tracing_perfetto::TraceBuffer::WithFromServer(std::move(endpoints->client));

    // 3) Connect to perfetto-bridge via ProducerConnector
    zx::result client_end = component::Connect<fuchsia_tracing_perfetto::ProducerConnector>();
    if (client_end.is_error()) {
      FX_LOGS(ERROR) << "Failed to connect to Producer Connector: " << client_end.status_string();
      return client_end.take_error();
    }

    // 4) Send one socket and our BufferReceiver client end to perfetto-bridge using
    //    ConnectProducer
    FX_LOGS(INFO) << "Connecting to perfetto-bridge";
    const fidl::SyncClient client{std::move(*client_end)};
    auto result = client->ConnectProducer(
        {std::move(remote_perfetto_socket), std::move(trace_buffer_receiver)});
    if (result.is_error()) {
      FX_LOGS(ERROR) << "ConnectProducer failed! " << result.error_value();
      return zx::error(ZX_ERR_NOT_CONNECTED);
    }

    // 5) When tracing starts, the Perfetto service sends a shared memory buffer to us using
    //    BufferReceiver for us to write trace data into.
    return zx::ok(binding_ref);
  }

  void ProvideBuffer(ProvideBufferRequest& request,
                     ProvideBufferCompleter::Sync& completer) override {
    FX_LOGS(INFO) << "Received Buffer from perfetto-bridge";
    completer.Reply({producer_->ConfigureSharedMemory(std::move(request.buffer()))});
  }

 private:
  std::unique_ptr<FuchsiaProducer> producer_;
};

int main() {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  async_dispatcher_t* dispatcher = loop.dispatcher();

  if (PerfettoTraceProvider::Serve(dispatcher).is_error()) {
    FX_LOGS(ERROR) << "Failed to start PerfettoTraceProvider!";
    return 1;
  }

  FX_LOGS(INFO) << "Serving Perfetto Trace Provider";
  loop.Run();
}

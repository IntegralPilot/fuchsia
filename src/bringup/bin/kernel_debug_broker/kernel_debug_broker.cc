// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <fidl/fuchsia.kernel/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async-loop/loop.h>
#include <lib/async/cpp/task.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fidl/cpp/wire/connect_service.h>
#include <lib/kcounter/provider.h>
#include <lib/kernel-debug/kernel-debug.h>
#include <lib/svc/outgoing.h>
#include <lib/zx/job.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/process.h>
#include <zircon/processargs.h>
#include <zircon/status.h>

#include <iterator>
#include <string_view>

#include "src/sys/lib/stdout-to-debuglog/cpp/stdout-to-debuglog.h"

// An instance of a zx_service_provider_t.
//
// Includes the |ctx| pointer for the zx_service_provider_t.
using zx_service_provider_instance_t = struct zx_service_provider_instance {
  // The service provider for which this structure is an instance.
  const zx_service_provider_t* provider;

  // The |ctx| pointer returned by the provider's |init| function, if any.
  void* ctx;
};

static void provider_init(async_dispatcher_t* dispatcher,
                          zx_service_provider_instance_t* instance) {
  if (instance->provider->ops->init) {
    zx_status_t status = async::PostTask(dispatcher, [instance]() {
      zx_status_t status = instance->provider->ops->init(&instance->ctx);
      ZX_ASSERT_MSG(status == ZX_OK, "%s", zx_status_get_string(status));
    });
    ZX_ASSERT_MSG(status == ZX_OK, "%s", zx_status_get_string(status));
  }
}

static zx_status_t provider_publish(zx_service_provider_instance_t* instance,
                                    async_dispatcher_t* dispatcher,
                                    const fbl::RefPtr<fs::PseudoDir>& dir) {
  const zx_service_provider_t* provider = instance->provider;

  if (!provider->services || !provider->ops->connect)
    return ZX_ERR_INVALID_ARGS;

  for (size_t i = 0; provider->services[i]; ++i) {
    const char* service_name = provider->services[i];
    zx_status_t status = dir->AddEntry(
        service_name,
        fbl::MakeRefCounted<fs::Service>([dispatcher, instance, service_name](zx::channel request) {
          return async::PostTask(dispatcher, [instance, dispatcher, service_name,
                                              request = std::move(request)]() mutable {
            instance->provider->ops->connect(instance->ctx, dispatcher, service_name,
                                             request.release());
          });
        }));
    if (status != ZX_OK) {
      for (size_t j = 0; j < i; ++j)
        dir->RemoveEntry(provider->services[j]);
      return status;
    }
  }

  return ZX_OK;
}

static void provider_release(async_dispatcher_t* dispatcher,
                             zx_service_provider_instance_t* instance) {
  if (instance->provider->ops->release) {
    async::PostTask(dispatcher, [instance]() { instance->provider->ops->release(instance->ctx); });
  }
  instance->ctx = nullptr;
}

static zx_status_t provider_load(zx_service_provider_instance_t* instance,
                                 async_dispatcher_t* dispatcher,
                                 const fbl::RefPtr<fs::PseudoDir>& dir) {
  if (instance->provider->version != SERVICE_PROVIDER_VERSION) {
    return ZX_ERR_INVALID_ARGS;
  }

  provider_init(dispatcher, instance);

  if (zx_status_t status = provider_publish(instance, dispatcher, dir); status != ZX_OK) {
    provider_release(dispatcher, instance);
    return status;
  }

  return ZX_OK;
}

int main(int argc, char** argv) {
  if (zx_status_t status = StdoutToDebuglog::Init(); status != ZX_OK) {
    fprintf(stderr, "kernel_debug_broker: unable to forward stdout to debuglog: %s\n",
            zx_status_get_string(status));
    return 1;
  }

  fidl::ClientEnd<fuchsia_io::Directory> svc;
  {
    zx::result result = component::OpenServiceRoot();
    if (result.is_error()) {
      fprintf(stderr, "kernel_debug_broker: unable to open service root: %s\n",
              result.status_string());
      return 1;
    }
    svc = std::move(result.value());
  }

  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  async_dispatcher_t* dispatcher = loop.dispatcher();
  svc::Outgoing outgoing(dispatcher);

  // Get the debug resource.
  zx::resource debug_resource;
  {
    zx::result client = component::ConnectAt<fuchsia_kernel::DebugResource>(svc);
    if (client.is_error()) {
      fprintf(stderr, "kernel_debug_broker: unable to connect to %s: %s\n",
              fidl::DiscoverableProtocolName<fuchsia_kernel::DebugResource>,
              client.status_string());
      return 1;
    }

    fidl::WireResult result = fidl::WireCall(client.value())->Get();
    if (!result.ok()) {
      fprintf(stderr, "kernel_debug_broker: unable to get root resource: %s\n",
              result.status_string());
      return 1;
    }
    auto& response = result.value();
    debug_resource = std::move(response.resource);
  }

  if (zx_status_t status = outgoing.ServeFromStartupInfo(); status != ZX_OK) {
    fprintf(stderr, "kernel_debug_broker: error: Failed to serve outgoing directory: %d (%s).\n",
            status, zx_status_get_string(status));
    return 1;
  }

  zx_service_provider_instance_t service_providers[] = {
      {
          .provider = kernel_debug_get_service_provider(),
          .ctx = reinterpret_cast<void*>(static_cast<uintptr_t>(debug_resource.get())),
      },
      {
          .provider = kcounter_get_service_provider(),
          .ctx = reinterpret_cast<void*>(dispatcher),
      },
  };

  for (size_t i = 0; i < std::size(service_providers); ++i) {
    if (zx_status_t status = provider_load(&service_providers[i], dispatcher, outgoing.svc_dir());
        status != ZX_OK) {
      fprintf(stderr, "kernel_debug_broker: error: Failed to load service provider %zu: %d (%s).\n",
              i, status, zx_status_get_string(status));
      return 1;
    }
  }

  zx_status_t status = loop.Run();

  for (auto& service_provider : service_providers) {
    provider_release(dispatcher, &service_provider);
  }

  return status;
}

// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-options/boot-options.h>
#include <lib/thread_sampler/thread_sampler.h>
#include <zircon/rights.h>

#include <fbl/alloc_checker.h>
#include <fbl/ref_ptr.h>
#include <kernel/ffi.h>
#include <ktl/utility.h>
#include <object/handle.h>
#include <object/sampler_dispatcher.h>

extern "C" {

zx_status_t cpp_sampler_dispatcher_create(
    const zx_sampler_config_t* config,
    ffi::Uninitialized<KernelHandle<SamplerDispatcher>>* handle_out) {
  // Set up the global sampler if it hasn't been set up yet.
  zx::result res = sampler::gThreadSampler.SetUp(*config);
  if (res.is_error()) {
    return res.status_value();
  }

  fbl::AllocChecker ac;
  auto disp = fbl::AdoptRef(new (&ac) SamplerDispatcher());
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }

  handle_out->Initialize(ktl::move(disp));
  return ZX_OK;
}

zx_status_t cpp_sampler_dispatcher_start(const SamplerDispatcher* dispatcher) {
  return sampler::gThreadSampler.Start().status_value();
}

zx_status_t cpp_sampler_dispatcher_stop(const SamplerDispatcher* dispatcher) {
  return sampler::gThreadSampler.Stop().status_value();
}

zx_status_t cpp_sampler_dispatcher_read_user(const SamplerDispatcher* dispatcher, void* ptr,
                                             size_t len, size_t* actual_out) {
  auto [status, read] = sampler::gThreadSampler.ReadUser(user_out_ptr<void>(ptr), len);
  *actual_out = read;
  return status;
}

}  // extern "C"

// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/crashlog.h>
#include <lib/zbitl/error-stdio.h>
#include <lib/zbitl/image.h>
#include <lib/zbitl/memory.h>
#include <lib/zx/result.h>
#include <stdio.h>

#include <fbl/ref_ptr.h>
#include <ktl/byte.h>
#include <ktl/span.h>
#include <ktl/type_traits.h>
#include <phys/boot-constants.h>
#include <platform/mexec.h>
#include <vm/vm_object.h>

#include <ktl/enforce.h>

namespace {

// Mexec data as gleaned from the physboot hand-off.
auto MexecDataZbi() { return zbitl::View{kBootConstants.mexec_data.get()}; }

}  // namespace

zx::result<size_t> WriteMexecData(ktl::span<ktl::byte> buffer) {
  // Storage or write errors resulting from a span-backed Image imply buffer
  // overflow.
  constexpr auto error = [](const auto& err) -> zx::result<size_t> {
    return zx::error{err.storage_error ? ZX_ERR_BUFFER_TOO_SMALL : ZX_ERR_INTERNAL};
  };
  constexpr auto extend_error = [](const auto& err) -> zx::result<size_t> {
    return zx::error{err.write_error ? ZX_ERR_BUFFER_TOO_SMALL : ZX_ERR_INTERNAL};
  };

  zbitl::Image image(buffer);
  if (auto result = image.clear(); result.is_error()) {
    zbitl::PrintViewError(result.error_value());
    return error(result.error_value());
  }

  {
    zbitl::View zbi = MexecDataZbi();
    auto extend_result = image.Extend(zbi.begin(), zbi.end());
    // First check for errors in zbi iteration as the iterator jumps to the end on errors which
    // could then cause spurious errors from Extend.
    if (auto result = zbi.take_error(); result.is_error()) {
      zbitl::PrintViewError(result.error_value());
      return zx::error{ZX_ERR_INTERNAL};
    }

    if (extend_result.is_error()) {
      zbitl::PrintViewCopyError(extend_result.error_value());
      return extend_error(extend_result.error_value());
    }
  }

  // Propagate any stashed crashlog to the next kernel.
  if (const fbl::RefPtr<VmObject> crashlog = crashlog_get_stashed()) {
    const zbi_header_t header = {
        .type = ZBI_TYPE_CRASHLOG,
        .length = static_cast<uint32_t>(crashlog->size()),
    };
    auto result = image.Append(header);
    if (result.is_error()) {
      printf("mexec: could not append crashlog: ");
      zbitl::PrintViewError(result.error_value());
      return error(result.error_value());
    }
    auto it = ktl::move(result).value();
    ktl::span<ktl::byte> payload = it->payload;
    zx_status_t status = crashlog->Read(payload.data(), 0, payload.size());
    if (status != ZX_OK) {
      return zx::error{status};
    }
  }

  return zx::ok(image.size_bytes());
}

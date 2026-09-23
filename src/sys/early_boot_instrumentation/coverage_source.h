// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SYS_EARLY_BOOT_INSTRUMENTATION_COVERAGE_SOURCE_H_
#define SRC_SYS_EARLY_BOOT_INSTRUMENTATION_COVERAGE_SOURCE_H_

#include <fidl/fuchsia.boot/cpp/wire.h>
#include <lib/zx/channel.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>

#include <string_view>

#include <fbl/unique_fd.h>
#include <sdk/lib/vfs/cpp/pseudo_dir.h>

namespace early_boot_instrumentation {

// Subdirectory names for each type of debugdata.
static constexpr std::string_view kDynamicDir = "dynamic";
static constexpr std::string_view kStaticDir = "static";

// llvm-profile sink and extension.
static constexpr std::string_view kLlvmSink = "llvm-profile";
static constexpr std::string_view kLlvmSinkExtension = "profraw";

// Alias for str to unique_ptr<PseudoDir> map that allows lookup by string_view.
using SinkDirMap = std::map<std::string, std::unique_ptr<vfs::PseudoDir>, std::less<>>;

// Given a handle to `boot_debug_data_dir`, will extract the debugdata vmos from it,
// and add them as VMO file into `sink_map`.
//
// The kernel encodes sink name, data type and module information in the path as described in
// "lib/instrumentation/debugdata.h".
//
// Usually `boot_debug_data_dir` is '/boot/kernel/i'.
//
// Each of the surfaced files is exposed with 'module_name-n.suffix', where 'n' is the index of the
// file, such that repeated module names are not overwritten.
zx::result<> ExposeBootDebugdata(fbl::unique_fd& boot_debug_data_dir, SinkDirMap& sink_map);

// Given a handle to `boot_logs_dir`, expose all files under `logs` namespace.
//
// Usually logs will be exposed under `/boot/kernel/i/logs`.
zx::result<> ExposeLogs(fbl::unique_fd& boot_logs_dir, vfs::PseudoDir& out_dir);

// Given a channel speaking the `fuchsia.boot.Items` protocol, this will obtain the
// `ZBI_ITEM_TYPE_DEBUGDATA`, of type `static`, to their respective sink in `sink_map`.
zx::result<> ExposeDebugDataZbiItem(fidl::ClientEnd<fuchsia_boot::Items> boot_items,
                                    vfs::PseudoDir& log_out_dir, SinkDirMap& sink_map);

// Given a channel speaking the `fuchsia.boot.SvcStash` protocol, this will extract all published
// debug data, and return a map from 'sink_name' to a root directory for each sink. Each root
// directory contains two child directories, 'static' and 'dynamic'.
//
// Following the `debugdata.Publisher/Publish` protocol, data associated to a publish request is
// considered 'static' if the provided token(`zx::eventpair`) in the request has the
// `ZX_EVENTPAIR_PEER_CLOSED` signal. Otherwise, it's considered 'dynamic'.
//
// Once the data associated with a request has been tagged as 'static' or 'dynamic' it is exposed
// as a `vfs::VmoFile` under the respective root directory of the `sink_name` associated with the
// request.
//
// The filenames are generated as follow:
//    Each stashed handle is assigned an index (monotonically increasing) 'svc_id'.
//    Each request in the stashed handle is assigned another index (monotonically increasing)
//    Each published vmo has a names 'vmo_name'.
//    'req_id'. Then the name generated for the data associated with the request(svc_id, req_id) =
//    "svc_id"-"req_id"."vmo_name".
// In essence "vmo_name" acts like the extension.
SinkDirMap ExtractDebugData(fidl::ServerEnd<fuchsia_boot::SvcStash> svc_stash);

}  // namespace early_boot_instrumentation

#endif  // SRC_SYS_EARLY_BOOT_INSTRUMENTATION_COVERAGE_SOURCE_H_

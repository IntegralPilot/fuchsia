// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/console/commands/verb_aspace.h"

#include "src/developer/debug/zxdb/client/process.h"
#include "src/developer/debug/zxdb/client/session.h"
#include "src/developer/debug/zxdb/client/target.h"
#include "src/developer/debug/zxdb/common/string_util.h"
#include "src/developer/debug/zxdb/console/command.h"
#include "src/developer/debug/zxdb/console/command_utils.h"
#include "src/developer/debug/zxdb/console/console.h"
#include "src/developer/debug/zxdb/console/format_table.h"
#include "src/developer/debug/zxdb/console/verbs.h"
#include "src/developer/debug/zxdb/expr/number_parser.h"
#include "src/developer/debug/zxdb/format/output_buffer.h"
#include "src/lib/fxl/strings/string_printf.h"

namespace zxdb {

namespace {

constexpr int kAddressLimitSwitch = 1;

const char kAspaceShortHelp[] = "aspace / as: Show address space for a process.";
const char kAspaceUsage[] = "aspace [ --limit <address> ] [ <address> ]";
const char kAspaceHelp[] = R"(
  Alias: "as"

  Shows the address space map for the given process.

  With no parameters, it shows the entire process address map.
  You can pass a single address and it will show all the regions that
  contain it.

  In addition to the address range, the output shows the koid of the VMO mapped
  to that location, the starting offset it was mapped at, and the number of
  committed pages in that region.

  Tip: To see more information about a VMO, use "handle -k <koid>".

Arguments

  --limit <address>
      Only reports mappings at addresses below <address>. This can be useful
      when a process has very many mappings that may not fit in a single
      terminal buffer.

Committed pages

  The "Cmt.Pgs" column shows the number of committed pages (not bytes) in that
  memory region in the mapped VMO. This can be surprising for memory mapped
  files like blobs and other shared VMOs.

  If a VMO is a child (as in the case of mapped blobs), the original data will
  be present in the parent VMO but the child VMO that is actually mapped will
  indirectly reference this data. The only pages in the child that will count as
  committed are those that are duplicated due to copy-on-write. This is why
  blobs and other files that are not modified will have a 0 committed page
  count.

Examples

  aspace
  aspace --limit 0x100000000
  aspace 0x530b010dc000
  process 2 aspace
)";

std::string PrintRegionSize(uint64_t size) {
  const uint64_t kOneK = 1024u;
  const uint64_t kOneM = kOneK * kOneK;
  const uint64_t kOneG = kOneM * kOneK;
  const uint64_t kOneT = kOneG * kOneK;

  if (size < kOneK)
    return fxl::StringPrintf("%" PRIu64 "B", size);
  if (size < kOneM)
    return fxl::StringPrintf("%" PRIu64 "K", size / kOneK);
  if (size < kOneG)
    return fxl::StringPrintf("%" PRIu64 "M", size / kOneM);
  if (size < kOneT)
    return fxl::StringPrintf("%" PRIu64 "G", size / kOneG);
  return fxl::StringPrintf("%" PRIu64 "T", size / kOneT);
}

std::string PrintRegionName(uint64_t depth, const std::string& name) {
  return std::string(depth * 2, ' ') + name;
}

// Set has_shared if the platform supports this flag.
std::string PrintRegionMmuFlags(const debug_ipc::AddressRegion& region, bool has_shared) {
  std::string str;
  str += region.read ? 'r' : '-';
  str += region.write ? 'w' : '-';
  str += region.execute ? 'x' : '-';
  if (has_shared) {
    str += region.shared ? 's' : 'p';
  }
  return str;
}

void OnAspaceComplete(fxl::RefPtr<CommandContext> cmd_context, const Err& err,
                      std::vector<debug_ipc::AddressRegion> map, bool print_totals,
                      std::optional<uint64_t> addr_limit) {
  Console* console = Console::get();
  if (err.has_error()) {
    console->Output(err);
    return;
  }

  if (map.empty()) {
    console->Output("Region not mapped.");
    return;
  }

  uint64_t total_mapped = 0;
  uint64_t total_committed = 0;

  const uint64_t page_size = console->context().session()->arch_info().page_size();
  debug::Platform platform = cmd_context->GetConsoleContext()->session()->platform();

  // Only Linux has the shared bit.
  bool has_shared = platform == debug::Platform::kLinux;

  std::vector<std::vector<std::string>> rows;
  for (const auto& region : map) {
    std::vector<std::string> row;

    if (addr_limit && region.base > addr_limit)
      break;

    row.push_back(to_hex_string(region.base));
    row.push_back(to_hex_string(region.base + region.size));
    row.push_back(PrintRegionMmuFlags(region, has_shared));
    row.push_back(PrintRegionSize(region.size));

    if (platform == debug::Platform::kFuchsia) {
      // Fuchsia-specific parts.

      // Only show VMO information for regions which have a VMO koid. Regions with no VMO will be
      // VMARs and showing offset and committed pages is misleading.
      bool has_koid = region.vmo_koid != 0;
      row.push_back(has_koid ? std::to_string(region.vmo_koid) : std::string());
      row.push_back(has_koid ? to_hex_string(region.vmo_offset) : std::string());
      row.push_back(has_koid ? std::to_string(region.committed_bytes / page_size) : std::string());

      if (has_koid) {
        total_mapped += region.size;
        total_committed += region.committed_bytes;
      }
    } else {
      // Non-Fuchsia has only the offset which is unconditionally shown.
      row.push_back(to_hex_string(region.vmo_offset));
    }
    row.push_back(PrintRegionName(region.depth, region.name));

    rows.push_back(std::move(row));
  }

  std::vector<ColSpec> colspec;
  colspec.push_back(ColSpec(Align::kRight, 0, "Start", 2));
  colspec.push_back(ColSpec(Align::kRight, 0, "End", 2));
  colspec.push_back(ColSpec(Align::kLeft, 0, "Prot", 1));
  colspec.push_back(ColSpec(Align::kRight, 0, "Size", 1));
  if (platform == debug::Platform::kFuchsia) {
    colspec.push_back(ColSpec(Align::kRight, 0, "Koid", 1));
    colspec.push_back(ColSpec(Align::kRight, 0, "Offset", 1));
    colspec.push_back(ColSpec(Align::kRight, 0, "Cmt.Pgs", 1));
  } else {
    colspec.push_back(ColSpec(Align::kRight, 0, "Offset", 1));
  }
  colspec.push_back(ColSpec(Align::kLeft, 0, "Name", 1));

  OutputBuffer out;
  FormatTable(colspec, rows, &out);

  // Format the section at the bottom showing statistics. These are formatted so the "=" align
  // horizontally (hence extra left-spacing on the strings).
  out.Append("\n");
  out.Append(Syntax::kHeading, "              Page size: ");
  out.Append(std::to_string(page_size));
  out.Append("\n");

  if (platform == debug::Platform::kFuchsia && print_totals) {
    out.Append(Syntax::kHeading, "     Total mapped bytes: ");
    out.Append(std::to_string(total_mapped));
    out.Append("\n");

    out.Append(Syntax::kHeading, "  Total committed pages: ");
    out.Append(std::to_string(total_committed / page_size));
    out.Append(" = " + std::to_string(total_committed) + " bytes\n");
    out.Append("                         (See \"help aspace\" for what committed pages mean.)\n");
  }

  console->Output(out);
}

void RunVerbAspace(const Command& cmd, fxl::RefPtr<CommandContext> cmd_context) {
  // Only a process can be specified.
  if (Err err = cmd.ValidateNouns({Noun::kProcess}); err.has_error())
    return cmd_context->ReportError(err);

  bool print_totals = true;
  uint64_t address = 0;
  if (cmd.args().size() == 1) {
    if (Err err = ReadUint64Arg(cmd, 0, "address", &address); err.has_error())
      return cmd_context->ReportError(err);
    print_totals = false;  // Adding up totals for a subregion is misleading.
  } else if (cmd.args().size() > 1) {
    return cmd_context->ReportError(
        Err(ErrType::kInput, "\"aspace\" takes zero or one parameter."));
  }

  std::optional<uint64_t> addr_limit;
  if (cmd.HasSwitch(kAddressLimitSwitch)) {
    auto err_or = StringToNumber(ExprLanguage::kC, cmd.GetSwitchValue(kAddressLimitSwitch));
    if (err_or.has_error()) {
      return cmd_context->ReportError(err_or.err());
    }

    uint64_t limit = 0;
    if (auto err = err_or.value().PromoteTo64(&limit); err.has_error()) {
      return cmd_context->ReportError(err);
    }

    addr_limit = limit;
    print_totals = false;
  }

  if (Err err = AssertRunningTarget(cmd_context->GetConsoleContext(), "aspace", cmd.target());
      err.has_error())
    return cmd_context->ReportError(err);

  cmd.target()->GetProcess()->GetAspace(
      address, [cmd_context, print_totals, addr_limit](const Err& err,
                                                       std::vector<debug_ipc::AddressRegion> map) {
        OnAspaceComplete(cmd_context, err, map, print_totals, addr_limit);
      });
}

}  // namespace

VerbRecord GetAspaceVerbRecord() {
  auto record = VerbRecord(&RunVerbAspace, {"aspace", "as"}, kAspaceShortHelp, kAspaceUsage,
                           kAspaceHelp, CommandGroup::kQuery);
  record.switches.push_back(SwitchRecord(kAddressLimitSwitch, true, "limit", 'l'));
  return record;
}

}  // namespace zxdb

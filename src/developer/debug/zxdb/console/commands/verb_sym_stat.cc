// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/console/commands/verb_sym_stat.h"

#include "src/developer/debug/zxdb/client/process.h"
#include "src/developer/debug/zxdb/client/session.h"
#include "src/developer/debug/zxdb/console/command.h"
#include "src/developer/debug/zxdb/console/console.h"
#include "src/developer/debug/zxdb/console/format_table.h"
#include "src/developer/debug/zxdb/console/verbs.h"
#include "src/developer/debug/zxdb/format/output_buffer.h"
#include "src/developer/debug/zxdb/symbols/module_symbol_status.h"
#include "src/developer/debug/zxdb/symbols/process_symbols.h"
#include "src/developer/debug/zxdb/symbols/system_symbols.h"
#include "src/lib/fxl/strings/string_printf.h"

namespace zxdb {

namespace {

constexpr int kDumpIndexSwitch = 1;

const char kSymStatShortHelp[] = "sym-stat: Print process symbol status.";
const char kSymStatUsage[] = "sym-stat [ --dump-index ]";
const char kSymStatHelp[] = R"(
  Prints out symbol information.

  With no arguments, this refreshes the symbol index and shows global
  information and information for the current (or specified) process.

  The global information includes the symbol search paths and how many files are
  indexed from each location. When a symbol path is a "build-id" directory
  hierarchy, the output prints "folder" since these are not indexed in advance.
  Instead, build-id folders are searched for a matching file name on demand.

  If there is a current process the output will includes which libraries are
  loaded, how many symbols each has, and where the symbol file is located.

Symbol troubleshooting

  Processes with zero or very few symbols likely indicate the file has had the
  symbols stripped (there are sometimes a few symbols even in stripped
  binaries). Make sure an unstripped binary exists in a directory in the
  "symbol-paths" setting and restart either the debugged program or the
  debugger.

  To append to the symbol search path, run for example:

    set symbol-paths += /home/you/project/out/debug/exe.unstripped

  or use the "-s" command-line flag. See "get symbol-paths" for more information
  on this setting and to view the current list.

Arguments

  --dump-index

      Dumps the symbol index which maps build IDs to local file paths. This
      can be useful for debugging cases of missing symbols.

      Note that this does not print out the information from "build-id" folders
      (denoted "folder" in the sym-stat output). These are loaded on-demand by
      searching for a file name with the corresponding build ID.

Example

  sym-stat

  process 2 sym-stat

  sym-stat --dump-index
)";

void SummarizeProcessSymbolStatus(ConsoleContext* context, Process* process, OutputBuffer* out) {
  // Get modules sorted by load address.
  std::vector<ModuleSymbolStatus> modules = process->GetSymbols()->GetStatus();
  std::sort(
      modules.begin(), modules.end(),
      [](const ModuleSymbolStatus& a, const ModuleSymbolStatus& b) { return a.base < b.base; });

  out->Append(Syntax::kHeading, fxl::StringPrintf("\nProcess %d symbol status\n\n",
                                                  context->IdForTarget(process->GetTarget())));

  for (const auto& module : modules) {
    out->Append(Syntax::kHeading, "  " + module.name + "\n");
    out->Append(fxl::StringPrintf("    Base: 0x%" PRIx64 "\n", module.base));
    out->Append("    Build ID: " + module.build_id);

    if (context->session()->system().GetDownloadManager()->HasDownload(module.build_id)) {
      out->Append(Syntax::kWarning, " (Downloading...)");
    }

    out->Append("\n");

    if (module.symbols_loaded) {
      out->Append("    Symbols loaded: Yes\n    Symbol file: ");
      out->Append(Syntax::kFileName, module.symbol_file);
      out->Append(module.files_indexed ? Syntax::kNormal : Syntax::kError,
                  fxl::StringPrintf("\n    Source files indexed: %zu", module.files_indexed));
      out->Append(module.functions_indexed ? Syntax::kNormal : Syntax::kError,
                  fxl::StringPrintf("\n    Symbols indexed: %zu", module.functions_indexed));
    } else {
      out->Append(Syntax::kError, "    Symbols loaded: No");
    }
    out->Append("\n\n");
  }

  if (modules.empty())
    out->Append(Syntax::kError, "  No known modules.\n");

  out->Append(Syntax::kWarning, "  👉 ");
  out->Append(Syntax::kComment,
              "Use \"libs\" to refresh the module list from the target or\n"
              "     \"libs --reload\" to additionally reload all symbols for the process.");
  out->Append(Syntax::kNormal, "\n\n");
}

void DumpIndexOverview(SystemSymbols* system_symbols, OutputBuffer* out) {
  out->Append(Syntax::kHeading, "Symbol index status\n\n");

  out->Append(Syntax::kComment, "  This command just refreshed the index.\n");

  std::vector<std::vector<OutputBuffer>> table;
  auto index_status = system_symbols->build_id_index().GetStatus();
  if (index_status.empty()) {
    out->Append(Syntax::kError, "  No symbol locations are indexed.");
    out->Append(
        "\n\n  Use the command-line switch \"zxdb -s <path>\" or the option \"symbol-paths\"\n"
        "  (see \"get/set symbol-paths\") to specify the location of your symbols.\n\n");
  } else {
    out->Append(Syntax::kComment,
                "  Use \"sym-stat --dump-index\" to see the individual mappings.\n\n");
    for (const auto& pair : index_status) {
      auto& row = table.emplace_back();
      auto syntax = pair.second ? Syntax::kNormal : Syntax::kError;

      if (pair.second != BuildIDIndex::kStatusIsFolder) {
        row.emplace_back(syntax, std::to_string(pair.second));
      } else {
        row.emplace_back(syntax, "(folder)");
      }

      row.emplace_back(syntax, pair.first);
    }
    FormatTable(
        {ColSpec(Align::kRight, 0, "Indexed", 2), ColSpec(Align::kLeft, 0, "Source path", 1)},
        table, out);
  }
}

void DumpBuildIdIndex(SystemSymbols* system_symbols, OutputBuffer* out) {
  // This command can confusingly list must less than people expect because most symbols come from
  // a .build-id directory structure.
  out->Append(Syntax::kWarning, " 👉 ");
  out->Append(Syntax::kComment,
              "Note: This only shows files that have been separately indexed by the\n"
              "    debugger. In many cases, symbols come from structured \".build-id\"\n"
              "    directories (the \"build-id-dirs\" setting) which are only searched on\n"
              "    demand and will not appear in this index.\n\n");

  const auto& build_id_to_files = system_symbols->build_id_index().build_id_to_files();
  if (build_id_to_files.empty()) {
    out->Append(Syntax::kError, "No separately indexed build IDs found.");
  } else {
    for (const auto& [id, files] : build_id_to_files)
      out->Append(fxl::StringPrintf("%s %s\n", id.c_str(), files.debug_info.c_str()));
  }
  out->Append("\n");
}

void RunVerbSymStat(const Command& cmd, fxl::RefPtr<CommandContext> cmd_context) {
  if (Err err = cmd.ValidateNouns({Noun::kProcess}); err.has_error())
    return cmd_context->ReportError(err);

  if (!cmd.args().empty())
    return cmd_context->ReportError(Err("\"sym-stat\" takes no arguments."));

  SystemSymbols* system_symbols =
      cmd_context->GetConsoleContext()->session()->system().GetSymbols();
  OutputBuffer out;

  if (cmd.HasSwitch(kDumpIndexSwitch)) {
    DumpBuildIdIndex(system_symbols, &out);
  } else {
    // Force an update of the symbol index.
    system_symbols->build_id_index().ClearCache();

    DumpIndexOverview(system_symbols, &out);

    // Process symbol status (if any).
    if (cmd.target() && cmd.target()->GetProcess()) {
      SummarizeProcessSymbolStatus(cmd_context->GetConsoleContext(), cmd.target()->GetProcess(),
                                   &out);
    }
  }

  cmd_context->Output(out);
}

}  // namespace

VerbRecord GetSymStatVerbRecord() {
  VerbRecord sym_stat(&RunVerbSymStat, {"sym-stat"}, kSymStatShortHelp, kSymStatUsage, kSymStatHelp,
                      CommandGroup::kSymbol);
  sym_stat.switches.emplace_back(kDumpIndexSwitch, false, "dump-index", 0);
  return sym_stat;
}

}  // namespace zxdb

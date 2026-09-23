// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/console/commands/verb_ps.h"

#include <iomanip>
#include <optional>
#include <set>
#include <sstream>
#include <string_view>

#include "src/developer/debug/shared/string_util.h"
#include "src/developer/debug/zxdb/client/process.h"
#include "src/developer/debug/zxdb/client/session.h"
#include "src/developer/debug/zxdb/client/system.h"
#include "src/developer/debug/zxdb/client/target.h"
#include "src/developer/debug/zxdb/console/command.h"
#include "src/developer/debug/zxdb/console/console.h"
#include "src/developer/debug/zxdb/console/verbs.h"
#include "src/developer/debug/zxdb/format/output_buffer.h"
#include "src/developer/debug/zxdb/format/string_util.h"

namespace zxdb {

namespace {

// Computes the set of attached job and process koids so they can be marked in the output.
std::set<uint64_t> ComputeAttachedKoidMap() {
  std::set<uint64_t> attached;

  System& system = Console::get()->context().session()->system();

  for (Target* target : system.GetTargets()) {
    if (Process* process = target->GetProcess())
      attached.insert(process->GetKoid());
  }

  return attached;
}

void OutputProcessTreeRecord(const debug_ipc::ProcessTreeRecord& rec, int indent,
                             const std::set<uint64_t>& attached, OutputBuffer* output) {
  // Row marker for attached processes/jobs.
  std::string prefix;
  Syntax syntax = Syntax::kNormal;
  if (auto found = attached.find(rec.koid); found != attached.end()) {
    syntax = Syntax::kHeading;
    prefix = GetCurrentRowMarker();
  } else {
    prefix = " ";  // Account for no prefix so everything is aligned.
  }

  // Indentation.
  prefix.append(indent * 2, ' ');

  // Record type.
  switch (rec.type) {
    case debug_ipc::TaskType::kJob:
      prefix.append("j: ");
      break;
    case debug_ipc::TaskType::kProcess:
      prefix.append("p: ");
      break;
    default:
      prefix.append("?: ");
      break;
  }

  output->Append(syntax, prefix);
  output->Append(Syntax::kSpecial, std::to_string(rec.koid));
  if (!rec.name.empty())
    output->Append(syntax, " " + rec.name);

  // Note if there are multiple components associated a particular job, we output
  // nothing here since there is either not enough information to be helpful or
  // too much information in the process graph.
  if (rec.components.size() == 1) {
    output->Append(" " + rec.components[0].moniker, TextForegroundColor::kCyan);
    output->Append(" " + rec.components[0].url, TextForegroundColor::kGray);
  }

  output->Append(syntax, "\n");

  for (const auto& child : rec.children)
    OutputProcessTreeRecord(child, indent + 1, attached, output);
}

// Recursively filters the given process tree. All jobs and processes that contain the given filter
// string in their name are matched. These are added to the result, along with any parent job nodes
// required to get to the matched records.
std::optional<debug_ipc::ProcessTreeRecord> FilterProcessTree(
    const debug_ipc::ProcessTreeRecord& rec, const std::string& filter) {
  debug_ipc::ProcessTreeRecord result;

  // A record matches if its (job) name or component name matches.
  bool matched = debug::StringContains(rec.name, filter);
  if (!matched && rec.components.size() == 1) {
    // Use the base name of the URL as the "component name".
    // e.g. "fuchsia-pkg://url#meta/foobar.cm" has a component name of "foobar.cm".
    std::string_view url = rec.components[0].url;
    std::string_view name = url.substr(url.find_last_of('/') + 1);
    matched = debug::StringContains(name, filter);
  } else if (!matched && !rec.components.empty()) {
    for (const auto& component : rec.components) {
      std::string_view url = component.url;
      std::string_view name = url.substr(url.find_last_of('/') + 1);
      matched = debug::StringContains(name, filter);
      if (matched)
        break;
    }
  }

  // If a record matches, show all its children.
  if (matched) {
    result.children = rec.children;
  } else {
    for (const auto& child : rec.children) {
      if (auto matched_child = FilterProcessTree(child, filter))
        result.children.push_back(*matched_child);
    }
  }

  // Return the node when it matches or any of its children do.
  if (matched || !result.children.empty()) {
    result.type = rec.type;
    result.koid = rec.koid;
    result.name = rec.name;
    result.components = rec.components;
    return result;
  }

  return std::nullopt;
}

void OnListProcessesComplete(fxl::RefPtr<CommandContext> cmd_context, const std::string& filter,
                             const debug_ipc::ProcessTreeReply& reply) {
  std::set<uint64_t> attached = ComputeAttachedKoidMap();

  OutputBuffer out;
  if (filter.empty()) {
    // Output everything.
    OutputProcessTreeRecord(reply.root, 0, attached, &out);
  } else {
    // Filter the results.
    if (auto filtered = FilterProcessTree(reply.root, filter)) {
      OutputProcessTreeRecord(*filtered, 0, attached, &out);
    } else {
      out.Append("No processes or jobs matching \"" + filter + "\".\n");
    }
  }
  cmd_context->Output(out);
}

const char kPsShortHelp[] = "ps: Prints the process tree of the debugged system.";
const char kPsUsage[] = "ps [ <filter-string> ]";
const char kPsHelp[] = R"(
  Prints the process tree of the debugged system.

  If a filter-string is provided only jobs and processes whose names contain the
  given case-sensitive substring. It does not support regular expressions.

  If a job is the root job of a component, the component information will also
  be printed.

  Jobs are annotated with "j: <job koid>"
  Processes are annotated with "p: <process koid>")";

void RunVerbPs(const Command& cmd, fxl::RefPtr<CommandContext> cmd_context) {
  std::string filter_string;
  if (!cmd.args().empty())
    filter_string = cmd.args()[0];

  cmd_context->GetConsoleContext()->session()->system().GetProcessTree(
      [filter_string, cmd_context](const Err& err, debug_ipc::ProcessTreeReply reply) {
        if (err.has_error())
          return cmd_context->ReportError(err);
        OnListProcessesComplete(cmd_context, filter_string, reply);
      });
}

}  // namespace

VerbRecord GetPsVerbRecord() {
  VerbRecord record(&RunVerbPs, {"ps"}, kPsShortHelp, kPsUsage, kPsHelp, CommandGroup::kGeneral);
  record.param_type = VerbRecord::kOneParam;  // Allow spaces in the filter string.
  return record;
}

}  // namespace zxdb

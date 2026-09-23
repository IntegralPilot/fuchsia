// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/console/commands/verb_attach.h"

#include "src/developer/debug/ipc/filter_utils.h"
#include "src/developer/debug/ipc/records.h"
#include "src/developer/debug/shared/string_util.h"
#include "src/developer/debug/zxdb/client/filter.h"
#include "src/developer/debug/zxdb/client/session.h"
#include "src/developer/debug/zxdb/console/command.h"
#include "src/developer/debug/zxdb/console/command_utils.h"
#include "src/developer/debug/zxdb/console/console.h"
#include "src/developer/debug/zxdb/console/verbs.h"
#include "src/developer/debug/zxdb/format/output_buffer.h"

namespace zxdb {

namespace {

constexpr int kSwitchJob = 1;
constexpr int kSwitchExact = 2;
constexpr int kSwitchWeak = 3;
constexpr int kSwitchRecursive = 4;
constexpr int kSwitchJobOnly = 5;

const char kAttachShortHelp[] = "attach: Attach to processes.";
const char kAttachUsage[] =
    "attach [ --job / -j <pid/koid> ] [ --exact ] [ --weak ] [ --recursive / -r ] [ --job-only ] [ <what> ]";
const char kAttachHelp[] = R"(
  Attaches to current or future process.

Arguments

    --job <koid>
    -j <koid>
        [Fuchsia only]
        Only attaching to processes under the job with an id of <koid>. The
        <what> argument can be omitted and all processes under the job will be
        attached.

    --job-only
        [Fuchsia only]
        Attach directly to the highest job that matches the given filter. If the
        filter matches a process directly, this will attach to the parent job.
        For components, it will attach to the root job of that component.
        Implies --weak.

    --exact
        Attaching to processes with an exact name. The argument will be
        interpreted as a filter that requires an exact match against the process
        name. This bypasses any heuristics below and is useful if the process
        name looks like a pid/koid, a URL, or a moniker.

    --weak
        Tells the backend to attach to any matching processes, but the front end
        will not request modules or load symbols until an exception is raised in
        the process. This significantly speeds up start up time with an initial
        filter, but requires an external event to stop the process. This is
        typically used by orchestration tools, rather than from the command
        line. This option can be specified in combination with any type of
        filter and all other flags to attach.

    --recursive
        Attaching recursively to a component means that when matched, an
        implicit moniker prefix filter will also be installed and will match any
        subsequent components that are launched under this component's realm.
        Note: this option does nothing for non-component filters.

Attaching to a process by a process id

  Numeric arguments will be interpreted as a process id (koid) that can be used
  to attach to a specific process. For example:

    attach 12345

  This can only attach to existing processes. Use the "ps" command to view all
  active processes, their names, and pids/koids.

Attaching to processes by a component moniker

  Arguments starting with "/" will be interpreted as an exact component moniker.
  This will create a filter that matches all processes in the component with the
  given moniker.

  Arguments that contain, but do not begin with, a "/" will be interpreted as a
  component moniker suffix.  This will create a filter similar to the exact
  component moniker matcher, but will match any monikers that end with the
  argument.

  NOTE: the latter interpretation happens only after inspecting the argument for
  a leading "/", and component URL.

Attaching to processes by a component URL

  Arguments that look like a URL, e.g., starting with "fuchsia-pkg://" or
  "fuchsia-boot://", will be interpreted as a component URL. This will create a
  filter that matches all processes in components with the given URL.

  NOTE: a component URL could be partial (https://fxbug.dev/42054323) so it's recommended
  to use "attaching by a component name" below.

Attaching to processes by a component name

  Arguments ending with ".cm" will be interpreted as a component name. The
  component name is defined as the base name of the component manifest. So a
  component with an URL "fuchsia-pkg://devhost/foobar#meta/foobar.cm" has a
  name "foobar.cm". This will create a filter that matches all processes in
  components with the given name.

Attaching to all processes within a realm

  Using any of the above component filters, using the --recursive option will
  attach to _all_ processes found under the realm of a matching component. The
  component can be specified with either exact moniker, moniker substring, or
  package URL. Note using a short moniker substring could unintentionally attach
  to many processes, which will slow down the system.

Attaching to the root job of a realm

  Using any of the above component filters, use the --job-only option to attach
  to the root job of that component. No processes will be attached unless
  explicitly requested. This can be useful when DebugAgent should only monitor
  and report exceptions from any and all child processes, but no interactive
  debugging is needed. Since all sub-realms of this realm will report exceptions
  through this job, we never need to attach to any child jobs. This can be used
  in conjunction with --recursive to have process objects prepared and ready for
  interactive debugging without ever reporting exceptions unless they reach the
  given realm's root job.

Attaching to processes by a process name

  Other arguments will be interpreted as a general filter which is a substring
  that will be used to matches any part of the process name. Matched processes
  will be attached.

How "attach" works

  Except attaching by a process id, all other "attach" commands will create
  filters. Filters are applied to all processes in the system, both current
  processes and future ones.

  You can:

    • See the current filters with the "filter" command.

    • Delete a filter with "filter [X] rm" where X is the filter index from the
      "filter" list. If no filter index is provided, the current filter will be
      deleted.

    • Change a filter's pattern with "filter [X] set pattern = <newvalue>".

Examples

  attach 2371
      Attaches to the process with pid/koid 2371.

  process 4 attach 2371
      Attaches process context 4 to the process with pid/koid 2371.

  attach foobar
      Attaches to processes with "foobar" in their process names.

  attach /core/foobar
      Attaches to processes in the component /core/foobar.

  attach --recursive /core/foobar
      Attaches to all processes found in the realm rooted at /core/foobar.

  attach foo/bar
      Attaches to processes in the component(s) with foo/bar in their moniker.

  attach --recursive foo/bar
      Attaches to all processes in all realms rooted at any moniker containing
      foo/bar.

  attach fuchsia-pkg://devhost/foobar#meta/foobar.cm
      Attaches to processes in components with the above component URL.

  attach --recursive fuchsia-pkg://devhost/foobar#meta/foobar.cm
      Attaches to all processes in the realm rooted at any moniker associated
      with the given URL.

  attach foobar.cm
      Attaches to processes in components with the above name.

  attach --job-only foobar.cm
      Attaches to the root job within the realm associated with foobar.cm

  attach --exact /pkg/bin/foobar
      Attaches to processes with a name "/pkg/bin/foobar".

  attach --job 2037
      Attaches to all processes under the job with koid 2037.
)";

void RunVerbAttach(const Command& cmd, fxl::RefPtr<CommandContext> cmd_context) {
  // Only process can be specified.
  if (Err err = cmd.ValidateNouns({Noun::kProcess}); err.has_error())
    return cmd_context->ReportError(err);

  // Should be non-null since we're running in a synchronous context.
  ConsoleContext* console_context = cmd_context->GetConsoleContext();

  // attach <koid> accepts no switch.
  uint64_t koid = 0;
  if (!cmd.HasSwitch(kSwitchJob) && !cmd.HasSwitch(kSwitchExact) &&
      ReadUint64Arg(cmd, 0, "process id/koid", &koid).ok()) {
    // Check for duplicate koids before doing anything else to avoid creating a container target
    // in this case. It's easy to hit enter twice which will cause a duplicate attach. The
    // duplicate target is the only reason to check here, the attach will fail later if there's
    // a duplicate (say, created in a race condition).
    if (console_context->session()->system().ProcessFromKoid(koid)) {
      return cmd_context->ReportError(
          Err("Process " + std::to_string(koid) + " is already being debugged."));
    }

    // Attach to a process by KOID.
    auto err_or_target = GetRunnableTarget(console_context, cmd);
    if (err_or_target.has_error())
      return cmd_context->ReportError(err_or_target.err());

    debug_ipc::AttachConfig::Priority priority = cmd.HasSwitch(kSwitchWeak)
                                                     ? debug_ipc::AttachConfig::Priority::kWeak
                                                     : debug_ipc::AttachConfig::Priority::kStrong;
    err_or_target.value()->Attach(
        koid, {.priority = priority, .target = debug_ipc::TaskType::kProcess},
        [cmd_context](fxl::WeakPtr<Target> target, const Err& err, uint64_t timestamp) mutable {
          // Don't display a message on success because the ConsoleContext will print the new
          // process information when it's detected.
          ProcessCommandCallback(target, false, err, cmd_context);
        });
    return;
  }

  // For all other cases, "process" cannot be specified.
  if (cmd.HasNoun(Noun::kProcess)) {
    return cmd_context->ReportError(Err("Attaching by filters doesn't support \"process\" noun."));
  }

  // When --job switch is on and --exact is off, require 0 or 1 argument.
  // Otherwise require 1 argument.
  if ((!cmd.HasSwitch(kSwitchJob) || cmd.HasSwitch(kSwitchExact) || !cmd.args().empty()) &&
      cmd.args().size() != 1) {
    return cmd_context->ReportError(Err("Wrong number of arguments to attach."));
  }

  // --job <koid> must be parsable as uint64.
  uint64_t job_koid = 0;
  if (cmd.HasSwitch(kSwitchJob) &&
      StringToUint64(cmd.GetSwitchValue(kSwitchJob), &job_koid).has_error()) {
    return cmd_context->ReportError(Err("--job only accepts a koid"));
  }

  // Now all the checks are performed. Create a filter.
  Filter* filter = console_context->session()->system().CreateNewFilter();

  if (cmd.HasSwitch(kSwitchWeak)) {
    filter->SetWeak(true);
  }

  if (cmd.HasSwitch(kSwitchRecursive)) {
    filter->SetRecursive(true);
  }

  if (cmd.HasSwitch(kSwitchJobOnly)) {
    if (cmd_context->GetConsoleContext()->session()->platform() != debug::Platform::kFuchsia) {
      return cmd_context->ReportError(Err("--job-only is only available when debugging Fuchsia."));
    }
    filter->SetJobOnly(true);
  }

  std::string pattern;
  if (!cmd.args().empty())
    pattern = cmd.args()[0];

  if (job_koid) {
    filter->SetJobKoid(job_koid);
  }

  std::string original_pattern = pattern;
  auto type = debug_ipc::ResolveFilterType(pattern, {.exact = cmd.HasSwitch(kSwitchExact)});

  if (pattern.size() < original_pattern.size()) {
    Console::get()->Output(OutputBuffer(
        Syntax::kWarning,
        "The filter is trimmed to " + std::to_string(kZirconMaxNameLength) +
            " characters because it's the maximum length for a process name in Zircon."));
  }

  filter->SetType(type);
  filter->SetPattern(pattern);

  console_context->SetActiveFilter(filter);

  // This doesn't use the default filter formatting to try to make it friendlier for people
  // that are less familiar with the debugger and might be unsure what's happening (this is normally
  // one of the first things people do in the debugger. The filter number is usually not relevant
  // anyway.
  if (pattern.empty()) {
    pattern = "job " + cmd.GetSwitchValue(kSwitchJob);
  }
  Console::get()->Output("Waiting for process matching \"" + pattern +
                         "\".\n"
                         "Type \"filter\" to see the current filters.");
}

}  // namespace

VerbRecord GetAttachVerbRecord() {
  VerbRecord attach(&RunVerbAttach, {"attach"}, kAttachShortHelp, kAttachUsage, kAttachHelp,
                    CommandGroup::kProcess);
  attach.switches.emplace_back(kSwitchJob, true, "job", 'j');
  attach.switches.emplace_back(kSwitchExact, false, "exact");
  attach.switches.emplace_back(kSwitchWeak, false, "weak");
  attach.switches.emplace_back(kSwitchRecursive, false, "recursive", 'r');
  attach.switches.emplace_back(kSwitchJobOnly, false, "job-only");
  return attach;
}

}  // namespace zxdb

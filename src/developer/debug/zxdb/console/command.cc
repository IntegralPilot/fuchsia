// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/console/command.h"

#include <lib/syslog/cpp/macros.h>

#include <algorithm>

#include "src/developer/debug/shared/message_loop.h"
#include "src/developer/debug/zxdb/client/analytics_event.h"
#include "src/developer/debug/zxdb/common/err.h"
#include "src/developer/debug/zxdb/console/console.h"
#include "src/developer/debug/zxdb/console/nouns.h"
#include "src/developer/debug/zxdb/console/verbs.h"
#include "src/lib/fxl/strings/string_printf.h"

namespace zxdb {

namespace {
const SwitchRecord* GetSwitchRecordFrom(int switch_id,
                                        const std::vector<SwitchRecord>& valid_switches) {
  for (const auto& valid_switch : valid_switches) {
    if (valid_switch.id == switch_id) {
      return &valid_switch;
    }
  }

  return nullptr;
}
}  // namespace

Command::Command() = default;
Command::~Command() = default;

bool Command::HasNoun(Noun noun) const { return nouns_.find(noun) != nouns_.end(); }

int Command::GetNounIndex(Noun noun) const {
  auto found = nouns_.find(noun);
  if (found == nouns_.end())
    return kNoIndex;
  return found->second;
}

void Command::SetNoun(Noun noun, int index) {
  FX_DCHECK(nouns_.find(noun) == nouns_.end());
  nouns_[noun] = index;
}

Err Command::ValidateNouns(std::initializer_list<Noun> allowed_nouns, bool allow_wildcard) const {
  for (const auto& pair : nouns_) {
    if (std::find(allowed_nouns.begin(), allowed_nouns.end(), pair.first) == allowed_nouns.end()) {
      return Err(ErrType::kInput, fxl::StringPrintf("\"%s\" may not be specified for this command.",
                                                    NounToString(pair.first).c_str()));
    } else if (!allow_wildcard && GetNounIndex(pair.first) == kWildcard) {
      return Err(ErrType::kInput, "Wildcard \"*\" specifier is not allowed with this command.");
    }
  }
  return Err();
}

bool Command::HasSwitch(int id) const { return switches_.find(id) != switches_.end(); }

std::string Command::GetSwitchValue(int id) const {
  auto found = switches_.find(id);
  if (found == switches_.end())
    return std::string();
  return found->second;
}

void Command::SetSwitch(int id, std::string str) { switches_[id] = std::move(str); }

CommandReport Command::BuildReport() const {
  CommandReport report;

  report.verb_id = static_cast<int>(verb_);

  if (verb_ != Verb::kNone) {
    report.verb = VerbToString(verb_);

    auto verb_record = GetVerbRecord(verb_);
    FX_DCHECK(verb_record);
    report.command_group = static_cast<int>(verb_record->command_group);
  }

  AddNounsToCommandReport(report);
  AddArgsToCommandReport(report);
  AddSwitchesToCommandReport(report);

  return report;
}

void Command::AddNounsToCommandReport(CommandReport& report) const {
  if (nouns_.empty())
    return;

  report.nouns.reserve(nouns_.size());
  for (const auto& noun : nouns_) {
    // This will always be found, the command has already been verified at this point.
    auto noun_record = GetNouns().find(noun.first)->second;
    if (report.command_group == static_cast<int>(CommandGroup::kNone)) {
      report.command_group = static_cast<int>(noun_record.command_group);
    }

    report.nouns.emplace_back(static_cast<int>(noun.first), NounToString(noun.first), noun.second);
  }
}

void Command::AddArgsToCommandReport(CommandReport& report) const {
  if (args_.empty())
    return;

  // Some commands take arguments such as filesystem paths that we don't want to capture in
  // analytics reporting. Skip the arguments for those commands.
  if (auto verb_record = GetVerbRecord(verb_); verb_record && verb_record->needs_elision)
    return;

  report.arguments = args_;
}

void Command::AddSwitchesToCommandReport(CommandReport& report) const {
  if (switches_.empty())
    return;

  report.switches.reserve(switches_.size());

  const SwitchRecord* record = nullptr;
  const std::vector<SwitchRecord>* valid_switches = nullptr;
  auto verb_record = GetVerbRecord(verb_);
  if (verb_record) {
    valid_switches = &verb_record->switches;
  } else if (!nouns_.empty()) {
    // Nouns all share the same switches, even for things that might not make sense like "thread
    // --types", so the presence of any noun means we can get the switch. Noun switches are not
    // valid when in conjunction with a verb.
    valid_switches = &GetNounSwitches();
  }

  for (const auto& [id, value] : switches_) {
    record = GetSwitchRecordFrom(id, *valid_switches);
    if (record) {
      std::string switch_value = record->has_value ? GetSwitchValue(record->id) : "";
      report.switches.emplace_back(record->id, record->name, switch_value);
    }
  }
}

void DispatchCommand(const Command& cmd, fxl::RefPtr<CommandContext> cmd_context) {
  if (cmd.verb() == Verb::kNone) {
    ExecuteNoun(cmd, cmd_context);
    return;
  }

  const auto& verbs = GetVerbs();
  const auto& found = verbs.find(cmd.verb());
  if (found == verbs.end()) {
    cmd_context->ReportError(
        Err(ErrType::kInput, "Invalid verb \"" + VerbToString(cmd.verb()) + "\"."));
    return;
  }

  found->second.exec(cmd, cmd_context);
}

}  // namespace zxdb

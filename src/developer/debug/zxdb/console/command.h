// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_CONSOLE_COMMAND_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_CONSOLE_COMMAND_H_

#include <lib/syslog/cpp/macros.h>

#include <initializer_list>
#include <map>
#include <string>
#include <vector>

#include "lib/fit/function.h"
#include "src/developer/debug/zxdb/common/err.h"
#include "src/developer/debug/zxdb/console/nouns.h"
#include "src/developer/debug/zxdb/console/verbs.h"

namespace zxdb {

class Breakpoint;
struct CommandReport;
class ConsoleContext;
class Filter;
class Frame;
class Target;
class Thread;
class SymbolServer;

// Command ---------------------------------------------------------------------

class Command {
 public:
  // This valid indicates that there was a noun specified but no index.
  // For example the command "process step" specifies the process noun but
  // not an index.
  static constexpr int kNoIndex = -1;
  // This indicates the operation should be done for all instances of this noun. Typically, this is
  // used to remove all filters or breakpoints, or detach from all processes. Typical use is like:
  // filter * rm
  static constexpr int kWildcard = -2;

  Command();
  ~Command();

  // Returns true if the noun was specified by the user.
  bool HasNoun(Noun noun) const;

  // Returns the index specified for the given noun. If the noun was not
  // specified or the index was not specified, returns kNoIndex (use HasNoun to
  // disambiguate).
  int GetNounIndex(Noun noun) const;

  // Sets that the given noun was present. index may be kNoIndex.
  void SetNoun(Noun noun, int index);

  const std::map<Noun, int>& nouns() const { return nouns_; }

  // Checks the specified nouns against the parameter listing the allowed ones.
  // If any nouns are specified that are not in the list, generates an error and
  // returns it. If |allow_wildcard| is false, and a wildcard specifier is
  // present, will generate an appropriate error message. Otherwise it will
  // return an empty error.
  Err ValidateNouns(std::initializer_list<Noun> allowed_nouns, bool allow_wildcard = false) const;

  Verb verb() const { return verb_; }
  void set_verb(Verb v) { verb_ = v; }

  // Returns whether a given switch was specified.
  bool HasSwitch(int id) const;

  // Returns the value corresponding to the given switch, or the empty string
  // if not specified.
  std::string GetSwitchValue(int id) const;

  void SetSwitch(int id, std::string str);

  const std::map<int, std::string>& switches() const { return switches_; }

  const std::vector<std::string>& args() const { return args_; }
  void set_args(std::vector<std::string> a) { args_ = std::move(a); }

  // The computed environment for the command. This is filled in with the
  // objects corresponding to the indices given on the command line, and
  // default to the current one for the current command line.
  //
  // If |HasNoun()| returns true, the corresponding getter here is guaranteed to
  // be non-null or have size() > 0. Most commands are interacting with a single
  // noun, so there are convenience getters for when only a single object is
  // needed/expected. These will only work when there is exactly one instance of
  // that noun associated with this command.
  Frame* frame() const { return frames_.size() == 1 ? frames_[0] : nullptr; }
  const std::vector<Frame*>& all_frames() const { return frames_; }
  void add_frame(Frame* f) { frames_.push_back(f); }
  Target* target() const {
    // Enforce at least one target exists.
    FX_DCHECK(!targets_.empty());
    return targets_[0];
  }
  const std::vector<Target*>& all_targets() const { return targets_; }
  void add_target(Target* t) { targets_.push_back(t); }
  Thread* thread() const { return threads_.size() == 1 ? threads_[0] : nullptr; }
  const std::vector<Thread*>& all_threads() const { return threads_; }
  void add_thread(Thread* t) { threads_.push_back(t); }
  Breakpoint* breakpoint() const { return breakpoints_.size() == 1 ? breakpoints_[0] : nullptr; }
  const std::vector<Breakpoint*>& all_breakpoints() const { return breakpoints_; }
  void add_breakpoint(Breakpoint* b) { breakpoints_.push_back(b); }
  Filter* filter() const { return filters_.size() == 1 ? filters_[0] : nullptr; }
  const std::vector<Filter*>& all_filters() const { return filters_; }
  void add_filter(Filter* f) { filters_.push_back(f); }
  SymbolServer* sym_server() const {
    return symbol_servers_.size() == 1 ? symbol_servers_[0] : nullptr;
  }
  const std::vector<SymbolServer*>& all_sym_servers() const { return symbol_servers_; }
  void add_sym_server(SymbolServer* s) { symbol_servers_.push_back(s); }

  CommandReport BuildReport() const;

 private:
  void AddNounsToCommandReport(CommandReport& report) const;
  void AddArgsToCommandReport(CommandReport& report) const;
  void AddSwitchesToCommandReport(CommandReport& report) const;

  // The nouns specified for this command. If not present here, the noun was not
  // written on the command line. If present but there was no index given for
  // it, the mapped value will be kNoIndex. Wildcards are represented as
  // |kWildcard|. Otherwise the mapped value will be the index specified.
  std::map<Noun, int> nouns_;

  // The effective context for the command. The explicitly specified process(es)/
  // thread(s)/etc. will be reflected here, and anything that wasn't explicit
  // will inherit the default.
  std::vector<Target*> targets_;               // Guaranteed non-empty for valid commands.
  std::vector<Thread*> threads_;               // Will be empty if not running.
  std::vector<Frame*> frames_;                 // Will be empty if no valid thread stopped.
  std::vector<Breakpoint*> breakpoints_;       // May be empty.
  std::vector<Filter*> filters_;               // May be empty.
  std::vector<SymbolServer*> symbol_servers_;  // May be empty.

  Verb verb_ = Verb::kNone;

  std::map<int, std::string> switches_;
  std::vector<std::string> args_;
};

// Command dispatch ------------------------------------------------------------

// Type for a callback that a CommandExecutor will receive
using CommandCallback = fit::callback<void(Err)>;

// Runs the given command.
void DispatchCommand(const Command& cmd, fxl::RefPtr<CommandContext> cmd_context);

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_CONSOLE_COMMAND_H_

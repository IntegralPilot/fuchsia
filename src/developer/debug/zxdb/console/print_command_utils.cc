// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/console/print_command_utils.h"

#include "src/developer/debug/zxdb/client/setting_schema_definition.h"
#include "src/developer/debug/zxdb/common/err.h"
#include "src/developer/debug/zxdb/console/command.h"
#include "src/developer/debug/zxdb/console/command_utils.h"
#include "src/developer/debug/zxdb/console/verbs.h"
#include "src/developer/debug/zxdb/expr/format_options.h"
#include "src/developer/debug/zxdb/format/format.h"

namespace zxdb {

namespace {

constexpr int kPrintCommandSwitchBase = 1000000;

constexpr int kVerboseFormat = kPrintCommandSwitchBase + 0;
constexpr int kForceAllTypes = kPrintCommandSwitchBase + 1;
constexpr int kForceNumberChar = kPrintCommandSwitchBase + 2;
constexpr int kForceNumberSigned = kPrintCommandSwitchBase + 3;
constexpr int kForceNumberUnsigned = kPrintCommandSwitchBase + 4;
constexpr int kForceNumberHex = kPrintCommandSwitchBase + 5;
constexpr int kForceNumberBin = kPrintCommandSwitchBase + 6;
constexpr int kMaxArraySize = kPrintCommandSwitchBase + 7;
constexpr int kRawOutput = kPrintCommandSwitchBase + 8;
constexpr int kDepth = kPrintCommandSwitchBase + 9;
constexpr int kPointerDepth = kPrintCommandSwitchBase + 10;

}  // namespace

void AppendPrintCommandSwitches(VerbRecord* record) {
  record->switches.emplace_back(kForceAllTypes, false, "types", 't');
  record->switches.emplace_back(kRawOutput, false, "raw", 'r');
  record->switches.emplace_back(kVerboseFormat, false, "verbose", 'v');
  record->switches.emplace_back(kForceNumberChar, false, "", 'c');
  record->switches.emplace_back(kForceNumberSigned, false, "", 'd');
  record->switches.emplace_back(kForceNumberUnsigned, false, "", 'u');
  record->switches.emplace_back(kForceNumberHex, false, "", 'x');
  record->switches.emplace_back(kForceNumberBin, false, "", 'b');
  record->switches.emplace_back(kMaxArraySize, true, "max-array");
  record->switches.emplace_back(kDepth, true, "depth");
  record->switches.emplace_back(kPointerDepth, true, "pointer-depth");
}

ErrOr<FormatBufferOptions> GetPrintCommandFormatOptions(const Command& cmd) {
  FormatBufferOptions options;

  options.pointer_expand_depth = 1;
  options.max_depth = 16;
  if (cmd.HasSwitch(kPointerDepth)) {
    int size = 0;
    if (Err err = StringToInt(cmd.GetSwitchValue(kPointerDepth), &size); err.has_error())
      return err;
    options.pointer_expand_depth = size;
  }
  if (cmd.HasSwitch(kDepth)) {
    int size = 0;
    if (Err err = StringToInt(cmd.GetSwitchValue(kDepth), &size); err.has_error())
      return err;
    options.max_depth = size;
  }

  // All current users of this want the smart form.
  //
  // This keeps the default wrap columns at 80. We can consider querying the actual console width.
  // But very long lines start putting many struct members on the same line which gets increasingly
  // difficult to read. 80 columns feels reasonably close to how much you can take in at once.
  //
  // Note also that this doesn't stricly wrap the output to 80 columns. Long type names or values
  // will still use the full width and will be wrapped by the console. This wrapping only affects
  // the splitting of items across lines.
  options.wrapping = FormatBufferOptions::Wrapping::kSmart;

  // Verbosity.
  if (cmd.HasSwitch(kForceAllTypes))
    options.verbosity = FormatBufferOptions::Verbosity::kAllTypes;
  else if (cmd.HasSwitch(kVerboseFormat))
    options.verbosity = FormatBufferOptions::Verbosity::kMedium;
  else
    options.verbosity = FormatBufferOptions::Verbosity::kMinimal;

  // Array size.
  if (cmd.HasSwitch(kMaxArraySize)) {
    int size = 0;
    if (Err err = StringToInt(cmd.GetSwitchValue(kMaxArraySize), &size); err.has_error())
      return err;
    options.max_array_size = static_cast<uint32_t>(size);
  }

  // Mapping from command-line parameter to format enum.
  constexpr size_t kFormatCount = 5;
  static constexpr std::pair<int, FormatBufferOptions::NumFormat> kFormats[kFormatCount] = {
      {kForceNumberChar, FormatBufferOptions::NumFormat::kChar},
      {kForceNumberUnsigned, FormatBufferOptions::NumFormat::kUnsigned},
      {kForceNumberSigned, FormatBufferOptions::NumFormat::kSigned},
      {kForceNumberHex, FormatBufferOptions::NumFormat::kHex},
      {kForceNumberBin, FormatBufferOptions::NumFormat::kBin}};

  int num_type_overrides = 0;
  for (const auto& cur : kFormats) {
    if (cmd.HasSwitch(cur.first)) {
      num_type_overrides++;
      options.num_format = cur.second;
    }
  }

  if (num_type_overrides == 0) {
    options.num_format = GetNumberFormatForTarget(cmd.target());
  }

  // Disable pretty-printing.
  if (cmd.HasSwitch(kRawOutput))
    options.enable_pretty_printing = false;

  if (num_type_overrides > 1)
    return Err("More than one type override (-b, -c, -d, -u, -x) specified.");
  return options;
}

}  // namespace zxdb

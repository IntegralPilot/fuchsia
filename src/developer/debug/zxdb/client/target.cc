// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/client/target.h"

#include "src/developer/debug/zxdb/client/setting_schema_definition.h"
#include "src/developer/debug/zxdb/common/host_util.h"
#include "src/developer/debug/zxdb/expr/format_options.h"
#include "src/developer/debug/zxdb/expr/vector_register_format.h"
#include "src/lib/files/path.h"

namespace zxdb {

// Schema Definition -----------------------------------------------------------

static const char kShowStdoutDescription[] =
    R"(  Whether this process should pipe its stdout/stderr to zxdb.
  If not set for a particular process, it will default to the global setting.)";

const char* ClientSettings::Target::kSourceMap = "source-map";
const char* ClientSettings::Target::kSourceMapDescription =
    R"(  Remap source file paths. The syntax "a=b" means to remap all source file paths
  starting with "a" with "b". "a" and "b" could be either relative or absolute.

  For example, if zxdb cannot find ./../../zircon/system/ulib/c/Scrt1.cc,
  "set source-map += ./../..=/path/to/fuchsia/checkout" will make zxdb search
  /path/to/fuchsia/checkout/zircon/system/ulib/c/Scrt1.cc instead.)";

const char* ClientSettings::Target::kIntegerFormat = "integer-format";
const char* ClientSettings::Target::kIntegerFormatDescription =
    R"(  How to display integers.

  The default way to display integers in commands like "print" and "locals".
  Explicit options given to those commands will override this setting.

  Possible values:

    dec : Decimal base-10 (default).
    hex : Hexadecimal base-16.
    bin : Binary base-2.)";

const char* ClientSettings::Target::kVectorFormat = "vector-format";
const char* ClientSettings::Target::kVectorFormatDescription =
    R"(  How to treat vector registers.

  This affects the display of vector registers in the "regs" command as well
  as what it means when you type a register name in an expression.

  Possible values:

    i8 / u8     : Array of signed/unsigned 8-bit integers.
    i16 / u16   : Array of signed/unsigned 16-bit integers.
    i32 / u32   : Array of signed/unsigned 32-bit integers.
    i64 / u64   : Array of signed/unsigned 64-bit integers.
    i128 / u128 : Array of signed/unsigned 128-bit integers.
    float       : Array of single-precision floating point.
    double      : Array of double-precision floating point.)";

const char* ClientSettings::Target::kAutoContinueWhenStepping = "auto-continue-when-stepping";
const char* ClientSettings::Target::kAutoContinueWhenSteppingDescription =
    R"(  How to handle "Single Step" exceptions when automatically continuing.

    When true (default), "Single Step" exceptions are included in the types of
    exceptions that are automatically marked as resolved and continued over when
    the process is detected to be a test. When false, "Single Step" exceptions
    are excluded and will cause a stop.

    Note: this does not necessarily mean that the process will reach the "Single
    Step" exception.

    Commands that can trigger "Single Step" exceptions are:

      next   / n
      step   / s
      nexti  / ni
      stepi  / si
      finish / fi)";

// static
std::vector<std::string> ClientSettings::Target::GetVectorFormatOptions() {
  return std::vector<std::string>{
      kVectorRegisterFormatStr_Signed8,   kVectorRegisterFormatStr_Unsigned8,
      kVectorRegisterFormatStr_Signed16,  kVectorRegisterFormatStr_Unsigned16,
      kVectorRegisterFormatStr_Signed32,  kVectorRegisterFormatStr_Unsigned32,
      kVectorRegisterFormatStr_Signed64,  kVectorRegisterFormatStr_Unsigned64,
      kVectorRegisterFormatStr_Signed128, kVectorRegisterFormatStr_Unsigned128,
      kVectorRegisterFormatStr_Float,     kVectorRegisterFormatStr_Double};
}

namespace {

fxl::RefPtr<SettingSchema> CreateSchema() {
  auto schema = fxl::MakeRefCounted<SettingSchema>();

  schema->AddBool(ClientSettings::System::kShowStdout, kShowStdoutDescription, true);

  schema->AddList(
      ClientSettings::Target::kSourceMap, ClientSettings::Target::kSourceMapDescription,
      // TODO(https://fxbug.dev/336868560): Fix the rust toolchain prebuilt to point
      // DW_AT_comp_dir to the right place and remove this hack.
      {"/b/s/w/ir/x/w/fuchsia-third_party-rust=" + files::GetDirectoryName(GetSelfPath()) +
       "/../../../prebuilt/third_party/rust/linux-x64/lib/rustlib/src/rust"});

  schema->AddBool(ClientSettings::Target::kAutoContinueWhenStepping,
                  ClientSettings::Target::kAutoContinueWhenSteppingDescription, true);

  schema->AddBool(ClientSettings::Thread::kDebugStepping,
                  ClientSettings::Thread::kDebugSteppingDescription, false);

  schema->AddString(ClientSettings::Target::kIntegerFormat,
                    ClientSettings::Target::kIntegerFormatDescription, kIntegerFormatDecimal,
                    {kIntegerFormatDecimal, kIntegerFormatHexadecimal, kIntegerFormatBinary});

  schema->AddString(
      ClientSettings::Target::kVectorFormat, ClientSettings::Target::kVectorFormatDescription,
      kVectorRegisterFormatStr_Double, ClientSettings::Target::GetVectorFormatOptions());

  schema->AddList(ClientSettings::Thread::kDisplay, ClientSettings::Thread::kDisplayDescription);

  return schema;
}

}  // namespace

// Target Implementation ---------------------------------------------------------------------------

Target::Target(Session* session)
    : ClientObject(session),
      // Implementations can set up fallbacks if needed.
      settings_(GetSchema(), nullptr),
      weak_factory_(this) {}

Target::~Target() = default;

fxl::WeakPtr<Target> Target::GetWeakPtr() { return weak_factory_.GetWeakPtr(); }

fxl::RefPtr<SettingSchema> Target::GetSchema() {
  // Will only run initialization once.
  InitializeSchemas();
  static fxl::RefPtr<SettingSchema> schema = CreateSchema();
  return schema;
}

}  // namespace zxdb

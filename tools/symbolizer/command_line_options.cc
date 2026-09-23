// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tools/symbolizer/command_line_options.h"

#include <lib/cmdline/args_parser.h>

#include <algorithm>
#include <filesystem>
#include <string>

namespace symbolizer {

namespace {

const char kHelpIntro[] = R"(symbolizer [<options>]

  Parses log from stdin and converts symbolizer markups into human readable
  stack traces using local or remote debug symbols.

Options

)";

const char kSymbolIndexHelp[] = R"(  --symbol-index=<path>
      Populates --ids-txt and --build-id-dir using the given symbol-index file,
      which defaults to ~/.fuchsia/debug/symbol-index. The file should be
      created and maintained by the "symbol-index" host tool.)";

const char kSymbolPathHelp[] = R"(  --symbol-path=<path>
  -s <path>
      Adds the given directory or file to the symbol search path. Multiple
      -s switches can be passed to add multiple locations. When a directory
      path is passed, the directory will be enumerated non-recursively to
      index all ELF files. When a file is passed, it will be loaded as an ELF
      file (if possible).)";

const char kBuildIdDirHelp[] = R"(  --build-id-dir=<path>
      Adds the given directory to the symbol search path. Multiple
      --build-id-dir switches can be passed to add multiple directories.
      The directory must have the same structure as a .build-id directory,
      that is, each symbol file lives at xx/yyyyyyyy.debug where xx is
      the first two characters of the build ID and yyyyyyyy is the rest.
      However, the name of the directory doesn't need to be .build-id.)";

const char kIdsTxtHelp[] = R"(  --ids-txt=<path>
      Adds the given file to the symbol search path. Multiple --ids-txt
      switches can be passed to add multiple files. The file, typically named
      "ids.txt", serves as a mapping from build ID to symbol file path and
      should contain multiple lines in the format of "<build ID> <file path>".)";

const char kSymbolCacheHelp[] = R"(  --symbol-cache=<path>
      Directory where we can keep a symbol cache, which defaults to
      ~/.fuchsia/debug/symbol-cache. If a symbol server has been specified,
      downloaded symbols will be stored in this directory. The directory
      structure will be the same as a .build-id directory, and symbols will
      be read from this location as though you had specified
      "--build-id-dir=<path>".)";

const char kPrivateSymbolServerHelp[] = R"(  --symbol-server=<url>
      Adds the given URL to symbol servers. Symbol servers host the debug
      symbols for prebuilt binaries and dynamic libraries. All URLs passed using
      this flag will need to correctly authenticate. Failure to authenticate
      will result in an unusable server. For public servers, use
      --public-symbol-server or set DEBUGINFOD_URLS in your environment.)";

const char kPublicSymbolServerHelp[] = R"(  --public-symbol-server=<url>
      Adds the given URL to symbol servers. Symbol servers host the debug
      symbols for prebuilt binaries and dynamic libraries. Public servers
      perform no authentication. Use --symbol-servers to specify private symbol
      servers using supported authentication schemes.)";

const char kHelpHelp[] = R"(  --help
  -h
      Prints this help.)";

const char kVersionHelp[] = R"(  --version
  -v
      Prints the version.)";

const char kVerboseHelp[] = R"(  --verbose
      Enables DEBUG-level logging to stderr.)";

const char kAuthHelp[] = R"(  --auth [deprecated]
      Starts the authentication process for symbol servers.)";

const char kOmitModuleLinesHelp[] = R"(  --omit-module-lines
      Omit the "[[[ELF module ...]]]" lines from the output.)";

const char kPrettifyBacktraceHelp[] = R"(  --prettify-backtrace
      Try to prettify backtraces.)";

const char kDumpfileOutputHelp[] = R"(  --dumpfile-output=<path>
      Write the dumpfile output to the given file.)";

using ::analytics::core_dev_tools::kAnalyticsHelp;
using ::analytics::core_dev_tools::kAnalyticsShowHelp;

}  // namespace

Error ParseCommandLine(int argc, const char* argv[], CommandLineOptions* options) {
  using analytics::core_dev_tools::AnalyticsOption;
  using analytics::core_dev_tools::ParseAnalyticsOption;

  std::vector<std::string> params;
  cmdline::ArgsParser<CommandLineOptions> parser;

  parser.AddSwitch("symbol-index", 0, kSymbolIndexHelp, &CommandLineOptions::symbol_index_files);
  parser.AddSwitch("symbol-path", 's', kSymbolPathHelp, &CommandLineOptions::symbol_paths);
  parser.AddSwitch("build-id-dir", 0, kBuildIdDirHelp, &CommandLineOptions::build_id_dirs);
  parser.AddSwitch("ids-txt", 0, kIdsTxtHelp, &CommandLineOptions::ids_txts);
  parser.AddSwitch("symbol-cache", 0, kSymbolCacheHelp, &CommandLineOptions::symbol_cache);
  parser.AddSwitch("symbol-server", 0, kPrivateSymbolServerHelp,
                   &CommandLineOptions::private_symbol_servers);
  parser.AddSwitch("public-symbol-server", 0, kPublicSymbolServerHelp,
                   &CommandLineOptions::public_symbol_servers);
  parser.AddSwitch("verbose", 0, kVerboseHelp, &CommandLineOptions::verbose);
  parser.AddSwitch("auth", 0, kAuthHelp, &CommandLineOptions::auth_mode);
  parser.AddSwitch("version", 'v', kVersionHelp, &CommandLineOptions::requested_version);
  parser.AddSwitch("omit-module-lines", 0, kOmitModuleLinesHelp,
                   &CommandLineOptions::omit_module_lines);
  parser.AddSwitch("prettify-backtrace", 0, kPrettifyBacktraceHelp,
                   &CommandLineOptions::prettify_backtrace);
  parser.AddSwitch("dumpfile-output", 0, kDumpfileOutputHelp, &CommandLineOptions::dumpfile_output);
  parser.AddSwitch("analytics", 0, kAnalyticsHelp, &CommandLineOptions::analytics);
  parser.AddSwitch("analytics-show", 0, kAnalyticsShowHelp, &CommandLineOptions::analytics_show);

  // Special --help switch which doesn't exist in the options structure.
  bool requested_help = false;
  parser.AddGeneralSwitch("help", 'h', kHelpHelp, [&requested_help]() { requested_help = true; });

  auto s = parser.Parse(argc, argv, options, &params);
  if (s.has_error()) {
    return s.error_message();
  }

  if (requested_help || !params.empty()) {
    return kHelpIntro + parser.GetHelp();
  }

  options->SetupDefaultsFromEnvironment();
  return Error();
}

void CommandLineOptions::SetupDefaultsFromEnvironment() {
  // Setup default values.
  if (const char* home = std::getenv("HOME"); home) {
    std::string home_str = home;
    if (!this->symbol_cache) {
      this->symbol_cache = home_str + "/.fuchsia/debug/symbol-cache";
    }
    if (this->symbol_index_files.empty()) {
      for (const auto& path : {home_str + "/.fuchsia/debug/symbol-index.json",
                               home_str + "/.fuchsia/debug/symbol-index"}) {
        std::error_code ec;
        if (std::filesystem::exists(path, ec)) {
          this->symbol_index_files.push_back(path);
        }
      }
    }
  }
  const char* raw_urls = std::getenv("DEBUGINFOD_URLS");
  if (raw_urls) {
    if (std::istringstream urls(raw_urls); !urls.str().empty()) {
      std::string url;
      while (std::getline(urls, url, ' ')) {
        if (std::find(this->public_symbol_servers.begin(), this->public_symbol_servers.end(),
                      url) == this->public_symbol_servers.end()) {
          this->public_symbol_servers.push_back(url);
        }
      }
    }
  }
}

}  // namespace symbolizer

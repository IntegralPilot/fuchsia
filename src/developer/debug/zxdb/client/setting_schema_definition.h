// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_SETTING_SCHEMA_DEFINITION_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_SETTING_SCHEMA_DEFINITION_H_

#include <string>
#include <vector>

namespace zxdb {

// This header contains the definitions for all the settings used within the client. They are within
// their own namespace to avoid collision. Usage:
//
//  system.GetString(settings::kSymbolPaths)

class SettingSchema;

// This is the global declaration of the setting names, so that we have a symbol for each of them.
// The definition of these symbols are in the appropiate context: (System for system, Target for
// target, etc.).
//
// Settings that appear at multiple levels should be declared (with their help message) at the
// most specific level they're needed.
struct ClientSettings {
  struct System {
    static const char* kAutoCastToDerived;
    static const char* kDebugMode;
    static const char* kAutoAttachLimbo;
    static const char* kShowFilePaths;
    static const char* kShowStdout;

    static const char* kConsoleMode;
    static const char* kConsoleMode_Shell;
    static const char* kConsoleMode_Embedded;
    static const char* kConsoleMode_EmbeddedInteractive;
    static const char* kConsoleMode_NonInteractive;
    static const char* kEmbeddedModeContext;

    static const char* kLanguage;
    static const char* kLanguage_Cpp;
    static const char* kLanguage_Rust;
    static const char* kLanguage_Auto;

    static const char* kSecondChanceExceptions;

    static const char* kSkipUnsymbolized;

    // Symbol lookup.
    static const char* kSymbolIndexFiles;
    static const char* kSymbolIndexInclude;
    static const char* kSymbolPaths;
    static const char* kBuildIdDirs;
    static const char* kIdsTxts;
    static const char* kSymbolCache;

    // Symbol servers added from the command line or environment variables are naturally ambiguous
    // to which bucket they belong in. Command line arguments are split into clear options,
    // environment variables are assumed to be public.
    static const char* kPrivateSymbolServers;
    static const char* kPublicSymbolServers;

    static const char* kEnableAnalytics;

    // Console options (having this here in the client is a layering violation but is not
    // worth creating some complicated system for dynamically injecting these to avoid.
    static const char* kUiTimeoutMs;

    // Source code context lines to display around the current line when we stop.
    static const char* kContextLinesAfter;
    static const char* kContextLinesBefore;
    static const char* kContextLines;
  };

  struct Target {
    // Remap source paths.
    static const char* kSourceMap;
    static const char* kSourceMapDescription;

    static const char* kIntegerFormat;  // Possible values are the kIntegerFormat*.
    static const char* kIntegerFormatDescription;

    static const char* kVectorFormat;  // Possible values are the kVectorRegisterFormatStr_*.
    static const char* kVectorFormatDescription;

    static const char* kAutoContinueWhenStepping;
    static const char* kAutoContinueWhenSteppingDescription;

    // Returns the possible options for kVectorFormat.
    static std::vector<std::string> GetVectorFormatOptions();
  };

  struct Thread {
    static const char* kDebugStepping;
    static const char* kDebugSteppingDescription;

    static const char* kDisplay;
    static const char* kDisplayDescription;
  };

  struct Breakpoint {
    static const char* kLocation;
    static const char* kLocationDescription;

    static const char* kScope;
    static const char* kScopeDescription;

    static const char* kEnabled;
    static const char* kEnabledDescription;

    static const char* kOneShot;
    static const char* kOneShotDescription;

    static const char* kType;
    static const char* kTypeDescription;

    static const char* kSize;
    static const char* kSizeDescription;

    // Possible values for kType.
    static const char* kType_Software;
    static const char* kType_Hardware;
    static const char* kType_ReadWrite;
    static const char* kType_Write;

    static const char* kStopMode;
    static const char* kStopModeDescription;

    // Possible values for kStopMode.
    static const char* kStopMode_None;
    static const char* kStopMode_Thread;
    static const char* kStopMode_Process;
    static const char* kStopMode_All;

    static const char* kHitCount;
    static const char* kHitCountDescription;

    static const char* kCondition;
    static const char* kConditionDescription;
  };

  struct Filter {
    static const char* kType;
    static const char* kTypeDescription;

    static const char* kPattern;
    static const char* kPatternDescription;

    static const char* kJob;
    static const char* kJobDescription;

    static const char* kWeak;
    static const char* kWeakDescription;

    static const char* kRecursive;
    static const char* kRecursiveDescription;

    static const char* kJobOnly;
    static const char* kJobOnlyDescription;
  };
};

// Returns true if the given key is a path-related setting.
bool IsPathSetting(const std::string& key);

// Schemas need to be initialized together because some schemas can add settings to other schemas.
// If we made it completely lazy, when the first thread is spun up, it could make new settings
// appear which is not what the user would expect.
void InitializeSchemas();

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_SETTING_SCHEMA_DEFINITION_H_

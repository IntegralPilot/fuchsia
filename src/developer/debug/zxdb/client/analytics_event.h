// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_ANALYTICS_EVENT_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_ANALYTICS_EVENT_H_

#include <chrono>

#include "src/developer/debug/zxdb/common/err.h"
#include "src/lib/analytics/cpp/core_dev_tools/google_analytics_4_client.h"

namespace zxdb {

// Base class for all zxdb analytics events. All events are associated with a session identifier,
// and the analytics library automatically adds a client/user identifier to all events.
//
// Derived classes should include methods to set individual parameters specific for that event.
// Details about the limits on events can be found here
// https://support.google.com/analytics/answer/12229021 and here
// https://support.google.com/analytics/answer/9267744?sjid=13155358058205528748-NC. Parameter types
// must be convertible to the valid types described for the base Ga4Event class.
class AnalyticsEvent : public analytics::core_dev_tools::Ga4Event {
 public:
  AnalyticsEvent(const std::string& name, const std::string& session_id);
};

// This is distinct from the analytics default Invoke event, since this initializes this session_id.
// This is issued upon any connection attempt after the session is constructed (we can't send the
// event during Session construction because that happens before the analytics backend is
// initialized, and that needs the session).
class SessionStarted final : public AnalyticsEvent {
 public:
  explicit SessionStarted(const std::string& session_id);
};

// Issued when the session is connected to a RemoteAPI implementation.
class SessionConnected final : public AnalyticsEvent {
 public:
  explicit SessionConnected(const std::string& session_id);

  enum class RemoteType {
    kRemoteAgent,  // Connected to a remote DebugAgent.
    kLocalAgent,   // Connected to a local DebugAgent.
    kMinidump,     // Connected to MinidumpRemoteAPI.
  };

  void SetRemoteType(RemoteType type);
};

// Issued when the Session is destructed.
class SessionEnded final : public AnalyticsEvent {
 public:
  explicit SessionEnded(const std::string& session_id);

  void SetSessionTime(std::chrono::milliseconds session_time);
};

// A struct to encapsulate all of the information regarding a specific command being executed,
// including any errors that are reported along the way, including parsing and asynchronous errors.
// This struct would really like to use some of the defined types defined for verbs and nouns, but
// we cannot introduce a dependency on the console in the client, so they are casted to an int when
// added to this struct.
struct CommandReport {
  // The verb's numerical id.
  int verb_id = 0;

  // This is the canonical name for the typed verb, if the user used an alias, it will not be
  // reflected here.
  std::string verb;

  struct NounReport {
    NounReport(int noun, const std::string& name, int index) : id(noun), name(name), index(index) {}

    // The noun's numerical id.
    int id = 0;

    // The canonical names for this noun. The command structure doesn't keep track of aliases used.
    std::string name;

    // The index given for this noun, if any. Note this can be negative e.g. for kNoIndex or
    // kWildcard.
    int index;
  };

  // Nouns for this command. The order will NOT be the order specified on the command line, but
  // rather the order in which the nouns are declared in |Nouns|. This is because the command does
  // no bookkeeping for the order of nouns, but rather just the presence (and possible indices).
  std::vector<NounReport> nouns;

  // The command group for this command, as it appears in the help listing.
  int command_group = 0;

  // Positional arguments. May be filtered out or ellided for certain commands.
  std::vector<std::string> arguments;

  struct SwitchReport {
    SwitchReport(int id, const std::string& name, const std::string& value)
        : id(id), name(name), value(value) {}
    // The id for this switch, as defined by the command.
    int id = 0;

    std::string name;

    // Not all switches have associated values, so this may be empty.
    std::string value;
  };

  // Switches, if provided.
  std::vector<SwitchReport> switches;

  // The command result.
  Err err;
};

class CommandEvent final : public AnalyticsEvent {
 public:
  explicit CommandEvent(const std::string& session_id);

  void FromCommandReport(const CommandReport& report);
};

class ConsoleType final : public AnalyticsEvent {
 public:
  explicit ConsoleType(const std::string& session_id);

  enum class Type {
    kCommandLine,
    kCommandLineEmbedded,
    kDebugAdapter,
  };

  void SetConsoleType(Type type);
};

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_ANALYTICS_EVENT_H_

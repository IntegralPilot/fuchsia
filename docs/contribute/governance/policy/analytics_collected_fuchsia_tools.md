# Analytics collected by Fuchsia tools

Fuchsia's core developer tools use Google Analytics to report feature usage and
crash reports to Google. How Google handles this data is described in the
[Google Privacy Policy](https://policies.google.com/privacy) and this handling
could include Google's examination of the collected data in aggregate to help
improve these tools, other Fuchsia tools, and the Fuchsia SDK.

The tools include `ffx` (and all its subtools), `zxdb`, `fidlcat`,
`symbolizer`, and the Fuchsia extension for VS Code. If you disable
analytics, an opt-out event is sent and after that no further analytics will be
sent by the tools to Google until analytics is enabled again. Otherwise, if
analytics is enabled, invocations and usage of these tools will result in data
collection including the following:

- CPU architecture (e.g. `x86_64`, `arm64`, etc.), kernel name (e.g. Linux,
  Darwin, etc...) of the system.
- The version of the tool and related Fuchsia environments (e.g. the Fuchsia SDK
  version).
- Usage of and interactions with features and whether certain features are
  enabled.
- Information on how a tool is invoked. (e.g. whether invoked in an CI
  environment, invoker information, etc.)
- The event of analytics being enabled or disabled.
- Success, failure, exceptions, errors, and timing of a task performed by a tool.
- Crashes of a tool.
- Operational metrics (e.g. symbolizer will report the number of modules with
  local symbols for each symbolized stack trace).
- A random unique ID ([RFC 4122 version-4 UUID][rfc-4122]{: .external}),
  generated for the current user when the user opts in to analytics.

When collecting data, Fuchsia plans to remove open text (e.g. argument value of
`ffx` flags) and other types of potentially identifying information (e.g. a full
stack trace).

[rfc-4122]: https://datatracker.ietf.org/doc/html/rfc4122
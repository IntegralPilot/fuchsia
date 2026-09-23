# Session Framework

Reviewed: 2022-04-11

Infrastructure and tools for the management of [session components][glossary.session-component].

## Getting started

To run `session_manager`, the platform service which runs sessions, see [bin/session_manager](/src/session/bin/session_manager).

Example sessions can be found in [examples/](/src/session/examples).

## Source Code Layout

The layout of the `//src/session` directory follows the [Fuchsia Source Code
Layout][source-code-layout].

The `session_manager` code lives in `//src/session/bin/session_manager`. High
level descriptions of the contents in the session subdirectories are as follows:

  - `bin`: implementations of session and element manager components
  - `examples`: example session implementations
  - `tools`: tools which interact with the `session_manager` and
  running `session`
  - `lib`: libraries used by `session_manager`
  - `tests`: integration tests for `tools` and `bin`

[glossary.session-component]: /docs/glossary.md#session-component
[source-code-layout]: /docs/development/source_code/layout.md

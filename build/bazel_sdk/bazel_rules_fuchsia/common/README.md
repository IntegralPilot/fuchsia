This directory contains shared target definitions and helper
`.bzl` files that are used by Fuchsia SDK external repositories,
such as `@fuchsia_sdk` or `@fuchsia_clang`, as well as the Fuchsia
platform (in-tree) Bazel workspace.

- Their content MUST NOT depend on definitions outside of this
  directory.

  I.e. they should be runnable even if no other Bazel repository is
  available / setup yet.

- They should not assume to be in any specific workspace
  (so no explicit reference to `@fuchsia_sdk` or
  `@rules_fuchsia` should exist in this directory).

  `load()` statements and target definitions that appear there
  should only use `//common:<path>` or `//common/<subpackage>:<path>`
  labels to address its content.

- The content may be copied or available from different
  repositories, using the same top-level `common` directory.

  I.e. `@fuchsia_sdk//common`, `@fuchsia_clang//common`
  or `@fuchsia_sdk_common//common` (in-tree) will refer to the same
  content.

- These are implementation details of the Fuchsia SDK rules
  and in-tree build, and should not be relied on by OOT Bazel
  workspaces. Their content might change drastically between
  releases.

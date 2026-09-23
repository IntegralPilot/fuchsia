#### Objective
Migrate target FIDL libraries from GN to Bazel.

### Migrated Libraries
- List the target FIDL libraries that need to be migrated.
Example:
fuchsia.lowpan.device, fuchsia.lowpan.driver
fuchsia.lowpan.experimental, fuchsia.lowpan.spinel
fuchsia.lowpan.test, fuchsia.lowpan.thread
fuchsia.media.playback, fuchsia.mediacodec
fuchsia.memory.debug, fuchsia.net.sockets


#### Implementation Steps
- List the steps to migrate the target FIDL libraries from GN to Bazel.
Example:
1. Create BUILD.bazel with fidl_library target.
2. Copy the comments in BUILD.gn to the relative position in BUILD.bazel.
3. Remove fidl() target and import("//build/fidl/fidl.gni") from BUILD.gn.
4. Run fx bazel2gn to sync back to GN build file.
5. Register in category_lists.bzl if applicable.
6. Register in bazel2gn_verification_targets.gni.


#### Verifications
- List the tests and commands which are run after all libraries are migrated.
Example:
* Verify Bazel build for each library:
	`fx bazel build --config=fuchsia_platform //sdk/fidl/<library_name>:<library_name>`
* Run Bazel rule test:
	`fx bazel build --config=fuchsia_platform //build/bazel/rules/tests`
* Verify compatibility tests:
	`fx build //sdk/fidl:compatibility_tests`
	`fx bazel build --config=fuchsia_platform //sdk/fidl:compatibility_tests`
* Run full build at the end:
	`fx build`


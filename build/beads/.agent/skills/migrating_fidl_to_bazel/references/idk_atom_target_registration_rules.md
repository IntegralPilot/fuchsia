Follow the rules below to add the IDK atom target to appropriate lists in `//sdk/fidl/category_lists.bzl` based on its category attribute in the `fidl_library` target:
- `partner` (and `stable` is `true`) -> `PARTNER_IDK_STABLE_FIDL_LIBRARY_ATOMS_LIST`
- `partner` (and `stable` is `false`) -> `PARTNER_IDK_UNSTABLE_FIDL_LIBRARY_ATOMS_LIST`
- `prebuilt` -> `PREBUILT_FIDL_LIBRARY_ATOMS_LIST`
- `host_tool` -> `HOST_TOOL_FIDL_LIBRARY_ATOMS_LIST`
- `compat_test` -> `COMPAT_TEST_FIDL_LIBRARY_ATOMS_LIST`
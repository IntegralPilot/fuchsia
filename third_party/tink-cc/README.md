# Tink C++ in Cobalt / Fuchsia

This directory contains the standalone C++ implementation of Tink (`tink-cc`), imported as a Git submodule (in Cobalt standalone) and checked out via Jiri / manifest (in Fuchsia tree).

## Structure
* `src/`: Upstream repository containing the C++ source code and Bazel build definitions (`BUILD.bazel`).
* `BUILD.gn`: The root configuration file defining `tink_config` and `tink_fips`.
* `proto/`, `tink/`, `tink/hybrid/`, ...: Structured GN package subdirectories generated automatically from the upstream Bazel build files.
* `tools/convert_for_cobalt`: Automated build file generator script.

---

## How to Uprev Tink C++

When updating (upreving) `tink-cc` to a new upstream commit or release, follow these steps to update the source tree and regenerate the GN build files:

### 1. Update the Submodule / Pin
* **In Cobalt standalone:**
  ```bash
  cd third_party/tink-cc/src
  git fetch origin
  git checkout <new_commit_hash_or_tag>
  cd ../../..
  ```
* **In Fuchsia tree:**
  Update the revision pin for `third_party/github.com/tink-crypto/tink-cc` in `manifests/third_party/all`, then run `jiri update`.

### 2. Regenerate GN Build Files
Run the `convert_for_cobalt` tool from the workspace root to parse the new upstream `BUILD.bazel` files and regenerate all subdirectory `BUILD.gn` files:
```bash
python3 third_party/tink-cc/tools/convert_for_cobalt
```

This script automatically:
* Cleans up any outdated or orphaned `.gn` files in the package subdirectories.
* Traverses the Bazel build graph inside `src/`.
* Creates new subdirectory folders as needed for any newly added upstream packages.
* Generates all `.gn` build files with the automatic generation warning header.

### 3. Verify Build
Test the build to ensure the new upstream targets compile cleanly:
* **In Cobalt standalone:** `python3 cobaltb.py build`
* **In Fuchsia tree:** `fx build`

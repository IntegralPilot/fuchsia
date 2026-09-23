# Fuchsia Dependency Discovery Tool

This tool generates an accurate, trustworthy inventory of third-party dependencies in the Fuchsia tree. It is designed to assist the vulnerability management program by identifying all external code included in the build or source tree.

## Conceptual Overview

The tool identifies dependencies by querying multiple sources of truth:
*   **Source Checkout**: Uses `jiri` to identify external repositories checked out in the source tree.
*   **Language Manifests**: Parses `Cargo.lock`, `go.mod`, and `requirements.txt` for language-specific dependencies.

### Third-Party Dependency Criteria

1.  Any Jiri project checked out under a `third_party` directory is considered to be a third party dependency.
2.  Any source directory with two instances of "third_party" containing build files or metadata is considered to be a transitive dependency.
    *   *Example*: `third_party/boringssl/src/third_party/googletest` qualifies as its own dependency, even if it's part of boringssl.
3.  For any third party dependencies under a language like Rust, Go, Python, the tool checks `Cargo.toml`, `go.mod`, `requirements.txt`.

## Prerequisites

Before running this script, you must have a configured Fuchsia build environment.

```
fx set <product>.<board>  # e.g., fx set core.x64
fx build
```

## Usage

Run the script from the Fuchsia root (or anywhere, it detects the root relative to itself):

```
python3 src/security/tools/find_deps/find_deps.py
```

## Output

The tool generates a CSV file named `deps_report.csv` in the current working directory.

### Columns

-   **Name**: The name of the dependency (e.g., from Jiri project name or package manifest).
-   **Path**: The relative path to the dependency within the Fuchsia tree.
-   **Remote**: The upstream repository URL (derived from Jiri).
-   **Estimated Version**: The version string extracted from manifests or the Jiri revision/tag.
-   **Source**: How the dependency was discovered (e.g., `Direct(Jiri)`, `Manifest(Cargo)`, `Transitive(Local)`).
-   **METADATA Location**: Path to the `METADATA` or `README.fuchsia` file if found.
-   **Link**: A direct link to the source code or metadata file (populated only for `Direct(Jiri)` dependencies).

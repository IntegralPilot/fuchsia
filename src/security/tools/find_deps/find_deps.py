# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import csv
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Dict, List, Optional, Set, TypedDict


class DependencyInfo(TypedDict):
    Name: str
    Path: str
    Remote: str
    Source: str
    Estimated_Version: str
    METADATA_Location: str
    Link: str


def get_jiri_projects(
    fuchsia_root: Path,
) -> Dict[str, Dict[str, Optional[str]]]:
    """Runs `jiri project -json-output` to get a list of active projects.

    Note: The listing of `jiri project` is almost entirely references to 3p dep
    mirrors, but it's technically a list of all the remote repos we might
    depend on so there will be some exceptions.

    Args:
        fuchsia_root: The root directory of the Fuchsia checkout.

    Returns:
        A dictionary mapping the relative path of the project to a dictionary
        containing project details: 'name', 'revision', and 'remote'.
    """
    projects = {}

    # Check fuchsia root/.jiri_root/bin/jiri, otherwise use jiri from PATH
    jiri_cmd = "jiri"
    local_jiri = fuchsia_root / ".jiri_root/bin/jiri"
    if local_jiri.exists():
        jiri_cmd = str(local_jiri)

    try:
        # Use a temporary file to store JSON output
        with tempfile.NamedTemporaryFile(mode="w+", delete=True) as tmp:
            cmd = [jiri_cmd, "project", "-json-output", tmp.name]
            subprocess.check_call(cmd, cwd=fuchsia_root)

            # Read back the JSON
            # Note: jiri writes to the file path provided
            data = json.load(tmp)

            for p in data:
                path = p.get("path")
                if path:
                    # Make path relative to fuchsia_root if it's absolute
                    if path.startswith(str(fuchsia_root)):
                        rel_path = os.path.relpath(path, fuchsia_root)
                    else:
                        rel_path = path

                    projects[rel_path] = {
                        "name": p.get("name"),
                        "revision": p.get("revision"),
                        "remote": p.get("remote"),
                    }
    except (subprocess.CalledProcessError, FileNotFoundError) as e:
        print(f"Warning: Failed to run jiri project: {e}", file=sys.stderr)
    except json.JSONDecodeError as e:
        print(f"Warning: Failed to parse jiri output: {e}", file=sys.stderr)

    return projects


def discover_local_deps(fuchsia_root: Path) -> Set[str]:
    """Scans third_party directory for build or metadata files.

    Args:
        fuchsia_root: The root directory of the Fuchsia checkout.

    Returns:
        A set of directories relative to fuchsia_root that contain build or
        metadata files.
    """
    paths = set()
    third_party = fuchsia_root / "third_party"
    if not third_party.exists():
        return paths

    try:
        # Find all directories containing BUILD.gn, BUILD.bazel, or metadata files
        cmd = [
            "find",
            "third_party",
            "(",
            "-iname",
            "BUILD.gn",
            "-o",
            "-iname",
            "BUILD.bazel",
            "-o",
            "-iname",
            "METADATA",
            "-o",
            "-iname",
            "METADATA.textproto",
            "-o",
            "-iname",
            "README.fuchsia",
            ")",
            "-printf",
            "%h\n",
        ]
        output = subprocess.check_output(cmd, cwd=fuchsia_root, text=True)
        for line in output.splitlines():
            line = line.strip()
            if line:
                paths.add(line)
    except subprocess.CalledProcessError as e:
        print(f"Warning: Failed to scan for local deps: {e}", file=sys.stderr)

    return paths


def parse_cargo_lock(lock_path: Path) -> Dict[str, str]:
    """Parses Cargo.lock to find package names and versions.

    Example Cargo.lock entry:
        [[package]]
        name = "anyhow"
        version = "1.0.95"
        source = "registry+https://github.com/rust-lang/crates.io-index"
        checksum = "34ac096ce696dc2fcab58effbb8ddbe440afee819145d9bb1a0ffe5f5494206c"

    Args:
        lock_path: Path to the Cargo.lock file.

    Returns:
        A dictionary mapping package names to their versions.
    """
    packages = {}
    if not lock_path.exists():
        print(f"Warning: Cargo.lock not found at {lock_path}", file=sys.stderr)
        return packages

    with open(lock_path, "r") as f:
        in_package = False
        current_name = None
        current_version = None
        for line in f:
            line = line.strip()
            if line == "[[package]]":
                if in_package:
                    if current_name and current_version:
                        packages[current_name] = current_version
                    elif current_name or current_version:
                        print(
                            f"Warning: Skipping incomplete package in Cargo.lock: name={current_name}, version={current_version}",
                            file=sys.stderr,
                        )
                in_package = True
                current_name = None
                current_version = None
            elif in_package:
                if line.startswith("name ="):
                    match = re.search(r'name\s*=\s*"([^"]+)"', line)
                    if match:
                        current_name = match.group(1)
                elif line.startswith("version ="):
                    match = re.search(r'version\s*=\s*"([^"]+)"', line)
                    if match:
                        current_version = match.group(1)
        # Handle last package
        if in_package:
            if current_name and current_version:
                packages[current_name] = current_version
            elif current_name or current_version:
                print(
                    f"Warning: Skipping incomplete package in Cargo.lock: name={current_name}, version={current_version}",
                    file=sys.stderr,
                )
    return packages


def parse_go_mod(mod_path: Path) -> Dict[str, str]:
    """Parses go.mod to find required modules and versions.

    Example go.mod entry:
        module go.fuchsia.dev/fuchsia

        go 1.23

        require (
            github.com/google/go-cmp v0.6.0
            golang.org/x/sys v0.10.0
        )

    Args:
        mod_path: Path to the go.mod file.

    Returns:
        A dictionary mapping module names to their versions.
    """
    modules = {}
    if not mod_path.exists():
        print(f"Warning: go.mod not found at {mod_path}", file=sys.stderr)
        return modules

    with open(mod_path, "r") as f:
        in_require = False
        for line in f:
            line = line.strip()
            if line.startswith("require ("):
                in_require = True
                continue
            if in_require and line.startswith(")"):
                in_require = False
                continue

            if in_require:
                parts = line.split()
                if len(parts) < 2:
                    print(
                        f"Warning: Skipping incomplete line in go.mod require block: '{line}'",
                        file=sys.stderr,
                    )
                    continue
                # parts[0] is module, parts[1] is version
                modules[parts[0]] = parts[1]
    return modules


def parse_requirements_txt(req_path: Path) -> Dict[str, str]:
    """Parses a requirements.txt file.

    See https://pip.pypa.io/en/stable/reference/requirements-file-format/

    Args:
        req_path: Path to the requirements.txt file.

    Returns:
        A dictionary mapping package names to their version specifiers.
    """
    packages = {}
    if not req_path.exists():
        print(
            f"Warning: requirements.txt not found at {req_path}",
            file=sys.stderr,
        )
        return packages

    # Try to use packaging.requirements for robust parsing
    try:
        from packaging.requirements import Requirement

        use_packaging = True
    except ImportError:
        use_packaging = False
        print(
            "Warning: packaging library not found, falling back to simple regex.",
            file=sys.stderr,
        )

    with open(req_path, "r") as f:
        lines = f.readlines()

    # Pre-process lines to handle line continuations
    combined_lines = []
    current_line = ""
    for line in lines:
        line = line.rstrip("\n")
        if line.endswith("\\"):
            current_line += line[:-1].strip() + " "
        else:
            current_line += line
            combined_lines.append(current_line)
            current_line = ""

    for line in combined_lines:
        line = line.strip()
        # Remove comments
        if "#" in line:
            line = line.split("#", 1)[0].strip()

        if not line:
            continue

        if line.startswith("-"):
            continue

        if use_packaging:
            # Clean up pip options (e.g. --hash) which packaging doesn't like
            # Helper: split on " --" to remove flags
            clean_line = line.split(" --")[0].strip()
            try:
                req = Requirement(clean_line)
                # Store the full specifier (e.g. ">=1.0,!=1.2")
                spec = str(req.specifier) if req.specifier else "Any"
                if spec.startswith("=="):
                    spec = spec[2:]
                packages[req.name] = spec
            except Exception as e:
                # Fall back to regex if packaging fails
                print(
                    f"Failed to parse '{line}' with packaging: {e}, falling back to regex",
                    file=sys.stderr,
                )
            else:
                continue

        # Fallback regex logic (same as original)
        match = re.match(r"^([a-zA-Z0-9_\-\.]+)(.*)", line)
        if match:
            name = match.group(1)
            rest = match.group(2).strip()
            version = "N/A"
            if rest:
                v_match = re.search(
                    r"(==|>=|<=|>|<|~=|!=)\s*([a-zA-Z0-9_\-\.]+)", rest
                )
                if v_match:
                    if v_match.group(1) == "==":
                        version = v_match.group(2)
                    else:
                        version = v_match.group(1) + v_match.group(2)
            packages[name] = version
    return packages


def find_requirements_txts(root_dir: Path) -> List[str]:
    """Finds all requirements.txt files in third_party and manifests/third_party/pylibs.

    Args:
        root_dir: The root directory to search in.

    Returns:
        A list of relative paths to requirements.txt files.
    """
    req_files = []
    search_paths = ["third_party", "manifests/third_party/pylibs"]

    for path in search_paths:
        if not (root_dir / path).exists():
            continue

        try:
            cmd = [
                "find",
                path,
                "-iname",
                "requirements.txt",
                "-not",
                "-path",
                "*/.*",
            ]
            output = subprocess.check_output(cmd, cwd=root_dir, text=True)
            for line in output.splitlines():
                if line.strip():
                    req_files.append(line.strip())
        except subprocess.CalledProcessError as e:
            print(
                f"Warning: Failed to search for requirements.txt in {path}: {e}",
                file=sys.stderr,
            )
    return req_files


def find_metadata(base_dir: Path, dep_path: str) -> str:
    """Checks for METADATA or README.fuchsia in the dep_path.

    Expected locations for metadata:
    1. Jiri/Git Projects: Project root (e.g. third_party/zstd).
    2. Rust Crates: third_party/rust_crates/vendor/<crate> or forks/<crate>.
    3. Go Modules: third_party/golibs/vendor/<module>.
    4. Python Packages: third_party/pylibs/<package> or third_party/pypi/<package>.
    5. Local Paths: Any discovered local path with metadata.

    Args:
        base_dir: The base directory to resolve paths from.
        dep_path: The relative path to the dependency.

    Returns:
        The relative path to the metadata file if found, else "N/A".
    """
    potential_files = ["METADATA", "METADATA.textproto", "README.fuchsia"]
    full_dep_path = base_dir / dep_path

    if not full_dep_path.exists():
        return "N/A"

    for f in potential_files:
        p = full_dep_path / f
        if p.exists():
            return str(p.relative_to(base_dir))

    return "N/A"


def collect_jiri_deps(
    fuchsia_root: Path, jiri_projects: Dict[str, Dict[str, Optional[str]]]
) -> List[DependencyInfo]:
    """Collects dependencies from Jiri projects.

    Args:
        fuchsia_root: The root directory of the Fuchsia checkout.
        jiri_projects: A dictionary of Jiri projects and their details.

    Returns:
        A list of DependencyInfo dictionaries.
    """
    deps: List[DependencyInfo] = []
    for path, info in jiri_projects.items():
        if "third_party" not in path:
            continue

        name = info.get("name", path)
        metadata = find_metadata(fuchsia_root, path)

        # If no metadata is found and "src" is the suffix, search in the parent directory for metadata
        if metadata == "N/A" and path.endswith("src"):
            metadata = find_metadata(fuchsia_root, path[:-4])

        deps.append(
            DependencyInfo(
                Name=name,
                Path=path,
                Remote=info.get("remote", "N/A") or "N/A",
                Source="Direct(Jiri)",
                Estimated_Version=info.get("revision", "N/A") or "N/A",
                METADATA_Location=metadata,
                Link="N/A",
            )
        )
    return deps


def collect_rust_deps(
    fuchsia_root: Path, rust_packages: Dict[str, str]
) -> List[DependencyInfo]:
    """Collects Rust dependencies from Cargo.lock.

    Args:
        fuchsia_root: The root directory of the Fuchsia checkout.
        rust_packages: A dictionary mapping package names to versions.

    Returns:
        A list of DependencyInfo dictionaries.
    """
    deps: List[DependencyInfo] = []
    rust_vendor_prefix = "third_party/rust_crates/vendor/"
    for pkg, ver in rust_packages.items():
        path = f"{rust_vendor_prefix}{pkg}"
        metadata = "third_party/rust_crates/Cargo.lock"

        deps.append(
            DependencyInfo(
                Name=pkg,
                Path=path,
                Remote="N/A",
                Source="Manifest(Cargo)",
                Estimated_Version=ver,
                METADATA_Location=metadata,
                Link="N/A",
            )
        )
    return deps


def collect_go_deps(
    fuchsia_root: Path, go_modules: Dict[str, str]
) -> List[DependencyInfo]:
    """Collects Go dependencies from go.mod.

    Args:
        fuchsia_root: The root directory of the Fuchsia checkout.
        go_modules: A dictionary mapping module names to versions.

    Returns:
        A list of DependencyInfo dictionaries.
    """
    deps: List[DependencyInfo] = []
    go_vendor_prefix = "third_party/golibs/vendor/"
    for mod, ver in go_modules.items():
        path = f"{go_vendor_prefix}{mod}"
        metadata = "third_party/golibs/go.mod"

        deps.append(
            DependencyInfo(
                Name=mod,
                Path=path,
                Remote="N/A",
                Source="Manifest(Go)",
                Estimated_Version=ver,
                METADATA_Location=metadata,
                Link="N/A",
            )
        )
    return deps


def collect_python_deps(
    fuchsia_root: Path,
) -> List[DependencyInfo]:
    """Collects Python dependencies from requirements.txt files.

    Args:
        fuchsia_root: The root directory of the Fuchsia checkout.

    Returns:
        A list of DependencyInfo dictionaries.
    """
    deps: List[DependencyInfo] = []
    python_pkg_info = {}
    req_files = find_requirements_txts(fuchsia_root)
    for req_file in req_files:
        pkgs = parse_requirements_txt(fuchsia_root / req_file)
        for p, v in pkgs.items():
            if p not in python_pkg_info:
                python_pkg_info[p] = {}
            python_pkg_info[p][req_file] = v

    for pkg, sources in python_pkg_info.items():
        for source_file, version in sources.items():
            path = f"python_package:{pkg}"

            # Heuristic for metadata
            metadata = "N/A"
            for candidate_base in [
                f"third_party/pypi/{pkg}",
                f"third_party/pylibs/{pkg}",
            ]:
                m = find_metadata(fuchsia_root, candidate_base)
                if m != "N/A":
                    metadata = m
                    break
            if metadata == "N/A":
                metadata = source_file

            deps.append(
                DependencyInfo(
                    Name=pkg,
                    Path=path,
                    Remote="N/A",
                    Source="Manifest(Pip)",
                    Estimated_Version=version,
                    METADATA_Location=metadata,
                    Link="N/A",
                )
            )
    return deps


def get_third_party_root(path: str) -> Optional[str]:
    """Finds the third_party root directory for a given path.

    Args:
        path: The path to check.

    Returns:
        The path to the third_party root (e.g. 'third_party/foo') if found,
        otherwise None.
    """
    parts = path.split("/")
    try:
        idx = parts.index("third_party")
        if idx + 1 < len(parts):
            return "/".join(parts[: idx + 2])
    except ValueError:
        pass
    return None


def collect_local_deps(
    fuchsia_root: Path,
    local_paths: Set[str],
    processed_paths: Set[str],
) -> List[DependencyInfo]:
    """Collects remaining local dependencies with filtering logic.

    Args:
        fuchsia_root: The root directory of the Fuchsia checkout.
        local_paths: A set of local paths with build/metadata files.
        processed_paths: A set of paths that have already been processed.

    Returns:
        A list of DependencyInfo dictionaries.
    """
    deps: List[DependencyInfo] = []

    # Pre-calculate roots of existing processed paths
    known_roots = set()
    for p in processed_paths:
        root = get_third_party_root(p)
        if root:
            known_roots.add(root)

    # Sort local_paths by length (shortest first) to ensure we process parents before children
    sorted_local_paths = sorted(local_paths, key=len)

    for path in sorted_local_paths:
        if path in processed_paths:
            continue

        # Find closest parent (longest prefix) in processed_paths
        closest_parent = None
        for pp in processed_paths:
            if path.startswith(pp + "/") or path == pp:
                if closest_parent is None or len(pp) > len(closest_parent):
                    closest_parent = pp

        if closest_parent:
            # Check relative path for nested third_party
            rel = path[len(closest_parent) + 1 :]
            if "third_party" in rel.split("/"):
                # Nested but new third_party layer -> Include
                metadata = find_metadata(fuchsia_root, path)
                deps.append(
                    DependencyInfo(
                        Name=path,
                        Path=path,
                        Remote="N/A",
                        Source="Transitive(Local)",
                        Estimated_Version="N/A",
                        METADATA_Location=metadata,
                        Link="N/A",
                    )
                )
                processed_paths.add(path)
                # Add new root if applicable
                new_root = get_third_party_root(path)
                if new_root:
                    known_roots.add(new_root)
            else:
                # Internal to existing dep -> Exclude
                continue
        else:
            # No parent found.
            # Check for sibling/root conflict with known roots.
            p_root = get_third_party_root(path)
            if p_root and p_root in known_roots:
                # Sibling Check: Exclude (share root with known project)
                continue

            # New top-level dependency
            metadata = find_metadata(fuchsia_root, path)
            deps.append(
                DependencyInfo(
                    Name=path,
                    Path=path,
                    Remote="N/A",
                    Source="Transitive(Local)",
                    Estimated_Version="N/A",
                    METADATA_Location=metadata,
                    Link="N/A",
                )
            )
            processed_paths.add(path)
            if p_root:
                known_roots.add(p_root)

    return deps


def write_report(
    rows: List[DependencyInfo],
    fuchsia_root: Path,
    output_file: str = "deps_report.csv",
):
    """Writes the dependency report to a CSV file.

    Args:
        rows: A list of DependencyInfo dictionaries.
        fuchsia_root: The root directory of the Fuchsia checkout.
        output_file: The name of the output CSV file.
    """
    # Add links and notes to the csv if applicable
    link_prefix = "https://source.corp.google.com/h/fuchsia/fuchsia/+/main:"
    for row in rows:
        # Only populate Link and Notes for Direct(Jiri) dependencies
        if row.get("Source") != "Direct(Jiri)":
            row["Link"] = "N/A"

            continue

        meta = row.get("METADATA_Location", "N/A")
        if meta != "N/A" and not meta.startswith("/"):
            row["Link"] = link_prefix + meta

        elif row["Path"].startswith("third_party"):
            row["Link"] = link_prefix + row["Path"]

    rows.sort(key=lambda x: x["Path"])

    with open(output_file, "w", newline="") as csvfile:
        fieldnames = [
            "Name",
            "Path",
            "Remote",
            "Estimated Version",
            "Source",
            "METADATA Location",
            "Link",
        ]
        writer = csv.DictWriter(csvfile, fieldnames=fieldnames)
        writer.writeheader()
        for row in rows:
            # Map DependencyInfo keys to CSV headers
            filtered_row = {
                "Name": row.get("Name", "N/A"),
                "Path": row.get("Path", "N/A"),
                "Remote": row.get("Remote", "N/A"),
                "Estimated Version": row.get("Estimated_Version", "N/A"),
                "Source": row.get("Source", "N/A"),
                "METADATA Location": row.get("METADATA_Location", "N/A"),
                "Link": row.get("Link", "N/A"),
            }
            writer.writerow(filtered_row)

    print(f"Generated {output_file} with {len(rows)} entries.")


def get_fuchsia_root() -> Path:
    """Finds the Fuchsia root directory.

    Checks FUCHSIA_DIR environment variable first.
    If not set, traverses up from the script directory until .fx-root is found.
    """
    if "FUCHSIA_DIR" in os.environ:
        return Path(os.environ["FUCHSIA_DIR"])

    # Traverse up looking for .fx-root
    current = Path(__file__).resolve().parent
    while current != current.parent:
        if (current / ".fx-root").is_file():
            return current
        current = current.parent

    # Fallback to the original relative method if .fx-root is missing (unlikely in a checkout)
    # This maintains behavior if the script is run in a weird environment without .fx-root
    print(
        "Warning: Could not find .fx-root, falling back to relative path assumption.",
        file=sys.stderr,
    )
    return Path(__file__).resolve().parents[4]


def main():
    fuchsia_root = get_fuchsia_root()
    print(f"Fuchsia Root: {fuchsia_root}")

    print("Fetching Jiri projects...")
    jiri_projects = get_jiri_projects(fuchsia_root)
    print(f"Found {len(jiri_projects)} Jiri projects.")

    print("Scanning for local third_party dependencies...")
    local_paths = discover_local_deps(fuchsia_root)
    print(f"Found {len(local_paths)} paths with build files in third_party.")

    cargo_lock_path = fuchsia_root / "third_party/rust_crates/Cargo.lock"
    go_mod_path = fuchsia_root / "third_party/golibs/go.mod"

    print("Parsing language manifests...")
    rust_packages = parse_cargo_lock(cargo_lock_path)
    go_modules = parse_go_mod(go_mod_path)

    all_deps = []
    processed_paths = set()

    jiri_deps = collect_jiri_deps(fuchsia_root, jiri_projects)
    all_deps.extend(jiri_deps)
    for d in jiri_deps:
        processed_paths.add(d["Path"])

    rust_deps = collect_rust_deps(fuchsia_root, rust_packages)
    all_deps.extend(rust_deps)
    for d in rust_deps:
        processed_paths.add(d["Path"])

    go_deps = collect_go_deps(fuchsia_root, go_modules)
    all_deps.extend(go_deps)
    for d in go_deps:
        processed_paths.add(d["Path"])

    python_deps = collect_python_deps(fuchsia_root)
    all_deps.extend(python_deps)
    # Python deps are virtual paths (python_package:name), usually not in processed_paths to block local paths

    local_deps = collect_local_deps(fuchsia_root, local_paths, processed_paths)
    all_deps.extend(local_deps)

    write_report(all_deps, fuchsia_root)


if __name__ == "__main__":
    main()

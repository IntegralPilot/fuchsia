#!/bin/bash
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# The main Cargo.toml is at third_party/rust_crates/Cargo.toml. This script is at
# third_party/rust_crates/forks/bluetooth/update_bluetooth.sh.
main_cargo_toml_path=$(realpath "$(dirname "${BASH_SOURCE[0]}")"/../../Cargo.toml)

if [ ! -f "$main_cargo_toml_path" ]; then
    echo "Error: Could not find main Cargo.toml at $main_cargo_toml_path"
    exit 1
fi

# Extract all dependency sections from the main Cargo.toml.
# This looks for any line that ends with .dependencies]
deps_sections=$(awk '/^\[.*dependencies\]/{p=1;next} /^\[/{p=0} p' "$main_cargo_toml_path")

declare -A external_crate_versions
declare -A fuchsia_versions
while IFS= read -r line; do
  # Trim leading whitespace and skip empty lines or comments
  line=$(echo "$line" | sed -e 's/^[ \t]*//' -e 's/[ \t]*#.*$//')
  if [[ -z "$line" || "$line" =~ ^bt- ]]; then
    continue
  fi
  crate_name=$(echo "$line" | cut -d'=' -f1 | tr -d ' "')
  external_crate_versions[$crate_name]="$line"

  # Extract the version number, handling formats like `version = "1.2.3"` and `= "1.2.3"` or `"1.2.3"`
  version=$(echo "$line" | sed -n 's/.*version\s*=\s*"\([^"]*\)".*/\1/p')
  if [ -z "$version" ]; then
    version=$(echo "$line" | sed -n 's/.*=\s*["=]*\([0-9.]\+\).*/\1/p')
  fi
  fuchsia_versions[$crate_name]="$version"
done <<< "$deps_sections"


export TMP_CHECKOUT=$(mktemp -d)

if [[ $# -ne 1 || $1 == "--help" ]]; then
  echo "Usage: ./update_bluetooth.sh [git_ref]";
  exit -1
fi

git_ref=$1;

git clone -n --depth=1 --filter=tree:0 https://bluetooth.googlesource.com/bluetooth $TMP_CHECKOUT

bluetooth_mirror_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd);

echo "Checking out $git_ref into $bluetooth_mirror_dir...";

cd $TMP_CHECKOUT;

git fetch https://bluetooth.googlesource.com/bluetooth $git_ref

git sparse-checkout set --no-cone /rust LICENSE
git checkout --quiet FETCH_HEAD -- .

declare -A bluetooth_versions
declare -A mismatched_crates
bluetooth_workspace_cargo_toml="$TMP_CHECKOUT/rust/Cargo.toml"

if [ -f "$bluetooth_workspace_cargo_toml" ]; then
    bluetooth_deps_section=$(awk '/^\[workspace\.dependencies\]/{p=1;next} /^\[/{p=0} p' "$bluetooth_workspace_cargo_toml")
    while IFS= read -r line; do
      line=$(echo "$line" | sed -e 's/^[ \t]*//' -e 's/[ \t]*#.*$//')
      if [[ -z "$line" ]]; then
        continue
      fi
      crate_name=$(echo "$line" | cut -d'=' -f1 | tr -d ' "')
      # Extract the version number, handling formats like `version = "1.2.3"` and `= "1.2.3"` or `"1.2.3"`
      version=$(echo "$line" | sed -n 's/.*version\s*=\s*"\([^"]*\)".*/\1/p')
      if [ -z "$version" ]; then
        version=$(echo "$line" | sed -n 's/.*=\s*["=]*\([0-9.]\+\).*/\1/p')
      fi
      bluetooth_versions[$crate_name]="$version"
    done <<< "$bluetooth_deps_section"

    for crate_name in "${!fuchsia_versions[@]}"; do
      fuchsia_version=${fuchsia_versions[$crate_name]}
      bluetooth_version=${bluetooth_versions[$crate_name]}

      if [[ -n "$bluetooth_version" && "$fuchsia_version" != "$bluetooth_version" ]]; then
        mismatched_crates[$crate_name]=1
      fi
    done
else
    echo "Warning: Could not find bluetooth workspace Cargo.toml at $bluetooth_workspace_cargo_toml. Skipping version check."
fi

licenses_to_solidify=$(find rust -name LICENSE);

for target in $licenses_to_solidify
do
  cp --remove-destination LICENSE $target
done

files_to_persist=(README.fuchsia update_bluetooth.sh)

for persist in ${files_to_persist[@]}
do
  cp -v $bluetooth_mirror_dir/$persist $TMP_CHECKOUT/rust/
done

#
# Update the Cargo.toml in the crates to remove all of *.workspace = true
# since rules_rust can't find the workspace file.
# See https://github.com/bazelbuild/rules_rust/issues/3059

crate_paths=rust/bt-*
declare -A missing_external_crates

for crate in ${crate_paths[@]}
do
  sed -i -e 's/edition.workspace = true/edition = "2021"/' $crate/Cargo.toml
  sed -i -e 's/license.workspace = true/license = "BSD-2-Clause"/' $crate/Cargo.toml

  # Handle internal crates (including dev-dependencies) by replacing workspace = true
  # with a relative path.
  for internal_crate_name in $crate_paths
  do
    crate_name=${internal_crate_name#"rust/"}
    # For dependencies like: `bt-foo.workspace = true`
    sed -i -e "s/$crate_name\.workspace = true/$crate_name = { path = \"..\\/$crate_name\" }/" $crate/Cargo.toml
    # For dependencies like: `bt-foo = { workspace = true, ... }`
    sed -i -e "s/$crate_name = { workspace = true *, /$crate_name = { path = \"..\\/$crate_name\", /" $crate/Cargo.toml
  done

  # Handle external crates by replacing workspace = true with the full version string.
  for external_crate_name in "${!external_crate_versions[@]}"
  do
    full_def=${external_crate_versions[$external_crate_name]}
    # Use | as a separator to avoid issues with special characters in the replacement string.
    sed -i -e "s|$external_crate_name.workspace = true|$full_def|" "$crate/Cargo.toml"
  done

  # Check for remaining workspace = true which indicates missing external crates in Fuchsia
  while IFS= read -r missing_crate; do
    if [[ -n "$missing_crate" ]]; then
      missing_external_crates[$missing_crate]=1
    fi
  done < <(grep -E "workspace[[:space:]]*=[[:space:]]*true" "$crate/Cargo.toml" | \
           sed -E -n -e 's/^[[:space:]]*([a-zA-Z0-9_-]+)\.workspace.*/\1/p' \
                     -e 's/^[[:space:]]*([a-zA-Z0-9_-]+)[[:space:]]*=[[:space:]]*\{.*workspace.*/\1/p' || true)
done

rm -r $bluetooth_mirror_dir
mkdir $bluetooth_mirror_dir
cp -rv rust/* $bluetooth_mirror_dir/

YELLOW="\033[0;33m"
RED="\033[0;31m"
GREEN="\033[0;32m"
RESET="\033[0m"

if [ ${#mismatched_crates[@]} -ne 0 ]; then
  echo ""
  echo -e "${YELLOW}================================================================================"
  echo "WARNING (NON-BLOCKING FOLLOW-UP): Bluetooth crate dependency versions are out of sync with Fuchsia."
  echo "You can proceed with the current roll, but please submit a change to the"
  echo "upstream bluetooth repository (bluetooth.git) later to update its rust/Cargo.toml."
  echo ""
  echo "The following crates have mismatched versions:"
  for crate_name in "${!mismatched_crates[@]}"; do
    echo "  - $crate_name (Fuchsia: ${fuchsia_versions[$crate_name]}, Bluetooth: ${bluetooth_versions[$crate_name]})"
  done
  echo ""
  echo "Update rust/Cargo.toml in the bluetooth repository with these lines:"
  for crate_name in "${!mismatched_crates[@]}"; do
    echo "    ${external_crate_versions[$crate_name]}"
  done
  echo -e "================================================================================${RESET}"
fi

if [ ${#missing_external_crates[@]} -ne 0 ]; then
  echo ""
  echo -e "${RED}================================================================================"
  echo "WARNING (BLOCKING): Missing external crates in Fuchsia's third_party/rust_crates/Cargo.toml."
  echo "Please add these crates to Fuchsia's third_party/rust_crates/Cargo.toml first."
  echo ""
  echo "The following external crates are used by Bluetooth but missing in Fuchsia:"
  for crate_name in "${!missing_external_crates[@]}"; do
    echo "  - $crate_name"
  done
  echo ""
  echo -e "Add an entry for them in ${YELLOW}third_party/rust_crates/Cargo.toml${RED} and rerun this script."
  echo -e "================================================================================${RESET}"
else
  echo ""
  echo -e "${GREEN}Run \`fx update-rustc-third-party\` to complete crate update. If it fails, try commenting out the targets that use the bluetooth crates and rerun the command again.${RESET}"
  echo ""
fi
echo "Done"
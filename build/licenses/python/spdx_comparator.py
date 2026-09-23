#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import dataclasses
import hashlib
import json
import logging
import re
from pathlib import Path
from typing import Any


@dataclasses.dataclass(frozen=True)
class _ExtractedLicense:
    """Per-license information that is used for comparisons"""

    path: str
    name: str
    text_hash: str

    # Debugging fields: Not part of compare or hash
    spdx_id: str = dataclasses.field(compare=False, hash=False, default="")
    url: str | None = dataclasses.field(compare=False, hash=False, default=None)
    text_sample: str = dataclasses.field(compare=False, hash=False, default="")
    debug_hint: str | None = dataclasses.field(
        compare=False, hash=False, default=None
    )

    def __gt__(self, other: "_ExtractedLicense") -> bool:
        return (
            f"{self.path}-{self.name}-{self.text_hash}"
            > f"{other.path}-{other.name}-{other.text_hash}"
        )

    @staticmethod
    def from_json_dict(input: dict[str, Any]) -> "_ExtractedLicense":
        # Example SPDX license dict:
        # {
        #     "name": "Bar Pkg",
        #     "licenseId": "LicenseRef-1234",
        #     "extractedText": "some text",
        #     "crossRefs": [
        #         {
        #             "url": "https://cs.opensource.google/fuchsia/fuchsia/+/main:bar/license"
        #         }
        #     ],
        #     "seeAlsos": [
        #         "https://cs.opensource.google/fuchsia/fuchsia/+/main:bar/license"
        #     ],
        #     "_hint": "some hint"
        # },

        name: str = input["name"].lower()

        # Remove version suffix which is common in rust crates in the legacy SPDX
        version_suffix_pattern = re.compile(r"-[\d.]+")
        version_suffix_match = version_suffix_pattern.search(name)
        if version_suffix_match:
            name = name[0 : version_suffix_match.start()]
        name = name.replace("-", "_")

        spdx_id: str = input["licenseId"][-8:]
        text: str = input["extractedText"]
        text = text.strip()
        text = text.replace(
            "\r", ""
        )  # Remove carriage returns. check-license has them, but python pipeline doesn't.

        sha = hashlib.sha256()
        sha.update(text.encode())
        text_hash = sha.hexdigest()[0:8]
        text_sample = text
        max_sample_size = 32
        if len(text_sample) > max_sample_size:
            text_sample = text_sample[0 : max_sample_size - 3] + "..."

        # Escape all kinds of newlines
        text_sample = text_sample.encode("unicode_escape").decode("utf-8")

        urls = []
        if "crossRefs" in input:
            for u in [d["url"] for d in input["crossRefs"]]:
                if u:
                    urls.append(u)
        elif "seeAlsos" in input:
            for u in input["seeAlsos"]:
                if u:
                    urls.append(u)
        path = None
        url = None
        for u in urls:
            url = u
            for prefix in [
                "https://cs.opensource.google/fuchsia/fuchsia/+/main:",
                "https://fuchsia.googlesource.com/fuchsia/+/${GIT_REVISION}/",
                "https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/",
            ]:
                if u.startswith(prefix):
                    path = u.replace(prefix, "")

        debug_hint = None
        if "_hint" in input:
            debug_hint = input["_hint"]

        assert path

        return _ExtractedLicense(
            path=path,
            name=name,
            spdx_id=spdx_id,
            text_hash=text_hash,
            text_sample=text_sample,
            url=url,
            debug_hint=debug_hint,
        )


# License path patterns that are expected to be missing
_expected_missing: list[re.Pattern[str]] = [
    re.compile(s)
    for s in [
        # COPYING and UNLICENSE files are not rust licenses
        "third_party/rust_crates/.*/COPYING",
        "third_party/rust_crates/.*/UNLICENSE",
        # Dart added by check-licenses, although not in build graph
        "third_party/dart-pkg/pub",
        # Redundant with third_party/kissfft/LICENSE:
        ".*/third_party/kissfft/COPYING",
        # Only libsparse actually used from android platform system
        "third_party/android/platform/system",
        # Python and Rust host tools
        "prebuilt/third_party/python3/linux-x64/.*",
        "prebuilt/third_party/rust/linux-x64/.*",
    ]
]


# TODO(132725): Remove once migration completes.
@dataclasses.dataclass(frozen=True)
class SpdxComparator:
    """Utility for comparing the licenses in 2 spdx files"""

    current_file: Path
    legacy_file: Path

    all: set[_ExtractedLicense] = dataclasses.field(default_factory=set)
    in_both: set[_ExtractedLicense] = dataclasses.field(default_factory=set)
    in_both_but_different: dict[
        _ExtractedLicense, _ExtractedLicense
    ] = dataclasses.field(default_factory=dict)
    added: set[_ExtractedLicense] = dataclasses.field(default_factory=set)
    missing: set[_ExtractedLicense] = dataclasses.field(default_factory=set)
    expected_missing: set[_ExtractedLicense] = dataclasses.field(
        default_factory=set
    )

    def compare(self) -> None:
        """Returns whether the files have the same licenses"""
        current_lics = self._read_spdx_licenses(self.current_file)
        legacy_lics = self._read_spdx_licenses(self.legacy_file)

        self.all.update(current_lics)
        self.all.update(legacy_lics)
        self.in_both.update(current_lics.intersection(legacy_lics))
        self.added.update(current_lics.difference(legacy_lics))
        self.missing.update(legacy_lics.difference(current_lics))

        for lic1 in list(self.missing):
            if not lic1.path:
                continue
            for lic2 in list(self.added):
                if not lic2.path:
                    continue
                if lic1.path == lic2.path and (
                    lic1.name != lic2.name or lic1.text_hash != lic2.text_hash
                ):
                    self.in_both_but_different[lic1] = lic2
                    self.in_both_but_different[lic2] = lic1
                    self.missing.remove(lic1)
                    self.added.remove(lic2)
                    break

        for lic in list(self.missing):
            for pattern in _expected_missing:
                if pattern.match(lic.path):
                    self.expected_missing.add(lic)
                    self.missing.remove(lic)

    def _read_spdx_licenses(self, path: Path | str) -> set[_ExtractedLicense]:
        with open(path, "r") as spdx_file:
            spdx_doc = json.load(spdx_file)
            output = set()
            if "hasExtractedLicensingInfos" in spdx_doc:
                for d in spdx_doc["hasExtractedLicensingInfos"]:
                    output.add(_ExtractedLicense.from_json_dict(d))
            return output

    def found_differences(self) -> bool:
        return len(self.added) != 0 or len(self.missing) != 0

    def log_differences(self, log_level: int, is_full_report: bool) -> None:
        message_lines = []

        def report(s: str) -> None:
            message_lines.append(s)

        def full_report(s: str) -> None:
            if is_full_report:
                report(s)

        if self.added or self.missing:
            report(
                f"{self.current_file} has DIFFERENT licenses then {self.legacy_file}:"
            )

        for el in sorted(list(self.all)):
            similar_to = None
            if el in self.in_both:
                prefix = " "
            elif el in self.in_both_but_different:
                prefix = "~"
                similar_to = self.in_both_but_different[el]
            elif el in self.added:
                prefix = "++"
            elif el in self.missing:
                prefix = "--"
            elif el in self.expected_missing:
                prefix = "#"
            else:
                assert False, "unreachable"

            msg = f"{prefix:5}{el.path} '{el.name}' {el.text_hash} spdx_id={el.spdx_id}"
            if similar_to:
                msg += f" similar_to_spdx_id={similar_to.spdx_id}"
            if el.debug_hint:
                msg += f" hint={el.debug_hint}"

            full_report(msg)

        full_report(
            f"""Key:
      Same: {len(self.in_both)}
  ~   Similar: {round(len(self.in_both_but_different) / 2)}
  #   Expected Missing: {len(self.expected_missing)}
  +   Added: {len(self.added)}
  -   Missing: {len(self.missing)}"""
        )

        for missing in sorted(list(self.missing)):
            report(
                f"Missing {missing.path} name={missing.name} hint={missing.debug_hint}"
            )

        logging.log(log_level, "\n".join(message_lines))

#!/usr/bin/env python3

# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
import re
import sys


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--project_name")
    parser.add_argument("--spdx_id")
    parser.add_argument("--license_id")
    parser.add_argument("--cut_after")
    parser.add_argument("--cut_entirely", action=argparse.BooleanOptionalAction)
    parser.add_argument("--exit_on_failure", default=False)
    args = parser.parse_args()

    with open(args.input, "r") as spdx:
        data = json.load(spdx)

    found_segment = False
    error_msg = "Did not find target license text in spdx file."
    if args.cut_entirely:
        license_refs = []
        # Removing a package from an SPDX file is complicated since the
        # package ID may be referenced in several different fields in the file.
        #
        # Instead, just delete the license text that applies to that package.
        # The ID of that field is defined in the "packages" list.
        # Each package is a dictionary, and the field labeled "licenseConcluded"
        # is a list of LicenseRef-* IDs.
        for p in data["packages"]:
            if "licenseConcluded" not in p:
                continue
            pop = False

            if args.project_name and p["name"] == args.project_name:
                pop = True
            if args.spdx_id and p["SPDXID"] == args.spdx_id:
                pop = True
            if args.license_id and p["licenseConcluded"] == args.license_id:
                pop = True

            if pop:
                license_refs.append(p.pop("licenseConcluded"))

        # Delete the license section from the SPDX file.
        i = 0
        while i < len(data["hasExtractedLicensingInfos"]):
            d = data["hasExtractedLicensingInfos"][i]
            if d["licenseId"] in license_refs:
                data["hasExtractedLicensingInfos"].pop(i)
                found_segment = True
                continue
            i += 1

    else:
        if not args.license_id:
            raise ValueError(
                "Parameter 'license_id' is required when 'cut_after' is set."
            )
        for d in data["hasExtractedLicensingInfos"]:
            if d["licenseId"] == args.license_id:
                error_msg = (
                    "Did not find string pattern {} in license text.".format(
                        args.cut_after
                    )
                )
                text = d["extractedText"]
                match = re.search(args.cut_after, text)
                if match:
                    d["extractedText"] = text[: match.end()]
                    found_segment = True
                break

    if not found_segment:
        if args.exit_on_failure:
            raise ValueError(error_msg)
        else:
            print(error_msg)

    with open(args.output, "w") as spdx:
        json.dump(data, spdx, ensure_ascii=False, indent="    ")


if __name__ == "__main__":
    sys.exit(main())

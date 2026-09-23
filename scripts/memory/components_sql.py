# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
import os
import os.path
import sqlite3
import sys
from typing import Any, Dict

# This scripts creates a SQLite database with the following schema (expressed here in DBML):
#
# // Kernel system-level memory statistics.
# Table kernel_stats {
#   name TEXT      // Name of the statistic
#   value INTEGER  // Value of the statistic
# }
#
# // Principals on the system.
# Table principals {
#     // ID of the principal.
#     id INTEGER [PRIMARY KEY]
#     // Human-readable name of the principal.
#     name TEXT
#     // Kind of the principal (e.g. Component or Part)
#     principal_kind TEXT
#     // Type of the principal (e.g. Runnable or Part)
#     principal_type TEXT
#     // Optional: parent (attributor) of the principal.
#     parent INTEGER [ref: > principals.id]
# }
#
# // Names of resources
# Table resource_names {
#     // Identifier of the resource name.
#     id INTEGER [PRIMARY KEY]
#     // Name of the resource
#     name TEXT
# }
#
# // Memory resources
# Table resources {
#     // Kernel object identifier of the resource
#     koid INTEGER [PRIMARY KEY]
#     // ID of the name of this resource
#     name_id INTEGER [ref: > resource_names.id]
#     // Type of this resource (e.g. job, process or vmo)
#     type TEXT
# }
#
# // Virtual Memory Object (a type of resource)
# Table vmos {
#     // Kernel object identifier of the VMO. Has a one-to-(zero or one) link with resources.koid.
#     koid INTEGER [PRIMARY KEY]
#     // Optional: parent of this VMO
#     parent INTEGER [ref: > vmos.koid]
#     // Memory usage of this VMO
#     private_committed_bytes INTEGER
#     private_populated_bytes INTEGER
#     scaled_committed_bytes INTEGER
#     scaled_populated_bytes INTEGER
#     total_committed_bytes INTEGER
#     total_populated_bytes INTEGER
#     // Flags (bitfield) of this VMO
#     flags INTEGER
# }
#
# // Blob annotations for VMOs
# Table blob_annotations {
#     koid INTEGER [ref: > vmos.koid]
#     manifest TEXT // Path to package's manifest
#     path TEXT     // Path of blob in package
# }
#
# // Associative table to join principals and resources.
# Table principals_resources {
#     principal_id INTEGER [primary key, ref: > principals.id]
#     resource_id INTEGER [ref: > resources.koid]
# }
#
# ref: vmos.koid - resources.koid [delete: cascade]


def main() -> int:
    parser = argparse.ArgumentParser(
        "components_sql",
        description="This script converts a memory snapshot generated using `ffx --machine json profile memory components --detailed` into a SQLite database",
    )
    parser.add_argument(
        "input",
        help="Input memory profile: detailed machine JSON output of ffx profile memory components. Use '-' for stdin",
    )
    parser.add_argument("output", help="Output file")

    args = parser.parse_args()
    if args.input == "-":
        json_input = json.load(sys.stdin)
    else:
        with open(args.input) as f:
            json_input = json.load(f)
    process_json_input(json_input, args.output)
    return 0


def process_json_input(json_input: Dict[str, Any], output_path: str) -> None:
    if "Detailed" not in json_input:
        print(
            "Error: The input JSON does not contain a 'Detailed' key. "
            "Please ensure the input is generated using "
            "`ffx --machine json profile memory components --detailed`.",
            file=sys.stderr,
        )
        sys.exit(1)
    else:
        data = json_input["Detailed"]

    if os.path.exists(output_path):
        os.remove(output_path)

    con = sqlite3.connect(output_path)

    con.execute(
        """CREATE TABLE kernel_stats (
        name TEXT PRIMARY KEY,
        value INTEGER
    )"""
    )

    con.executemany(
        """INSERT INTO kernel_stats VALUES (?, ?)""",
        data["kernel"]["memory_statistics"].items(),
    )
    con.executemany(
        """INSERT INTO kernel_stats VALUES (?, ?)""",
        [
            (k, v)
            for k, v in data["kernel"]["compression_statistics"].items()
            if isinstance(v, int)
        ],
    )
    con.executemany(
        """INSERT INTO kernel_stats VALUES (?, ?)""",
        data["performance"].items(),
    )

    con.execute(
        """CREATE TABLE principals (
        id INTEGER PRIMARY KEY,
        name TEXT,
        principal_kind TEXT,
        principal_type TEXT,
        parent INTEGER,
        FOREIGN KEY(parent) REFERENCES principals(id)
    )"""
    )

    con.execute(
        """CREATE TABLE resource_names (
        id INTEGER PRIMARY KEY,
        name TEXT
    )"""
    )

    con.executemany(
        "INSERT INTO resource_names VALUES (?, ?)",
        enumerate(data["resource_names"]),
    )

    con.execute(
        """CREATE TABLE resources (
        koid INTEGER PRIMARY KEY,
        name_id INTEGER,
        type TEXT,
        FOREIGN KEY(name_id) REFERENCES resource_names(id)
    )"""
    )

    con.execute(
        """CREATE TABLE vmos (
        koid INTEGER PRIMARY KEY,
        parent INTEGER,
        private_committed_bytes INTEGER,
        private_populated_bytes INTEGER,
        scaled_committed_bytes INTEGER,
        scaled_populated_bytes INTEGER,
        total_committed_bytes INTEGER,
        total_populated_bytes INTEGER,
        flags INTEGER,
        FOREIGN KEY(parent) REFERENCES resources(koid),
        FOREIGN KEY(koid) REFERENCES resources(koid) ON DELETE CASCADE
    )"""
    )

    con.execute(
        """CREATE TABLE blob_annotations (
        koid INTEGER,
        manifest TEXT,
        path TEXT,
        FOREIGN KEY(koid) REFERENCES vmos(koid) ON DELETE CASCADE
    )"""
    )

    data_resources = data["resources"]
    for data_resource in data_resources:
        res = data_resource["resource"]
        koid = res["koid"]
        name_id = res["name_index"]
        [(resource_type_name, resource_type_value)] = list(
            res["resource_type"].items()
        )

        con.execute(
            "INSERT INTO resources VALUES (?, ?, ?)",
            (koid, name_id, resource_type_name),
        )

        if resource_type_name == "Vmo":
            vmo = resource_type_value
            con.execute(
                "INSERT INTO vmos VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                (
                    koid,
                    vmo["parent"],
                    vmo["private_committed_bytes"],
                    vmo["private_populated_bytes"],
                    vmo["scaled_committed_bytes"],
                    vmo["scaled_populated_bytes"],
                    vmo["total_committed_bytes"],
                    vmo["total_populated_bytes"],
                    vmo["flags"],
                ),
            )

        for annotation in data_resource["annotations"]:
            if "Blob" in annotation:
                blob = annotation["Blob"]
                con.execute(
                    "INSERT INTO blob_annotations VALUES (?, ?, ?)",
                    (koid, blob["manifest"], blob["path"]),
                )

    con.execute(
        """CREATE TABLE principals_resources (
        principal_id INTEGER,
        resource_id INTEGER,
        FOREIGN KEY(principal_id) REFERENCES principals(id),
        FOREIGN KEY(resource_id) REFERENCES resources(koid)
    )"""
    )

    principals = data["principals"]
    for data_principal in principals:
        principal = data_principal["principal"]
        id = principal["identifier"]
        [(kind, name)] = list(principal["description"].items())

        principal_type = principal["principal_type"]

        con.execute(
            "INSERT INTO principals VALUES (?, ?, ?, ?, ?)",
            (
                id,
                name,
                kind,
                principal_type,
                principal["parent"],
            ),
        )

        for r in data_principal["resources"]:
            con.execute(
                "INSERT INTO principals_resources VALUES (?, ?)", (id, r)
            )

    con.commit()
    con.close()


if __name__ == "__main__":
    main()

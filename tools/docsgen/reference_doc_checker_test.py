#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
import argparse
import glob
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tarfile
import zipfile


def stage_ref_docs(ref_data, outpath):
    if os.path.exists(outpath) and os.path.isdir(outpath):
        shutil.rmtree(outpath)
    os.makedirs(outpath, exist_ok=True)

    tocfile = outpath.joinpath("_toc.yaml")

    with open(tocfile, "w") as toc:
        toc.write("toc:\n")
        for r in ref_data:
            dest = outpath.joinpath(r["dest_folder"])
            # patch up some path mismatches
            if r["name"] == "assemblydoc" and r["dest_folder"].startswith(
                "sdk/"
            ):
                dest = outpath.joinpath(os.path.basename(r["dest_folder"]))
            elif r["name"] == "cmldoc" and r["dest_folder"].startswith("sdk/"):
                dest = outpath.joinpath(os.path.basename(r["dest_folder"]))
            elif r["name"] == "clidoc" and r["dest_folder"].startswith("sdk/"):
                dest = outpath.joinpath(os.path.basename(r["dest_folder"]))
            elif r["name"] == "fidldoc" and r["dest_folder"].startswith("sdk/"):
                dest = outpath.joinpath(os.path.basename(r["dest_folder"]))
            elif r["name"] == "bazel_sdk" and r["dest_folder"].startswith(
                "sdk/"
            ):
                dest = outpath.joinpath(os.path.basename(r["dest_folder"]))
            elif r["name"] == "driversdoc" and r["dest_folder"].startswith(
                "sdk/"
            ):
                dest = outpath.joinpath(os.path.basename(r["dest_folder"]))
            os.makedirs(dest, exist_ok=True)

            folder = str(outpath)
            folder = str(dest)[len(folder) + 1 :]

            if r["name"] not in ["cmldoc", "driversdoc"]:
                toc.write(f"- title: \"{r['name']}\"\n")
                toc.write("  section:\n")
                toc.write(f"  - include: /reference/{folder}/_toc.yaml\n")

            if "archive" in r:
                archive_data = r["archive"]
                src_file = archive_data["origin_file"]
                base_folder = (
                    archive_data["base_folder"]
                    if "base_folder" in archive_data
                    else None
                )
                if src_file.endswith(".zip"):
                    with zipfile.ZipFile(src_file) as zf:
                        for m in zf.infolist():
                            if base_folder and m.filename.startswith(
                                base_folder
                            ):
                                dest_name = m.filename[len(base_folder) + 1 :]
                            else:
                                dest_name = m.filename
                            if dest_name:
                                if not m.is_dir():
                                    filename = os.path.join(dest, dest_name)
                                    os.makedirs(
                                        os.path.dirname(filename), exist_ok=True
                                    )
                                    with open(filename, "wb") as w:
                                        w.write(zf.read(m))
                elif src_file.endswith(".tar.gz"):
                    with tarfile.open(src_file, mode="r:*") as tf:
                        for m in tf.getmembers():
                            if base_folder and m.name.startswith(base_folder):
                                dest_name = m.name[len(base_folder) + 1 :]
                            elif base_folder and m.name.startswith(
                                f"./{base_folder}"
                            ):
                                dest_name = m.name[
                                    len(f"./{base_folder}") + 1 :
                                ]
                            else:
                                dest_name = m.name
                            if dest_name:
                                if not m.isdir():
                                    mbuf = tf.extractfile(m)
                                    filename = os.path.join(dest, dest_name)
                                    os.makedirs(
                                        os.path.dirname(filename), exist_ok=True
                                    )
                                    with open(filename, "wb") as w:
                                        w.write(mbuf.read())
                else:
                    raise ValueError(f"Unknown archive type: {src_file}")
            elif "origin_files" in r:
                for src_file in r["origin_files"]:
                    shutil.copy(src_file, dest)
            else:
                raise ValueError("no archive, and no origin_files: ", r)


def run_docchecker(doc_checker_bin, outpath, src_root):
    doc_checker_result = subprocess.run(
        [
            doc_checker_bin,
            "--reference-docs-root",
            outpath,
            "--allow-fuchsia-src-links",
            "--root",
            src_root,
        ]
    )

    if doc_checker_result.returncode:
        raise ValueError(
            f"doc-checker returned non-zero: {doc_checker_result.stderr}"
        )


def write_depfile(output_name, src_root, depfile):
    # get all markdown files
    mdfiles = glob.glob(f"{src_root}docs/**/*.md", recursive=True)
    print(f"{output_name}:", " ".join([str(f) for f in mdfiles]), file=depfile)
    # get all yaml files
    yamlfiles = glob.glob(f"{src_root}docs/**/*.yaml", recursive=True)
    print(
        f"{output_name}:", " ".join([str(f) for f in yamlfiles]), file=depfile
    )


def main() -> int:
    parser = argparse.ArgumentParser(
        description="""
        This script is used to stage reference docs and run doc-checker on them.
        """,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument(
        "--input",
        type=str,
        required=True,
        help="json file listing reference docs",
    )
    parser.add_argument(
        "--output",
        type=str,
        required=True,
        help="zipfile path for final output",
    )
    parser.add_argument(
        "--depfile",
        type=str,
        required=True,
        help="depfile path",
    )
    parser.add_argument(
        "--doc-checker",
        type=str,
        required=True,
        help="doc-checker binary path",
    )
    parser.add_argument(
        "--src-root",
        type=str,
        required=True,
        help="source root",
    )

    args = parser.parse_args()
    outpath = pathlib.Path(args.output).absolute()
    outpath = outpath.parent.joinpath(outpath.stem)

    with open(args.input) as f:
        data = json.load(f)

    stage_ref_docs(data, outpath)

    run_docchecker(args.doc_checker, outpath, args.src_root)

    with open(args.depfile, "w") as depfile:
        write_depfile(args.output, args.src_root, depfile)

    if not args.output.endswith(".zip"):
        raise ValueError(
            f"Invalid output file {args.output}. Filename must end in .zip"
        )
    shutil.make_archive(args.output[:-4], "zip", outpath)

    # clean up
    if os.path.exists(outpath) and os.path.isdir(outpath):
        shutil.rmtree(outpath)

    return 0


if __name__ == "__main__":
    sys.exit(main())

#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import json
import textwrap
from argparse import ArgumentParser, Namespace
from collections.abc import Collection, Iterable
from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from shutil import rmtree
from subprocess import run
from sys import argv
from tempfile import TemporaryDirectory

# better errors when running local tests
use_lxml = False
if use_lxml:
    from lxml import html


# This script generates //build/rust/tests/rustdoc tests and
# https://github.com/rust-lang/rust/tree/master/tests/rustdoc tests.

# This script is only used to create the directory tree in
# //build/rust/tests/rustdoc, and has no impact beyond that.


@dataclass(frozen=True)
class FileExists:
    path: Path
    fail: bool
    cci: bool
    """Whether this assertion depends on Cross Crate Information (information aggregated from
    multiple crates). Checking for the presence of a search index would be an example."""

    def run(self, root: Path) -> None:
        cont = root / self.path
        assert cont.is_file(), f"{cont} does not exist, or is not a file"

    def render_as_gn_test(self, count: int) -> str:
        if self.cci:
            # We should not render GN tests that check for the presence of
            # cross-crate information. The way that cross-crate information is
            # encoded before it's rendered is an implementation detail of
            # rustdoc. Instead, the //build/rust/tests/rusdoc-link tests should
            # assert for the presence and content of rendered cross-crate
            # files.
            return ""
        assertionKind = "assertFalse" if self.fail else "assertTrue"
        negation = "not " if self.fail else ""
        return (
            f"  def testFileExists{count}(self) -> None:\n"
            f"    self.{assertionKind}(\n"
            f'       (self._path / "{self.path}").is_file(),\n'
            f'       msg=f"expected `{self.path}` to {negation}be a file in {{repr(self._path)}}",\n'
            f"    )\n"
        )

    def render(self) -> str:
        bang = "!" if self.fail else ""
        return f"//@ {bang}has {self.path}"


@dataclass(frozen=True)
class HasRaw:
    file: Path
    content: str
    fail: bool
    cci: bool
    """Whether this assertion depends on Cross Crate Information (information aggregated from
    multiple crates). Checking for the presence of a search index would be an example."""

    def run(self, root: Path) -> None:
        cont = root / self.file
        assert cont.is_file(), f"{cont} does not exist"
        cont = cont.read_text()
        assert (
            self.content in cont
        ), f"found `{cont}`, to contain `{self.content}` in {root / self.file}"

    def render_as_gn_test(self, count: int) -> str:
        if self.cci:
            # We should not render GN tests that check for the presence of
            # cross-crate information. The way that cross-crate information is
            # encoded before it's rendered is an implementation detail of
            # rustdoc. Instead, the //build/rust/tests/rusdoc-link tests should
            # assert for the presence and content of rendered cross-crate
            # files.
            return ""
        assertionKind = "assertNotIn" if self.fail else "assertIn"
        return (
            f"  def testFileContainsRaw{count}(self) -> None:\n"
            f'    found = (self._path / "{self.file}").read_text()\n'
            f"    self.{assertionKind}(\n"
            f"       {repr(self.content)},\n"
            f"       found,\n"
            f"    )\n"
        )

    def render(self) -> str:
        bang = "!" if self.fail else ""
        return f"//@ {bang}hasraw {self.file} '{self.content}'"


@dataclass(frozen=True)
class Has:
    file: Path
    xpath: str
    content: str
    full: bool
    fail: bool
    cci: bool
    """Whether this assertion depends on Cross Crate Information (information aggregated from
    multiple crates). Checking for the presence of a search index would be an example."""

    def run(self, root: Path):
        cont = root / self.file
        assert cont.is_file(), f"{cont} does not exist"
        if use_lxml:
            cont = html.parse(cont)
            cont = cont.xpath(self.xpath)
            assert len(cont) == 1, (
                f"{self.xpath} in {root / self.file} should have selected a single"
                f" element, instead got {len(cont)}"
            )
            cont = cont[0].text_content()
        else:
            cont = cont.read_text()
        if self.full and use_lxml:
            assert (
                self.content == cont
            ), f"found `{cont}`, expecting `{self.content}` in {root / self.file}"
        else:
            assert (
                self.content in cont
            ), f"found `{cont}`, to contain `{self.content}` in {root / self.file}"

    def render_as_gn_test(self, count: int) -> str:
        if self.cci:
            # We should not render GN tests that check for the presence of
            # cross-crate information. The way that cross-crate information is
            # encoded before it's rendered is an implementation detail of
            # rustdoc. Instead, the //build/rust/tests/rusdoc-link tests should
            # assert for the presence and content of rendered cross-crate
            # files.
            return ""
        assertionKind = "assertNotIn" if self.fail else "assertIn"
        "not " if self.fail else ""
        # TODO: better xpath support
        return (
            f"  def testFileContains{count}(self) -> None:\n"
            f'    found = (self._path / "{self.file}").read_text()\n'
            f"    self.{assertionKind}(\n"
            f"       {repr(self.content)},\n"
            f"       found,\n"
            f"    )\n"
        )

    def render(self):
        bang = "!" if self.fail else ""
        command = "has" if self.full else "matches"
        return (
            f"//@ {bang}{command} {self.file} '{self.xpath}' '{self.content}'"
        )


class Merge(Enum):
    SHARED = "shared"
    NONE = "none"
    FINALIZE = "finalize"


@dataclass(frozen=True)
class CrateConfig:
    merge: Merge | None = None
    parts_out_dir: bool = False
    include_parts_dir: frozenset["Crate"] = frozenset()
    enable_index_page: bool = False
    separate_out_dir: bool = False
    extra_flags: tuple[str] = tuple()
    examples: tuple["Crate", str] | None = None  # crate, path


@dataclass(frozen=True)
class Crate:
    contents: str
    name: str
    deps: frozenset["Crate"]
    transdeps: frozenset["Crate"]
    crate_type: str
    ext: str

    def new(
        contents: str,
        name: str,
        deps: Collection["Crate"],
        crate_type: str = "lib",
        ext: str = "rmeta",
    ) -> "Crate":
        return Crate(
            contents=contents,
            name=name,
            deps=frozenset(deps),
            transdeps=frozenset(t for d in deps for t in d.deps | d.transdeps),
            crate_type=crate_type,
            ext=ext,
        )

    def makefile(
        self, crate_name: str, config: CrateConfig, rustdoc: Path, rustc: Path
    ) -> str:
        merge = "" if config.merge is None else f"--merge={config.merge.value}"
        include_parts_dir = " ".join(
            [
                f"--include-parts-dir=../info/{c.name}/doc.parts"
                for c in config.include_parts_dir
            ]
        )
        extra_flags = " ".join(config.extra_flags)
        if config.examples is not None:
            c, p = config.examples
            extra_flags += f" --scrape-examples-output-path"
            extra_flags += f" {p}"
            extra_flags += f" --scrape-examples-target-crate={c.name}"
        parts_out_dir = (
            ""
            if not config.parts_out_dir
            else f"--parts-out-dir=../info/{crate_name}/doc.parts"
        )
        separate_out_dir = (
            f"--out-dir=../docs/{crate_name}/doc"
            if config.separate_out_dir
            else "--out-dir=."
        )
        dep_meta = " ".join([f"meta/lib{d.name}.{d.ext}" for d in self.deps])
        dep_touch = " ".join([f"document-{d.name}" for d in self.deps])
        deps = " ".join(
            [
                f"--extern={d.name}=../meta/lib{d.name}.{d.ext}"
                for d in self.deps
            ]
        )
        enable_index_page = (
            "--enable-index-page" if config.enable_index_page else ""
        )
        flags = (
            f"-Zunstable-options --crate-type={self.crate_type}"
            f" --crate-name={self.name} --edition=2021 {deps} -L dependency=../meta"
        )
        extern_proc_macro = (
            "--extern=proc_macro" if self.crate_type == "proc-macro" else ""
        )
        emit_metadata = "--emit=metadata" if self.ext == "rmeta" else ""
        snippet = f"""
{self.name}/lib.rs:
\t@mkdir -p {self.name}
\t@printf {json.dumps(self.contents)} > $@

meta/lib{self.name}.{self.ext}: {self.name}/lib.rs {dep_meta}
\t+@cd doc && {rustc} ../{self.name}/lib.rs {flags} {extern_proc_macro} {emit_metadata} -o ../meta/lib{self.name}.{self.ext}

document-{self.name}: {self.name}/lib.rs {dep_meta} {dep_touch}
\t+@cd doc && {rustdoc} ../{self.name}/lib.rs {flags} {extern_proc_macro} {extra_flags} {include_parts_dir} {parts_out_dir} {separate_out_dir}  {merge} {enable_index_page}
"""
        doc_target = f"document-{self.name}"
        return snippet, doc_target

    def render_as_rustdoc_test(
        self,
        path: Path,
        aux: Path,
        configs: dict["Crate", "Config"],
        extra_header: str,
    ):
        header = ""
        for d in self.deps:
            aux.mkdir(exist_ok=True)
            d.render_as_rustdoc_test(aux, aux, configs, "")
            header += f"//@ aux-build:{d.name}.rs\n"
        if len(self.deps) > 0:
            header += "//@ build-aux-docs\n"
        if self.crate_type == "proc-macro":
            header += f"//@ force-host\n"
            header += f"//@ no-prefer-dynamic\n"
        separate_out_dir = (
            ""
            if not configs[self].separate_out_dir
            else "//@ unique-doc-out-dir\n"
        )
        flags = []
        flags.extend(configs[self].extra_flags)
        if configs[self].merge is not None:
            flags.append(f"--merge={configs[self].merge.value}")
        for c in configs[self].include_parts_dir:
            flags.append(f"--include-parts-dir=info/doc.parts/{c.name}")
        if configs[self].parts_out_dir:
            flags.append(f"--parts-out-dir=info/doc.parts/{self.name}")
        if configs[self].enable_index_page:
            flags.append("--enable-index-page")
        if self.crate_type != "lib":
            flags.append(f"--crate-type={self.crate_type}")
        if self.crate_type == "proc-macro":
            flags.append(f"--extern=proc_macro")
        if configs[self].examples is not None:
            c, p = configs[self].examples
            flags += [f"--scrape-examples-output-path={p}"]
            flags += [f"--scrape-examples-target-crate={c.name}"]
        if len(flags) > 0:
            flags.append("-Zunstable-options")
        flags = "".join(f"//@ doc-flags:{f}\n" for f in flags)
        if len(flags) > 0:
            flags += "\n"
        contents = self.contents + "\n" if len(self.contents) > 0 else ""
        Path(path, self.name).with_suffix(".rs").write_text(
            f"{header}{separate_out_dir}{flags}" f"{extra_header}{contents}"
        )

    def render_as_gn_target(
        self,
        config_root: Path,
        configs: dict["Crate", CrateConfig],
        targets: dict[str, str],
        is_index: bool,
    ):
        if self.name in targets:
            return self.name

        source = Path("..", "src", self.name).with_suffix(".rs")
        Path(config_root, source).write_text(
            f"// Copyright 2024 The Fuchsia Authors. All rights reserved.\n"
            f"// Use of this source code is governed by a BSD-style license that can be\n"
            f"// found in the LICENSE file.\n"
            f"#![allow(unused_crate_dependencies, unused_extern_crates)]\n"
            f"{self.contents}\n",
        )

        for d in self.deps:
            d.render_as_gn_target(config_root, configs, targets, False)
        deps = [f'":{d.name}", ' for d in self.deps]
        public_deps = [f'"{target_name(d)}", ' for d in self.deps]
        merge = (
            f""
            if configs[self].merge is None
            else f'  rustdoc_merge = "{configs[self].merge.value}"\n'
        )
        extra_flags = list(f'"{f}", ' for f in configs[self].extra_flags)
        extra_flags += [
            f
            for c in configs[self].include_parts_dir
            for f in (
                f'"--include-parts-dir", ',
                f'rebase_path("$target_gen_dir/doc.parts/{c.name}", root_build_dir), ',
            )
        ]
        if configs[self].enable_index_page:
            extra_flags += ['"--enable-index-page", ']
        rustdoc_out_dir = (
            f"$target_gen_dir/{self.name}.doc"
            if configs[self].separate_out_dir
            else "$target_gen_dir/doc"
        )
        crate_type_target = {
            "lib": "rustc_library",
            "proc-macro": "rustc_macro",
            "bin": "rustc_binary",
        }[self.crate_type]
        if configs[self].examples is not None:
            c, p = configs[self].examples
            extra_flags += f'"--scrape-examples-output-path", '
            extra_flags += (
                f'rebase_path("{rustdoc_out_dir}/{p}", root_build_dir), '
            )
            extra_flags += f'"--scrape-examples-target-crate={c.name}", '
        deps = "".join(deps)
        public_deps = "".join(public_deps)
        extra_flags = "".join(extra_flags)
        rustdoc_parts_dir = (
            f'  rustdoc_parts_dir = "$target_gen_dir/doc.parts/{self.name}"\n'
            if configs[self].parts_out_dir
            else ""
        )
        target = (
            f'{crate_type_target}("{self.name}") {{\n'
            f'  edition = "2024"\n'
            f"  define_rustdoc_test_override = true\n"
            f'  name = "{self.name}"\n'
            f"  deps = [{deps}]\n"
            f"  public_deps = [{public_deps}]\n"
            f"  testonly = true\n"
            f'  source_root = "{source}"\n'
            f'  sources = [ "{source}" ]\n'
            f"  quiet_clippy = true\n"
            f'  rustdoc_out_dir = "{rustdoc_out_dir}"\n'
            f"  rustdoc_args = [{extra_flags}]\n"
            f'  zip_rustdoc_to = "$target_gen_dir/{self.name}.doc.zip"\n'
            f"{merge}"
            f"{rustdoc_parts_dir}"
            f"}}\n"
        )
        targets[self.name] = (
            target,
            f'":{self.name}", ',
        )
        return self.name


def target_name(c: "Crate") -> str:
    if c.crate_type == "proc-macro":
        return f":{c.name}_proc_macro.rustdoc"
    return f":{c.name}.rustdoc"


@dataclass
class Config:
    name: str
    desc: str
    index: Crate
    configs: dict[Crate, CrateConfig]
    assertions: list[Has | HasRaw | FileExists]
    no_mergeable_rustdoc: bool

    def render_as_gn_test(self, path: Path):
        # tests share source files
        src = path / "src"
        src.mkdir(parents=True, exist_ok=True)

        config_root = path / self.name
        config_root.mkdir(parents=True, exist_ok=True)
        targets = dict()
        self.index.render_as_gn_target(config_root, self.configs, targets, True)
        targets, build_name = zip(*targets.values())
        targets = "".join(targets)
        public_deps = "".join(f'"{target_name(c)}", ' for c in self.configs)
        build = (
            f"# Copyright 2024 The Fuchsia Authors. All rights reserved.\n"
            f"# Use of this source code is governed by a BSD-style license that can be\n"
            f"# found in the LICENSE file.\n"
            f'import("//build/python/python_host_test.gni")\n'
            f'import("//build/rust/rustc_binary.gni")\n'
            f'import("//build/rust/rustc_library.gni")\n'
            f'import("//build/rust/rustc_macro.gni")\n'
            f'import("//build/testing/host_test_data.gni")\n'
            f"\n"
            f"# Generated by //build/rust/tests/create_rustdoc_tests.py\n"
            f"# Test explanation: {self.desc}\n"
            f"\n"
            f'group("{self.name}") {{\n'
            f"  testonly = true\n"
            f'  deps = [ ":host-test($host_toolchain)" ]\n'
            f"}}\n"
            f"\n"
            f"if (is_host) {{\n"
            f'  python_host_test("host-test") {{\n'
            f'    main_source = "test.py"\n'
            f'    deps = [":host-test-data"]\n'
            f'    extra_args = [rebase_path("$target_gen_dir/{self.index.name}.doc.zip.copy", root_build_dir)]\n'
            f"  }}\n"
            f'  host_test_data("host-test-data") {{\n'
            f'    sources = ["$target_gen_dir/{self.index.name}.doc.zip"] \n'
            f"    public_deps = [{public_deps}]\n"
            f'    outputs = [ "$target_gen_dir/{self.index.name}.doc.zip.copy" ]\n'
            f"  }}\n"
            f"}}\n"
            f"\n"
            f"{targets}"
        )
        Path(config_root, "BUILD.gn").write_text(build)
        assertions = "".join(
            a.render_as_gn_test(i) for i, a in enumerate(dedup(self.assertions))
        )
        run_test = (
            f"# Copyright 2024 The Fuchsia Authors. All rights reserved.\n"
            f"# Use of this source code is governed by a BSD-style license that can be\n"
            f"# found in the LICENSE file.\n"
            f"\n"
            f"# Generated by //build/rust/tests/create_rustdoc_tests.py\n"
            f"# Test explanation: {self.desc}\n"
            f"\n"
            f"from unittest import TestCase\n"
            f"from zipfile import Path\n"
            f"from sys import argv\n"
            f"import unittest #noqa\n"
            f"_doc_zip = argv.pop()\n"
            f"class Test(TestCase):\n"
            f"  _path: Path\n"
            f"  @classmethod\n"
            f"  def setUpClass(cls) -> None:\n"
            f"    cls._path = Path(_doc_zip)\n"
            f"{assertions}"
        )
        Path(config_root, "test.py").write_text(run_test)

    def render_as_rustdoc_test(self, path: Path):
        path = Path(path, self.name)
        aux = path / "auxiliary"
        aux.mkdir(exist_ok=True, parents=True)
        extra_header = "".join(
            dedup(f"{a.render()}\n" for a in self.assertions)
        )
        if len(extra_header) > 0:
            extra_header += "\n"
        extra_header += "".join(f"// {w}\n" for w in textwrap.wrap(self.desc))
        self.index.render_as_rustdoc_test(path, aux, self.configs, extra_header)

    def run(self, rustc: Path, rustdoc: Path):
        root = "root"
        with TemporaryDirectory() as root:
            root = Path(root)
            root.mkdir(exist_ok=True)
            root = Path(root)
            makes, targets = zip(
                *(
                    c.makefile(
                        c.name, self.configs[c], rustc=rustc, rustdoc=rustdoc
                    )
                    for c in self.configs
                )
            )
            Path(root, "meta").mkdir(exist_ok=True)
            Path(root, "doc").mkdir(exist_ok=True)
            Path(root, "Makefile").write_text("".join(makes))
            print("".join(makes))

            run(["make", "-j100", *targets], cwd=root)

            for t in self.assertions:
                try:
                    t.run(root / "doc")
                except AssertionError as e:
                    if not t.fail:
                        print(f"config {self.name} failed")
                        raise e
                    return
                assert (
                    not t.fail
                ), f"assertion in {self.name} should fail {repr(t)}"


def dedup(items: Iterable[any]) -> list:
    ret = []
    seen = set()
    for item in items:
        if item not in seen:
            ret.append(item)
        seen.add(item)
    return ret


charlie = Crate.new(
    contents=('pub const CHARLIE: &\'static str = "const";'),
    name="charlie",
    deps=set(),
    ext="rlib",
)

bravo = Crate.new(
    contents=(
        'extern crate charlie; fn main() { println!("hello {}", charlie::CHARLIE); '
    ),
    name="bravo",
    deps={charlie},
    crate_type="bin",
)

march = Crate.new(
    contents=(
        f'#![crate_type = "proc-macro"]\n'
        f"extern crate charlie;\n"
        f"extern crate proc_macro;\n"
        f"use proc_macro::TokenStream;\n"
        f"#[proc_macro]\n"
        f"pub fn make_constant(_item: TokenStream) -> TokenStream {{\n"
        f'    format!("pub {{}} NOVEMBER: () = ();", charlie::CHARLIE).parse().unwrap()\n'
        f"}}"
    ),
    name="march",
    crate_type="proc-macro",
    deps={charlie},
    ext="so",
)

november = Crate.new(
    contents=(f"extern crate {march.name};"),
    name="november",
    deps={march},
)

# want to be sure that when we find an item in the search index that it actually is the one we mean
item_E = f"Echo"
item_F = f"Foxtrot"

foxtrot = Crate.new(
    contents=f"pub trait {item_F} {{}}",
    name="foxtrot",
    deps=set(),
)

echo = Crate.new(
    contents=(
        f"extern crate foxtrot;\npub enum {item_E} {{}}\nimpl foxtrot::{item_F} for"
        f" {item_E} {{}}"
    ),
    name="echo",
    deps={foxtrot},
)

item_Q = f"Quebec"
item_T = f"Tango"
item_S = f"Sierra"
item_R = f"Romeo"

quebec = Crate.new(
    contents=f"pub struct {item_Q};",
    name="quebec",
    deps=set(),
)

tango = Crate.new(
    contents=f"extern crate quebec;\npub trait {item_T} {{}}",
    name="tango",
    deps={quebec},
)

sierra = Crate.new(
    contents=(
        f"extern crate tango;\npub struct {item_S};\nimpl tango::{item_T} for"
        f" {item_S} {{}}"
    ),
    name="sierra",
    deps={tango},
)

romeo = Crate.new(
    contents=f"extern crate sierra;\npub type {item_R} = sierra::{item_S};",
    name="romeo",
    deps={sierra},
)

indigo = Crate.new(
    contents=f"",
    name="indigo",
    deps={quebec, tango, sierra, romeo},
)

t_i_header = Has(
    file="index.html",
    xpath="//h1",
    content="List of all crates",
    full=True,
    fail=False,
    cci=True,
)


def index_but_crate_absent(c, i) -> list:
    return [
        FileExists(
            path="index.html",
            fail=False,
            cci=True,
        ),
        Has(
            file="index.html",
            xpath=f"//h1",
            content="List of all crates",
            full=True,
            fail=False,
            cci=True,
        ),
        Has(
            file="index.html",
            xpath=f'//ul[@class="all-items"]//a[@href="{c.name}/index.html"]',
            content=c.name,
            full=True,
            fail=True,
            cci=True,
        ),
    ]


def test_index_has_crate(c, i, fail: bool) -> list:
    """index.html is cross-crate information, and a crate should only appear here if and only if it was written to the shared output directory,

    overwritten with --merge=finalize, or included with --include-parts-dir from
    an external crate-index.html
    and subsequently linked.
    """
    if fail:
        return [
            FileExists(
                path="index.html",
                fail=True,
                cci=True,
            )
        ]
    return [
        FileExists(
            path="index.html",
            fail=False,
            cci=True,
        ),
        Has(
            file="index.html",
            xpath=f"//h1",
            content="List of all crates",
            full=True,
            fail=False,
            cci=True,
        ),
        Has(
            file="index.html",
            xpath=f'//ul[@class="all-items"]//a[@href="{c.name}/index.html"]',
            content=c.name,
            full=True,
            fail=False,
            cci=True,
        ),
    ]


def test_root_has_item(c, kind, name, fail: bool):
    """{crate name}/index.html is specific to each crate, and does not serve as a cci part.

    should appear if rendered to a fixed out-dir or copied with
    --include-rendered-docs
    """
    return FileExists(
        path=f"{c.name}/{kind}.{name}.html",
        fail=fail,
        cci=False,
    )


def test_fixed_crate_impl(c, kind, trait, implr, implr_alias):
    """regular trait implementation, does not rely on cross-crate trait linking"""
    return [
        FileExists(
            path=f"{c.name}/{kind}.{implr_alias}.html",
            fail=False,
            cci=False,
        ),
        HasRaw(
            file=f"{c.name}/{kind}.{implr_alias}.html",
            content=f"{trait}",
            fail=False,
            cci=False,
        ),
    ]


def test_cross_crate_impl_not_exist(c, name):
    """regular trait implementation, not cross-crate trait implementation"""
    return FileExists(
        path=f"trait.impl/{c.name}/trait.{name}.js",
        fail=True,
        cci=True,
    )


def test_cross_crate_impl(c, name, implr, kind: str):
    """regular trait implementation, not cross-crate trait implementation"""
    return HasRaw(
        file=f"trait.impl/{c.name}/trait.{name}.js",
        content=f"{kind}.{implr}.html",
        fail=False,
        cci=True,
    )


def test_search_index(name: str, fail: bool):
    """check for the presence of the item in the search index"""
    return HasRaw(
        file=f"search-index.js",
        content=f"{name}",
        fail=fail,
        cci=True,
    )


def test_type_impl_not_exists(
    kind: str, original_crate: Crate, original: str
) -> FileExists:
    return FileExists(
        path=f"type.impl/{original_crate.name}/{kind}.{original}.js",
        fail=True,
        cci=True,
    )


def test_type_impl(
    kind: str,
    original_crate: Crate,
    original: str,
    trait_name: str,
    alias_name: str,
) -> list:
    return [
        FileExists(
            path=f"type.impl/{original_crate.name}/{kind}.{original}.js",
            fail=False,
            cci=True,
        ),
        HasRaw(
            file=f"type.impl/{original_crate.name}/{kind}.{original}.js",
            content=f"{trait_name}",
            fail=False,
            cci=True,
        ),
        HasRaw(
            file=f"type.impl/{original_crate.name}/{kind}.{original}.js",
            content=f"{alias_name}",
            fail=False,
            cci=True,
        ),
    ]


def assert_no_duplicate_config_names(configs: Iterable[Config]):
    seen = set()
    for c in configs:
        assert c.name not in seen, f"found duplicate name {c.name}"
        seen.add(c.name)


def main(args: Namespace):
    configs = []

    configs.append(
        Config(
            name="kitchen_sink_separate_dirs",
            desc=(
                "document everything in the default mode, there are separate out"
                " directories that are linked together"
            ),
            index=indigo,
            configs={
                quebec: CrateConfig(
                    merge=Merge.NONE,
                    separate_out_dir=True,
                    parts_out_dir=True,
                    enable_index_page=True,
                ),
                tango: CrateConfig(
                    merge=Merge.NONE,
                    separate_out_dir=True,
                    parts_out_dir=True,
                    enable_index_page=True,
                ),
                sierra: CrateConfig(
                    merge=Merge.NONE,
                    separate_out_dir=True,
                    parts_out_dir=True,
                    enable_index_page=True,
                ),
                romeo: CrateConfig(
                    merge=Merge.NONE,
                    separate_out_dir=True,
                    parts_out_dir=True,
                    enable_index_page=True,
                ),
                indigo: CrateConfig(
                    merge=Merge.FINALIZE,
                    include_parts_dir=frozenset([quebec, tango, sierra, romeo]),
                    enable_index_page=True,
                ),
            },
            assertions=[
                t_i_header,
                *(
                    assertion
                    for j, c in enumerate(
                        [indigo, quebec, romeo, sierra, tango]
                    )
                    for assertion in test_index_has_crate(c, j, False)
                ),
                test_root_has_item(quebec, "struct", item_Q, True),
                test_root_has_item(romeo, "type", item_R, True),
                test_root_has_item(sierra, "struct", item_S, True),
                test_root_has_item(tango, "trait", item_T, True),
                test_cross_crate_impl(tango, item_T, item_S, "struct"),
                test_search_index(item_Q, False),
                test_search_index(item_R, False),
                test_search_index(item_S, False),
                test_search_index(item_T, False),
                *test_type_impl(
                    "struct",
                    sierra,
                    item_S,
                    trait_name=item_T,
                    alias_name=item_R,
                ),
            ],
            no_mergeable_rustdoc=False,
        )
    )

    configs.append(
        Config(
            name="working_dir_examples",
            desc="checks to make sure that --scrape-examples-output-path resolves to the correct directory",
            index=quebec,
            configs={
                quebec: CrateConfig(merge=None, examples=(quebec, "examples")),
            },
            assertions=[FileExists(path="examples", fail=False, cci=False)],
            no_mergeable_rustdoc=True,
        )
    )

    configs.append(
        Config(
            name="overwrite",
            desc=(
                "since tango is documented with --merge=finalize, we overwrite q's"
                " cross-crate information"
            ),
            index=sierra,
            configs={
                quebec: CrateConfig(
                    merge=Merge.NONE,
                    enable_index_page=True,
                ),
                tango: CrateConfig(
                    merge=Merge.FINALIZE,
                    enable_index_page=True,
                ),
                sierra: CrateConfig(
                    merge=Merge.SHARED,
                    enable_index_page=True,
                ),
            },
            assertions=[
                test_root_has_item(quebec, "struct", item_Q, False),
                test_root_has_item(sierra, "struct", item_S, False),
                test_root_has_item(tango, "trait", item_T, False),
                *test_fixed_crate_impl(
                    sierra, "struct", item_T, item_S, item_S
                ),
                test_cross_crate_impl(tango, item_T, item_S, "struct"),
                test_search_index(item_T, False),
                test_search_index(item_S, False),
                test_search_index(item_Q, True),  # XXX
            ],
            no_mergeable_rustdoc=False,
        )
    )

    configs.append(
        Config(
            name="overwrite_but_include",
            desc=(
                "we overwrite quebec and tango's cross-crate information, but we include"
                " the info from tango meaning that it should appear in the out dir"
            ),
            index=sierra,
            configs={
                quebec: CrateConfig(
                    merge=Merge.NONE,
                    parts_out_dir=True,
                    enable_index_page=True,
                ),
                tango: CrateConfig(
                    merge=Merge.NONE,
                    parts_out_dir=True,
                    enable_index_page=True,
                ),
                sierra: CrateConfig(
                    merge=Merge.FINALIZE,
                    include_parts_dir=frozenset([tango]),
                    enable_index_page=True,
                ),
            },
            assertions=[
                test_root_has_item(quebec, "struct", item_Q, False),
                test_root_has_item(sierra, "struct", item_S, False),
                test_root_has_item(tango, "trait", item_T, False),
                *test_fixed_crate_impl(
                    sierra, "struct", item_T, item_S, item_S
                ),
                test_cross_crate_impl(tango, item_T, item_S, "struct"),
                test_search_index(item_T, False),
                test_search_index(item_S, False),
                test_search_index(item_Q, True),  # XXX
            ],
            no_mergeable_rustdoc=False,
        )
    )

    configs.append(
        Config(
            name="transitive_finalize",
            desc="write only overwrites stuff in the output directory",
            index=sierra,
            configs={
                quebec: CrateConfig(
                    merge=Merge.FINALIZE,
                    enable_index_page=True,
                ),
                tango: CrateConfig(
                    merge=Merge.FINALIZE,
                    enable_index_page=True,
                ),
                sierra: CrateConfig(
                    merge=Merge.FINALIZE,
                    enable_index_page=True,
                ),
            },
            assertions=[
                *(
                    assertion
                    for i, c in enumerate([sierra, tango])
                    for assertion in test_index_has_crate(c, i, False)
                ),
                test_root_has_item(sierra, "struct", item_S, False),
                test_root_has_item(tango, "trait", item_T, False),
                *test_fixed_crate_impl(
                    sierra, "struct", item_T, item_S, item_S
                ),
                test_cross_crate_impl(tango, item_T, item_S, "struct"),
                test_search_index(item_S, False),
            ],
            no_mergeable_rustdoc=False,
        )
    )

    assertions = [
        *(
            assertion
            for i, c in enumerate([quebec, sierra, tango])
            for assertion in test_index_has_crate(c, i, False)
        ),
        test_root_has_item(quebec, "struct", item_Q, False),
        test_root_has_item(sierra, "struct", item_S, False),
        test_root_has_item(tango, "trait", item_T, False),
        *test_fixed_crate_impl(sierra, "struct", item_T, item_S, item_S),
        test_cross_crate_impl(tango, item_T, item_S, "struct"),
        test_search_index(item_T, False),
        test_search_index(item_S, False),
        test_search_index(item_Q, False),
    ]

    configs.append(
        Config(
            name="overwrite_but_separate",
            desc=(
                "If these were documeted into the same directory, the info would"
                " be overwritten. However, since they are merged, we can still"
                " recover all of the cross-crate information"
            ),
            index=sierra,
            configs={
                quebec: CrateConfig(
                    merge=Merge.NONE,
                    separate_out_dir=True,
                    parts_out_dir=True,
                    enable_index_page=True,
                ),
                tango: CrateConfig(
                    merge=Merge.NONE,
                    separate_out_dir=True,
                    parts_out_dir=True,
                    enable_index_page=True,
                ),
                sierra: CrateConfig(
                    merge=Merge.FINALIZE,
                    include_parts_dir=frozenset([tango, quebec]),
                    enable_index_page=True,
                ),
            },
            assertions=[
                *(
                    assertion
                    for i, c in enumerate([quebec, sierra, tango])
                    for assertion in test_index_has_crate(c, i, False)
                ),
                test_root_has_item(sierra, "struct", item_S, False),
                test_cross_crate_impl(tango, item_T, item_S, "struct"),
                test_search_index(item_T, False),
                test_search_index(item_S, False),
                test_search_index(item_Q, False),
            ],
            no_mergeable_rustdoc=False,
        )
    )

    configs.append(
        Config(
            name="two",
            desc="simple test to assert that we can do a two-level aux-build",
            index=echo,
            configs={
                foxtrot: CrateConfig(
                    merge=None,
                ),
                echo: CrateConfig(
                    merge=None,
                ),
            },
            assertions=[],
            no_mergeable_rustdoc=True,
        )
    )

    configs.append(
        Config(
            name="cargo_two",
            desc=(
                "document two crates in the same way that cargo does, writing"
                " them both into the same output directory"
            ),
            index=echo,
            configs={
                foxtrot: CrateConfig(
                    merge=None,
                    enable_index_page=True,
                ),
                echo: CrateConfig(
                    merge=None,
                    enable_index_page=True,
                ),
            },
            assertions=[
                *(
                    assertion
                    for i, c in enumerate([foxtrot, echo])
                    for assertion in test_index_has_crate(c, i, False)
                ),
                test_root_has_item(echo, "enum", item_E, False),
                test_root_has_item(foxtrot, "trait", item_F, False),
                *test_fixed_crate_impl(echo, "enum", item_F, item_E, item_E),
                test_cross_crate_impl(foxtrot, item_F, item_E, "enum"),
                test_search_index(item_F, False),
                test_search_index(item_E, False),
            ],
            no_mergeable_rustdoc=True,
        )
    )

    configs.append(
        Config(
            name="index_on_last",
            desc=(
                "only declare --enable-index-page to the last rustdoc invocation"
            ),
            index=echo,
            configs={
                foxtrot: CrateConfig(
                    merge=None,
                ),
                echo: CrateConfig(
                    merge=None,
                    enable_index_page=True,
                ),
            },
            assertions=[
                *(
                    assertion
                    for i, c in enumerate([foxtrot, echo])
                    for assertion in test_index_has_crate(c, i, False)
                ),
                test_root_has_item(echo, "enum", item_E, False),
                test_root_has_item(foxtrot, "trait", item_F, False),
                *test_fixed_crate_impl(echo, "enum", item_F, item_E, item_E),
                test_cross_crate_impl(foxtrot, item_F, item_E, "enum"),
                test_search_index(item_F, False),
                test_search_index(item_E, False),
            ],
            no_mergeable_rustdoc=True,
        )
    )

    configs.append(
        Config(
            name="cargo_two_no_index",
            desc=(
                "document two crates in the same way that cargo does. do not"
                " provide --enable-index-page"
            ),
            index=echo,
            configs={
                foxtrot: CrateConfig(
                    merge=None,
                ),
                echo: CrateConfig(
                    merge=None,
                ),
            },
            assertions=[
                test_root_has_item(echo, "enum", item_E, False),
                test_root_has_item(foxtrot, "trait", item_F, False),
                *test_fixed_crate_impl(echo, "enum", item_F, item_E, item_E),
                test_cross_crate_impl(foxtrot, item_F, item_E, "enum"),
                test_search_index(item_F, False),
                test_search_index(item_E, False),
            ],
            no_mergeable_rustdoc=True,
        )
    )

    configs.append(
        Config(
            name="write_docs_somewhere_else",
            desc=(
                "test the fact that our test runner will document this crate"
                " somewhere else"
            ),
            index=echo,
            configs={
                foxtrot: CrateConfig(
                    separate_out_dir=True,
                    merge=None,
                ),
                echo: CrateConfig(
                    merge=None,
                ),
            },
            assertions=[
                test_root_has_item(echo, "enum", item_E, False),
                test_root_has_item(foxtrot, "trait", item_F, True),
                *test_fixed_crate_impl(echo, "enum", item_F, item_E, item_E),
                test_cross_crate_impl(foxtrot, item_F, item_E, "enum"),
                test_search_index(item_F, True),
                test_search_index(item_E, False),
            ],
            no_mergeable_rustdoc=True,
        )
    )

    configs.append(
        Config(
            name="two_separate_out_dir",
            desc=(
                "document two crates in different places, and merge their docs"
                " after they are generated"
            ),
            index=echo,
            configs={
                foxtrot: CrateConfig(
                    separate_out_dir=True,
                    parts_out_dir=True,
                    merge=None,
                ),
                echo: CrateConfig(
                    include_parts_dir=frozenset([foxtrot]),
                    merge=None,
                ),
            },
            assertions=[
                test_root_has_item(echo, "enum", item_E, False),
                *test_fixed_crate_impl(echo, "enum", item_F, item_E, item_E),
                test_cross_crate_impl(foxtrot, item_F, item_E, "enum"),
                test_search_index(item_F, False),
                test_search_index(item_E, False),
            ],
            no_mergeable_rustdoc=False,
        )
    )

    configs.append(
        Config(
            name="cargo_transitive",
            desc=(
                "We use the regular and transitive dependencies."
                " Both should appear in the item docs for the final crate."
            ),
            index=sierra,
            configs={
                quebec: CrateConfig(
                    merge=None,
                    separate_out_dir=True,
                    enable_index_page=True,
                ),
                tango: CrateConfig(
                    merge=None,
                    separate_out_dir=True,
                    enable_index_page=True,
                ),
                sierra: CrateConfig(
                    merge=None,
                    enable_index_page=True,
                ),
            },
            assertions=[
                *index_but_crate_absent(quebec, indigo),
                *index_but_crate_absent(tango, indigo),
                *test_index_has_crate(sierra, indigo, False),
                test_root_has_item(quebec, "struct", item_Q, True),
                test_root_has_item(tango, "trait", item_T, True),
                test_root_has_item(sierra, "struct", item_S, False),
                *test_fixed_crate_impl(
                    sierra, "struct", item_T, item_S, item_S
                ),
                test_cross_crate_impl(tango, item_T, item_S, "struct"),
                test_search_index(item_T, True),
                test_search_index(item_Q, True),
                test_search_index(item_S, False),
            ],
            no_mergeable_rustdoc=True,
        )
    )

    configs.append(
        Config(
            name="cargo_transitive_no_index",
            desc=(
                "We use the regular and transitive dependencies."
                "Both should appear in the item docs for the final crate."
                " The index page is not generated, however."
            ),
            index=sierra,
            configs={
                quebec: CrateConfig(merge=None, separate_out_dir=True),
                tango: CrateConfig(merge=None, separate_out_dir=True),
                sierra: CrateConfig(merge=None, separate_out_dir=False),
            },
            assertions=[
                *test_index_has_crate(quebec, indigo, True),
                *test_index_has_crate(tango, indigo, True),
                *test_index_has_crate(sierra, indigo, True),
                test_root_has_item(quebec, "struct", item_Q, True),
                test_root_has_item(tango, "trait", item_T, True),
                test_root_has_item(sierra, "struct", item_S, False),
                *test_fixed_crate_impl(
                    sierra, "struct", item_T, item_S, item_S
                ),
                test_cross_crate_impl(tango, item_T, item_S, "struct"),
                test_search_index(item_T, True),
                test_search_index(item_Q, True),
                test_search_index(item_S, False),
            ],
            no_mergeable_rustdoc=True,
        )
    )

    configs.append(
        Config(
            name="cargo_transitive_read_write",
            desc=(
                "similar to cargo_workflow_transitive, but we use"
                " --merge=read-write, which is the default."
            ),
            index=sierra,
            configs={
                quebec: CrateConfig(
                    merge=Merge.SHARED,
                    enable_index_page=True,
                ),
                tango: CrateConfig(
                    merge=Merge.SHARED,
                    enable_index_page=True,
                ),
                sierra: CrateConfig(
                    merge=Merge.SHARED,
                    enable_index_page=True,
                ),
            },
            assertions=assertions,
            no_mergeable_rustdoc=False,
        )
    )

    configs.append(
        Config(
            name="transitive_merge_none",
            desc=(
                "We avoid writing any cross-crate information, preferring to"
                " include it with --include-parts-dir."
            ),
            index=sierra,
            configs={
                quebec: CrateConfig(
                    merge=Merge.NONE,
                    parts_out_dir=True,
                    enable_index_page=True,
                ),
                tango: CrateConfig(
                    merge=Merge.NONE,
                    parts_out_dir=True,
                    enable_index_page=True,
                ),
                sierra: CrateConfig(
                    merge=Merge.FINALIZE,
                    include_parts_dir=frozenset([quebec, tango]),
                    enable_index_page=True,
                ),
            },
            assertions=assertions,
            no_mergeable_rustdoc=False,
        )
    )

    configs.append(
        Config(
            name="transitive_merge_read_write",
            desc=(
                "We can use read-write to emulate the default behavior of"
                " rustdoc, when --merge is left out."
            ),
            index=sierra,
            configs={
                quebec: CrateConfig(
                    merge=Merge.FINALIZE,
                    enable_index_page=True,
                ),
                tango: CrateConfig(
                    merge=Merge.SHARED,
                    enable_index_page=True,
                ),
                sierra: CrateConfig(
                    merge=Merge.SHARED,
                    enable_index_page=True,
                ),
            },
            assertions=assertions,
            no_mergeable_rustdoc=False,
        )
    )

    configs.append(
        Config(
            name="transitive",
            desc="simple test to see if we support building crates with transitive deps",
            index=sierra,
            configs={
                quebec: CrateConfig(
                    merge=None,
                ),
                tango: CrateConfig(
                    merge=None,
                ),
                sierra: CrateConfig(
                    merge=None,
                ),
            },
            assertions=[],
            no_mergeable_rustdoc=True,
        )
    )

    configs.append(
        Config(
            name="transitive_no_info",
            desc=(
                "--merge=none on all crates does not generate any cross-crate"
                " info"
            ),
            index=sierra,
            configs={
                quebec: CrateConfig(
                    merge=Merge.NONE,
                    enable_index_page=True,
                ),
                tango: CrateConfig(
                    merge=Merge.NONE,
                    enable_index_page=True,
                ),
                sierra: CrateConfig(
                    merge=Merge.NONE,
                    enable_index_page=True,
                ),
            },
            assertions=[
                *(
                    assertion
                    for i, c in enumerate([sierra, tango])
                    for assertion in test_index_has_crate(c, i, True)
                ),
                test_root_has_item(sierra, "struct", item_S, False),
                test_root_has_item(tango, "trait", item_T, False),
                *test_fixed_crate_impl(
                    sierra, "struct", item_T, item_S, item_S
                ),
                test_cross_crate_impl_not_exist(tango, item_T),
                FileExists(path="search-index.js", fail=True, cci=True),
            ],
            no_mergeable_rustdoc=False,
        )
    )

    configs.append(
        Config(
            name="no_merge_separate",
            desc=(
                "we don't generate any cross-crate info if --merge=none, even if"
                " we document crates separately"
            ),
            index=sierra,
            configs={
                quebec: CrateConfig(
                    merge=Merge.NONE,
                    separate_out_dir=True,
                    enable_index_page=True,
                ),
                tango: CrateConfig(
                    merge=Merge.NONE,
                    separate_out_dir=True,
                    enable_index_page=True,
                ),
                sierra: CrateConfig(
                    merge=Merge.NONE,
                    enable_index_page=True,
                ),
            },
            assertions=[
                *(
                    assertion
                    for i, c in enumerate([sierra, tango])
                    for assertion in test_index_has_crate(c, i, True)
                ),
                test_root_has_item(sierra, "struct", item_S, False),
                *test_fixed_crate_impl(
                    sierra, "struct", item_T, item_S, item_S
                ),
                test_cross_crate_impl_not_exist(tango, item_T),
                FileExists(path="search-index.js", fail=True, cci=True),
            ],
            no_mergeable_rustdoc=False,
        )
    )

    configs.append(
        Config(
            name="no_merge_write_anyway",
            desc="we --merge=none, so --parts-out-dir doesn't do anything",
            index=sierra,
            configs={
                quebec: CrateConfig(
                    merge=Merge.NONE,
                    parts_out_dir=True,
                    enable_index_page=True,
                ),
                tango: CrateConfig(
                    merge=Merge.NONE,
                    parts_out_dir=True,
                    enable_index_page=True,
                ),
                sierra: CrateConfig(
                    merge=Merge.NONE,
                    parts_out_dir=True,
                    enable_index_page=True,
                ),
            },
            assertions=[
                *(
                    assertion
                    for i, c in enumerate([sierra, tango])
                    for assertion in test_index_has_crate(c, i, True)
                ),
                test_root_has_item(sierra, "struct", item_S, False),
                test_root_has_item(tango, "trait", item_T, False),
                *test_fixed_crate_impl(
                    sierra, "struct", item_T, item_S, item_S
                ),
                test_cross_crate_impl_not_exist(tango, item_T),
                FileExists(path="search-index.js", fail=True, cci=True),
            ],
            no_mergeable_rustdoc=False,
        )
    )

    configs.append(
        Config(
            name="single_crate_constant",
            desc="check that the constant appears in the docs",
            index=charlie,
            configs={
                charlie: CrateConfig(
                    merge=None,
                    enable_index_page=True,
                ),
            },
            assertions=[
                *test_index_has_crate(charlie, 0, False),
                test_root_has_item(charlie, "constant", "CHARLIE", False),
                test_search_index("CHARLIE", False),
            ],
            no_mergeable_rustdoc=True,
        )
    )

    configs.append(
        Config(
            name="proc_macro_link_separate",
            desc="checks that we can build a proc macro that uses another crate. Use separate out dirs as we are not testing merging.",
            index=march,
            configs={
                charlie: CrateConfig(
                    merge=None,
                    separate_out_dir=True,
                    enable_index_page=True,
                ),
                march: CrateConfig(
                    merge=None,
                    enable_index_page=True,
                ),
            },
            assertions=[
                *index_but_crate_absent(charlie, 0),
                test_root_has_item(charlie, "constant", "CHARLIE", True),
                test_search_index("CHARLIE", True),
                *test_index_has_crate(march, 0, False),
                test_root_has_item(march, "macro", "make_constant", False),
                test_search_index("make_constant", False),
            ],
            no_mergeable_rustdoc=True,
        )
    )

    configs.append(
        Config(
            name="proc_macro_link",
            desc="checks that we can build a proc macro that use another crate",
            index=march,
            configs={
                charlie: CrateConfig(
                    merge=None,
                    enable_index_page=True,
                ),
                march: CrateConfig(
                    merge=None,
                    enable_index_page=True,
                ),
            },
            assertions=[
                *test_index_has_crate(charlie, 0, False),
                test_root_has_item(charlie, "constant", "CHARLIE", False),
                test_search_index("CHARLIE", False),
                *test_index_has_crate(march, 1, False),
                test_root_has_item(march, "macro", "make_constant", False),
                test_search_index("make_constant", False),
            ],
            no_mergeable_rustdoc=True,
        )
    )

    configs.append(
        Config(
            name="proc_macro_use",
            desc="checks that we can build a proc macro that use another crate, with a separate out dir",
            index=november,
            configs={
                charlie: CrateConfig(
                    merge=None,
                    separate_out_dir=True,
                    enable_index_page=True,
                ),
                march: CrateConfig(
                    merge=None,
                    separate_out_dir=True,
                    enable_index_page=True,
                ),
                november: CrateConfig(
                    merge=None,
                    enable_index_page=True,
                ),
            },
            assertions=[
                *index_but_crate_absent(charlie, 0),
                test_root_has_item(charlie, "constant", "CHARLIE", True),
                test_search_index("CHARLIE", True),
                *index_but_crate_absent(march, 1),
                test_root_has_item(march, "macro", "make_constant", True),
                test_search_index("make_constant", True),
                *test_index_has_crate(november, 2, False),
                test_root_has_item(november, "constant", "NOVEMBER", True),
            ],
            no_mergeable_rustdoc=True,
        )
    )

    # single crate stuff
    assertions = [
        *test_index_has_crate(quebec, 0, False),
        test_root_has_item(quebec, "struct", item_Q, False),
        test_search_index(item_Q, False),
    ]

    configs.append(
        Config(
            name="single_crate_baseline",
            desc="there's nothing cross-crate going on here",
            index=quebec,
            configs={
                quebec: CrateConfig(
                    merge=None,
                    enable_index_page=True,
                ),
            },
            assertions=assertions,
            no_mergeable_rustdoc=True,
        )
    )

    configs.append(
        Config(
            name="single_crate_read-write",
            desc=(
                "read-write is the default and this does the same as"
                " `single-crate`"
            ),
            index=quebec,
            configs={
                quebec: CrateConfig(
                    merge=Merge.SHARED,
                    enable_index_page=True,
                ),
            },
            assertions=assertions,
            no_mergeable_rustdoc=False,
        )
    )

    configs.append(
        Config(
            name="single_crate_write_anyway",
            desc=(
                "we can --parts-out-dir, but that doesn't do anything other"
                " than create the file"
            ),
            index=quebec,
            configs={
                quebec: CrateConfig(
                    merge=None,
                    parts_out_dir=True,
                    enable_index_page=True,
                ),
            },
            assertions=assertions,
            no_mergeable_rustdoc=False,
        )
    )

    configs.append(
        Config(
            name="single_crate_finalize",
            desc=(
                "there is nothing to read from the output directory if we use a"
                " single crate"
            ),
            index=quebec,
            configs={
                quebec: CrateConfig(
                    merge=Merge.FINALIZE,
                    enable_index_page=True,
                ),
            },
            assertions=assertions,
            no_mergeable_rustdoc=False,
        )
    )

    configs.append(
        Config(
            name="single_crate_no_index",
            desc="there's nothing cross-crate going on here",
            index=quebec,
            configs={
                quebec: CrateConfig(merge=None),
            },
            assertions=[
                test_root_has_item(quebec, "struct", item_Q, False),
                test_search_index(item_Q, False),
            ],
            no_mergeable_rustdoc=True,
        )
    )

    configs.append(
        Config(
            name="single_merge_none_useless_write",
            desc="--merge=none doesn't write anything, despite --parts-out-dir",
            index=quebec,
            configs={
                quebec: CrateConfig(
                    merge=Merge.NONE,
                    parts_out_dir=True,
                    enable_index_page=True,
                ),
            },
            assertions=[
                *test_index_has_crate(quebec, 0, True),
                test_root_has_item(quebec, "struct", item_Q, False),
                FileExists(path="search-index.js", fail=True, cci=True),
            ],
            no_mergeable_rustdoc=False,
        )
    )

    configs.append(
        Config(
            name="kitchen_sink",
            desc="document everything in the default mode",
            index=indigo,
            configs={
                quebec: CrateConfig(
                    merge=None,
                    enable_index_page=True,
                ),
                tango: CrateConfig(
                    merge=None,
                    enable_index_page=True,
                ),
                sierra: CrateConfig(
                    merge=None,
                    enable_index_page=True,
                ),
                romeo: CrateConfig(
                    merge=None,
                    enable_index_page=True,
                ),
                indigo: CrateConfig(
                    merge=None,
                    enable_index_page=True,
                ),
            },
            assertions=[
                t_i_header,
                *(
                    assertion
                    for j, c in enumerate(
                        [indigo, quebec, romeo, sierra, tango]
                    )
                    for assertion in test_index_has_crate(c, j, False)
                ),
                test_root_has_item(quebec, "struct", item_Q, False),
                test_root_has_item(romeo, "type", item_R, False),
                test_root_has_item(sierra, "struct", item_S, False),
                test_root_has_item(tango, "trait", item_T, False),
                *test_fixed_crate_impl(
                    sierra, "struct", item_T, item_S, item_S
                ),
                test_cross_crate_impl(tango, item_T, item_S, "struct"),
                test_search_index(item_Q, False),
                test_search_index(item_R, False),
                test_search_index(item_S, False),
                test_search_index(item_T, False),
                *test_type_impl(
                    "struct",
                    sierra,
                    item_S,
                    trait_name=item_T,
                    alias_name=item_R,
                ),
            ],
            no_mergeable_rustdoc=True,
        )
    )

    assert_no_duplicate_config_names(configs)

    for config in [c for c in configs if c.no_mergeable_rustdoc]:
        config.run(rustc=args.rustc, rustdoc=args.rustdoc)
    print(
        "local tests passed. remove no_mergeable_rustdoc filter for complete coverage"
    )

    if args.rustdoc_destination is not None:
        Path("cross-crate-info")
        rmtree(args.rustdoc_destination, ignore_errors=True)
        for config in configs:
            config.render_as_rustdoc_test(args.rustdoc_destination)

    if args.fuchsia_destination is not None:
        configs = [c for c in configs if c.no_mergeable_rustdoc]
        configs = [
            c
            for c in configs
            if all(
                v.separate_out_dir or k == c.index for k, v in c.configs.items()
            )
        ]
        print(
            "saving", *(c.name for c in configs), "to", args.fuchsia_destination
        )
        print("use fx format-code")
        rmtree(args.fuchsia_destination, ignore_errors=True)
        for c in configs:
            c.render_as_gn_test(args.fuchsia_destination)
        gn_test_build = "".join(
            f'"//build/rust/tests/rustdoc/{c.name}", ' for c in configs
        )
        Path(args.fuchsia_destination / "BUILD.gn").write_text(
            f"# Copyright 2024 The Fuchsia Authors. All rights reserved.\n"
            f"# Use of this source code is governed by a BSD-style license that can be\n"
            f"# found in the LICENSE file.\n"
            f'group("rustdoc") {{\n'
            f"  testonly = true\n"
            f"  deps = [{gn_test_build}]\n"
            f"metadata = {{\n"
            f"    # Exclude the test universe crates from the global\n"
            f"    # rust_target_mapping.json. These crates aren't meaningful to\n"
            f"    # consumers of the Fuchsia rustdoc index, who are looking for\n"
            f"    # API documentation.\n"
            f"    rust_test_barrier = []\n"
            f"}}\n"
            f"}}\n"
        )


def _main_arg_parser() -> ArgumentParser:
    parser = ArgumentParser(description="generates rustdoc tests")
    parser.add_argument(
        "--rustdoc",
        default=Path(
            "prebuilt/third_party/rust/linux-x64/bin/rustdoc"
        ).resolve(),
        type=Path,
        help="path directly to the rustdoc executable",
    )
    parser.add_argument(
        "--rustdoc-destination",
        type=Path,
        help="optional: where to output rust-lang/rust:tests/rustdoc/cross-crate-info tests",
    )
    parser.add_argument(
        "--fuchsia-destination",
        default=Path("build/rust/tests/rustdoc").resolve(),
        type=Path,
        help="where to output fuchsia tests",
    )
    parser.add_argument(
        "--rustc",
        default=Path("prebuilt/third_party/rust/linux-x64/bin/rustc").resolve(),
        type=Path,
        help="path directly to the rustc executable",
    )
    parser.set_defaults(func=main)

    return parser


if __name__ == "__main__":
    parser = _main_arg_parser()
    args = parser.parse_args(argv[1:])
    args.func(args)

# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Classes for using an SPDX document and its sub-elements"""


import dataclasses
import hashlib
import json
import re
from collections import defaultdict
from typing import Any

try:
    # Bazel build uses fully-qualified package names.
    from fuchsia.tools.licenses.common_types import *
except ImportError:
    # Bazel uses shorter package names.
    from common_types.common_types import *

# Actually 2.2.2, but only SPDX-N.M is used in JSON serialization.
_default_spdx_json_version = "SPDX-2.2"
_supported_spdx_json_versions = [_default_spdx_json_version, "SPDX-2.3"]
_spdx_document_ref = "SPDXRef-DOCUMENT"


@dataclasses.dataclass(frozen=True)
class SpdxLicenseExpression:
    """
    Holds an SPDX license expression string.

    Implementing https://spdx.github.io/spdx-spec/v2-draft/SPDX-license-expressions/
    is rather complex, but for our purposes we only need to extract and replace
    the ids of licenses in the expression, not parse the expression itself.
    """

    # A formatted string template. Will contain {0}, {1}, ... as placeholder for the various licenses.
    expression_template: str
    license_ids: tuple[str]

    def create(expression_str: str, location_for_error: str | None = None):
        assert expression_str != None

        expression_template = []
        license_refs = {}

        remaining_str = expression_str
        while remaining_str:
            # Try to match LicenseRef-... or License-...:
            # Note that License- is not part of the SPDX spec, but nevertheless
            # some common SPDX libs use it.
            match = re.match(
                r"^(LicenseRef|License)-[a-zA-Z0-9-\.]+", remaining_str
            )
            if match:
                assert match.pos == 0
                remaining_str = remaining_str[match.end() :]
                ref = match.group()
                if ref not in license_refs:
                    license_refs[ref] = len(license_refs.keys())
                expression_template.append("{%s}" % license_refs[ref])
                continue

            # Try to match other expression tokens: AND, OR, WITH, (, ), + and whitespace...
            match = re.match(r"^AND|^OR|^WITH|^\(|^\)|^\+|^\s+", remaining_str)
            if match:
                remaining_str = remaining_str[match.end() :]
                assert match.pos == 0
                expression_template.append(match.group())
                continue

            raise LicenseException(
                f"Invalid license expression token '{remaining_str}'",
                location_for_error,
            )

        # Temporary workaround for https://fxbug.dev/42068819#c3. Only the last ref is meaningful
        if len(license_refs) > 1:
            key_list = list(license_refs.keys())
            if key_list[0].endswith("NOTICE.txt-0") and key_list[-1].endswith(
                "LICENSE-0"
            ):
                return SpdxLicenseExpression.create(
                    f"{key_list[0].replace('NOTICE.txt-0', 'NOTICE.txt')} AND {key_list[-1].replace('LICENSE-0', 'LICENSE')}"
                )

        return SpdxLicenseExpression(
            expression_template="".join(expression_template),
            license_ids=tuple(license_refs.keys()),
        )

    def serialize(self):
        return self.expression_template.format(*self.license_ids)

    def replace_license_ids(self, id_replacer: "SpdxIdReplacer"):
        return dataclasses.replace(
            self,
            license_ids=tuple(
                [id_replacer.get_replaced_id(id) for id in self.license_ids]
            ),
        )


@dataclasses.dataclass(frozen=True)
class SpdxPackage:
    """Container for an SPDX package element"""

    spdx_id: str
    name: str
    copyright_text: str | None = None
    license_concluded: SpdxLicenseExpression | None = None
    homepage: str | None = None
    debug_hint: list[str] | None = None

    def to_json_dict(self):
        output = {"SPDXID": self.spdx_id, "name": self.name}
        _maybe_set(output, "copyrightText", self.copyright_text)
        _maybe_set(output, "homepage", self.homepage)
        if self.license_concluded:
            output["licenseConcluded"] = self.license_concluded.serialize()
        _maybe_set(output, "_hint", self.debug_hint)
        return output

    def from_json_dict(input: DictReader):
        license_concluded_str = input.get_string_or_none("licenseConcluded")

        license_concluded = (
            SpdxLicenseExpression.create(license_concluded_str, input.location)
            if license_concluded_str
            else None
        )
        homepage = input.get_string_or_none("homepage")
        copyright_text = input.get_string_or_none("copyrightText")

        name = input.get("name")
        if name.startswith("third_party/"):
            # TODO(b/316188315): Remove once fixed upstream.
            name = name[len("third_party/") :]

        debug_hint = input.get_string_list("_hint")

        return SpdxPackage(
            spdx_id=input.get("SPDXID"),
            name=name,
            copyright_text=copyright_text,
            license_concluded=license_concluded,
            homepage=homepage,
            debug_hint=debug_hint,
        )

    def replace_ids(
        self,
        package_id_replacer: "SpdxIdReplacer",
        license_id_replacer: "SpdxIdReplacer",
    ) -> "SpdxPackage":
        license_concluded = self.license_concluded
        if license_concluded:
            license_concluded = license_concluded.replace_license_ids(
                license_id_replacer
            )
        return dataclasses.replace(
            self,
            spdx_id=package_id_replacer.get_replaced_id(self.spdx_id),
            license_concluded=license_concluded,
        )


@dataclasses.dataclass(frozen=True)
class SpdxExtractedLicensingInfo:
    """
    Container for an SPDX license element.

    Corresponds with SPDX 2.2.2 specification:
    https://spdx.github.io/spdx-spec/other-licensing-information-detected/
    """

    license_id: str
    name: str
    extracted_text: str
    cross_refs: list[str] = dataclasses.field(default_factory=list)
    see_also: list[str] = dataclasses.field(default_factory=list)
    debug_hint: list[str] | None = None

    def to_json_dict(self):
        output = {
            "name": self.name,
            "licenseId": self.license_id,
            "extractedText": self.extracted_text,
        }
        if self.cross_refs:
            output["crossRefs"] = [
                {
                    "url": u,
                }
                for u in self.cross_refs
            ]
        _maybe_set(output, "seeAlsos", self.see_also)
        _maybe_set(output, "_hint", self.debug_hint)

        return output

    def from_json_dict(input: DictReader):
        license_id = input.get("licenseId")
        name = input.get("name")
        if name.startswith("third_party/"):
            # TODO(b/316188315): Remove once fixed upstream.
            name = name[len("third_party/") :]

        cross_refs = [
            ref_dict.get("url")
            for ref_dict in input.get_readers_list("crossRefs")
        ]
        cross_refs = [s for s in cross_refs if s]

        # 'seeAlso' sometimes appears as 'seeAlsos'
        see_also = input.get_or(
            "seeAlso", default=input.get_or("seeAlsos", default=[])
        )
        see_also = [s for s in see_also if s]

        extracted_text = input.get("extractedText")

        debug_hint = input.get_string_list("_hint")

        return SpdxExtractedLicensingInfo(
            license_id=license_id,
            name=name,
            extracted_text=extracted_text,
            cross_refs=cross_refs,
            see_also=see_also,
            debug_hint=debug_hint,
        )

    def merge_with(self, other: "SpdxExtractedLicensingInfo"):
        unified_cross_refs = _unify_and_sort_lists(
            other.cross_refs, self.cross_refs
        )
        unified_see_also = _unify_and_sort_lists(other.see_also, self.see_also)
        unified_debug_hint = _unify_and_sort_lists(
            other.debug_hint, self.debug_hint
        )

        return dataclasses.replace(
            self,
            cross_refs=unified_cross_refs,
            see_also=unified_see_also,
            debug_hint=unified_debug_hint,
        )

    def replace_id(
        self, license_id_replacer: "SpdxIdReplacer"
    ) -> "SpdxExtractedLicensingInfo":
        return dataclasses.replace(
            self,
            license_id=license_id_replacer.get_replaced_id(self.license_id),
        )

    def extracted_text_lines(self):
        return self.extracted_text.splitlines()

    def unique_links(self):
        links = []
        links.extend(self.cross_refs)
        links.extend(self.see_also)
        return sorted(list(set(links)))

    @staticmethod
    def content_based_license_id(name: str, text: str) -> str:
        """Returns an ids that is based on the content of the license: Name and Text (stripped)"""
        md5 = hashlib.md5()
        md5.update(name.strip().encode("utf-8"))
        md5.update(text.strip().encode("utf-8"))
        digest = md5.hexdigest()
        return f"LicenseRef-{digest}"


@dataclasses.dataclass(frozen=True)
class SpdxRelationship:
    """Container for an SPDX relationship element"""

    spdx_element_id: str
    related_spdx_element: str
    relationship_type: str

    def to_json_dict(self):
        return {
            "spdxElementId": self.spdx_element_id,
            "relatedSpdxElement": self.related_spdx_element,
            "relationshipType": self.relationship_type,
        }

    def from_json_dict(input: DictReader):
        return SpdxRelationship(
            spdx_element_id=input.get("spdxElementId"),
            related_spdx_element=input.get("relatedSpdxElement"),
            relationship_type=input.get("relationshipType"),
        )

    def replace_ids(
        self, package_id_replacer: "SpdxIdReplacer"
    ) -> "SpdxRelationship":
        return dataclasses.replace(
            self,
            spdx_element_id=package_id_replacer.get_replaced_id(
                self.spdx_element_id
            ),
            related_spdx_element=package_id_replacer.get_replaced_id(
                self.related_spdx_element
            ),
        )


@dataclasses.dataclass(frozen=True)
class SpdxDocument:
    """Container for an SPDX document element"""

    file_path: str
    name: str
    namespace: str
    creators: list[str]
    describes: list[str]
    packages: list[SpdxPackage]
    relationships: list[SpdxRelationship]
    extracted_licenses: list[SpdxExtractedLicensingInfo]
    spdx_id: str = _spdx_document_ref

    def refactor_ids(
        self,
        package_id_factory: "SpdxPackageIdFactory",
    ):
        """
        Returns a copy of the document with all ids refactored.

        Uses package_id_factory to replace existing package ids.
        Uses SpdxExtractedLicensingInfo.content_based_license_id() to
        replace license ids, potentially merging license elements with
        identical content.
        """

        package_id_replacer = SpdxIdReplacer(doc_location=self.file_path)
        license_id_replacer = SpdxIdReplacer(doc_location=self.file_path)

        new_extracted_licenses_by_id: dict[str, SpdxExtractedLicensingInfo] = {}
        for el in self.extracted_licenses:
            new_id = SpdxExtractedLicensingInfo.content_based_license_id(
                el.name, el.extracted_text
            )
            license_id_replacer.replace_id(el.license_id, new_id)
            el = el.replace_id(license_id_replacer)
            duplicate_el = new_extracted_licenses_by_id.get(new_id, None)
            if duplicate_el:
                el = el.merge_with(duplicate_el)
            new_extracted_licenses_by_id[new_id] = el

        new_packages: list[SpdxPackage] = []
        for p in self.packages:
            package_id_replacer.replace_id(
                p.spdx_id, package_id_factory.new_id()
            )
            new_packages.append(
                p.replace_ids(package_id_replacer, license_id_replacer)
            )

        new_describes = [
            package_id_replacer.get_replaced_id(d) for d in self.describes
        ]
        new_relationships = [
            r.replace_ids(package_id_replacer) for r in self.relationships
        ]
        return dataclasses.replace(
            self,
            describes=new_describes,
            packages=new_packages,
            relationships=new_relationships,
            extracted_licenses=new_extracted_licenses_by_id.values(),
        )

    def to_json(self, spdx_json_file_path):
        json_dict = self.to_json_dict()
        with open(spdx_json_file_path, "w") as output_file:
            json.dump(json_dict, output_file, indent=4)

    def to_json_dict(self):
        describes_json = sorted(self.describes)
        packages_json = [
            p.to_json_dict()
            for p in sorted(self.packages, key=lambda x: (x.name, x.spdx_id))
        ]
        relationships_json = [
            r.to_json_dict()
            for r in sorted(
                self.relationships,
                key=lambda x: (x.spdx_element_id, x.related_spdx_element),
            )
        ]
        licenses_json = [
            e.to_json_dict()
            for e in sorted(
                self.extracted_licenses, key=lambda x: (x.name, x.license_id)
            )
        ]

        return {
            "spdxVersion": _default_spdx_json_version,
            "SPDXID": self.spdx_id,
            "name": self.name,
            "documentNamespace": self.namespace,
            "creationInfo": {
                "creators": self.creators,
            },
            "dataLicense": "CC0-1.0",
            "documentDescribes": describes_json,
            "packages": packages_json,
            "relationships": relationships_json,
            "hasExtractedLicensingInfos": licenses_json,
        }

    def from_json(spdx_json_file_path: str) -> "SpdxDocument":
        with open(spdx_json_file_path, "r") as input_file:
            return SpdxDocument.from_json_dict(
                spdx_json_file_path, json.load(input_file)
            )

    def from_json_dict(
        spdx_json_file_path: str, json_dict: dict[str, Any]
    ) -> "SpdxDocument":
        reader = DictReader(json_dict, f"{spdx_json_file_path}")
        return SpdxDocument.from_json_dict_reader(spdx_json_file_path, reader)

    def from_json_dict_reader(
        spdx_json_file_path: str, doc_dict: DictReader
    ) -> "SpdxDocument":
        """Parses an SPDX json dictionary into an SpdxDocument"""

        name = doc_dict.get("name")
        document_spdx_id = doc_dict.get("SPDXID")
        namespace = doc_dict.get("documentNamespace")
        spdx_version = doc_dict.get("spdxVersion")
        if spdx_version not in _supported_spdx_json_versions:
            raise LicenseException(
                f"Only {_supported_spdx_json_versions} are supported but '{spdx_version}' found",
                doc_dict.location,
            )
        creators = doc_dict.get_reader("creationInfo").get(
            "creators", expected_type=list
        )

        describes = doc_dict.get_or("documentDescribes", [], expected_type=list)
        packages = [
            SpdxPackage.from_json_dict(d)
            for d in doc_dict.get_readers_list("packages", dedup=True)
        ]
        relationships = [
            SpdxRelationship.from_json_dict(d)
            for d in doc_dict.get_readers_list("relationships", dedup=True)
        ]
        # Ignore relationships between the document and packages - we don't care for these
        relationships = [
            r
            for r in relationships
            if r.spdx_element_id != document_spdx_id
            and r.related_spdx_element != document_spdx_id
        ]

        extracted_licenses = [
            SpdxExtractedLicensingInfo.from_json_dict(d)
            for d in doc_dict.get_readers_list(
                "hasExtractedLicensingInfos", dedup=True
            )
        ]

        return SpdxDocument(
            file_path=spdx_json_file_path,
            name=name,
            namespace=namespace,
            creators=creators,
            describes=describes,
            packages=packages,
            relationships=relationships,
            extracted_licenses=extracted_licenses,
            spdx_id=document_spdx_id,
        )


class SpdxIndex:
    """Builds an index for optimized lookup across an SpdxDocument"""

    def __init__(
        self,
        spdx_doc_file_path: str,
        license_by_id: dict[str, SpdxExtractedLicensingInfo],
        package_by_id: dict[str, SpdxPackage],
        packages_by_license_id: dict[str, set[str]],
        child_packages_by_parent_id: dict[str, set[str]],
        parent_packages_by_child_id: dict[str, set[str]],
    ):
        self._spdx_doc_file_path = spdx_doc_file_path
        self._license_by_id = license_by_id
        self._package_by_id = package_by_id
        self._packages_by_license_id = packages_by_license_id
        self._child_packages_by_parent_id = child_packages_by_parent_id
        self._parent_packages_by_child_id = parent_packages_by_child_id

    def get_root_packages(self):
        return [
            p
            for p in self._package_by_id.values()
            if not self.get_parent_packages(p)
        ]

    def get_packages_by_license(self, license: SpdxExtractedLicensingInfo):
        id = license.license_id
        if id in self._packages_by_license_id:
            return self.get_packages_by_ids(self._packages_by_license_id[id])
        else:
            raise LicenseException(
                f"No packages associated with '{license.license_id}' (name={license.name}, links={license.unique_links()})",
                self._spdx_doc_file_path,
            )

    def get_license_by_id(self, id: str):
        if id in self._license_by_id:
            return self._license_by_id[id]
        else:
            raise LicenseException(
                f"No license with id '{id}", self._spdx_doc_file_path
            )

    def get_package_by_id(self, id: str):
        if id in self._package_by_id:
            return self._package_by_id[id]
        else:
            raise LicenseException(
                f"No package with id '{id}", self._spdx_doc_file_path
            )

    def get_packages_by_ids(self, ids: list[str]):
        return [self.get_package_by_id(id) for id in ids]

    def get_parent_packages(self, package: SpdxPackage):
        id = package.spdx_id
        if id in self._parent_packages_by_child_id:
            return self.get_packages_by_ids(
                self._parent_packages_by_child_id[id]
            )
        else:
            return []

    def get_child_packages(self, package: SpdxPackage):
        id = package.spdx_id
        if id in self._child_packages_by_parent_id:
            return self.get_packages_by_ids(
                self._child_packages_by_parent_id[id]
            )
        else:
            return []

    def dependency_chains_for_license(
        self, license: SpdxExtractedLicensingInfo
    ) -> list[list[SpdxPackage]]:
        """ "
        Computes all the dependencies of a given license.

        Returns a list of list of packages. Each list of packages is a dependency chain
        from the root of the SPDX document to the license.
        """

        def path_recursion(
            current_path: list[SpdxPackage], current_package: SpdxPackage
        ):
            parents = self.get_parent_packages(current_package)
            if not parents:
                # End of the chain: Output the current path in reverse
                path = current_path[::-1]
                output.append(path)
            else:
                for p in parents:
                    current_path.append(p)
                    path_recursion(current_path, p)
                    current_path.pop()

        output = []

        for p in self.get_packages_by_license(license):
            path_recursion(current_path=[p], current_package=p)

        return output

    def create(input: SpdxDocument) -> "SpdxIndex":
        """Constructs an SpdxIndex for the given SpdxDocument"""
        license_by_id = {}
        for el in input.extracted_licenses:
            if el.license_id in license_by_id:
                raise LicenseException(
                    f"license id '{el.license_id}' defined multiple times",
                    input.file_path,
                )
            license_by_id[el.license_id] = el

        package_by_id = {}
        packages_by_license_id = defaultdict(set)
        for p in input.packages:
            id = p.spdx_id
            if id in package_by_id:
                raise LicenseException(
                    f"spdx id {id} defined multiple times", input.file_path
                )
            package_by_id[id] = p

            if p.license_concluded:
                for license_id in p.license_concluded.license_ids:
                    if license_id not in license_by_id:
                        raise LicenseException(
                            f"license_conclude '{license_id}' used but no such license defined",
                            input.file_path,
                        )
                    packages_by_license_id[license_id].add(id)

        child_packages_by_parent_id = defaultdict(set)
        parent_packages_by_child_id = defaultdict(set)

        for r in input.relationships:
            parent = r.spdx_element_id
            child = r.related_spdx_element
            if parent == input.spdx_id or child == input.spdx_id:
                # Ignore relationship to the document itself
                continue
            if parent not in package_by_id:
                raise LicenseException(
                    f"spdx id '{parent}' used in relationship but there is no element with that id",
                    input.file_path,
                )
            if child not in package_by_id:
                raise LicenseException(
                    f"spdx id '{child}' used in relationship but there is no element with that id",
                    input.file_path,
                )
            if r.relationship_type in ["CONTAINS", "DESCENDANT_OF"]:
                child_packages_by_parent_id[parent].add(child)
                parent_packages_by_child_id[child].add(parent)

        return SpdxIndex(
            spdx_doc_file_path=input.file_path,
            license_by_id=license_by_id,
            package_by_id=package_by_id,
            packages_by_license_id=packages_by_license_id,
            child_packages_by_parent_id=child_packages_by_parent_id,
            parent_packages_by_child_id=parent_packages_by_child_id,
        )


class SpdxPackageIdFactory:
    """Factory for monotonically increasing SPDX package ids"""

    _next_id: int

    def __init__(self):
        self._next_id = -1

    def new_id(self):
        self._next_id = self._next_id + 1
        return "SPDXRef-Package-{id}".format(id=self._next_id)


@dataclasses.dataclass(frozen=False)
class SpdxDocumentBuilder:
    """A builder for SpdxDocument"""

    root_package_name: str
    creators: list[str]
    root_package: SpdxPackage = None
    _describes: list["str"] = dataclasses.field(default_factory=list)
    _packages_by_id: dict[str, SpdxPackage] = dataclasses.field(
        default_factory=dict
    )
    _relationships: list[SpdxRelationship] = dataclasses.field(
        default_factory=list
    )
    _extracted_licenses_by_id: dict[
        str, SpdxExtractedLicensingInfo
    ] = dataclasses.field(default_factory=dict)
    _package_id_factory: SpdxPackageIdFactory = dataclasses.field(
        default_factory=SpdxPackageIdFactory
    )

    @staticmethod
    def create(
        root_package_name: str,
        creators: list[str],
        root_package_homepage: str | None = None,
    ) -> "SpdxDocumentBuilder":
        builder = SpdxDocumentBuilder(
            root_package_name=root_package_name,
            creators=creators,
        )
        builder._add_root_package(
            name=root_package_name,
            homepage=root_package_homepage,
        )
        return builder

    def next_package_id(self) -> str:
        return self._package_id_factory.new_id()

    def has_package(self, package_or_package_id: str | SpdxPackage) -> bool:
        package_id = (
            package_or_package_id
            if isinstance(package_or_package_id, str)
            else package_or_package_id.spdx_id
        )
        return package_id in self._packages_by_id

    def add_package(self, package: SpdxPackage) -> None:
        assert package.spdx_id
        assert (
            package.spdx_id not in self._packages_by_id
        ), f"{package} already in document as {self._packages_by_id[package.spdx_id]}"

        self._packages_by_id[package.spdx_id] = package
        self._describes.append(package.spdx_id)

    def _add_root_package(self, name: str, homepage: str | None):
        assert not self.root_package
        self.root_package = SpdxPackage(
            spdx_id=self._package_id_factory.new_id(),
            name=name,
            homepage=homepage,
        )
        self.add_package(self.root_package)

    def add_license(self, license: SpdxExtractedLicensingInfo) -> None:
        existing_license = self._extracted_licenses_by_id.get(
            license.license_id, None
        )
        if existing_license:
            license = license.merge_with(existing_license)
        self._extracted_licenses_by_id[license.license_id] = license

    def add_relationship(self, rel: SpdxRelationship) -> None:
        assert self.has_package(rel.spdx_element_id)
        assert self.has_package(rel.related_spdx_element)
        self._relationships.append(rel)

    def add_contains_relationship(
        self, spdx_element_id: str, related_spdx_element: str
    ) -> None:
        self.add_relationship(
            SpdxRelationship(
                spdx_element_id,
                related_spdx_element,
                relationship_type="CONTAINS",
            ),
        )

    def add_contained_by_root_package_relationship(
        self, related_spdx_element: str
    ) -> None:
        self.add_contains_relationship(
            self.root_package.spdx_id, related_spdx_element
        )

    def add_document(
        self, parent_package: SpdxPackage, doc: SpdxDocument
    ) -> None:
        """Adds all the elements of the given document to the currently built document.

        All licenses, packages and relationships are copied, and new "contains" relationships
        are introduced between |parent_package| and the root packages in the given document.

        The ids of licenses and packages are replaced to integrate without collisions within
        the build document's id space. Licenses are given content-based ids, to de-duplicate
        repeated ids.
        """
        doc = doc.refactor_ids(self._package_id_factory)
        index: SpdxIndex = SpdxIndex.create(doc)

        for license in doc.extracted_licenses:
            self.add_license(license)
        for p in doc.packages:
            self.add_package(p)
        for rel in doc.relationships:
            self.add_relationship(rel)

        for root_pkg in index.get_root_packages():
            self.add_contains_relationship(
                parent_package.spdx_id, root_pkg.spdx_id
            )

    def build(self) -> SpdxDocument:
        return SpdxDocument(
            file_path=None,
            name=self.root_package_name,
            namespace="",
            creators=self.creators.copy(),
            describes=self._describes.copy(),
            packages=list(self._packages_by_id.values()),
            relationships=self._relationships.copy(),
            extracted_licenses=list(self._extracted_licenses_by_id.values()),
        )


class SpdxIdReplacer:
    """Helper for replacing Spdx Ids"""

    _replaced_ids: dict[str, str]
    _doc_location: str

    def __init__(self, doc_location: str = None):
        self._doc_location = doc_location
        self._replaced_ids = {}

    def replace_id(self, old_id, new_id):
        """Maps an old id to a new id that replaces it"""
        if old_id in self._replaced_ids:
            raise LicenseException(
                f"Can't map old_id='{old_id}' to new_id='{new_id}'. It is already mapped to '{self._replaced_ids[old_id]}'",
                self._doc_location,
            )
        self._replaced_ids[old_id] = new_id

    def is_replaced_id(self, old_id: str) -> bool:
        return old_id in self._replaced_ids

    def get_replaced_id(self, old_id):
        """Returns the new id associated with the given id"""
        if old_id is None:
            return old_id
        if old_id not in self._replaced_ids:
            raise LicenseException(
                f"Spdx id '{old_id}' doesn't refer to any known element",
                self._doc_location,
            )
        return self._replaced_ids[old_id]


def _maybe_set(output_dict: dict[str, Any], key: str, value: Any):
    if value:
        output_dict[key] = value


def _unify_and_sort_lists(list1, list2):
    """Unifies and sorts 2 lists, removing duplicate values"""
    if not list1 and not list2:
        return []
    elif not list1:
        return sorted(list2)
    elif not list2:
        return sorted(list1)
    else:
        unique_values = set()
        unique_values.update(list1)
        unique_values.update(list2)
        return sorted(list(unique_values))

# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Types for classifying licenses"""

import dataclasses
import json
from collections import Counter, defaultdict
from collections.abc import Callable, Collection
from hashlib import md5
from typing import Any, ClassVar

from fuchsia.tools.licenses.common_types import *
from fuchsia.tools.licenses.spdx_types import *


@dataclasses.dataclass(frozen=True)
class IdentifiedSnippet:
    """Information about a single license snippet (text part of a large license text)"""

    # Built-in 'identified_as' values.
    UNIDENTIFIED_IDENTIFICATION: ClassVar[str] = "[UNIDENTIFIED]"
    IGNORABLE_IDENTIFICATION: ClassVar[str] = "[IGNORABLE]"
    COPYRIGHT_IDENTIFICATION: ClassVar[str] = "[COPYRIGHT]"
    WILDCARD_HEADER_IDENTIFICATION: ClassVar[str] = "[NOTICE]"

    # Condition for build-in identified_as
    DEFAULT_CONDITION_BY_IDENTIFICATION: ClassVar[dict[str, str]] = {
        UNIDENTIFIED_IDENTIFICATION: "unidentified",
        IGNORABLE_IDENTIFICATION: "ignorable",
        COPYRIGHT_IDENTIFICATION: "copyright",
        WILDCARD_HEADER_IDENTIFICATION: "notice",
    }

    identified_as: str
    confidence: float
    start_line: int
    end_line: int

    conditions: set[str] = dataclasses.field(default_factory=set)
    # Conditions from overriding rules
    overriden_conditions: set[str] = dataclasses.field(default_factory=set)
    # Optional public source code mirroring urls (supplied by some override rules)
    public_source_mirrors: list[str] = None
    # Dependents that were not matched by any rule
    dependents_unmatched_by_overriding_rules: set[str] = dataclasses.field(
        default_factory=set
    )
    # Conditions that were not matched by any rule
    conditions_unmatched_by_overriding_rules: set[str] = dataclasses.field(
        default_factory=set
    )
    # all rules that matched this IdentifiedSnippet
    overriding_rules: list["ConditionOverrideRule"] = dataclasses.field(
        default_factory=list
    )

    # verification results
    verified: bool = None
    verification_message: str = None
    verified_conditions: set[str] = None

    # checksum for snippet text
    snippet_checksum: str = None
    snippet_text: str = None

    # A suggested override rule
    suggested_override_rule: "ConditionOverrideRule" = None

    def from_identify_license_dict(
        dictionary: dict[str, Any],
        location: Any,
    ) -> "IdentifiedSnippet":
        """
        Create a IdentifiedSnippet instance from a dictionary in the output format of
        https://github.com/google/licenseclassifier/tree/main/tools/identify_license.

        i.e.
        {
            "Name": str
            "Confidence": int or float
            "StartLine": int
            "EndLine": int
            "Condition": str
        }

        "Name" will be "Unclassified" for licenses that the tool can't identify.
        """
        r = DictReader(dictionary, location)

        identified_as = r.get("Name")
        match identified_as:
            case "Unclassified":
                identified_as = IdentifiedSnippet.UNIDENTIFIED_IDENTIFICATION
            case "Ignorable":
                identified_as = IdentifiedSnippet.IGNORABLE_IDENTIFICATION
            case "Copyright":
                identified_as = IdentifiedSnippet.COPYRIGHT_IDENTIFICATION

        # Wildcard header texts need to be handled separately, as they do not
        # come with a condition. See b/355649014 for more information.
        if any(
            identified_as.startswith(item)
            for item in ["anyStartsWith", "anyContains", "anyEndsWith"]
        ):
            identified_as = IdentifiedSnippet.WILDCARD_HEADER_IDENTIFICATION

        # Confidence could be an int or a float. Convert to a float.
        try:
            confidence = r.get("Confidence", expected_type=float)
        except LicenseException:
            confidence = float(r.get("Confidence", expected_type=int))

        # License classifier may return a string that is multiple conditions separated by ' '
        condition = r.get_or("Condition", expected_type=str, default=None)
        if not condition:
            condition = (
                IdentifiedSnippet.DEFAULT_CONDITION_BY_IDENTIFICATION.get(
                    identified_as, None
                )
            )
            assert (
                condition
            ), f"Condition not found and no default condition for identification {identified_as}"
            conditions = set([condition])
        else:
            conditions = set(condition.split(" "))

        return IdentifiedSnippet(
            identified_as=identified_as,
            confidence=confidence,
            start_line=r.get("StartLine", expected_type=int),
            end_line=r.get("EndLine", expected_type=int),
            conditions=conditions,
        )

    def to_json_dict(self):
        # The fields are output in a certain order to produce a more readable output.
        out = {
            "identified_as": self.identified_as,
            "conditions": sorted(list(self.conditions)),
            "verified": self.verified,
        }

        if self.verification_message:
            out["verification_message"] = self.verification_message
        if self.verified_conditions:
            out["verified_conditions"] = sorted(list(self.verified_conditions))
        if self.overriden_conditions:
            out["overriden_conditions"] = sorted(
                list(self.overriden_conditions)
            )
        if self.conditions_unmatched_by_overriding_rules:
            out["conditions_unmatched_by_overriding_rules"] = sorted(
                list(self.conditions_unmatched_by_overriding_rules)
            )
        if self.dependents_unmatched_by_overriding_rules:
            out["dependents_unmatched_by_overriding_rules"] = sorted(
                list(self.dependents_unmatched_by_overriding_rules)
            )
        if self.overriding_rules:
            out["overriding_rules"] = [
                r.to_json_dict() for r in self.overriding_rules
            ]
        if self.suggested_override_rule:
            out[
                "suggested_override_rule"
            ] = self.suggested_override_rule.to_json_dict()
        if self.public_source_mirrors:
            out["public_source_mirrors"] = self.public_source_mirrors

        out.update(
            {
                "confidence": self.confidence,
                "start_line": self.start_line,
                "end_line": self.end_line,
                "snippet_checksum": self.snippet_checksum,
                "snippet_text": self.snippet_text,
            }
        )
        return out

    def from_json_dict(reader: DictReader) -> "IdentifiedSnippet":
        suggested_override_rule = None
        if reader.exists("suggested_override_rule"):
            suggested_override_rule = ConditionOverrideRule.from_json_dict(
                reader.get_reader("suggested_override_rule"), reader.location
            )

        overriding_rules = None
        if reader.exists("overriding_rules"):
            overriding_rules = [
                ConditionOverrideRule.from_json_dict(r, reader.location)
                for r in reader.get_readers_list("overriding_rules")
            ]

        return IdentifiedSnippet(
            identified_as=reader.get("identified_as"),
            conditions=reader.get_string_set("conditions"),
            verified=reader.get_or("verified", default=False),
            verification_message=reader.get_or(
                "verification_message", default=None
            ),
            verified_conditions=reader.get_string_set("verified_conditions"),
            overriden_conditions=set(
                reader.get_string_list("overriden_conditions")
            ),
            public_source_mirrors=reader.get_or(
                "public_source_mirrors", default=None, expected_type=list
            ),
            conditions_unmatched_by_overriding_rules=reader.get_string_set(
                "conditions_unmatched_by_overriding_rules"
            ),
            dependents_unmatched_by_overriding_rules=reader.get_string_set(
                "dependents_unmatched_by_overriding_rules"
            ),
            overriding_rules=overriding_rules,
            suggested_override_rule=suggested_override_rule,
            confidence=reader.get("confidence", expected_type=float),
            start_line=reader.get("start_line", expected_type=int),
            end_line=reader.get("end_line", expected_type=int),
            snippet_checksum=reader.get("snippet_checksum"),
            snippet_text=reader.get("snippet_text"),
        )

    def number_of_lines(self):
        return self.end_line - self.start_line + 1

    def add_snippet_text(self, lines: list[str]):
        text = "\n".join(lines[self.start_line - 1 : self.end_line])
        checksum = md5(text.encode("utf-8")).hexdigest()
        return dataclasses.replace(
            self, snippet_text=text, snippet_checksum=checksum
        )

    def override_conditions(
        self,
        license: "LicenseClassification",
        rules: list["ConditionOverrideRule"],
    ):
        all_matching_rules = []

        new_conditions = set()
        public_source_mirrors = set()

        remaining_conditions = set(self.conditions)
        remaining_dependents = set(license.dependents)
        for rule in rules:
            # Check that the in optimization in LicenseClassification was applied
            assert rule.match_license_names.matches(license.name)

            # Match identification, checksome, condition, dependents
            if not rule.match_identifications.matches(self.identified_as):
                continue
            if not rule.match_snippet_checksums.matches(self.snippet_checksum):
                continue
            if not rule.match_conditions.matches_any(self.conditions):
                continue
            if not rule.match_dependents.matches_any(license.dependents):
                continue

            # Matched!
            all_matching_rules.append(rule)
            remaining_conditions.difference_update(
                rule.match_conditions.get_matches(self.conditions)
            )
            remaining_dependents.difference_update(
                rule.match_dependents.get_matches(license.dependents)
            )
            new_conditions.add(rule.override_condition_to)
            if rule.public_source_mirrors:
                public_source_mirrors.update(rule.public_source_mirrors)

        if all_matching_rules:
            return dataclasses.replace(
                self,
                overriden_conditions=set(new_conditions),
                conditions_unmatched_by_overriding_rules=remaining_conditions,
                dependents_unmatched_by_overriding_rules=remaining_dependents,
                overriding_rules=all_matching_rules,
                public_source_mirrors=sorted(list(public_source_mirrors)),
            )
        else:
            return self

    def _format_conditions(self, conditions: set[str]) -> str:
        assert conditions, f"{conditions} cannot be empty"
        l = list(conditions)
        if len(l) == 1:
            return f"'{l[0]}'"
        return str(sorted(l))

    def verify_conditions(
        self, license: "LicenseClassification", allowed_conditions: set[str]
    ):
        """Sets the 'verified' and 'verification_message' fields"""
        verified = True
        message = None
        disallowed_conditions = self.conditions.difference(allowed_conditions)
        disallowed_overriden_conditions = self.overriden_conditions.difference(
            allowed_conditions
        )
        disallowed_remaining_conditions = (
            self.conditions_unmatched_by_overriding_rules.difference(
                allowed_conditions
            )
        )

        if not self.overriding_rules:
            # Simple case: No overriding rules were involved.
            if disallowed_conditions:
                verified = False
                message = f"{self._format_conditions(disallowed_conditions)} condition is not an allowed."
        elif disallowed_remaining_conditions:
            verified = False
            rule_paths = [r.rule_file_path for r in self.overriding_rules]
            message = (
                f"The condition {self._format_conditions(disallowed_remaining_conditions)} is not allowed and"
                f" was not matched by any of these rules: {rule_paths}"
            )
        elif disallowed_overriden_conditions:
            # Some overriding rules were involved: Check their overriding conditions.
            rule_paths = [
                r.rule_file_path
                for r in self.overriding_rules
                if r.override_condition_to in disallowed_overriden_conditions
            ]
            verified = False
            message = (
                f"The condition {self._format_conditions(disallowed_overriden_conditions)} is not allowed."
                f" They were introduced by these rules: {rule_paths}."
            )
        elif self.dependents_unmatched_by_overriding_rules:
            # Some license dependents didn't match any rule. Check the original
            # conditions.
            if disallowed_conditions:
                rule_paths = [r.rule_file_path for r in self.overriding_rules]
                verified = False
                message = (
                    f"The overriding rules {rule_paths} changed the conditions to "
                    f"{self._format_conditions(self.overriden_conditions)} but the rules don't match the dependencies "
                    f"{self.dependents_unmatched_by_overriding_rules} that remain with the "
                    f"condition {self._format_conditions(disallowed_conditions)} which is not allowed."
                )

        if verified:
            assert message == None
            suggested_override_rule = None
            verified_conditions = self.conditions
            if self.overriding_rules:
                verified_conditions = self.overriden_conditions.union(
                    self.conditions_unmatched_by_overriding_rules
                )
            assert not verified_conditions.difference(
                allowed_conditions
            ), f"Verified conditions have disallowed conditions"
        else:
            assert message != None
            suggested_override_rule = (
                ConditionOverrideRule.suggested_for_snippet(
                    license, self, allowed_conditions
                )
            )
            verified_conditions = set()

        return dataclasses.replace(
            self,
            verified=verified,
            verification_message=message,
            verified_conditions=verified_conditions,
            suggested_override_rule=suggested_override_rule,
        )

    def detailed_verification_message(
        self, license: "LicenseClassification"
    ) -> str:
        """Returns a very detailed verification failure message or None"""

        if self.verified:
            return None

        dependents_str = "\n".join([f"  {d}" for d in license.dependents])
        license_links = "\n".join([f"  {l}" for l in license.links])
        snippet = self.snippet_text
        max_snippet_length = 1000
        if len(snippet) > max_snippet_length:
            snippet = snippet[0:max_snippet_length] + "<TRUNCATED>"

        message = f"""
License '{license.name}' has a snippet identified as '{self.identified_as}' with conditions {self.conditions}.

Verification message:
{self.verification_message}

License links:
{license_links}

The license is depended on by:
{dependents_str}

Snippet begin line: {self.start_line}
Snippet end line: {self.end_line}
Snippet checksum: {self.snippet_checksum}
Snippet: <begin>
{snippet}
<end>
SPDX License Ref: {license.license_id}

To fix this verification problem you should either:
1. Remove the dependency on projects with this license in the dependent code bases.
2. If the dependency is required and approved by the legal council of your project,
   you apply a local condition override, such as:
{json.dumps(self.suggested_override_rule.to_json_dict(), indent=4)}
"""
        return message


@dataclasses.dataclass(frozen=True)
class LicenseClassification:
    """Classification results for a single license"""

    license_id: str
    identifications: list[IdentifiedSnippet]
    name: str = None
    links: list[str] = None
    dependents: list[str] = None

    # Whether the project is shipped.
    is_project_shipped: bool = None
    # Whether notice is shipped.
    is_notice_shipped: bool = None
    # Whether source code is shipped.
    is_source_code_shipped: bool = None

    # license size & identification stats
    size_bytes: int = None
    size_lines: int = None
    unidentified_lines: int = None

    def to_json_dict(self):
        return {
            "license_id": self.license_id,
            "name": self.name,
            "links": self.links,
            "dependents": self.dependents,
            "is_project_shipped": self.is_project_shipped,
            "is_notice_shipped": self.is_notice_shipped,
            "is_source_code_shipped": self.is_source_code_shipped,
            "identifications": [m.to_json_dict() for m in self.identifications],
            "identification_stats": {
                "size_bytes": self.size_bytes,
                "size_lines": self.size_lines,
                "unidentified_lines": self.unidentified_lines,
            },
        }

    def from_json_dict(reader: DictReader) -> "LicenseClassification":
        identifications = [
            IdentifiedSnippet.from_json_dict(r)
            for r in reader.get_readers_list("identifications")
        ]
        stats_reader = reader.get_reader("identification_stats")

        return LicenseClassification(
            license_id=reader.get("license_id"),
            name=reader.get("name"),
            links=reader.get_string_list("links"),
            dependents=reader.get_string_list("dependents"),
            is_project_shipped=reader.get_or(
                "is_project_shipped", default=None, expected_type=bool
            ),
            is_notice_shipped=reader.get_or(
                "is_notice_shipped", default=None, expected_type=bool
            ),
            is_source_code_shipped=reader.get_or(
                "is_source_code_shipped", default=None, expected_type=bool
            ),
            identifications=identifications,
            size_bytes=stats_reader.get_or(
                "size_bytes", default=None, expected_type=int, accept_none=True
            ),
            size_lines=stats_reader.get_or(
                "size_lines", default=None, expected_type=int, accept_none=True
            ),
            unidentified_lines=stats_reader.get_or(
                "unidentified_lines",
                default=None,
                expected_type=int,
                accept_none=True,
            ),
        )

    def add_license_information(
        self, index: SpdxIndex
    ) -> "LicenseClassification":
        spdx_license = index.get_license_by_id(self.license_id)
        snippet_lines = spdx_license.extracted_text_lines()
        identifications = [
            i.add_snippet_text(snippet_lines) for i in self.identifications
        ]
        links = []
        if spdx_license.cross_refs:
            links.extend(spdx_license.cross_refs)
        if spdx_license.see_also:
            links.extend(spdx_license.see_also)
        chains = index.dependency_chains_for_license(spdx_license)
        dependents = [">".join([p.name for p in chain]) for chain in chains]
        # Sort and dedup dependent chains: There might be duplicate chains since
        # the package names are not globally unique.
        dependents = sorted(set(dependents))
        return dataclasses.replace(
            self,
            identifications=identifications,
            name=spdx_license.name,
            links=links,
            dependents=dependents,
        )

    def set_is_shipped_defaults(
        self,
        default_is_project_shipped,
        default_is_notice_shipped,
        default_is_source_code_shipped,
    ) -> "LicenseClassification":
        def default_if_none(value, default):
            if value == None:
                return default
            else:
                return value

        return dataclasses.replace(
            self,
            is_project_shipped=default_if_none(
                self.is_project_shipped, default_is_project_shipped
            ),
            is_notice_shipped=default_if_none(
                self.is_notice_shipped, default_is_notice_shipped
            ),
            is_source_code_shipped=default_if_none(
                self.is_source_code_shipped, default_is_source_code_shipped
            ),
        )

    def compute_identification_stats(self, index: SpdxIndex):
        spdx_license = index.get_license_by_id(self.license_id)

        extracted_text = spdx_license.extracted_text
        extracted_lines = spdx_license.extracted_text_lines()

        lines_identified = 0
        for identification in self.identifications:
            lines_identified += identification.number_of_lines()

        return dataclasses.replace(
            self,
            size_bytes=len(extracted_text),
            size_lines=len(extracted_lines),
            unidentified_lines=len(extracted_lines) - lines_identified,
        )

    def _transform_identifications(
        self, function: Callable[[IdentifiedSnippet], IdentifiedSnippet]
    ) -> "LicenseClassification":
        """Returns a copy of this object with the identifications transformed by function"""
        return dataclasses.replace(
            self, identifications=[function(i) for i in self.identifications]
        )

    def override_conditions(self, rule_set: "ConditionOverrideRuleSet"):
        # Optimize by filtering rules that match the license name and any dependents
        relevant_rules = []
        for rule in rule_set.rules:
            if rule.match_license_names.matches(self.name):
                if rule.match_dependents.matches_any(self.dependents):
                    relevant_rules.append(rule)

        if relevant_rules:
            return self._transform_identifications(
                lambda x: x.override_conditions(self, relevant_rules)
            )
        else:
            return self

    def verify_conditions(self, allowed_conditions: set[str]):
        return self._transform_identifications(
            lambda x: x.verify_conditions(self, allowed_conditions)
        )

    def verification_errors(self) -> list[str]:
        out = []
        for i in self.identifications:
            msg = i.detailed_verification_message(self)
            if msg:
                out.append(msg)
        return out

    def all_public_source_mirrors(self) -> list[str]:
        out = []
        for i in self.identifications:
            if i.public_source_mirrors:
                out.extend(i.public_source_mirrors)
        return sorted(list(set(out)))

    def determine_is_notice_shipped(
        self, conditions_requiring_shipped_notice: list[str]
    ) -> "LicenseClassification":
        is_shipped = False
        for i in self.identifications:
            conditions = i.verified_conditions if i.verified else i.conditions
            if conditions.intersection(conditions_requiring_shipped_notice):
                is_shipped = True
                break
        return dataclasses.replace(self, is_notice_shipped=is_shipped)


@dataclasses.dataclass(frozen=True)
class LicensesClassifications:
    classifications_by_id: dict[str, LicenseClassification]

    def create_empty() -> "LicenseClassification":
        return LicensesClassifications(classifications_by_id={})

    def from_identify_license_output_json(
        identify_license_output_path: str,
        license_paths_by_license_id: dict[str, str],
    ) -> "LicensesClassifications":
        import os

        json_output = json.load(open(identify_license_output_path, "r"))

        # Expected results from https://github.com/google/licenseclassifier/tree/main/tools/identify_license
        # have the following json layout:
        # [
        #     {
        #         "Filepath": ...
        #         "Classifications: [
        #             {
        #                 "Name": ...
        #                 "Confidence": int or float
        #                 "StartLine": int
        #                 "EndLine": int
        #                 "Condition": str
        #             },
        #             { ...},
        #             ...
        #         ]
        #     },
        #     { ... },
        #     ...
        # ]

        results_by_file_path = {}
        for one_output in json_output:
            file_name = one_output["Filepath"]
            assert file_name not in results_by_file_path
            results_by_file_path[file_name] = one_output["Classifications"]

        identifications_by_license_id = defaultdict(list)
        for license_id, file_name in license_paths_by_license_id.items():
            found_any = False
            if file_name in results_by_file_path.keys():
                for match_json in results_by_file_path[file_name]:
                    identified_snippet = (
                        IdentifiedSnippet.from_identify_license_dict(
                            dictionary=match_json,
                            location=identify_license_output_path,
                        )
                    )
                    identifications_by_license_id[license_id].append(
                        identified_snippet
                    )
                    found_any = True

            if not found_any:
                # b/494274295: Under heavy parallel load in CQ, the identify_license
                # tool can flakily hit its internal 1-second wall-time timeout for
                # some files and return zero classifications.
                #
                # To prevent these flakes from crashing the build, we fall back to
                # treating the entire file as a single [UNIDENTIFIED] placeholder
                # snippet. This allows the license to be resolved via policy override
                # rules (or fail standard verification if no override exists).
                num_lines = 1
                if os.path.exists(file_name):
                    with open(file_name, "r") as f:
                        num_lines = len(f.readlines())

                placeholder_snippet = IdentifiedSnippet(
                    identified_as=IdentifiedSnippet.UNIDENTIFIED_IDENTIFICATION,
                    confidence=0.0,
                    start_line=1,
                    end_line=max(1, num_lines),
                    conditions=set(["unidentified"]),
                )
                identifications_by_license_id[license_id].append(
                    placeholder_snippet
                )

        license_classifications = {}
        for (
            license_id,
            identifications,
        ) in identifications_by_license_id.items():
            license_classifications[license_id] = LicenseClassification(
                license_id=license_id, identifications=identifications
            )

        return LicensesClassifications(license_classifications)

    def to_json_list(self) -> list[Any]:
        output = []
        for license_id in sorted(self.classifications_by_id.keys()):
            output.append(self.classifications_by_id[license_id].to_json_dict())
        return output

    def to_json(self, json_file_path: str):
        with open(json_file_path, "w") as output_file:
            json.dump(self.to_json_list(), output_file, indent=4)

    def from_json_list(
        input: list[Any], location: str
    ) -> "LicensesClassifications":
        if not isinstance(input, list):
            raise LicenseException(
                f"Expected a list of classification json values, but got {type(input)}",
                location,
            )
        classifications_by_id = {}
        for value in input:
            if not isinstance(value, dict):
                raise LicenseException(
                    f"Expected json dict but got {type(input)}", location
                )
            value_reader = DictReader(value, location)
            classification = LicenseClassification.from_json_dict(value_reader)
            if classification.license_id in classifications_by_id:
                raise LicenseException(
                    f"Multiple classifications with license_id '{classification.license_id}'",
                    location,
                )
            classifications_by_id[classification.license_id] = classification

        return LicensesClassifications(classifications_by_id)

    def from_json(json_file_path: str) -> "LicensesClassifications":
        with open(json_file_path, "r") as f:
            try:
                json_obj = json.load(f)
            except json.decoder.JSONDecodeError as e:
                raise LicenseException(
                    f"Failed to parse json: {e}", json_file_path
                )
            return LicensesClassifications.from_json_list(
                json_obj, json_file_path
            )

    def _transform_each_classification(
        self, function: Callable[[LicenseClassification], LicenseClassification]
    ) -> "LicensesClassifications":
        """Returns a copy of this object with the classifications transformed by function"""
        new = self.classifications_by_id.copy()
        for k, v in new.items():
            new[k] = function(v)
        return dataclasses.replace(self, classifications_by_id=new)

    def _transform_each_identification(
        self, function: Callable[[IdentifiedSnippet], IdentifiedSnippet]
    ) -> "LicensesClassifications":
        """Returns a copy of this object with the classifications' identified snippets transformed by function"""
        return self._transform_each_classification(
            lambda x: x._transform_identifications(function)
        )

    def set_default_condition(
        self, default_condition: str
    ) -> "LicensesClassifications":
        return self._transform_each_identification(
            lambda x: x.set_condition(default_condition)
        )

    def set_is_shipped_defaults(
        self,
        is_project_shipped: bool,
        is_notice_shipped: bool,
        is_source_code_shipped: bool,
    ) -> "LicensesClassifications":
        return self._transform_each_classification(
            lambda x: x.set_is_shipped_defaults(
                is_project_shipped, is_notice_shipped, is_source_code_shipped
            )
        )

    def add_classifications(
        self, to_add: list[LicenseClassification]
    ) -> "LicensesClassifications":
        new = self.classifications_by_id.copy()
        for license_classification in to_add:
            license_id = license_classification.license_id
            assert license_id not in new, f"{license_id} already exists"
            new[license_id] = license_classification
        return dataclasses.replace(self, classifications_by_id=new)

    def replace_classifications(
        self, to_replace: list[LicenseClassification]
    ) -> "LicensesClassifications":
        new = self.classifications_by_id.copy()
        for license_classification in to_replace:
            new[license_classification.license_id] = license_classification
        return dataclasses.replace(self, classifications_by_id=new)

    @staticmethod
    def select_majority_identifications(
        runs: list[list[IdentifiedSnippet]],
    ) -> tuple[list[IdentifiedSnippet], int]:
        """Selects the majority identification result across multiple runs.

        Returns a tuple of (winning_snippets, vote_count).
        In case of a tie (e.g. 1-1-1 across 3 runs), prefers runs without
        [UNIDENTIFIED] snippets, then higher total confidence.
        """
        assert runs, "runs must not be empty"

        def snippet_key(s: IdentifiedSnippet) -> tuple[Any, ...]:
            return (
                s.identified_as,
                s.confidence,
                s.start_line,
                s.end_line,
                tuple(sorted(s.conditions)),
            )

        def run_key(snippets: list[IdentifiedSnippet]) -> tuple[Any, ...]:
            return tuple(snippet_key(s) for s in snippets)

        counts = Counter(run_key(r) for r in runs)
        snippets_by_key = {}
        for r in runs:
            k = run_key(r)
            if k not in snippets_by_key:
                snippets_by_key[k] = r

        def rank_key(k: tuple[Any, ...]) -> tuple[int, int, float]:
            vote_count = counts[k]
            has_unidentified = any(
                s[0] == IdentifiedSnippet.UNIDENTIFIED_IDENTIFICATION for s in k
            )
            non_unidentified_score = 0 if has_unidentified else 1
            total_confidence = sum(float(s[1]) for s in k)
            return (vote_count, non_unidentified_score, total_confidence)

        best_key = max(snippets_by_key.keys(), key=rank_key)
        return snippets_by_key[best_key], counts[best_key]

    def add_licenses_information(self, spdx_index: SpdxIndex):
        return self._transform_each_classification(
            lambda x: x.add_license_information(spdx_index)
        )

    def compute_identification_stats(self, spdx_index: SpdxIndex):
        return self._transform_each_classification(
            lambda x: x.compute_identification_stats(spdx_index)
        )

    def override_conditions(
        self, rule_set: "ConditionOverrideRuleSet"
    ) -> "LicensesClassifications":
        return self._transform_each_classification(
            lambda x: x.override_conditions(rule_set)
        )

    def verify_conditions(
        self, allowed_conditions: set[str]
    ) -> "LicensesClassifications":
        return self._transform_each_classification(
            lambda x: x.verify_conditions(allowed_conditions)
        )

    def verification_errors(self):
        error_messages = []
        for c in self.classifications_by_id.values():
            error_messages.extend(c.verification_errors())
        return error_messages

    def identifications_count(self):
        c = 0
        for v in self.classifications_by_id.values():
            c += len(v.identifications)
        return c

    def failed_verifications_count(self):
        c = 0
        for v in self.classifications_by_id.values():
            for i in v.identifications:
                if not i.verified:
                    c += 1
        return c

    def licenses_count(self):
        return len(self.classifications_by_id)

    def license_ids(self):
        return self.classifications_by_id.keys()

    def determine_is_notice_shipped(
        self, conditions_requiring_shipped_notice: list[str]
    ):
        return self._transform_each_classification(
            lambda x: x.determine_is_notice_shipped(
                conditions_requiring_shipped_notice
            )
        )


@dataclasses.dataclass(frozen=True)
class AsterixStringExpression:
    """Utility for partial string matching (asterix matches)"""

    starts_with_asterix: bool
    ends_with_asterix: bool
    parts: list[str]

    def create(expression: str) -> "AsterixStringExpression":
        return AsterixStringExpression(
            starts_with_asterix=expression.startswith("*"),
            ends_with_asterix=expression.endswith("*"),
            parts=[p for p in expression.split("*") if p],
        )

    def matches(self, value) -> bool:
        if not self.parts:
            return True
        offset = 0

        if not self.starts_with_asterix and not value.startswith(self.parts[0]):
            return False

        for part in self.parts:
            # Uses rfind (right-most find) instead of find to make * greedy.
            next_match = value.rfind(part, offset)
            if next_match == -1:
                return False
            offset = next_match + len(part)

        return offset == len(value) or self.ends_with_asterix


@dataclasses.dataclass(frozen=True)
class StringMatcher:
    """
    A utility to perform override rule string matching.

    Supports exact and * matches.
    """

    all_expressions: list[str]

    exact_expressions: set[str]
    asterix_expressions: list[AsterixStringExpression]

    def create(expressions: list[str]) -> "StringMatcher":
        assert isinstance(expressions, list)
        exact_expressions = set()
        asterix_expressions = []
        for e in expressions:
            assert isinstance(e, str)
            if "*" in e:
                asterix_expressions.append(AsterixStringExpression.create(e))
            else:
                exact_expressions.add(e)

        return StringMatcher(
            all_expressions=expressions,
            exact_expressions=exact_expressions,
            asterix_expressions=asterix_expressions,
        )

    def create_match_everything() -> "StringMatcher":
        return StringMatcher.create(["*"])

    def to_json(self) -> Any:
        return self.all_expressions

    def matches(self, input: str) -> bool:
        if input in self.exact_expressions:
            return True
        for asterix_expression in self.asterix_expressions:
            if asterix_expression.matches(input):
                return True
        return False

    def get_matches(self, inputs: list[str]) -> list[str]:
        """
        Matches all the inputs against the internal expressions.

        Returns the ones that match or an empty list if none matched.
        """
        return [i for i in inputs if self.matches(i)]

    def matches_any(self, inputs: Collection[str]) -> bool:
        """
        Matches all the inputs against the internal expressions.

        Returns true if any inputs where matched.
        """
        if not self.all_expressions or not inputs:
            return False
        for input in inputs:
            if self.matches(input):
                return True
        return False

    def matches_all(self, inputs: list[str]) -> bool:
        """
        Matches all the inputs against the internal expressions.

        Returns true if all inputs where matched.
        """
        if not self.all_expressions or not inputs:
            return False

        for input in inputs:
            if not self.matches(input):
                return False
        return True


@dataclasses.dataclass(frozen=True)
class ConditionOverrideRule:
    """Rule for overriding a classified license condition"""

    # Path to the condition override rule.
    rule_file_path: str
    # Will override the condition to this condition
    override_condition_to: str
    # Optional public source mirroring urls.
    public_source_mirrors: list[str]
    # Issue tracker URL.
    bug: str
    # Email subject line containing counsel approval.
    email_subject_line: str
    # List facilitates easier to read multi-line comments in JSON.
    comment: list[str]

    # matching
    match_license_names: StringMatcher
    match_identifications: StringMatcher
    match_conditions: StringMatcher
    match_dependents: StringMatcher
    match_snippet_checksums: StringMatcher

    def from_json_dict(dictionary, rule_file_path) -> "ConditionOverrideRule":
        if isinstance(dictionary, DictReader):
            reader = dictionary
        else:
            reader = DictReader(dictionary=dictionary, location=rule_file_path)

        override_condition_to = reader.get("override_condition_to")
        bug = reader.get("bug")
        if not bug:
            raise LicenseException(
                "'bug' fields cannot be empty", rule_file_path
            )
        email_subject_line = reader.get_or(
            "email_subject_line", expected_type=str, default=""
        )
        comment = reader.get_string_list("comment")

        def verify_list_not_empty(list_value) -> str:
            if not list_value:
                return "list is empty"
            for v in list_value:
                if not v:
                    return "empty value in list"
            return None

        criteria_reader = reader.get_reader("match_criteria")

        def read_required_matcher_field(name) -> StringMatcher:
            value = criteria_reader.get(
                name, expected_type=list, verify=verify_list_not_empty
            )
            return StringMatcher.create(value)

        match_license_names = read_required_matcher_field("license_names")
        match_conditions = read_required_matcher_field("conditions")
        match_dependents = read_required_matcher_field("dependents")
        match_identifications = read_required_matcher_field("identifications")

        # Checksum matching is optional except for unidentified snippets.
        match_snippet_checksums = criteria_reader.get_or(
            "snippet_checksums", expected_type=list, default=None
        )

        if match_snippet_checksums == None:
            match_snippet_checksums = StringMatcher.create_match_everything()
        else:
            match_snippet_checksums = StringMatcher.create(
                match_snippet_checksums
            )

        # If there is a rule_file_path value in the dict, use it instead.
        rule_file_path = reader.get_or("rule_file_path", default=rule_file_path)
        public_source_mirrors = reader.get_or(
            "public_source_mirrors", default=None, expected_type=list
        )

        return ConditionOverrideRule(
            rule_file_path=rule_file_path,
            override_condition_to=override_condition_to,
            public_source_mirrors=public_source_mirrors,
            bug=bug,
            email_subject_line=email_subject_line,
            comment=comment,
            match_license_names=match_license_names,
            match_identifications=match_identifications,
            match_conditions=match_conditions,
            match_dependents=match_dependents,
            match_snippet_checksums=match_snippet_checksums,
        )

    def to_json_dict(self):
        # Fields are output in a certain order for better readability
        out = {}
        if self.rule_file_path:
            out["rule_file_path"] = self.rule_file_path

        if self.public_source_mirrors:
            out["public_source_mirrors"] = self.public_source_mirrors

        out.update(
            {
                "override_condition_to": self.override_condition_to,
                "bug": self.bug,
                "email_subject_line": self.email_subject_line,
                "comment": self.comment,
                "match_criteria": {
                    "license_names": self.match_license_names.to_json(),
                    "identifications": self.match_identifications.to_json(),
                    "conditions": self.match_conditions.to_json(),
                    "snippet_checksums": self.match_snippet_checksums.to_json(),
                    "dependents": self.match_dependents.to_json(),
                },
            }
        )
        return out

    def suggested_for_snippet(
        license: LicenseClassification,
        snippet: IdentifiedSnippet,
        allowed_conditions: set[str],
    ) -> "ConditionOverrideRule":
        """Creates an override rule suggestion for the given license snippet"""
        dependents = license.dependents
        if snippet.dependents_unmatched_by_overriding_rules:
            dependents = snippet.dependents_unmatched_by_overriding_rules
        conditions = snippet.conditions
        if snippet.conditions_unmatched_by_overriding_rules:
            conditions = snippet.conditions_unmatched_by_overriding_rules
        return ConditionOverrideRule(
            rule_file_path=None,
            override_condition_to="<CHOOSE ONE OF "
            + ", ".join([f"'{c}'" for c in allowed_conditions])
            + ">",
            public_source_mirrors=None,
            bug="<INSERT TICKET URL>",
            email_subject_line="<INSERT EMAIL SUBJECT LINE FOR COUNSEL APPROVAL, IF APPLICABLE>",
            comment=["<INSERT DOCUMENTATION FOR OVERRIDE RULE>"],
            match_license_names=StringMatcher.create([license.name]),
            match_snippet_checksums=StringMatcher.create(
                [snippet.snippet_checksum]
            ),
            match_identifications=StringMatcher.create([snippet.identified_as]),
            match_conditions=StringMatcher.create(
                list(conditions.difference(allowed_conditions))
            ),
            match_dependents=StringMatcher.create(list(dependents)),
        )


@dataclasses.dataclass(frozen=True)
class ConditionOverrideRuleSet:
    rules: list[ConditionOverrideRule]

    def merge(
        self, other: "ConditionOverrideRuleSet"
    ) -> "ConditionOverrideRuleSet":
        new = list(self.rules)
        new.extend(other.rules)
        return dataclasses.replace(self, rules=new)

    def from_json(file_path: str) -> "ConditionOverrideRuleSet":
        with open(file_path, "r") as f:
            try:
                json_obj = json.load(f)
            except json.decoder.JSONDecodeError as e:
                raise LicenseException(f"Failed to parse json: {e}", file_path)

            if not isinstance(json_obj, list) and not isinstance(
                json_obj, dict
            ):
                raise LicenseException(
                    f"Expected List[dict] or dict at top-level json but found {type(json_obj)}",
                    file_path,
                )

            if isinstance(json_obj, dict):
                json_obj = [json_obj]

            rules = []
            for child_json in json_obj:
                if not isinstance(child_json, dict):
                    raise LicenseException(
                        f"Expected dict but found {type(child_json)}", file_path
                    )
                rules.append(
                    ConditionOverrideRule.from_json_dict(
                        DictReader(child_json, file_path),
                        rule_file_path=file_path,
                    )
                )

            return ConditionOverrideRuleSet(rules)

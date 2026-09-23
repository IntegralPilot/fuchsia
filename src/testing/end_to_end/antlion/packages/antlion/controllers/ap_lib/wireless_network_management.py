#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from typing import NewType

from antlion.controllers.ap_lib.radio_measurement import NeighborReportElement

BssTransitionCandidateList = NewType(
    "BssTransitionCandidateList", list[NeighborReportElement]
)


class BssTerminationDuration:
    """Representation of BSS Termination Duration subelement.

    See IEEE 802.11-2020 Figure 9-341.
    """

    def __init__(self, duration: int):
        """Create a BSS Termination Duration subelement.

        Args:
            duration: number of minutes the BSS will be offline.
        """
        # Note: hostapd does not currently support setting BSS Termination TSF,
        # which is the other value held in this subelement.
        self._duration = duration

    @property
    def duration(self) -> int:
        return self._duration


class BssTransitionManagementRequest:
    """Representation of BSS Transition Management request.

    See IEEE 802.11-2020 9.6.13.9.
    """

    def __init__(
        self,
        preferred_candidate_list_included: bool = False,
        abridged: bool = False,
        disassociation_imminent: bool = False,
        ess_disassociation_imminent: bool = False,
        disassociation_timer: int = 0,
        validity_interval: int = 1,
        bss_termination_duration: BssTerminationDuration | None = None,
        session_information_url: str | None = None,
        candidate_list: BssTransitionCandidateList | None = None,
    ):
        """Create a BSS Transition Management request.

        Args:
            preferred_candidate_list_included: whether the candidate list is a
                preferred candidate list, or (if False) a list of known
                candidates.
            abridged: whether a preference value of 0 is assigned to all BSSIDs
                that do not appear in the candidate list, or (if False) AP has
                no recommendation for/against anything not in the candidate
                list.
            disassociation_imminent: whether the STA is about to be
                disassociated by the AP.
            ess_disassociation_imminent: whether the STA will be disassociated
                from the ESS.
            disassociation_timer: the number of beacon transmission times
                (TBTTs) until the AP disassociates this STA (default 0, meaning
                AP has not determined when it will disassociate this STA).
            validity_interval: number of TBTTs until the candidate list is no
                longer valid (default 1).
            bss_termination_duration: BSS Termination Duration subelement.
            session_information_url: this URL is included if ESS disassociation
                is immiment.
            candidate_list: zero or more neighbor report elements.
        """
        # Request mode field, see IEEE 802.11-2020 Figure 9-924.
        self._preferred_candidate_list_included = (
            preferred_candidate_list_included
        )
        self._abridged = abridged
        self._disassociation_imminent = disassociation_imminent
        self._ess_disassociation_imminent = ess_disassociation_imminent

        # Disassociation Timer, see IEEE 802.11-2020 Figure 9-925
        self._disassociation_timer = disassociation_timer

        # Validity Interval, see IEEE 802.11-2020 9.6.13.9
        self._validity_interval = validity_interval

        # BSS Termination Duration, see IEEE 802.11-2020 9.6.13.9 and Figure 9-341
        self._bss_termination_duration = bss_termination_duration

        # Session Information URL, see IEEE 802.11-2020 Figure 9-926
        self._session_information_url = session_information_url

        # BSS Transition Candidate List Entries, IEEE 802.11-2020 9.6.13.9.
        self._candidate_list = candidate_list

    @property
    def preferred_candidate_list_included(self) -> bool:
        return self._preferred_candidate_list_included

    @property
    def abridged(self) -> bool:
        return self._abridged

    @property
    def disassociation_imminent(self) -> bool:
        return self._disassociation_imminent

    @property
    def bss_termination_included(self) -> bool:
        return self._bss_termination_duration is not None

    @property
    def ess_disassociation_imminent(self) -> bool:
        return self._ess_disassociation_imminent

    @property
    def disassociation_timer(self) -> int | None:
        if self.disassociation_imminent:
            return self._disassociation_timer
        # Otherwise, field is reserved.
        return None

    @property
    def validity_interval(self) -> int:
        return self._validity_interval

    @property
    def bss_termination_duration(self) -> BssTerminationDuration | None:
        return self._bss_termination_duration

    @property
    def session_information_url(self) -> str | None:
        return self._session_information_url

    @property
    def candidate_list(self) -> BssTransitionCandidateList | None:
        return self._candidate_list

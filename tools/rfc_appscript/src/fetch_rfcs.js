// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
//
// This file contains javascript which gets deployed to scripts.google.com, in
// order to power custom logic for the internal RFC tracker.

const CHANGE_ID_COL = 'Change Id';

const RFCS_DIR = 'docs/contribute/governance/rfcs';

// Translation: if the beginning of the subject is [rfc] or [rfcs], possibly
// preceded by [other][tags].
const SUBJECT_TAG_RE = /^((\S+]))?\[rfcs?]/i;

// Returns an Array of change_ids.
function _fetchOpenRfcsCls() {
  const data = parseGerritResponse(UrlFetchApp.fetch(
    GERRIT_API_URL + '/changes/?q='
    + 'dir:' + encodeURIComponent(RFCS_DIR)
    + '+is:open'
    + '&n=100'));

  const changeIds = [];

  for (const cl of data) {
    if (cl.subject.match(SUBJECT_TAG_RE)) {
      changeIds.push(cl.change_id);
    }
  }

  return changeIds;
}

// Returns {[change_id]: true, ...}
function _getExistingRfcsFromAppSheet() {
  const rows = callAppSheetAPI({
    "Action": "Find",
    "Properties": {},
  });

  const res = {};
  for (const row of rows) {
    res[row['Change ID']] = true;
  }

  return res;
}

// Gets the list of open RFC CLs from gerrit and compares that to the set of
// RFCs in the AppSheet tracker. For any CLs missing from the tracker, creates
// stub rows with just the change_id filled in.
function addNewRfcsToAppSheet() {
  const gerritCls = _fetchOpenRfcsCls();
  const trackerCls = _getExistingRfcsFromAppSheet();

  // Accumulate create requests so that requests can be batched.
  let createRows = [];
  for (const changeId of gerritCls) {
    if (!(changeId in trackerCls)) {
      createRows.push({ [CHANGE_ID_COL]: changeId });
    }
  }

  if (createRows.length != 0) {
    callAppSheetAPI({
      "Action": "Add",
      "Properties": {},
      Rows: createRows,
    });
  }
}

// Call the gerrit API and return some specific information about the named CL.
function getClInfo(changeId) {
  const cl = parseGerritResponse(UrlFetchApp.fetch(
    // include _account_id, email and username fields when referencing accounts.
    GERRIT_API_URL + `/changes/${changeId}?o=DETAILED_ACCOUNTS`
  ));

  // Remove subject prefix tags (e.g. '[rfc][docs]')
  const title = cl.subject.replace(/^(\S+]\s)/i, '');

  return {
    title,
    number: cl._number,
    author: cl.owner.email,
    created: cl.created,
    updated: cl.updated,
    submitted: cl.submitted,
    status: cl.status,
    work_in_progress: Boolean(cl.work_in_progress),
  };
}

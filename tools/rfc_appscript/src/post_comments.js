// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
//
// This file contains functions that post comments on gerrit changes for RFCs.

const LAST_CALL_COMMENT = 'This RFC is now in Last Call! Reviewers, please \
post any remaining comments in the next 7 days. Note that the RFC will only \
be accepted once all the comment threads have come to conclusion.';

const CONGRATULATIONS = [
  'Congratulations!', 'Huzzah!', 'Woot!', 'Mazel tov!', 'Felicitations!',
  'Great job!', 'Well done!', 'Nailed it!', 'Nice!', 'GG!', '👏👏👏', '🎉🎉🎉', '🙌',
  'Sweet!', 'Fantastic!'];

const ACCESS_TOKEN_URL =
  'https://iamcredentials.googleapis.com/v1/projects/-/serviceAccounts/rfcbot@fuchsia-infra.iam.gserviceaccount.com:generateAccessToken';

// Returns an OAuth token for rfcbot@fuchsia-infra.iam.gserviceaccount.com,
// which can be used to post comments to gerrit.
function getRobotOAuthToken() {
  const payload = {
    scope: ['https://www.googleapis.com/auth/gerritcodereview']
  };

  const response = UrlFetchApp.fetch(ACCESS_TOKEN_URL, {
    'method': 'post',
    'contentType': 'application/json',
    'headers': {
      // This OAuth token is for the user the script is running as.
      'Authorization': `Bearer ${ScriptApp.getOAuthToken()}`,
    },
    'payload': JSON.stringify(payload),
  });

  const parsed = JSON.parse(response);
  return parsed.accessToken;
}

// Post `message` as a top-level comment to the current revision of `changeId`.
function _postComment(changeId, message) {
  const payload = { message };

  var options = {
    'method': 'post',
    'contentType': 'application/json',
    'payload': JSON.stringify(payload)
  };

  const response = UrlFetchApp.fetch(GERRIT_API_URL
    + `/a/changes/${changeId}/revisions/current/review?access_token=`
    + getRobotOAuthToken(),
    options);

  console.log("Post comment response: " + response.getResponseCode());
}

// Post a top-level comment to the current revision of `changeId`, telling
// everyone that the RFC is now in last call.
function postLastCallComment(changeId) {
  _postComment(changeId, LAST_CALL_COMMENT);
}

// Post a top-level comment to the current revision of `changeId`, telling
// everyone that the RFC was accepted and given the designation `rfcNumber`,
// which should look like "RFC-0123".
function postAcceptedComment(changeId, rfcNumber) {
  const congrats = CONGRATULATIONS[
    Math.floor(Math.random() * CONGRATULATIONS.length)];

  _postComment(changeId, `This has been accepted as ${rfcNumber}! ${congrats}`);
}

// Post a top-level comment to the current revision of `changeId`, telling
// everyone that the RFC has been put on hold. `facilitator` should be the email
// address of the RFC's current facilitator.
function postOnHoldComment(changeId, facilitator) {
  const message = `This RFC has not been updated for multiple weeks, and is \
now On Hold. This means the RFC will not be actively facilitated by the \
Fuchsia Eng Council (FEC). If you wish to move forward with this RFC, please \
let your facilitator (${facilitator}) or the FEC (eng-council@fuchsia.dev) \
know and we'll be happy to re-engage. Thanks!`;

  _postComment(changeId, message);
}

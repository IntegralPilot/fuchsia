// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
//
// This file contains constants and functions to be used by other files in this
// directory.

const scriptProperties = PropertiesService.getScriptProperties();

const GERRIT_API_URL = 'https://fuchsia-review.googlesource.com';

// Takes a value returned by UrlFetchApp.fetch() and logs and returns a parsed
// JSON object with the result.
//
// See
// https://gerrit-review.googlesource.com/Documentation/rest-api-changes.html
// for documentation.
function parseGerritResponse(response) {
  const rawJson = response.getContentText().substring(5);
  console.log('Gerrit Response: ', rawJson);
  return JSON.parse(rawJson);
}

const APPSHEET_API_URL = 'https://api.appsheet.com/api/v2/apps/4bb7fc16-9d88-4802-a122-9c29483a1bce/tables/CLs/Action';

// Calls the AppSheet API to trigger an action on the RFCs table of the
// production instance.
//
// See this help page for docs on the format: https://support.google.com/appsheet/topic/10105767
function callAppSheetAPI(payload) {
  const response = UrlFetchApp.fetch(APPSHEET_API_URL, {
    'method': 'post',
    'contentType': 'application/json',
    'headers': {
      'ApplicationAccessKey': scriptProperties.getProperty('APPSHEET_KEY'),
    },
    'payload': JSON.stringify(payload),
  });

  return JSON.parse(response.getContentText());
}

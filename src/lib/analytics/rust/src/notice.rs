// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub const FULL_NOTICE: &str = "Welcome to Fuchsia! - https://fuchsia.dev

Fuchsia developer tools use Google Analytics to report feature usage
statistics and basic crash reports. Google may examine the collected data
in aggregate to help improve these tools, other Fuchsia tools, and the
Fuchsia SDK.

Analytics are not sent on this very first run. To disable reporting, type

    ffx config analytics disable

To display the current setting, type

    ffx config analytics show

If you opt out of analytics, an opt-out event will be sent, and then no
further information will be sent by the developer tools to Google.

By using Fuchsia developer tools, you agree to the Google Terms of Service.
Note: The Google Privacy Policy describes how data is handled in your use of
this service.

Read about the data we send:
https://fuchsia.dev/fuchsia-src/contribute/governance/policy/analytics_collected_fuchsia_tools?hl=en

See Google's privacy policy:
https://policies.google.com/privacy
";

pub const BRIEF_NOTICE: &str = "Welcome to Fuchsia!

As part of the Fuchsia developer tools, this tool uses Google Analytics to
report feature usage statistics and basic crash reports. Google may examine the
collected data in aggregate to help improve the developer tools, other
Fuchsia tools, and the Fuchsia SDK.

To disable reporting, type

    ffx config analytics disable

To display the current setting, type

    ffx config analytics show

If you opt out of analytics, an opt-out event will be sent, and then no further
information will be sent by the developer tools to Google.

Read about the data we send :
https://fuchsia.dev/fuchsia-src/contribute/governance/policy/analytics_collected_fuchsia_tools?hl=en

See Google's privacy policy:
https://policies.google.com/privacy
";

pub const GOOGLER_ENHANCED_NOTICE: &str = "
Help us improve analytics by enabling enhanced analytics!

You are identified as a Googler since your hostname is in a googler domain, c.googlers.com
or corp.google.com.

To better understand how Fuchsia tools are used, and to help improve these tools and your
workflow, Google already has an option, as you know, to collect basic, very redacted,
analytics listed in https://fuchsia.dev/fuchsia-src/contribute/governance/policy/analytics_collected_fuchsia_tools.
As a Googler, you can help us even more by opting in to enhanced analytics:

  ffx config analytics enable-enhanced

Enabling enhanced analytics may collect the following additional information,
in accordance with Google's employee privacy policy (go/employee-privacy-policy):

  go/fuchsia-internal-analytics-collection

  Before any data is sent, we will replace the value of $USER with the literal string '$USER'
and also redact $HOSTNAME in a similar way.

To collect only basic analytics, enter

  ffx config analytics enable

If you want to disable all analytics, enter

  ffx config analytics disable

To display the current setting and what is collected, enter

  ffx config analytics show

You will continue to receive this notice until you select an option.

See Google's employee privacy policy:
go/employee-privacy-policy
";

pub const SHOW_NOTICE_TEMPLATE: &str = "
Analytics collection status is currently set to {status} for Fuchsia developer
tools, including

fx, ffx (and all its subtools), foxtrot, zxdb, fidlcat, symbolizer, and the Fuchsia extension for VS Code

To enable enhanced analytics for all these tools (Note: this only works for Googlers), type
  ffx config analytics enable-enhanced
To collect basic analytics only, enter
  ffx config analytics enable
If you want to disable all analytics, enter
  ffx config analytics disable

When enabled, a random unique user ID (UUID) will be created for the current
user and it is used to collect some anonymized analytics of the session and user
workflow in order to improve the user experience. To see what is collected for basic analytics:
  https://fuchsia.dev/fuchsia-src/contribute/governance/policy/analytics_collected_fuchsia_tools.

When enhanced analytics is enabled, the following data may also be collected:

  go/fuchsia-internal-analytics-collection

When analytics is disabled, any existing UUID is deleted, and a new
UUID will be created if analytics is later re-enabled.
";

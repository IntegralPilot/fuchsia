<!-- Generated with `fx rfc` -->

<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0243" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}

<!-- mdformat on -->

## Summary

A device that is connected to a wireless network where there are multiple access
points will gain the ability to roam between them. "Roam" means the client
device moves its connection from one access point (AP) to another AP within the
same wireless network, without incurring a full disconnection during the
transition. A device may roam for many reasons: perhaps the device has moved
farther from its current AP and closer to another AP; or perhaps a device's
connection to its current AP has degraded in some way (e.g. high error rate). In
cases like these, a device may attempt to roam to another AP to avoid incurring
a full disconnection. This document addresses the API changes and logic
necessary to support Fullmac-initiated and WLAN Policy-initiated roaming.

## Motivation

Maintaining WLAN connectivity sometimes requires that a device uses a different
AP from the one it is currently using. Most commonly, this is because:

*   the current AP is no longer reachable
*   the connection to the current AP has degraded in some way

Roaming allows a device to move to a new AP with minimal disruption, using
802.11 reassociation. Without roaming, a device has to disconnect (or learn that
it has been disconnected) from the original AP, tear down all its networking
connections, and begin a new connection to an AP.

Contrast this with the situation when roaming is supported: a device can select
a new AP while still connected to the original AP, and perform the
reassociation. Reassociation is observed to take approximately 70 ms, while a
disconnect followed by a reconnect is observed to take approximately 130 ms.
While reassociation isn't a panacea, other features that further reduce the
disruption require reassociation support. Once the plumbing for roaming is
present (i.e. this design), a slew of further roaming enhancements will become
possible with future work, such as:

*   **Roam before an interruption happens**: With BSS Transition Management
    (IEEE 802.11-2020 4.3.19.3), an AP can notify a device that it should roam
    before an upcoming interruption.
*   **Reduce duration of roaming disruption, sometimes to zero**: With Fast BSS
    Transition (IEEE 802.11-2020 4.5.4.8), a device can perform most of the
    risky and time-consuming steps of the roam before leaving the original AP.
*   **Take advantage of hardware offloads**: Roaming feature offloads present in
    some Fullmac chip firmware can implement roaming protocols, reduce power
    usage by performing roaming logic on-chip rather than in the OS, and reduce
    delay between roam decision and roam start.
*   **Give WLAN Policy greater control over the connection**: Significant work
    is underway in WLAN Policy to decide when and where to roam. The API and
    logic changes described here are essential to allow WLAN Policy to initiate
    a roam, and respond when a roam attempt completes.

## Stakeholders

*Facilitator:*

neelsa@google.com

*Reviewers:*

*   silberst@google.com: for WLAN
*   karthikrish@google.com: for WLAN Drivers
*   swiggett@google.com: for WLAN Core
*   haydennix@google.com: for WLAN Policy
*   sbalana@google.com: for Connectivity Automation testing

*Consulted:*

*   Netstack: brunodalbo@google.com
*   Network Policy: dhobsd@google.com
*   WLAN Core: chcl@google.com, kiettran@google.com
*   WLAN Drivers: rsakthi@google.com
*   WLAN Policy: mnck@google.com, nmccracken@google.com

*Socialization:*

*   Prototypes were created for multiple variations on the possible
    implementation of WLAN Core logic, which were discussed in meetings with
    WLAN Core and separately with the WLAN team lead
*   Draft RFC was circulated and discussed in detail with Connectivity Drivers,
    WLAN Core, WLAN Policy, Network Policy, and Netstack teams
*   Multiple ancillary documents were drafted to explore relevant topics,
    especially around SME state machine changes, EAPOL behavior, and disconnect
    behavior

## Requirements

The key words "MUST", "MUST NOT", "REQUIRED", "SHALL", "SHALL NOT", "SHOULD",
"SHOULD NOT", "RECOMMENDED", "MAY", and "OPTIONAL" in this document are to be
interpreted as described in
[IETF RFC 2119](https://tools.ietf.org/html/rfc2119). Requirements are included
below.

## Terms

*   Device: client device in an 802.11 wireless network; in 802.11 terms,
    "device" means non-AP STA in this document
*   Station (STA): a device connected to an 802.11 wireless network
*   Access point (AP): connects one or more client devices to a network via the
    802.11 wireless networking protocol
    *   current AP: when a device is connected to a wireless network, it can
        only be associated with one AP at any given time; the current AP is the
        AP that the device is associated to at the present moment
    *   original AP: where the device is associated at the beginning of a roam
        attempt
    *   target AP: when a device is attempting a roam to a different AP, the
        target AP is where the device is attempting to associate; at completion
        of a successful roam attempt, the target AP becomes the current AP
*   Roam: when a client device moves its connection from one AP to another AP
    within the same wireless network, without incurring a full disconnection
    during the transition; in this document, "roam" is the set of actions taken
    to move the connection, which includes authentication and reassociation with
    the target AP, and coordination between layers of Fuchsia WLAN software
*   Reassociation: 802.11-specified mechanism to move a client device's
    association from one AP to another AP (see IEEE 802.11-2020 4.5.3.4)

## Design

Coordination of roaming requires FIDL API changes affecting almost all of WLAN:
Fullmac, MLME, SME, and WLAN Policy. The API specified in this RFC supports two
major modes of operation:

*   Fullmac-initiated roam
*   WLAN Policy-initiated roam
*   *(For a brief discussion of how Softmac roaming might work, see
    [Appendix C: Looking ahead to Softmac roaming](#appendix-c))*

In a **Fullmac-initiated roam**, the decision of when and where to roam is made
by the Fullmac firmware. In order for a roam to succeed, the start of the roam
MUST be communicated upward through MLME and SME, and the result of the roam
MUST be communicated upward through MLME, SME, and into WLAN Policy. A
Fullmac-initiated roam attempt begins with Fullmac issuing a `RoamStartInd`, and
the result of the roam attempt results in WLAN Policy receiving a
`ConnectTransaction.OnRoamResult`. The messages follow these paths:

*   Fullmac firmware roam start notification to Fullmac driver -> Fullmac
    `RoamStartInd` -> MLME `RoamStartInd` -> SME
*   Fullmac firmware roam result notification to Fullmac driver -> Fullmac
    `RoamResultInd` -> MLME `RoamResultInd` -> SME
    `ConnectTransaction.OnRoamResult` -> WLAN Policy

In a **WLAN Policy-initiated roam**, the decision of when and where to roam is
made by WLAN Policy and MUST be communicated downward through SME, MLME, Fullmac
driver, and finally into device firmware. The device firmware attempts the roam,
and the result of the roam attempt MUST be reported upward through Fullmac,
MLME, SME, and into WLAN Policy. A WLAN Policy-initiated roam attempt begins
with WLAN Policy issuing a `ClientSme.Roam`, and the result of the roam attempt
eventually results in WLAN Policy receiving a `ConnectTransaction.OnRoamResult`.
The messages follow these paths:

*   WLAN Policy -> SME `ClientSme.Roam` -> MLME `RoamReq` -> Fullmac `RoamReq`
    -> Fullmac firmware roam command
*   Fullmac firmware roam result notification to Fullmac driver -> Fullmac
    `RoamConf` -> MLME `RoamConf` -> SME `ConnectTransaction.OnRoamResult` ->
    WLAN Policy

To implement these API changes, there will be logic changes throughout WLAN:

*   Fullmac driver changes to send firmware commands that initiate a roam, and
    respond to notification from firmware about the progress and eventual result
    of a roam
*   MLME changes to open/close the 802.1X controlled port as the roam progresses
    through authentication, reassociation, and RSNA setup
*   SME changes to introduce a new internal SME client state (*Roaming*)
*   SME and WLAN Policy changes to give WLAN Policy the ability to initiate a
    roam, and to notify WLAN Policy of the result of a roam (WLAN Policy
    receives the same roam result notification whether the roam was
    Fullmac-initiated or WLAN Policy-initiated)

This RFC does not specify how roaming decisions are to be made by Fullmac or
WLAN Policy, other than to advise that roaming decisions:

*   SHOULD incorporate available higher order connectivity signals (such as
    Internet connectivity of APs) when deciding whether to roam
*   and SHOULD incorporate similar signals after a successful roam to gauge
    whether the roam yielded a working connection

### SME will get a new *Roaming* internal client state

SME needs to track specific data about a client when it is roaming, and manage
transitions between the roaming state and its other internal states. A new
internal state named *Roaming* will be added to the SME client state machine.
Like the SME *Connecting* state, *Roaming* state is where SME attempts to
authenticate and associate with an AP (which we will refer to as the "target
AP"). There are a few key differences:

*   the only valid transition into *Roaming* state is from *Associated* state;
    while there are many existing valid transitions into *Connecting* state
*   *Roaming* has special handling for receipt of certain events; where
    *Connecting* transitions to *Idle* state on receipt of a 802.11
    deauthentication, or transitions to *Disconnecting* on receipt of a
    disassociation, *Roaming* MUST only handle deauthentication and
    disassociation frames from the target AP
*   *Roaming* state handles events that would not be handled by *Connecting*
    (`RoamResultInd` and `RoamConf`)
*   when SME moves from *Associated* to *Roaming*, the current AP changes
    because SME is attempting to connect to the target AP; during transitions
    between *Associated* and *Connecting*, the AP does not change

Here is a high level overview of the SME client state machine (changes in red):

![SME state machine overview diagram showing new Roaming state and transitions
between SME states](resources/0243_wlan_roaming/sme_state_changes_overview.png)

Here's a more detailed SME client state machine diagram for Fullmac-initiated
roam, showing the happy path (`RoamStartInd` followed by successful
`RoamResultInd`) and relevant failure paths:

![SME state machine diagram showing events that cause transitions between
Roaming and other SME states in Fullmac-initiated
roam](resources/0243_wlan_roaming/sme_state_changes_fullmac_initiated_roam.png)

Here's a similar SME client state machine diagram for WLAN Policy-initiated
roam, showing the happy path (`RoamReq` followed by successful `RoamConf`) and
relevant failure paths:

![SME state machine diagram showing events that cause transitions between
Roaming and other SME states in WLAN Policy-initiated
roam](resources/0243_wlan_roaming/sme_state_changes_wlan_policy_initiated_roam.png)

See [Fullmac-initiated roam](#fullmac-initiated-roam) and
[WLAN Policy-initiated roam](#wlan-policy-initiated-roam) sections below for
more details on how SME transitions to/from the *Roaming* state.

### Fullmac-initiated roam {#fullmac-initiated-roam}

Some Fullmac device firmware can be configured to decide when and where to roam.
Here's a high-level overview of how a roam initiated by Fullmac is handled by
WLAN:

![Stack diagram showing the flow of roam start and roam result messages for a
Fullmac-initiated roam](resources/0243_wlan_roaming/fullmac_initiated_roam.png)

When a Fullmac-initiated roam attempt begins (**step 1** in the above diagram),
the Fullmac driver MUST send a `RoamStartInd` up to MLME (**step 2**). The
`RoamStartInd` MUST be sent before the firmware begins the authentication or
reassociation to the target AP. Like a `ConnectReq` (which creates an initial
connection to an AP), `RoamStartInd` contains the BSS Description of the target
AP. MLME SHOULD provide the full BSS Description to SME (see **step 4 below**).

The `RoamStartInd` also MUST contain:

*   the BSSID of the target AP (stored separately, in case the BSS Description
    is missing)
*   a boolean indicating whether the original association was maintained at roam
    start time (for Fast BSS Transition support, see [Appendix A](#appendix-a)).

As more roaming features are implemented, other fields MAY be added to
`RoamStartInd`.

Because the roam attempt MUST use the same security configuration that was used
to connect to the original AP when roaming to a target AP, Fullmac MUST retain
the original security configuration that was used for the initial connection to
the AP. Fullmac MUST NOT reassociate to an AP that does not match the original
security configuration used in the initial connection. See
[Appendix D: Reassociation request security configuration](#appendix-d) for more
details.

When a roam attempt is in progress, the Fullmac driver:

*   MAY reject incoming scan requests (this is the case for brcmfmac, the only
    current Fullmac driver)
*   MAY cancel a scan that is in progress when the roam attempt starts
*   MUST return an error for incoming connect requests
    *   this is similar to existing Fullmac behavior (in brcmfmac), which does
        not service (and returns an error for) an incoming connect request when
        there is a connection attempt in progress; we will do the same for
        connect attempts when a roam attempt is in progress
*   MUST return an error for incoming roam requests
    *   again, similar to existing connect behavior in Fullmac, but for roam
        attempts
*   MUST service incoming disconnect requests

Because of these constraints, the Fullmac driver MUST set an internal roam
timeout for completion of 802.11 authentication and reassociation. The timeout
duration does not include any post-reassociation actions (e.g. EAPOL, DHCP). If
the reassociation does not complete before the timeout:

*   Fullmac MUST initiate a disconnect if the original association has not been
    maintained
*   see [Appendix A](#appendix-a) for changes when Fast BSS Transition is
    supported

The roam timeout is needed to avoid becoming stuck in a state where important
functions such as scan and connect cannot be performed. The timeout SHOULD be
set substantially longer than a typical authentication and reassociation (which
have been observed to be in the neighborhood of 100 ms). A reasonable default
roam timeout MAY be somewhere around 1 second.

When MLME receives a `RoamStartInd` from Fullmac (**steps 2 and 3**), it MUST
close the 802.1X controlled port. At this point, MLME MUST consider the device
to be:

*   authenticated with the original AP
*   not associated with the original AP
*   in pending RSNA state (if security configuration requires RSNA)

MLME informs SME that the roam has started via MLME `RoamStartInd` (**step 4**).
SME uses the BSS Description in MLME `RoamStartInd` to perform 802.11
authentication and RSNA setup in concert with Fullmac. If the BSS Description
provided by MLME `RoamStartInd` is missing or invalid, SME MUST fail the roam.

Upon receipt of MLME `RoamStartInd`, SME updates its internal state to represent
that the device is roaming. Significant state and logic are already maintained
in SME to support authentication and RSNA, processes which are performed in
concert with the Fullmac firmware/driver and MLME. Fuchsia WLAN uses SME as the
802.11 authenticator (rather than an authenticator that might be provided by
Fullmac firmware). Also, Fuchsia WLAN uses SME as an external supplicant for
RSNA (rather than using a firmware supplicant, or a third party supplicant like
wpa_supplicant).

If SME detects a malformed `RoamStartInd` (e.g. missing fields, or invalid
data), SME knows that the roam has failed due to an internal error. At the point
that Fullmac issued the `RoamStartInd`, the device was still associated with the
original AP, though Fullmac may have since left the channel and/or band of the
original AP shortly after. SME will take the following actions:

*   SME MUST consider this to be a disconnect, even if the `RoamStartInd`
    indicates that the original association is maintained, because the invalid
    roam attempt might otherwise continue
*   SME MUST send WLAN Policy an `OnRoamResult` indicating that the disconnect
    was due to a malformed `RoamStartInd`
*   SME MUST send a deauthenticate to the target AP
*   SME MAY send a deauthenticate to the original AP
*   SME MUST transition to *Disconnecting*

If `RoamStartInd` is not malformed, SME MUST transition to *Roaming*.

Note that WLAN Policy is not informed of the start of a roam. Stated
differently, `RoamStartInd` is propagated up to SME, but the start of a roam is
not communicated to WLAN Policy through the `ConnectTransaction` protocol. At
design time, WLAN Policy team did not see a need for such an event, but it can
be added if need arises. From observations of actual roams, lack of roam start
notification to WLAN Policy results in a short duration where WLAN Policy is
unaware that a roam is in progress; sending a roam start event from SME to WLAN
Policy would reduce that duration by approximately 50 ms.

After sending the `RoamStartInd` to MLME, the Fullmac firmware begins the
process of 802.11 authentication with the target AP (**step 5**). Authentication
is coordinated with higher layers in the same fashion as a regular connection.
When 802.11 authentication is complete, the firmware begins the process of
802.11 reassociation with the target AP. *(See IEEE 802.11-2020 11.3.5.4 for
details about how authentication, reassociation, and RSNA are coordinated).*
Once Fullmac starts the reassociation exchange, the device is no longer
associated with the original AP (but see
[Looking ahead to Fast BSS Transition](#appendix-a) for some subtle details).

Upon success, failure, or timeout of a Fullmac-initiated roam attempt (**step
6**), the Fullmac driver MUST send a `RoamResultInd` up to MLME (**steps 7 and
8**). The `RoamResultInd` provides information similar to what is provided in a
`ConnectConf`. Most importantly, it includes the 802.11 status code of the roam
attempt. Similar to `RoamStartInd`, `RoamResultInd` MUST contain a boolean field
to indicate whether the original association is maintained (see
[Looking ahead to Fast BSS Transition](#appendix-a)).
`original_association_maintained` MUST be false if the roam succeeded, because a
successful roam always incurs disassociation from the original AP.

`RoamResultInd` also MUST contain a boolean field to indicate whether the device
is authenticated with the target AP:

*   `target_bss_authenticated` MUST be true if the roam attempt succeeded
*   if `status_code` is anything other than success, `target_bss_authenticated`
    specifies whether the device is currently authenticated with the target AP
*   this field is present to inform SME of authenticated state with the target
    AP, because SME MAY decide to send a deauthenticate request to the target AP
    during its cleanup after a failed roam attempt

Upon receipt of `RoamResultInd`, MLME MUST keep the 802.1X controlled port
closed (but see [Looking ahead to Fast BSS Transition](#appendix-a)). If the
roam succeeded:

*   device MUST be associated with the indicated AP
*   device MAY still be authenticated with the original AP
*   device MUST NOT be associated with the original AP

If the roam failed:

*   device MAY still be authenticated with the original AP
*   device MAY be authenticated with the target AP, as indicated by
    `target_bss_authenticated`

MLME sends `RoamResultInd` up to SME (**step 9**).

Upon receipt of the MLME `RoamResultInd`, SME does the following if the roam
succeeded:

*   SME MUST move its internal state from *Roaming* to *Associated*, using the
    target AP as the current AP
*   SME MUST NOT transition out of *Associated* upon receipt of any
    disassociate/deauthenticate frames from the original AP
*   if the security configuration requires RSNA: RSNA setup occurs, coordinated
    between Fullmac firmware, Fullmac driver, MLME, and SME in the same fashion
    as the RSNA setup that occurs at first connection
*   upon successful RSNA setup (or if the network does not require RSNA), the
    802.1X controlled port will be opened in the same fashion as a regular
    successful connection

If `RoamResultInd` indicates that the roam failed, SME instead does the
following:

*   if `target_bss_authenticated` is true:
    *   SME MAY request deauthentication from the original AP (but see
        [Disconnect behavior](#disconnect-behavior) for some subtleties here)
    *   SME MAY request deauthentication from the target AP
    *   if SME decides to request deauthentication from the target AP, SME MUST
        transition from *Roaming* into *Disconnecting*
    *   otherwise SME MUST transition from *Roaming* into *Idle*
    *   in case of SAE timeout with the target AP:
        *   SME MAY request deauthentication from the original AP
        *   SME MUST transition from *Roaming* directly to *Idle*
*   if `target_bss_authenticated` is false:
    *   SME MAY request deauthentication from the original AP (but see
        [Disconnect behavior](#disconnect-behavior) for some subtleties here)
    *   if SME decides to request deauthentication from the original AP, SME
        MUST transition from *Roaming* into *Disconnecting*
    *   otherwise SME MUST transition from *Roaming* into *Idle*

SME communicates with WLAN Policy over a per-connection channel using the
existing `ConnectTransaction` API. SME MUST send an `OnRoamResult` to WLAN
Policy to inform it that a roam attempt has completed (**step 10**). The
`RoamResult` is similar to SME `ConnectResult`. `RoamResult`:

*   MUST include BSSID of the target AP
*   MUST include a status code indicating whether the roam succeeded
*   MUST include a boolean indicating whether the original association has been
    maintained
    *   MUST be false if the roam succeeded
    *   see [Looking ahead to Fast BSS Transition](#appendix-a) for more details
*   SHOULD include the BSS description of the target AP, regardless of success/
    failure (but this field MAY be empty if the roam failed)
*   MUST include an SME `DisconnectInfo` if a disconnect was incurred; in other
    words, this is REQUIRED if roam failed and original association was not
    maintained

Upon receipt of `OnRoamResult`, WLAN Policy will update its internal state. If
the roam succeeded, WLAN Policy MUST continue to maintain the existing
`ConnectTransaction` with SME, but with the device now associated to the target
AP. If the result indicates roam failure:

*   if the original association was not preserved, WLAN Policy MUST close the
    `ConnectTransaction` and end the connection
*   after a roam failure, WLAN Policy will begin the process of creating a new
    connection, similar to how it responds to any lost connection

### WLAN Policy-initiated roam {#wlan-policy-initiated-roam}

WLAN Policy will gain the ability to initiate a roam attempt and learn of its
outcome. A high-level overview of how a roam initiated by WLAN Policy is handled
by WLAN:

![Stack diagram showing the flow of roam request and roam confirmation messages
for a WLAN Policy-initiated
roam](resources/0243_wlan_roaming/wlan_policy_initiated_roam.png)

When WLAN Policy initiates a roam attempt, it does so by issuing a
`ClientSme.Roam` to SME (**step 1 in the above diagram**). `Roam` contains a
`RoamRequest`, which is conceptually similar to SME `ConnectRequest`, though
with fewer options because SSID and security configuration MUST NOT change
during the roam (see
[Appendix D: Reassociation security configuration](#appendix-d) for more
background on the security configuration used in a roam).

Because the roam attempt MUST use the same security configuration that was used
to connect to the original AP when roaming to a target AP, WLAN Policy MUST
retain the original security configuration that was used for the initial
connection to the AP. WLAN Policy MUST NOT attempt to reassociate to an AP that
does not match the original security configuration used in the initial
connection.

In order for WLAN Policy to retain the security configuration used for the
connection to the original AP, it must first obtain it, so SME
`ConnectTransaction.ConnectResult` MUST gain a new field that informs WLAN
Policy of the exact security configuration that was used in the original
association request.

Upon receipt of a `ClientSme.Roam` from WLAN Policy, SME behaves the same as if
it had received a `RoamStartInd` from MLME, except that it sends a `RoamReq`
down to MLME (**step 2**).

Upon receipt of a `RoamReq` from SME, MLME behaves the same as if it had
received a `RoamStartInd` from Fullmac, except that it sends a `RoamReq` down to
Fullmac (**steps 3 and 4**).

Upon receipt of a `RoamReq` from MLME, Fullmac sends commands to the firmware to
begin the authentication and reassociation processes (**step 5**), and then the
firmware does just as it would for a Fullmac-initiated roam (**step 6**). When
the firmware notifies the Fullmac driver of the result of the roam attempt
(**step 7**), Fullmac sends a `RoamConf` up to MLME (**steps 8 and 9**).
`RoamConf` is similar to `RoamResultInd`.

Upon receipt of `RoamConf`, MLME behaves the same as if it had received a
`RoamResultInd`, except that it sends a `RoamConf` up to SME (**step 10**).

Upon receipt of `RoamConf`, SME behaves the same as if it had received a
`RoamResultInd` from MLME. SME sends an `OnRoamResult` to WLAN Policy over the
`ConnectTransaction` (**step 11**).

Upon receipt of `OnRoamResult`, WLAN Policy takes the same actions as those
discussed above at the end of a Fullmac-initiated roam.

## Implementation

### Phase 1: API and logic introduced without pushing to production devices

The API changes can be made in two Gerrit changes: one for Fullmac-initiated
roam, and another for WLAN Policy-initiated roam.

The strategy for roaming changes so far has been to introduce the roaming logic
changes, without enabling them on production devices. Fullmac-initiated roaming
is guarded by a driver feature in the brcmfmac driver. There are unit and
integration tests to ensure that the roaming features are disabled, to prevent
the feature from being accidentally enabled.

For WLAN Policy-initiated roaming, the API can be introduced without anything
calling the SME `ClientSme.Roam`/`ConnectTransaction.OnRoamResult` API until
WLAN Policy is confident that it can be enabled. Once the API is present, WLAN
Policy can begin using roam success/failure as an input to its connection
quality tracking, and WLAN Policy logic can decide whether to use the
`ClientSme.Roam` API for a specific AP, or even whether to use the
`ClientSme.Roam` API at all.

WLAN Policy MUST have the ability to enable or disable roaming features at
interface creation time. This allows WLAN Policy to:

*   know that Fullmac-initiated roaming is enabled or disabled on a specific
    interface
*   fully disable roaming features on an interface in case of a runtime problem

A simple API change to `fuchsia.wlan.device` is necessary to support this need.
A minimal implementation requires adding a single field to `CreateIfaceRequest`
to allow the caller to specify the roaming feature(s) that must be enabled
during interface creation.

Note that for Fullmac interfaces, existing firmware can only enable features
like roaming offloads at interface creation time.

### Phase 2: Integration testing

For Fullmac-initiated roaming, the change can be tested on local developer
builds (e.g. by uncommenting roaming features in the brcmfmac Fullmac driver
code), and can be integration tested on real device and AP hardware using
[Antlion](https://fuchsia.googlesource.com/antlion). There is already an Antlion
integration test suite for Fullmac-initiated roaming
(`WlanWirelessNetworkManagementTest`), which tests successful and unsuccessful
roaming attempts. The existing test suite requires a Fuchsia Fullmac-capable
device that is instrumented to roam between two networks on a single physical AP
device. Increasing integration coverage via a new Antlion test suite for
additional Fullmac-initiated roaming scenarios is also expected.

For WLAN Policy-initiated roaming, the changes can be tested on local developer
builds, and can be integration tested on real device and AP hardware using
Antlion (by giving WLAN Policy the ability to enable roaming on an interface at
runtime, see discussion in phase 1 above). Increasing integration coverage via a
new Antlion test suite for additional WLAN Policy-initiated roaming scenarios is
also expected.

Antlion test suites that exercise Fullmac-initiated or WLAN Policy-initiated
roaming MUST know whether those features are in use on the Fuchsia
device-under-test (DUT). This is necessary because Antlion needs to know whether
to run or skip roaming tests on that DUT. For the existing Fullmac-initiated
roaming test suite, Antlion configuration has special directives for WLAN
features: presence of a roaming feature in the Antlion config tells Antlion to
run Fullmac-initiated roaming tests, otherwise those tests are skipped. The
implementation SHOULD give Antlion the explicit ability to query for feature
support (using WLAN Driver Features), and SHOULD give Antlion the explicit
ability to enable roaming features on a DUT before running tests on that DUT.

The existing Fullmac-initiated roaming Antlion test suite, as well as any new
test suites, MUST be runnable on Connectivity Automation testbeds with a WLAN
Antlion testbed.

A Fullmac-initiated roaming capable WLAN testbed:

*   MUST contain at least one Fullmac-initiated roaming capable physical Fuchsia
    device
*   MUST contain Antlion-compatible physical AP device(s) capable of providing
    two or more simultaneous APs (in 802.11 terms, two or more BSSs within the
    same ESS)
*   SHOULD allow for variable signal attenuation
*   MAY enclose devices in radio shields to decrease interference

When such a testbed is available, developers will be able to these tests against
run work-in-progress changes using existing Antlion tryjobs infrastructure.

Deeper integration testing (e.g. using a third-party lab for testing roaming
scenarios) is likely to be used as well.

### Phase 3: Rollout to production devices

Pending the outcome of integration testing, rollout can begin. WLAN Policy MUST
have the ability to enable and disable roaming features on a per-interface
basis, as discussed in phase 1 above.

Rollout of Fullmac-initiated and WLAN Policy-initiated roaming will be
controlled by WLAN Policy, using WLAN Policy's typical feature rollout
mechanisms (or a new rollout mechanism, at their option). WLAN Policy already
has logic for connection selection, connection quality, and recovery from
connection failure. WLAN Policy frequently makes changes to these functions in
the usual course of operation. It is likely that additional runtime safeguards
specific to roaming will be introduced by WLAN Policy, but this RFC does not
endeavor to specify them.

## Performance

Metrics that we will need to evaluate during the rollout phase include:

*   Roam attempt / roam success, as counts and rate
*   Roam time to complete, as histogram
*   Roam timeout, as count and rate
*   Roam success followed by unusable interface, as count
*   Roam failure reason, broken down by `DisconnectInfo` (roughly, disconnect
    source and reason)
*   Periods of excessive roaming

We will need to monitor metrics that are not roaming-specific as well:

*   non-roaming disconnect counts (should not increase significantly)
*   uptime ratio (should not decrease significantly)
*   number of idle interface autoconnects (should not increase significantly)

## Ergonomics

This RFC does not specify any end user control of roaming. Other operating
systems provide end users no control over roaming (e.g. MacOS), or very limited
control via special configuration (e.g. Windows, Linux). At this time we do not
expect WLAN Policy to provide an end user control for roaming. Therefore,
Fuchsia end users do not take on significant cognitive load from the
introduction of Fullmac-initiated or WLAN Policy-initiated roaming. The only
expected user visible effect is that some transitions between access points will
be faster. To the end user, a failed roam will be indistinguishable from any
other connection interruption, and the recovery from the interruption will be
just like a connection interruption that did not involve roaming: a regular scan
and connect.

## Backwards Compatibility

The 802.11 spec has included roaming (reassociation) since long before Fuchsia
existed.

The primary concern for backwards compatibility is that the introduction of
roaming does not frequently cause a working network connection to become
unusable, for the many existing devices that use Fuchsia WLAN. As the rollout
section above mentions, giving WLAN Policy the ability to explicitly decide when
to enable and use roaming features will go a long way to ensure that the use of
roaming features does not leave interfaces in a degraded or unusable state. WLAN
Policy MAY:

*   throttle roam scans
*   denylist problematic APs
*   implement backoff to throttle roam attempts
*   disable roaming at runtime

## Security considerations

Three security concerns exist:

*   original AP and target AP MUST have the same SSID
*   the target AP must match the security parameters that were specified in the
    connection to the original AP
*   decision of which AP to connect to has always been the purview of WLAN
    Policy; Fullmac-initiated roaming delegates some of this responsibility to
    Fullmac firmware

WLAN SME already validates that SSID and security parameters match the original
security configuration, and the same mechanism will be used to validate them for
the target AP. Failure of these validation steps results in tearing down the
connection. See [Appendix D](#appendix-d) for more detail on the security
configuration used in a roam attempt.

As for responsibility for choosing an AP:

*   For Fullmac-initiated roaming, if the Fullmac firmware has found a suitable
    AP to roam to, we have explicitly given it the ability to roam to it. And in
    the course of completing the roam, WLAN SME will validate that the AP is
    suitable for the original security configuration (as described in
    [Appendix D](#appendix-d)).
*   For WLAN Policy-initiated roaming, WLAN Policy is already in charge of
    deciding which AP to connect to. WLAN Policy-initiated roaming only provides
    a different mechanism for connecting to a target AP (using 802.11
    reassociation, rather than 802.11 association).

## Privacy considerations

This design provides the API that allows Fullmac and WLAN Policy to initiate a
roam and learn of the roam result. These actions incur use of the same
information already used for existing (non-roaming) connection and disconnection
logic. This design does not alter the current privacy posture of WLAN software.

## Testing

*   Unit tests already exist for Fullmac driver (using the brcmfmac Sim
    Framework)
*   Integration tests on real device and AP hardware already exist for
    Fullmac-initiated roaming (using the Antlion
    WlanWirelessNetworkManagementTest suite)
*   Further integration tests will be created that exercise roaming scenarios
    for Fullmac-initiated and WLAN Policy-initiated roaming
*   Connectivity Automation will use these Antlion tests in their lab facilities
*   Third party roaming tests may also be used

## Documentation

FIDL changes will include significant documentation for new messages, fields,
and methods. Additional documentation beyond the inline documentation is not
planned.

## Drawbacks, alternatives, and unknowns

### BSSID Blocklist extension

Fullmac-initiated roaming is likely to require a BSSID blocklist API (similar to
the one that exists in Android), allowing WLAN Policy to specify certain APs
that should not be connected to. This would allow WLAN Policy to avoid a
situation where the Fullmac firmware repeatedly roams to an AP that WLAN Policy
knows is not functional.

### Whether to share control between Fullmac and WLAN Policy

This RFC specifies that WLAN Policy will have control over whether a Fullmac
interface has roaming logic enabled. The expectation is that either Fullmac or
WLAN Policy is in control of roaming logic for an interface, but probably not
both. A situation where both Fullmac and WLAN Policy are simultaneously
initiating roam attempts might cause "thrashing" between APs if Fullmac and WLAN
Policy disagree about which AP is best. A BSSID Blocklist API would give WLAN
Policy the ability to set a "guardrail" on Fullmac's roaming decisions, without
having more than one initiator of roaming decisions. If we decide that there is
a valid reason to have both Fullmac and WLAN Policy initiating roam attempts,
this will require additional design.

### Disconnect behavior {#disconnect-behavior}

Very careful analysis of disconnect scenarios is ongoing. We need to be
confident that WLAN is resilient in the face of disconnects that occur during
periods of ambiguity (e.g. when Fullmac knows that the roam has started, but
higher layers like SME do not).

The current disconnection logic in SME attempts to deauthenticate from the AP as
a cleanup measure in most disconnection scenarios. When a roam attempt causes
the device to leave the channel (or even band) of the original AP:

*   Fullmac may not know whether the device is still authenticated with the
    original AP
*   because Fullmac doesn't know, Fullmac cannot inform SME whether the device
    is conclusively still authenticated with the original AP in some cases
*   if SME decides during its cleanup to request deauthentication from the
    original AP (by sending a deauthenticate request to Fullmac), this
    deauthenticate request may not be serviceable by Fullmac, because Fullmac
    may be unable to reach the original AP

In this and other cases that involve disconnects, we may discover that we need
different disconnect behavior at the Fullmac and/or SME layers.

### Interactions with existing "reconnect" logic in WLAN Policy and SME

WLAN Policy and SME have existing "reconnect" behaviors, where re-establishing a
previously working connection is attempted. For example, when a connection is
disassociated but not deauthenticated, SME can preserve the `ConnectTransaction`
protocol between WLAN Policy and SME while SME attempts to associate to the AP.
If the SME reconnect succeeds, SME was able to skip the authentication overhead
and reduce the disconnected time. Behaviors like this will likely need to be
scrutinized for how they interact with roaming. As a hypothetical example, there
may be cases where we discover that we need to deauthenticate more aggressively
during or after roam attempts in order to prevent reconnects from encountering
problems after a failed roam attempt. We may also find that we need to change
the timing of reconnects, or adjust other parameters.

### Interactions with DHCP and layer 3 management {#layer-3-interactions}

WLAN software manages the data link layer, also known as "layer 2." Above layer
2 is the network layer, also known as "layer 3," which is managed by DHCP,
Netstack, Network Policy, and other management software. When a roam attempt
occurs, disruptions in layer 2 and layer 3 are likely:

*   a brief pause in the flow of layer 2 traffic is likely, for the duration
    that the device is disconnected from the original AP but not yet fully
    connected to the target AP
*   roam attempts cause the 802.1X controlled port to close, which will cause a
    layer 3 link down, followed by a link up if the attempt succeeds

In Fuchsia, a layer 3 link down currently causes a full DHCP client teardown and
init. This may go unnoticed by the user, or may result in user-visible loss of
IP connectivity. This "layer 2 disruption causing layer 3 disruption" scenario
is not unique to roaming: a disconnect followed by a connect (even to the same
AP) causes a full DHCP client teardown and init now.

This layer 3 disruption could be reduced with additional design (outside the
scope of this RFC) to:

*   give layer 3 software the option of optimizing for layer 2 disruptions that
    are expected to provide the same layer 3 connectivity, by providing enough
    information about the layer 2 disruption to layer 3 software
*   decide on the right checks to verify layer 3 connectivity (e.g. DNAv4, ARP
    probe, DHCP actions, existing or new reachability checks) to use when such a
    layer 2 disruption occurs
*   coordinate the logic for performing these layer 3 checks, backed by a
    fallback to full DHCP client teardown and init when necessary

The exact mechanism(s), post-roam verifications, and logic needed would have to
be worked out. Android has some prior art here (see
[Android updateLayer2Information](#prior-art)), and IETF RFCs for DNAv4 and
DNAv6 also cover this problem space (see [DNAv4 and DNAv6](#prior-art)).

## Prior art and references {#prior-art}

*   [Android Wi-Fi Network Selection](https://source.android.com/docs/core/connect/wifi-network-selection)
*   [Android SSID and BSSID Blocking](https://source.android.com/docs/core/connect/wifi-network-selection#ssid-bssid-blocking)
*   [Android updateLayer2Information](https://cs.android.com/android/platform/superproject/main/+/main:packages/modules/Wifi/service/java/com/android/server/wifi/ClientModeImpl.java?q=symbol%3A%5Cbcom.android.server.wifi.ClientModeImpl.updateLayer2Information%5Cb%20case%3Ayes)
*   DNAv4 and DNAv6:
    *   [Detecting Network Attachment in IPv4 (DNAv4)](https://www.ietf.org/rfc/rfc4436.txt)
    *   [Simple Procedures for Detecting Network Attachment in IPv6](https://www.ietf.org/rfc/rfc6059.txt)
*   [Linux cfg80211 subsystem](https://docs.kernel.org/driver-api/80211/cfg80211.html#)
    (see documentation of functions connect and cfg80211_roamed)
*   [macOS wireless roaming for enterprise customers](https://support.apple.com/en-us/102002)
*   [Microsoft WifiCx OID_WDI_TASK_ROAM](https://learn.microsoft.com/en-us/windows-hardware/drivers/netcx/oid-wdi-task-roam)
*   [wpa_supplicant Developers' documentation](https://w1.fi/wpa_supplicant/devel/#_wpa_supplicant)
    (though little proper documentation exists)

## Appendix A: Looking ahead to Fast BSS Transition {#appendix-a}

Fast BSS Transition is not currently supported in Fuchsia WLAN. Fast BSS
Transition allows a device to move its association to the target AP, with RSNA
intact. When Fast BSS Transition becomes supported by WLAN, a few behaviors will
change and the current design takes these behavior changes into account.

At initial connection time, WLAN Policy will need to see one (or more)
additional information elements (abbreviated IE) that are specific to Fast BSS
Transition support. It is likely these would be communicated to WLAN Policy
within SME `ConnectTransaction.ConnectResult` (similar to the way this RFC
specifies that security config is communicated to WLAN Policy). Fast BSS
Transition APs advertise a "mobility domain" IE that the device can use to
identify other APs that accept Fast BSS Transitions from the current AP.

At the start of a roam that specifies that Fast BSS Transition is in use, WLAN
MUST retain associated state and RSNA with the original AP. This is true whether
the roam is Fullmac-initiated by `RoamStartInd` or WLAN Policy-initiated by
`RoamReq`. One important ramification is that MLME will not close the controlled
port at the start of a Fast BSS Transition roam. The original connection will
continue as usual, while the new connection is being established via Fast BSS
Transition. If the Fast BSS Transition roam succeeds and
`original_association_maintained` is true:

*   MLME MUST keep the 802.1X controlled port open
*   because the controlled port was never closed, the device may only experience
    a brief pause in layer 2 (observed to take approximately 20 ms) with no IP
    connectivity disruption

If the roam attempt fails (or reaches a roam attempt timeout):

*   `RoamResultInd`/`RoamConf` `original_association_maintained` specifies
    whether the association with the original AP has been maintained throughout
    the unsuccessful roam attempt
*   Fullmac SHOULD NOT initiate a disconnect if the original association has
    been maintained (but there may be cases where the driver knows that recovery
    from the failure/timeout requires a disconnect)

If MLME sends a `RoamResultInd`/`RoamConf` to SME indicating that a Fast BSS
Transition roam failed:

*   SME MUST transition from *Roaming* to *Associated* with the original AP
*   if `target_bss_authenticated` is true, SME MAY request deauthentication from
    the target AP (see [Disconnect behavior](#disconnect-behavior) for some
    subtle details)

When Fast BSS Transition is supported, WLAN Policy will be able to maintain the
existing `ConnectTransaction` with the original AP even after WLAN Policy
receives an `OnRoamResult` that indicates a failed Fast BSS Transition roam
attempt.

Fast BSS Transition will also add another consideration to
[Interactions with DHCP and layer 3 management](#layer-3-interactions): some
successful roam attempts will not close the 802.1X controlled port during the
attempt, so there will be no layer 3 link down. Without a layer 3 link down
followed by a layer 3 link up, layer 3 management software will need some other
signal to know that post-roam verification actions should occur.

Another ramification is that various state machines (MLME, SME, WLAN Policy)
will need to maintain state for more than one AP. While this can be done by
adding fields to existing states, it's likely that refactoring these state
machines to be multiple-BSS aware will become more attractive once Fast BSS
Transition is on the horizon. A quick look at the SME client state machine
illustrates the quality of changes that might be necessary:

*   At start of a Fast BSS Transition roam, SME will need to model the fact that
    the device is still associated with RSNA on the original AP, while also
    modeling that the device is authenticating with the target AP
*   When authentication with the target AP is complete, SME needs to model that
    the device is still associated with RSNA on the original AP, while it is
    reassociating to the target AP
*   At completion of a successful Fast BSS Transition roam:
    *   the device will move to 802.11 *associated with RSNA* state (see IEEE
        802.11-2020 11.3.1) with the target AP
    *   the device will drop to *authenticated* state with the original AP
    *   the device may subsequently lose *authenticated* state with the original
        AP, e.g. by the original AP sending the device a deauthenticate
        indication; but this does not affect the connection with the target AP
*   At a completion of a failed Fast BSS Transition roam attempt, the resulting
    device state is more complicated:
    *   if the failure occurred before the device lost its association with the
        original AP, the device is still *associated with RSNA* on the original
        AP, and may be *authenticated* with the target AP
    *   if the failure happened after the device lost its association with the
        original AP, the device is no longer *associated* to any AP, and may be
        *authenticated* with the original AP and/or the target AP

## Appendix B: Looking ahead to fallback-capable roaming {#appendix-b}

802.11 specifies that a device can be authenticated with multiple APs, but it
can only be associated with a single AP at any given time. When a roam attempt
fails, the device is still authenticated with the original AP, unless a
deauthentication indication has been received from the original AP. This means
that the SME could maintain enough state with the original AP to allow it to
transition from *Roaming* state back into *Connecting* state, skipping the
authentication process with the original AP. This would shorten the time to get
back to associated state with the original AP. This was prototyped by adding two
new states to the SME client state machine:

*   *Roaming*, as described in this RFC
*   *RoamingWithFallback*, which retains enough state about the original AP to
    go back into *Connecting* state with the original AP upon roam failure

This prototype of a fallback-capable SME client state machine is not currently
being pursued. After discussion with WLAN Core team, the general consensus was
that implementing fallback-capable roaming would be easier if the SME client
state machine were refactored to explicitly keep state for multiple APs. There
may also be Fullmac firmware limitations to contend with--some firmware requires
that the Fullmac driver disconnect and reset the firmware state upon a roam
failure, which may preclude the ability to fallback.

If we decide to implement fallback-capable roaming, it can coexist with Fast BSS
Transition. Fallback-capable roaming endeavors to keep authenticated state with
the original AP; while Fast BSS Transition endeavors to keep associated state
and RSNA. We could use both Fast BSS Transition and fallback-capable roaming, or
just one of them, or neither.

## Appendix C: Looking ahead to roaming on Softmac {#appendix-c}

A "Softmac" WLAN device differs from Fullmac in one important way. A Fullmac
device has most of the 802.11 MLME logic implemented in firmware, while a
Softmac device has most of the WLAN MLME logic implemented in driver software.
Because Fullmac handles construction and parsing of reassociation frames,
orchestrates the roaming process, and can even decide when and where to roam,
the Fullmac API needs significant changes for roaming (i.e. this RFC). But in
Softmac, the API and logic changes required for roaming support are likely to be
different in nature:

*   It is likely that WLAN Policy's roaming control logic (not described in this
    RFC) will be used to decide when and where to roam, rather than implementing
    separate logic for this in the Softmac driver or in Fuchsia MLME/SME
*   Logic for constructing and parsing 802.11 reassociation frames, and
    orchestrating the roaming process, is likely to be implemented in MLME
    rather than adding more to the Softmac API surface
*   Softmac must keep track of the current channel for many reasons, and since a
    roam attempt may change the channel or band, this logic may need to be
    adjusted
*   All of this comes with the caveat that no prototyping of Softmac roaming has
    been undertaken, so it's possible that we may encounter Softmac firmware
    that requires a different approach

## Appendix D: Reassociation security configuration {#appendix-d}

When a device roams (via reassociation), the security configuration specified in
the reassociation request frame MUST be the same security configuration that was
used for the original connection (see IEEE 802.11-2020 11.3.5.4). This
requirement merits a deeper discussion of how this affects the API changes
specified in this RFC.

WLAN Policy stores saved networks, which are specified in `NetworkConfig` which
aims to represent them in human-understandable terms:

*   SSID
*   high level protection type for the networks (e.g. WPA2, or WPA3, but not the
    many possible variations within those broad types)
*   credentials (e.g. password)

When WLAN Policy decides to connect to a specific AP, WLAN Policy sends SME a
`ConnectRequest` which contains additional details about the AP:

*   BSS Description for the AP that the device should connect to
*   802.11 authentication type (e.g. 802.11 open authentication, or WPA3 SAE)

SME then:

*   compares this more detailed specification to the security configuration(s)
    offered by the AP (contained in the BSS Description)
*   decides on the security configuration that will be included in the
    association request
*   generates the exact bytes that will be sent to the AP in the association
    request
    *   for example, for a WPA2 network SME generates an RSNE information
        element
    *   that RSNE might have the RSN capabilities `MFPR` bit set to 1, meaning
        that management frame protection (MFP, see IEEE 802.11-2020 9.4.2.24.4)
        is required, so the device will not join a network that doesn't support
        MFP
    *   or SME might have set `MFPR` to 0 (meaning not required), while setting
        `MFPC` to 1, meaning that the device can join a network regardless of
        whether MFP is supported or required on that network
*   propagates this security configuration down into MLME, and eventually into
    the Fullmac firmware that will send the association request to the AP

The association will only succeed if the AP security configuration can match
against the SME-specified security configuration (see IEEE 802.11-2020 12.6.3
for how this works). IEEE 802.11-2020 11.3.5.4 REQUIRES that the same RSNE that
was included in the original association request is used in any subsequent
reassociation (roam) requests.

For Fullmac-initiated roam attempts, this does not impose any need for any
additional API. SME decides the security configuration that will be used in the
original association request, Fullmac stores this configuration, and Fullmac
uses this security configuration for any roam attempts. Because WLAN Policy is
not choosing the target AP, WLAN Policy never needs to know the security
configuration for Fullmac-initiated roam attempts in this case.

But for WLAN Policy-initiated roaming, there is a need for WLAN Policy to know
the SME-specified security configuration that was used to connect to the
original AP, because:

*   WLAN Policy is going to choose a target AP for the roam attempt
*   but if WLAN Policy does not know the SME-specified security configuration
    that was included in the original association request, it cannot know for
    sure that the AP it picks will match the original security configuration
*   if WLAN Policy picks an incompatible target AP, the roam will fail

The solution is to add a field to SME `ConnectResult` that will share the
security IE with WLAN Policy at initial connection time.

If we discover that there are cases where Fullmac firmware uses a different
security configuration during a roam:

*   SME SHOULD almost certainly fail the roam rather than take a risk that the
    security configuration is less secure than the original security
    configuration; and SME should complain loudly
*   WLAN Policy SHOULD almost certainly disable roaming for that device to
    prevent this from occurring again; and WLAN Policy should complain loudly

If we discover a need for changing security configuration during a roam, we will
have to evaluate that need against the security configuration invariant imposed
by IEEE 802.11-2020 11.3.5.4. Considering that previous revisions of 802.11 did
not clarify this question, it is possible that we will encounter Fullmac
firmware which does not uphold the invariant. If we discover a case that we need
to support, we could add API fields/methods to explicitly allow WLAN Policy to
get and/or set the security configuration.

We may also discover that we want WLAN to uphold stronger security guarantees
than just the 802.11 spec-imposed constraints. For WLAN Policy-initiated
roaming, WLAN Policy will be free to enforce stronger security by choosing only
APs that match stricter security criteria than what was used in the initial
connection, still subject to the SME validation that security parameters match
the original security configuration. For Fullmac-initiated roaming, it is likely
that a driver API would need to be introduced to allow changing the
association/reassociation request IEs, and/or firmware would have to be modified
with additional configuration options to provide this behavior.

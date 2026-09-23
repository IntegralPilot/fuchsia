# GAVDP PTS Instructions

## Setup
Source code setup:
* These tests require a build that disables all external profile clients (sometimes called an
"arrested" build).

Target command-line tools used to pass GAVDP tests in PTS, invoked from the shell when specified in
the instructions:
* `bt-avdtp-tool`
* `bt-cli`

## IXIT Values
No changes

## TESTS

### GAVDP/ACP/APP/CON/BV-01-C
1. (target shell 1) `bt-avdtp-tool -d 500`
2. (target shell 2) `bt-cli`
3. *Start test*
4. PTS: *"Create an AVDTP signaling channel."*
5. (bt-cli) `connect <peer-id>`

*Note: Last command ("connect") may need to be run more than once*

### GAVDP/ACP/APP/TRC/BV-02-C
1. (target shell 1) `bt-avdtp-tool -d 500`
2. (target shell 2) `bt-cli`
3. *Start test*
4. PTS: *"Please wait while PTS queries the IUD SDP record."*
5. PTS: *"Create an AVDTP signaling channel."*
6. (bt-cli) `connect <peer-id>`

*Note: Last command ("connect") may need to be run more than once*

7. *DUT should be playing audio at this point*
8. PTS: *"Suspend the streaming channel."*
9. (bt-avdtp-tool) `suspend <peer-id>`
10. PTS: *"Is the IUT receiving streaming media from PTS?"*
11. (PTS) `Yes`

### GAVDP/INT/APP/CON/BV-01-C
1. (target shell 1) `bt-avdtp-tool -d 500`
2. (target shell 2) `bt-cli`
3. *Start test*
4. PTS: *"Please wait while PTS queries the IUD SDP record."*

### GAVDP/INT/APP/TRC/BV-02-C
1. (target shell 1) `bt-avdtp-tool -d 500`
2. (target shell 2) `bt-cli`
3. *Start test*
4. PTS: *"Please wait while PTS queries the IUD SDP record."*
5. PTS: *"Is the IUT receiving streaming media from PTS?"*
6. (PTS) `Yes`

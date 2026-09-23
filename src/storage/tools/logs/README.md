# Extracting logs on zedboot and bringup

For tests which run emulators without networking, we need a way to extract logs
from the device that is more consistent than serial. These tools provide that
way.

The first tool, run-with-logs, is a Fuchsia binary which takes either a path to
a block device or a bus path to match, and a command line invocation as an
argument, and runs the command line invocation, reading the stdout and stderr
and then writing it to the block device in a simple format.

The second tool, extract-logs, is a host binary which takes a path to a file and
locations to write stdout and stderr to, and extracts the logs from the file.

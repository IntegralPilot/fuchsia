# Copyright 2019 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import fcntl
import os
import re
import subprocess
import sys
import termios
import time

LINUX_BIN_WIRESHARK = "wireshark"
LINUX_BIN_TSHARK = "tshark"
TARGET_TMP_DIR = "/tmp/"


def has_cmd(binary_name):
    return any(
        os.access(os.path.join(path, binary_name), os.X_OK)
        for path in os.environ["PATH"].split(os.pathsep)
    )


def has_wireshark_env():
    """Test if wireshark GUI can run.

    Returns:
      (True, "") if wireshark can run in the environment.
      (False, Error_String) otherwise.
    """
    platform = os.uname()
    print(platform)
    if not platform:
        return False, "Failed to get uname"
    if platform[0].lower() != "linux":
        return False, "Supported only on Linux"

    if not has_cmd(LINUX_BIN_WIRESHARK):
        return False, "can't find %s" % LINUX_BIN_WIRESHARK

    # All look good.
    return True, ""


def run_cmd(cmd):
    """Run subprocess.run() safely and returns the stdout.

    Args:
      cmd: a command line string.

    Returns:
      The stdout outcome of the shell command.
    """
    result = subprocess.run(
        cmd.split(), stdout=subprocess.PIPE, stderr=subprocess.PIPE
    )
    return result.stdout.decode()


def can_run_cmd(cmd):
    """Test if the environment can run cmd.

    Args:
      cmd: a command line string.

    Returns:
      True if the command can run without error.
      False otherwise.
    """
    try:
        subprocess.check_call(
            cmd.split(), stdout=subprocess.PIPE, stderr=subprocess.STDOUT
        )
        return True
    except subprocess.CalledProcessError:
        return False


def invoke_shell(cmd):
    """invoke_shell() uses shell=True to support the command
    that includes shell escape sequences.

    Args:
      cmd: command string, which may include escape sequence.
    """
    print("Invoke shell:" + cmd)
    p = subprocess.Popen(cmd, shell=True)
    p.wait()


def get_interface_names():
    """Get all network interface names of the target except for the loopback.

    Returns:
      A list of interface names.
    """
    result = run_cmd("fx shell tcpdump --list-interfaces")

    names = []
    for line in result.split("\n"):
        if not line:
            break

        names.append(re.match("^\d+\.([\w-]+)\s", line).groups()[0])

    return names


def has_fuzzy_name(name_under_test, names):
    if not name_under_test:
        return False
    for n in names:
        if name_under_test in n:
            return True
    return False


def build_cmd(args):
    """Build cmd line for sniffing and displaying.

    Args:
      args: command line arguments.

    Returns:
      cmd string.
    """

    fx_workflow_filter = (
        ' "not ('
        "port ssh or dst port 8083 or dst port 2345 or port 1900 "
        "or (ip6 and (dst portrange 33330-33341 or dst portrange 33337-33338))"
        ')"'
    )

    # Pay special attention to the escape sequence
    # This command goes through the host shell, and ssh shell.
    cmd_prefix = (
        'tcpdump -l --packet-buffered -i "%s" --no-promiscuous-mode '
        % (args.interface_name)
    )
    cmd_suffix = fx_workflow_filter
    cmd_options = ""
    host_cmd = ""

    output_file = None
    if args.file:
        output_file = "%s%s" % (TARGET_TMP_DIR, args.file)

    # Build more options
    if args.wireshark:
        (result, err_str) = has_wireshark_env()
        if not result:
            msg = (
                "Does not have a working wireshark envirionment. "
                "Note it requires graphical environment "
                "such as X Display: %s" % err_str
            )
            print(msg)
            return
        cmd_options += "-w -"
        if output_file:
            cmd_suffix += " | tee %s" % output_file
        host_cmd += " | wireshark -k -i -"
    elif output_file:
        cmd_options += "-w %s " % output_file

    cmd = "fx shell '" + cmd_prefix + cmd_options + cmd_suffix + "'" + host_cmd

    if args.timeout:
        cmd = ("timeout %ss " % args.timeout) + cmd

    return cmd


def get_keystroke_unblocking():
    """Returns a keystroke in a non-blocking way."""

    fd = sys.stdin.fileno()

    attr_org = termios.tcgetattr(fd)
    flags_org = fcntl.fcntl(fd, fcntl.F_GETFL)

    attr_new = attr_org[::]
    attr_new[3] = attr_new[3] & ~termios.ICANON & ~termios.ECHO
    flags_new = flags_org | os.O_NONBLOCK

    try:
        termios.tcsetattr(fd, termios.TCSANOW, attr_new)
        fcntl.fcntl(fd, fcntl.F_SETFL, flags_new)
        return sys.stdin.read(1)
    finally:
        termios.tcsetattr(fd, termios.TCSAFLUSH, attr_org)
        fcntl.fcntl(fd, fcntl.F_SETFL, flags_org)


def do_sniff(cmd):
    """Run user-interruptible sniffer.

    Args:
      cmd: command string, which may include escape sequence.
    """
    print("Run: {}".format(cmd))
    p = subprocess.Popen(cmd, shell=True)
    while p.poll() is None:
        time.sleep(0.07)  # To tame the CPU cycle consumption
        user_key = get_keystroke_unblocking()
        if user_key in ["q", "c", "Q", "C"]:
            print(" ... forced stop by user ({})".format(user_key))
            run_cmd("fx shell killall tcpdump")
            p.terminate()


def move_out_dumpfile(filename):
    """Move the PCAPNG dump file from the target device to the host device.

    Args:
      filename: filename stored in the target. Empty string if no file was stored.
    """
    if not filename:
        return

    full_path = "%s%s" % (TARGET_TMP_DIR, filename)
    cmd = "cd %s" % os.environ["FUCHSIA_OUT_DIR"]
    cmd += '; fx scp "[$(fx get-device-addr)]:%s" .' % full_path
    cmd += "; fx shell rm -rf %s" % full_path
    invoke_shell(cmd)


def is_target_ready():
    """Tests if the target Fuchsia device is ready to capture packets.

    Returns:
      True if the target is ready. False otherwise.
    """
    if not can_run_cmd("fx shell exit"):
        print("failed to run: the target device unreachable by 'fx shell'")
        return False
    if not can_run_cmd("fx shell tcpdump --version"):
        msg = (
            "failed to run: the target does not have 'tcpdump'. "
            "Build with '--with-base //third_party/tcpdump' "
            "and reload the target"
        )
        print(msg)
        return False
    return True


def main():
    print(
        "WARNING: `fx sniff` is deprecated and will be removed in a future release.\n"
        "Please use the host-side `ffx net capture` plugin or target-side `net capture` tool.\n"
        "See docs/development/debugging/packet_capture.md for details.\n",
        file=sys.stderr,
    )

    if not is_target_ready():
        sys.exit(1)

    iface_names = get_interface_names()
    iface_name_help = "Choose one interface name from: " + ", ".join(
        iface_names
    )

    parser = argparse.ArgumentParser(
        description="Capture packets on the target, Display on the host"
    )

    parser.add_argument(
        "interface_name", nargs="?", default="", help=iface_name_help
    )
    parser.add_argument(
        "-t", "--timeout", default=30, help="Time duration to sniff"
    )
    parser.add_argument(
        "--wireshark", action="store_true", help="Display on Wireshark."
    )
    parser.add_argument(
        "--file",
        type=str,
        default="",
        help="Store PCAPNG file in //out directory. May use with --wireshark option",
    )

    args = parser.parse_args()

    # Sanitize the file name
    if args.file:
        if not args.file.endswith(".pcapng"):
            args.file = args.file + ".pcapng"

    if not has_fuzzy_name(args.interface_name, iface_names):
        print(iface_name_help)
        sys.exit(1)

    do_sniff(build_cmd(args))
    print("\nEnd of fx sniff")

    move_out_dumpfile(args.file)
    sys.exit(0)


if __name__ == "__main__":
    sys.exit(main())

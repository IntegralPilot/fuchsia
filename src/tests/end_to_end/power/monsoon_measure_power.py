#!/usr/bin/env fuchsia-vendored-python
#
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import inspect
import os
import sys


# Now parse our args.  Users may extend the path using arguments as well.
def ParseArgs():
    parser = argparse.ArgumentParser(
        formatter_class=argparse.RawDescriptionHelpFormatter,
        description="Captures power monitor data from a Monsoon HV Power Monitor",
        epilog=inspect.cleandoc(
            """
            Sample replacement behaviors:

            When calibration samples or dropped samples are encountered during capture, users may
            select from one of four different behaviors for each of the types of samples in need of
            replacement (calibration vs. dropped).  They are:

            omit     : Remove the samples from the output completely.
            zero     : Output samples with fixed values of zero for voltage and current.
            repeat   : Repeat the value of the last valid sample (this is the default).
            sentinel : Output samples with a fixed value of (mA, V, raw_aux) == (1000, 12, 1000).

            Hermetic environment issues:

            Note that when running tests directly in the hermetic Fuchsia development environment,
            additional paths may need to be passed to the script in order for it to function
            properly.  Notably, the Monsoon Power Monitor python library (found here
            https://github.com/msoon/PyMonsoon), and libusb (which the Monsoon library depends on)
            may be missing from the environment.

            Passing the -p command line argument to the script can be difficult when it is being
            invoked automatically by the testing framework.  In cases like this, users may make use
            of the MONSOON_PYTHON_LIBS environment variable in order to work around the issue.  It
            contains a ";" separated list of paths which will be added to the location that the
            script will search during imports.

            For example, if a user had checked out the Monsoon python library at `~/monsoon`, and had
            used apt-get to install python's libusb support into their local system, the could set
            their environment using:

            export MONSOON_PYTHON_LIBS="~/monsoon;/usr/lib/python3/dist-packages"

            Before invoking the test.  In addition to getting the paths configured correctly to find
            the extra libraries, the script will need to be told the serial number of the specific
            Monsoon instance they are supposed to talk to.  While this can be passed on the command
            line, the MONSOON_SERIAL environment variable can also be used to configure this
            parameter if access to the command line invocation of the script is impractical.  If the
            user's Monsoon was serial number 12345, the export would be:

            export MONSOON_SERIAL=12345
            """
        ),
    )

    parser.add_argument(
        "-sn",
        "--serialno",
        type=int,
        default=None,
        help="The serial number of the Monsoon to use.",
    )
    parser.add_argument(
        "-format", "--out_format", help="ignored, we only support csv"
    )

    parser.add_argument(
        "-p",
        "--syspath",
        default=[],
        action="append",
        help="Add one or more paths to the locations we look for python libraries.  Can be used to "
        "locate libraries like libusb or the Monsoon python library which may not be present in "
        "the hermetic fuchsia environment",
    )

    parser.add_argument(
        "-out",
        "--csv_out",
        required=True,
        help="defines the file to output csv to",
    )

    parser.add_argument(
        "-dsh",
        "--dropped_sample_handling",
        choices=["omit", "zero", "repeat", "sentinel"],
        default="repeat",
        help="Defines the behavior to use when dealing with detected dropped samples",
    )

    parser.add_argument(
        "-csh",
        "--calibration_sample_handling",
        choices=["omit", "zero", "repeat", "sentinel"],
        default="repeat",
        help="Defines the behavior to use when dealing with inline calibration samples",
    )

    parser.add_argument(
        "-a",
        "--raw_aux",
        action="store_true",
        default=False,
        help="When passed, causes the script to capture and output the raw values of the aux channel"
        " which can be useful for synchronization purposes",
    )

    args = parser.parse_args()

    if args.serialno is None:
        args.serialno = os.environ.get("MONSOON_SERIAL", None)

    if args.serialno is None:
        print(
            "Missing require Monsoon serial number.  Either set the MONSOON_SERIAL environment"
            "variable, or pass the serial number to use with --serialno"
        )
        parser.print_help()
        exit(-1)

    return parser.parse_args()


ARGS = ParseArgs()
sys.path.extend(ARGS.syspath)

import enum
import pprint as pp
import signal
import struct
import time

import Monsoon.HVPM as HVPM
import Monsoon.Operations as op
import Monsoon.pmapi as pmapi


class SampleHandling(enum.Enum):
    OMIT = 1
    ZERO = 2
    REPEAT = 3
    SENTINEL = 4


def LookupSampleHandling(name):
    return SampleHandling.__members__[name.upper()]


class Sample:
    def __init__(self, unpacked_data):
        self.main_coarse = unpacked_data[0]
        self.main_fine = unpacked_data[1]
        self.usb_coarse = unpacked_data[2]
        self.usb_fine = unpacked_data[3]
        self.aux_coarse = unpacked_data[4]
        self.aux_fine = unpacked_data[5]
        self.main_aux_voltage = unpacked_data[6]
        self.usb_voltage = unpacked_data[7]
        self.main_gain = unpacked_data[8]
        self.usb_gain = unpacked_data[9]

    def __str__(self):
        return pp.pformat(self.__dict__)


class Packet:
    def __init__(self, data):
        hdr = struct.unpack_from("HBB", data, 0)
        self.dropped_count = hdr[0]
        self.flags = hdr[1]
        self.sample_count = hdr[2]

        sample_payloads = struct.iter_unpack(">HHHHhhHHBB", data[4:])
        self.samples = [Sample(x) for x in sample_payloads]

    def __str__(self):
        x = "%12s : %5d 0x%04x\n" % (
            "dropped_count",
            self.dropped_count,
            self.dropped_count,
        )
        x += "%12s : %5d 0x%04x\n" % ("flags", self.flags, self.flags)
        x += "%12s : %5d 0x%04x\n" % (
            "sample_count",
            self.sample_count,
            self.sample_count,
        )
        for s in self.samples:
            x += str(s) + "\n"
        return x


class AvgWindow:
    def __init__(self, size):
        self.data = [0.0 for x in range(size)]
        self.ndx = 0
        self.sum = 0
        self.avg = None

    def AddVal(self, val):
        """Updates the averaging window by replacing the oldest value."""
        self.sum += val - self.data[self.ndx]
        self.data[self.ndx] = val
        self.ndx += 1
        if self.ndx >= len(self.data):
            self.ndx = 0
            self.avg = self.sum / len(self.data)

    def val(self):
        return self.avg


class CalibrationWindow:
    def __init__(self, scale, zero_offset):
        self.scale = scale
        self.zero_offset = zero_offset
        self.ref_window = AvgWindow(5)
        self.zero_window = AvgWindow(5)
        self.ready = False

    def AddRefPoint(self, val):
        """Adds a reference point to the reference calibration window."""
        self.ref_window.AddVal(val)
        if (
            self.ref_window.val() is not None
            and self.zero_window.val() is not None
        ):
            self.ready = True

    def AddZeroPoint(self, val):
        """Adds a zero point to the zero calibration window."""
        self.zero_window.AddVal(val)
        if (
            self.ref_window.val() is not None
            and self.zero_window.val() is not None
        ):
            self.ready = True

    def CalibratePoint(self, val):
        """Returns a calibrated current measurement given a raw value and the
        associated average zero and reference values from the calibration windows.
        """
        cal_ref = self.ref_window.val()
        cal_zero = self.zero_window.val()
        if cal_ref is None or cal_zero is None:
            return None

        zero = cal_zero + self.zero_offset
        d = cal_ref - zero
        slope = self.scale / d if d != 0 else 0

        return (val - zero) * slope

    def is_ready(self):
        return self.ready


class CalibrationChannel:
    def __init__(
        self, coarse_scale_off, fine_scale_off, fine_threshold, val_fetcher
    ):
        self.coarse_window = CalibrationWindow(*coarse_scale_off)
        self.fine_window = CalibrationWindow(*fine_scale_off)
        self.fine_threshold = fine_threshold
        self.val_fetcher = val_fetcher

    def is_ready(self):
        return self.coarse_window.is_ready() and self.fine_window.is_ready()

    def CalibrateSample(self, sample):
        """Returns a calibrated current measurement based on a raw sample and
        the averaging windows for the associated channel."""
        coarse, fine = self.val_fetcher(sample)

        # Switch to fine-based measurements for increased accuracy when the
        # fine value drops below the threshold. Raw fine values are in
        # microamps while raw coarse values and the final calibrated value are
        # in milliamps, hence the conversion for fine-based measurements.
        if fine < self.fine_threshold:
            return self.fine_window.CalibratePoint(fine) / 1000
        else:
            return self.coarse_window.CalibratePoint(coarse)

    def AddRefPoint(self, sample):
        """Adds a reference point to the coarse and fine averaging windows."""
        coarse, fine = self.val_fetcher(sample)
        self.coarse_window.AddRefPoint(coarse)
        self.fine_window.AddRefPoint(fine)

    def AddZeroPoint(self, sample):
        """Adds a zero point to the coarse and fine averaging windows."""
        coarse, fine = self.val_fetcher(sample)
        self.coarse_window.AddZeroPoint(coarse)
        self.fine_window.AddZeroPoint(fine)


class Sampler:
    def __init__(self, monitor, outfile):
        self.STOP_NOW = False
        self.monitor = monitor
        self.outfile = outfile
        self.main_cal = CalibrationChannel(
            (
                self.monitor.statusPacket.mainCoarseScale,
                self.monitor.statusPacket.mainCoarseZeroOffset,
            ),
            (
                self.monitor.statusPacket.mainFineScale,
                self.monitor.statusPacket.mainFineZeroOffset,
            ),
            self.monitor.fineThreshold,
            lambda sample: (sample.main_coarse, sample.main_fine),
        )
        self.aux_cal = CalibrationChannel(
            (self.monitor.statusPacket.auxCoarseScale, 0.0),
            (self.monitor.statusPacket.auxFineScale, 0.0),
            self.monitor.auxFineThreshold,
            lambda sample: (sample.aux_coarse, sample.aux_fine),
        )
        # Magic numbers for voltage scaling were taken from
        # https://github.com/msoon/PyMonsoon/blob/master/Monsoon/sampleEngine.py#L66
        self.main_voltage_scale = monitor.mainvoltageScale * (62.5 / 1e6)

        self.orig_sigint = None
        self.orig_sigterm = None

    def sigint_stop(self, *unused_args):
        self.STOP_NOW = True
        signal.signal(signal.SIGINT, self.orig_sigint)
        self.orig_sigint = None

    def sigterm_stop(self, *unused_args):
        self.STOP_NOW = True
        signal.signal(signal.SIGINT, self.orig_sigterm)
        self.orig_sigterm = None

    def sample(self):
        # Write the header to the output file and immediately flush it.  The test
        # framework is going to wait until the first data show up in the output file
        # before it actually starts any tests.
        hdr = "Mandatory Timestamp,Current,Voltage"
        if ARGS.raw_aux:
            hdr += ",Raw Aux"
        self.outfile.write("%s\n" % hdr)
        self.outfile.flush()

        # Set up our signal handlers to catch our shutdown signals, then capture raw
        # packets until we receive one of those signals.  Don't process the packets
        # yet, we will take care of that once capturing is complete.
        self.orig_sigint = signal.signal(signal.SIGINT, self.sigint_stop)
        self.orig_sigterm = signal.signal(signal.SIGTERM, self.sigterm_stop)

        print("Capturing packets until SIGINT")
        payloads = []
        self.monitor.StartSampling()
        while not self.STOP_NOW:
            before = time.monotonic_ns()
            payload = self.monitor.BulkRead()
            after = time.monotonic_ns()
            payloads.append((payload, before, after))
        self.monitor.stopSampling()

        print("Processing %d collected packets" % (len(payloads),))
        first_packet = None
        last_packet = None
        last_packet_sample_count = None
        last_sample = None
        nominal_period_nsec = 200000
        min_raw_aux = None
        samples = []

        def GetHandler(handling_str):
            handling = LookupSampleHandling(handling_str)
            if handling == SampleHandling.OMIT:
                return lambda _: None
            if handling == SampleHandling.ZERO:
                return lambda _: (0, 0, 0, False)
            if handling == SampleHandling.REPEAT:
                return lambda last: last
            if handling == SampleHandling.SENTINEL:
                return lambda _: (1000.0, 12.0, 1000.0, False)

        drop_handler = GetHandler(ARGS.dropped_sample_handling)
        cal_handler = GetHandler(ARGS.calibration_sample_handling)

        # Now that we are done capturing, go back and process the samples filling
        # our calibration windows and producing a list of samples with calibrated
        # currents and translated voltages
        for payload, before, after in payloads:
            packet = Packet(payload)
            packet.cts = after

            if last_packet is not None:
                if last_packet.dropped_count != packet.dropped_count:
                    if last_packet.dropped_count < packet.dropped_count:
                        drop_count = (
                            packet.dropped_count - last_packet.dropped_count
                        )
                        print(
                            "WARNING: dropped %d packets, starting from sample count %d"
                            % (drop_count, len(samples))
                        )
                        if last_sample is not None:
                            filler = drop_handler(last_sample)
                            samples.extend([filler for i in range(drop_count)])
                    else:
                        print(
                            "WARNING: ignoring bad drop count (last %d, this %d)"
                            % (last_packet.dropped_count, packet.dropped_count)
                        )

                last_packet = packet
                last_packet_sample_count = len(samples)

            for s in packet.samples:
                # Identify sample type and process accordingly.
                sample_type = s.usb_gain & 0x30

                # Process measurement samples to determine voltage and current
                if sample_type == op.SampleType.Measurement:
                    if self.main_cal.is_ready() and self.aux_cal.is_ready():
                        current = self.main_cal.CalibrateSample(s)
                        voltage = s.main_aux_voltage * self.main_voltage_scale
                        raw_aux_fine = s.aux_fine

                        if last_sample is None:
                            first_packet = packet
                            last_packet = packet

                        out_sample = last_sample = (
                            current,
                            voltage,
                            raw_aux_fine,
                            True,
                        )
                        min_raw_aux = (
                            raw_aux_fine
                            if min_raw_aux is None
                            else min(raw_aux_fine, min_raw_aux)
                        )
                # Feed caliibration samples (zero and reference) to the
                # averaging windows.
                elif sample_type == op.SampleType.ZeroCal:
                    self.main_cal.AddZeroPoint(s)
                    self.aux_cal.AddZeroPoint(s)
                    out_sample = cal_handler(last_sample)
                elif sample_type == op.SampleType.refCal:
                    self.main_cal.AddRefPoint(s)
                    self.aux_cal.AddRefPoint(s)
                    out_sample = cal_handler(last_sample)
                else:
                    print(
                        "WARNING: skipping sample with bad type (%02x)"
                        % (sample_type,)
                    )

                if last_sample is not None:
                    samples.append(out_sample)

        # Finally, go back and do our best to fix up our timestamps, outputting our
        # samples as we go.

        # Monsoon is _supposed_ to produce samples at a nominal rate of 5 KHz,
        # however, in reality it does not.  It tends to run about 700ppm slow.
        #
        # In order for our data to line up with collected trace data, we need to
        # measure and do our best to fix this.  We use the host clock and the
        # capture time of the packets as our reference here, trusting that the host
        # clock and the target clock are close enough to each other that the drift
        # experienced over the test time is small enough to be tolerable.
        #
        # For now, we simply divide the time between the first and last captured
        # packets with the number of samples present between the two to produce an
        # average sample rate.  This does not account for any drift back and forth
        # during the capture, but at least it will align all of the samples to the
        # trace data over the duration of the capture.
        host_nsec = last_packet.cts - first_packet.cts
        measured_period_nsec = host_nsec / last_packet_sample_count
        ppm = (
            1e6
            * (measured_period_nsec - nominal_period_nsec)
            / nominal_period_nsec
        )

        print(
            "Measured Period is %.3f nSec (nominal %.3f nSec ppm %.3f)"
            % (measured_period_nsec, nominal_period_nsec, ppm)
        )
        print("Minimum raw fine aux current value was %d" % (min_raw_aux,))

        # Write finalized measurements to the output file
        for i, s in enumerate(samples):
            if s is not None:
                common_output = (i * measured_period_nsec, s[0], s[1])
                if ARGS.raw_aux:
                    aux_val = s[2] - (min_raw_aux if s[3] else 0)
                    self.outfile.write(
                        "%d,%.7f,%.7f,%d\n" % (*common_output, aux_val)
                    )
                else:
                    self.outfile.write("%d,%.7f,%.7f\n" % common_output)


def main():
    try:
        # TODO(https://fxbug.dev/485380087): Need a way to find libusb-1.0.so.o on the system.
        # Prime (cache) libusb1's location based on expected default. This is needed
        # because the default location is not being found.
        import usb.backend.libusb1 as libusb1

        backend = libusb1.get_backend(
            find_library=lambda x: "/usr/lib/x86_64-linux-gnu/libusb-1.0.so.0"
        )

        if backend is None:
            raise RuntimeError(
                "No backend found using /usr/lib/x86_64-linux-gnu/libusb-1.0.so.0"
            )

        # Open and close the monitor in case teardown failed to complete previously.
        HVMON = HVPM.Monsoon()
        HVMON.setup_usb(ARGS.serialno, pmapi.USB_protocol())
        HVMON.closeDevice()

        # Create the monitor we will use and read the status packet so we have
        # access to the calibration values.
        HVMON = HVPM.Monsoon()
        HVMON.setup_usb(ARGS.serialno, pmapi.USB_protocol())
        HVMON.fillStatusPacket()

        # Set USB passthrough mode to "Auto", which disables USB while taking
        # measurements.
        # NOTE(hjfreyer): This is an experiment to get the On-Wrist-Idle test
        # case working. We should probably be enabling/disabling USB via a
        # proper DMS API.
        HVMON.setUSBPassthroughMode(2)

        # Create our sampler and tell it to go, outputting to the user specified
        # file as we do.
        sampler = Sampler(HVMON, open(ARGS.csv_out, "w"))
        sampler.sample()

    except OSError as err:
        print("OS Error : " % (err,))
        if err.filename is not None:
            print("Filename : %s" % (err.filename,))

    finally:
        HVMON.closeDevice()

    print("Done")


if __name__ == "__main__":
    main()

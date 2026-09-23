// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include "radio_url.h"

#include <stdio.h>

#include <platform/exit_code.h>

#include "openthread-system.h"

const char *otSysGetRadioUrlHelpString(void) {
#define OT_RADIO_URL_HELP_BUS                          \
  "Radio Url format:"                                  \
  "    {Protocol}://${PATH_TO_DEVICE}?${Parameters}\n" \
  "\n"

#if OPENTHREAD_POSIX_CONFIG_SPINEL_SPI_INTERFACE_ENABLE
#define OT_SPINEL_SPI_RADIO_URL_HELP_BUS                                                                     \
  "Protocol=[spinel+spi*]           Specify the Spinel interface as the Spinel SPI interface\n"              \
  "    spinel+spi://${PATH_TO_SPI_DEVICE}?${Parameters}\n"                                                   \
  "Parameters:\n"                                                                                            \
  "    gpio-int-device[=gpio-device-path]\n"                                                                 \
  "                                  Specify a path to the Linux sysfs-exported GPIO device for the\n"       \
  "                                  `I̅N̅T̅` pin. If not specified, `SPI` interface will fall back to\n" \
  "                                  polling, which is inefficient.\n"                                       \
  "    gpio-int-line[=line-offset]\n"                                                                        \
  "                                  The offset index of `I̅N̅T̅` pin for the associated GPIO device.\n"  \
  "                                  If not specified, `SPI` interface will fall back to polling,\n"         \
  "                                  which is inefficient.\n"                                                \
  "    gpio-reset-dev[=gpio-device-path]\n"                                                                  \
  "                                  Specify a path to the Linux sysfs-exported GPIO device for the\n"       \
  "                                  `R̅E̅S̅` pin.\n"                                                     \
  "    gpio-reset-line[=line-offset]"                                                                        \
  "                                  The offset index of `R̅E̅S̅` pin for the associated GPIO device.\n"  \
  "    spi-mode[=mode]               Specify the SPI mode to use (0-3).\n"                                   \
  "    spi-speed[=hertz]             Specify the SPI speed in hertz.\n"                                      \
  "    spi-cs-delay[=usec]           Specify the delay after C̅S̅ assertion, in µsec.\n"                  \
  "    spi-reset-delay[=ms]          Specify the delay after R̅E̅S̅E̅T̅ assertion, in milliseconds.\n"  \
  "    spi-align-allowance[=n]       Specify the maximum number of 0xFF bytes to clip from start of\n"       \
  "                                  MISO frame. Max value is 16.\n"                                         \
  "    spi-small-packet=[n]          Specify the smallest packet we can receive in a single transaction.\n"  \
  "                                  (larger packets will require two transactions). Default value is 32.\n" \
  "\n"
#else
#define OT_SPINEL_SPI_RADIO_URL_HELP_BUS
#endif  // OPENTHREAD_POSIX_CONFIG_SPINEL_SPI_INTERFACE_ENABLE

#if OPENTHREAD_POSIX_CONFIG_SPINEL_HDLC_INTERFACE_ENABLE
#define OT_SPINEL_HDLC_RADIO_URL_HELP_BUS                                                          \
  "Protocol=[spinel+hdlc*]           Specify the Spinel interface as the Spinel HDLC interface\n"  \
  "    forkpty-arg[=argument string]  Command line arguments for subprocess, can be repeated.\n"   \
  "    spinel+hdlc+uart://${PATH_TO_UART_DEVICE}?${Parameters} for real uart device\n"             \
  "    spinel+hdlc+forkpty://${PATH_TO_UART_DEVICE}?${Parameters} for forking a pty subprocess.\n" \
  "Parameters:\n"                                                                                  \
  "    uart-parity[=even|odd]         Uart parity config, optional.\n"                             \
  "    uart-stop[=number-of-bits]     Uart stop bit, default is 1.\n"                              \
  "    uart-baudrate[=baudrate]       Uart baud rate, default is 115200.\n"                        \
  "    uart-flow-control              Enable flow control, disabled by default.\n"                 \
  "    uart-reset                     Reset connection after hard resetting RCP(USB CDC ACM).\n"   \
  "\n"
#else
#define OT_SPINEL_HDLC_RADIO_URL_HELP_BUS
#endif  // OPENTHREAD_POSIX_CONFIG_SPINEL_HDLC_INTERFACE_ENABLE

#if OPENTHREAD_POSIX_CONFIG_SPINEL_VENDOR_INTERFACE_ENABLE

#ifndef OT_VENDOR_RADIO_URL_HELP_BUS
#define OT_VENDOR_RADIO_URL_HELP_BUS "\n"
#endif  // OT_VENDOR_RADIO_URL_HELP_BUS

#define OT_SPINEL_VENDOR_RADIO_URL_HELP_BUS OT_VENDOR_RADIO_URL_HELP_BUS
#else
#define OT_SPINEL_VENDOR_RADIO_URL_HELP_BUS
#endif  // OPENTHREAD_POSIX_CONFIG_SPINEL_VENDOR_INTERFACE_ENABLE

#if OPENTHREAD_POSIX_CONFIG_MAX_POWER_TABLE_ENABLE
#define OT_RADIO_URL_HELP_MAX_POWER_TABLE                                                                \
  "    max-power-table               Max power for channels in ascending order separated by commas,\n"   \
  "                                  If the number of values is less than that of supported channels,\n" \
  "                                  the last value will be applied to all remaining channels.\n"        \
  "                                  Special value 0x7f disables a channel.\n"
#else
#define OT_RADIO_URL_HELP_MAX_POWER_TABLE
#endif

  return "RadioURL:\n" OT_RADIO_URL_HELP_BUS OT_SPINEL_SPI_RADIO_URL_HELP_BUS
      OT_SPINEL_HDLC_RADIO_URL_HELP_BUS
          OT_SPINEL_VENDOR_RADIO_URL_HELP_BUS OT_RADIO_URL_HELP_MAX_POWER_TABLE
         "    region[=region-code]          Set the radio's region code. The region code must be an\n"
         "                                  ISO 3166 alpha-2 code.\n"
         "    cca-threshold[=dbm]           Set the radio's CCA ED threshold in dBm measured at antenna connector.\n"
         "    enable-coex[=1|0]             If not specified, RCP coex operates with its default configuration.\n"
         "                                  Disable coex with 0, and enable it with other values.\n"
         "    fem-lnagain[=dbm]             Set the Rx LNA gain in dBm of the external FEM.\n"
         "    no-reset                      Do not send Spinel reset command to RCP on initialization.\n"
         "    skip-rcp-compatibility-check  Skip checking RCP API version and capabilities during initialization.\n"
#if OPENTHREAD_CONFIG_MULTIPAN_RCP_ENABLE
         "    iid                           Set the Spinel Interface ID for this process. Valid values are 0-3.\n"
         "    iid-list                      List of IIDs a host can subscribe to receive spinel frames other than \n"
         "                                  provided in 'iid' argument. If not specified, host will subscribe to \n"
         "                                  the interface ID provided in 'iid` argument. Valid values are 0-3. \n"
         "                                  Upto three IIDs can be provided with each IID separated by ',' \n"
         "                                  e.g. iid-list=1,2,3 \n"
#endif
      ;
}

namespace ot {
namespace Posix {

void RadioUrl::Init(const char *aUrl) {
  if (aUrl != nullptr) {
    VerifyOrDie(strnlen(aUrl, sizeof(mUrl)) < sizeof(mUrl), OT_EXIT_INVALID_ARGUMENTS);
    strncpy(mUrl, aUrl, sizeof(mUrl) - 1);
    SuccessOrDie(Url::Url::Init(mUrl));
  }
}

}  // namespace Posix
}  // namespace ot

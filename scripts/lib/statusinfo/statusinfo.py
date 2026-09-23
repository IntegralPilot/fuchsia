# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import datetime
import re
import shutil
import typing
from dataclasses import dataclass

import colorama

_ANSI_REGEX = re.compile(r"\x1b\[[^a-zA-Z]*[a-zA-Z]")


def strip_ansi(input: str) -> str:
    """Strip ANSI escape sequences from the given string.

    Args:
        input (str): String to strip.

    Returns:
        str: Input string with ANSI escape sequences removed

    >>> strip_ansi("\x1b[1mThis text is highlighted\x1b[0m. This is not")
    'This text is highlighted. This is not'
    """

    return _ANSI_REGEX.sub("", input)


def ellipsize(input: str, width: int) -> str:
    """Fits the given string into the given width, possibly with ellipses.

    Ellipsis uses unicode '…'.

    Args:
        input (str): String to fit
        width (int): Maximum width

    Returns:
        str: String that is at most |width| characters.

    >>> ellipsize('hello', 6)
    'hello'

    >>> ellipsize('hello world', 6)
    'hello…'

    >>> a = ellipsize(green_highlight('hello') + ' world', 7)
    >>> b = green_highlight('hello') + ' …'
    >>> a == b
    True

    >>> a = ellipsize(green_highlight('hello') + ' world', 6)
    >>> b = green_highlight('hello') + '…'
    >>> a == b
    True

    >>> a = ellipsize(green_highlight('hello world'), 4)
    >>> b = green_highlight('hel…')
    >>> a == b
    True

    >>> bytes(ellipsize('abc ' + green_highlight('defg'), 6).encode())
    b'abc \\x1b[32m\\x1b[1md\\xe2\\x80\\xa6\\x1b[0m'
    """
    if not _ANSI_REGEX.search(input):
        # No ANSI escapes, simply ensure the string is truncated
        # to the correct length, and add ellipsis if the string was
        # truncated.
        if len(input) > width:
            return input[: width - 1] + "…"
        else:
            return input
    else:
        # ANSI escapes present, now it gets tricky.
        # Iterate over each character in the string, treating any
        # escape sequences as a single character. All escape sequences
        # are printed verbatim, but individual characters are only
        # printed if they will fit in the given width. When we reach
        # the end of the allowed width, write a final character and
        # keep track of where we were. If another printable character
        # is encountered we will replace the previously printed
        # character with an ellipsis and not print any more. This
        # ensures that if printable characters exactly fit in the
        # width, we do not need an ellipsis. If they do not fit,
        # then the ellipsis will have the same styling as the last
        # printable character.
        @dataclass
        class Character:
            char: str
            is_ansi: bool

        def each_character(input: str) -> typing.Iterator[Character]:
            match: re.Match[str]
            cur_index: int = 0
            for match in _ANSI_REGEX.finditer(input):
                for ch in input[cur_index : match.start()]:
                    yield Character(ch, False)
                yield Character(match.group(), True)
                cur_index = match.end()
            for ch in input[cur_index:]:
                yield Character(ch, False)

        total_chars: int = 0
        output: str = ""
        insert_ellipsis_index: int | None = None
        for ch in each_character(input):
            if ch.is_ansi:
                output += ch.char
            elif total_chars == width - 1:
                insert_ellipsis_index = len(output)
                output += ch.char
                total_chars += 1
            elif total_chars == width and insert_ellipsis_index is not None:
                output = (
                    output[0:insert_ellipsis_index]
                    + "…"
                    + output[insert_ellipsis_index + 1 :]
                )
                insert_ellipsis_index = None
            elif total_chars >= width:
                pass
            else:
                output += ch.char
                total_chars += 1
        return output


def format_duration(seconds: float) -> str:
    """Format a duration of seconds for display.

    Args:
        seconds (float): Number of seconds to format.

    Returns:
        str: Seconds formatted as a string.

    >>> format_duration(10)
    '10.0s'

    >>> format_duration(60)
    '1:00.0'

    >>> format_duration(32.33)
    '32.3s'

    >>> format_duration(1.2)
    '1.2s'

    >>> format_duration(134.31)
    '2:14.3'
    """
    if seconds >= 60:
        return f"{int(seconds/60)}:{seconds % 60:04.1f}"
    else:
        return f"{seconds % 60:.1f}s"


def _split_by_weights(weights: list[int], size: int) -> list[int]:
    """Split the given size into an array of sizes by the given weights.

    Weights do not need to sum to a specific number, and the overall size
    is distributed proportionally to each element.

    The output list will be the same length as the weight list.

    Args:
        weights (list[int]): How much to weight each element of the array.
        size (int): The total length being distributed.

    Returns:
        list[int]: Array containing amount of |size| assigned to each weight.

    >>> _split_by_weights([10, 20, 70], 100)
    [10, 20, 70]
    >>> _split_by_weights([10, 1, 10, 1, 30], 80)
    [15, 1, 15, 1, 46]
    >>> _split_by_weights([48, 1, 48], 80)
    [39, 1, 39]
    """
    total = sum(weights)

    output = [int(size * weight / total) for weight in weights]
    if sum(output) < size:
        # See if we can redistribute some of the remaining size to
        # any element that was zeroed out.
        remaining = size - sum(output)
        zeroes = len([x for x in output if x == 0])
        if zeroes:
            # Check that we have at least 1 remaining character for each
            # zeroed element.
            per_zero = remaining // zeroes
            if per_zero > 0:
                output = [o + 1 if o == 0 else o for o in output]
    return output


def _make_progress_bar(proportion: float, width: int) -> str:
    """Make a nicely formatted ASCII progress bar.

    Args:
        proportion (float): The proportion of the bar to fill.
        width (int): The width of the bar.

    Raises:
        ValueError: Proportion is greater than 1.0
        ValueError: Width is less than 3

    Returns:
        str: Formatted progress bar.

    >>> _make_progress_bar(0, 10)
    '[        ]'

    >>> _make_progress_bar(.01, 10)
    '[>       ]'

    >>> _make_progress_bar(.49, 10)
    '[===>    ]'

    >>> _make_progress_bar(.5, 10)
    '[====>   ]'

    >>> _make_progress_bar(.98, 10)
    '[=======>]'

    >>> _make_progress_bar(1, 10)
    '[========]'

    >>> _make_progress_bar(2, 10)
    Traceback (most recent call last):
      ...
    ValueError: Proportion must be out of 1.0

    >>> _make_progress_bar(-1, 10)
    Traceback (most recent call last):
      ...
    ValueError: Proportion must be out of 1.0

    >>> _make_progress_bar(1, 2)
    Traceback (most recent call last):
      ...
    ValueError: Width must be at least 3
    """
    if proportion > 1.0 or proportion < 0:
        raise ValueError("Proportion must be out of 1.0")
    if width < 3:
        raise ValueError("Width must be at least 3")

    width -= 2

    filled = int(proportion * width)

    ret = ""
    if filled == width:
        ret = "=" * width
    elif filled == 0 and proportion == 0:
        ret = " " * width
    elif filled == 0:
        ret = ">" + " " * (width - 1)
    else:
        ret = "=" * (filled) + ">" + " " * (width - filled - 1)
    return f"[{ret}]"


def _pad_to_size(input: str, width: int, left: bool = False) -> str:
    """Ensure that the input takes exactly the given width of characters.

    Args:
        input (str): A string to pad
        width (int): The final string length
        left (bool, default False): If true, pad from the left.

    Returns:
        str: Padded string.

    >>> _pad_to_size('hello', 8)
    'hello   '

    >>> _pad_to_size('world', 4)
    'wor…'

    >>> _pad_to_size('again', 8, left=True)
    '   again'

    >>> _pad_to_size('😀 emoji', 7, left=True)
    '😀 emoji'

    >>> _pad_to_size('😀 emoji', 6, left=True)
    '😀 emo…'

    >>> bytes(_pad_to_size(green_highlight('Hello'), 4), 'utf-8')
    b'\\x1b[32m\\x1b[1mHel\\xe2\\x80\\xa6\\x1b[0m'

    >>> bytes(_pad_to_size(green_highlight('    Hello'), 10), 'utf-8')
    b'\\x1b[32m\\x1b[1m    Hello\\x1b[0m '
    """
    input = ellipsize(input, width)
    input_len = len(strip_ansi(input))
    if input_len < width:
        if not left:
            input += " " * (width - input_len)
        if left:
            input = " " * (width - input_len) + input

    return input


def _wrap(style_list: list[typing.Any], string: str, style: bool = True) -> str:
    """Wrap a string in a style, resetting the style after the string is printed.

    Args:
        style_list (list[typing.Any]): List of colorama styles to wrap with.
        string (str): String to wrap.
        style (bool, optional): If False, don't actually style. Defaults to True.

    Returns:
        str: Styled string.

    >>> bytes(_wrap([colorama.Fore.GREEN], 'Hello'), 'utf-8')
    b'\\x1b[32mHello\\x1b[0m'

    >>> bytes(_wrap([colorama.Fore.GREEN], 'Hello', style=False), 'utf-8')
    b'Hello'
    """
    if style:
        return "".join(style_list + [string]) + colorama.Style.RESET_ALL
    else:
        return string


def highlight(input: str, style: bool = True) -> str:
    """Highlight the input string.

    Args:
        input (str): String to highlight.
        style (bool, optional): If False, don't actually style. Defaults to True.

    Returns:
        str: Styled string.
    """
    return _wrap([colorama.Style.BRIGHT], input, style=style)


def error_highlight(input: str, style: bool = True) -> str:
    """Color the input string red and highlight it.

    Args:
        input (str): String to highlight.
        style (bool, optional): If False, don't actually style. Defaults to True.

    Returns:
        str: Styled string.
    """
    return _wrap([colorama.Style.BRIGHT, colorama.Fore.RED], input, style=style)


def warning(input: str, style: bool = True) -> str:
    """Color the input string yellow.

    Args:
        input (str): String to style.
        style (bool, optional): If False, don't actually style. Defaults to True.

    Returns:
        str: Styled string.
    """
    return _wrap([colorama.Fore.YELLOW], input, style=style)


def green(input: str, style: bool = True) -> str:
    """Color the input string green.

    Args:
        input (str): String to style.
        style (bool, optional): If False, don't actually style. Defaults to True.

    Returns:
        str: Styled string.
    """
    return _wrap([colorama.Fore.GREEN], input, style=style)


def green_highlight(input: str, style: bool = True) -> str:
    """Color the input string green and make it highlighted.

    Args:
        input (str): String to highlight.
        style (bool, optional): If False, don't actually style. Defaults to True.

    Returns:
        str: Styled string.
    """
    return _wrap(
        [colorama.Fore.GREEN, colorama.Style.BRIGHT], input, style=style
    )


def dim(input: str, style: bool = True) -> str:
    """Dim the input string.

    Args:
        input (str): String to dim.
        style (bool, optional): If False, don't actually style. Defaults to True.

    Returns:
        str: Styled string.
    """
    return _wrap([colorama.Style.DIM], input, style=style)


def duration_progress(
    name: str,
    duration: datetime.timedelta,
    width: typing.Union[int, None] = None,
    style: bool = True,
) -> str:
    """Create a pretty duration bar.

    This is used for operations that may take an indefinite amount
    of time. Instead of showing a progress bar, simply display the
    duration of time elapsed.

    Args:
        name (str): Name to display on the line.
        duration (datetime.timedelta): The duration to format and display.
        width (typing.Union[int, None], optional): If set, limit
            to this width. Default is to use the screen width.
        style (bool, optional): Use color only if this is True.

    Returns:
        str: Pretty formatted line.

    >>> duration_progress('Testing', datetime.timedelta(microseconds=3201123), width=70, style=False)
    'Testing                                                [3.2s]        '
    >>> duration_progress('Testing a really really long string that will be truncated', datetime.timedelta(microseconds=3201123), width=70, style=False)
    'Testing a really really long string that will be tr…   [3.2s]        '
    """
    width = width or shutil.get_terminal_size().columns
    shape = _split_by_weights([75, 5, 20], width)
    label_width, padding_width, duration_width = shape

    return (
        _pad_to_size(name, label_width)
        + _pad_to_size("", padding_width)
        + dim(
            _pad_to_size(
                f"[{format_duration(duration.total_seconds())}]",
                duration_width,
            ),
            style=style,
        )
    )


def status_progress(
    label: str,
    proportion: float,
    width: typing.Union[int, None] = None,
    style: bool = True,
) -> str:
    """Create a pretty status progress bar.

    Args:
        label (str): Label for the progress bar.
        proportion (float): Proportion of the bar to be filled, out of 1.0.
        width (int, optional): Width of the bar to output. Default is to use the current terminal size.

    Returns:
        str: Pretty printed progress bar.

    >>> strip_ansi(status_progress("Downloading foo", .3, 80))
    'Downloading foo     [===============>                                  ]   30.0%'

    >>> strip_ansi(status_progress("Downloading everything ever", .04, 40))
    'Download… [>                       ] 4.…'

    >>> strip_ansi(status_progress("Downloading foo", .3, 80))
    'Downloading foo     [===============>                                  ]   30.0%'
    """
    width = width or shutil.get_terminal_size().columns
    shape = _split_by_weights([25, 65, 10], width)
    label_width, bar_width, info_width = shape

    pct = f"{100.0*proportion:4.1f}%"
    return (
        highlight(_pad_to_size(label, label_width - 1), style=style)
        + " "
        + _make_progress_bar(proportion, bar_width)
        + _pad_to_size(pct, info_width, left=True)
    )


if __name__ == "__main__":
    import doctest

    doctest.testmod()

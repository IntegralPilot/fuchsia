#!/usr/bin/env fuchsia-vendored-python
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
import logging
import os
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from urllib import request

# prompt 1 - get key lines to highlight in stack trace.
_PROMPT_GET_KEY_LINES = (
    "You are an expert Fuchsia OS developer's assistant.\n"
    "Your task is to analyze the following test failure stack trace and return a\n"
    "JSON object. Your response MUST be ONLY a valid, minified JSON object.\n"
    "\n"
    "The JSON object must have the following structure:\n"
    # Doubled braces here to escape them for .format()
    "{{\n"
    '  "primary_key_line": "...",\n'
    '  "secondary_key_lines": ["...", "..."]\n'
    # And doubled braces here
    "}}\n"
    "\n"
    "INSTRUCTIONS:\n"
    "1. `primary_key_line`: You MUST identify the single most important line\n"
    "   that indicates the root cause. This line MUST be an exact, VERBATIM\n"
    "   copy of a line from the stack trace. It is the single line a\n"
    "   developer would click on to debug the error. \n"
    "   **IT MUST BE AN ACTUAL LINE FROM THE LOG.**\n"
    "   **DO NOT** return generic summaries like '[FAILED] tests::it_works',\n"
    "   'test failed.', 'fatal exception', or 'thread X failed...'.\n"
    "   The primary line MUST be actionable. A developer needs to be able to look\n"
    "   at the line and know where to start debugging. Generic messages like\n"
    "   'thread panicked' are useless. The line must contain a file path, a line\n"
    "   number, or a specific error that directly points to the code that failed.\n"
    "   Triple-check this. Think from the point of a developer: will this line\n"
    "   give them additional insight, or is it something obvious they already know?\n"
    "   **DO** select the specific line with the PANIC, the assertion error, or\n"
    "   the file path and line number (e.g., '[...][it_works] ERROR: [...]').\n"
    "\n"
    "2. `secondary_key_lines`: You MUST identify a list of 0 or more other\n"
    "   lines that provide helpful context. Use this for cases of uncertainty\n"
    "   or to highlight related symptoms. This value MUST be an array of\n"
    "   strings, each copied verbatim. If there are no other useful lines,\n"
    "   return an empty array: [].\n"
    "\n"
    "Stack Trace:\n"
    "{stack_trace}\n"
)

# prompt 2 - pass git diff as well to get root cause analysis.
_PROMPT_ANALYZE_FAILURE_V2 = (
    "\nYou are an expert Fuchsia OS developer's assistant. Your task is to\n"
    "analyze a test failure and provide a concise, standardized debugging\n"
    "report. Format your entire response for a plain text terminal. Do not use\n"
    "any markdown.\n"
    "\n"
    "Your response MUST strictly follow this format, using these exact headers:\n"
    "\n"
    "## POTENTIAL ERROR\n"
    "Analyze the stack trace and git diff to determine the most likely root\n"
    "cause. Provide ONLY the root cause analysis. Keep the analysis as brief\n"
    "as possible, ideally under 4 lines, but expand if necessary to include\n"
    "all relevant context.\n"
    "\n"
    "IMPORTANT: Do not include the original stack trace or git diff in your\n"
    "response. Your output must ONLY contain the analysis sections, starting\n"
    "with the '## POTENTIAL ERROR' header.\n"
    "\n"
    "---\n"
    "CONTEXT\n"
    "---\n"
    "\n"
    "Stack Trace:\n"
    "{stack_trace}\n"
    "\n"
    "Git Diff:\n"
    "{git_diff}\n"
)

# prompt 3 - get files for gemini analysis
_PROMPT_VERBOSITY_3_ASK_FOR_FILES = (
    "\nYou are an automated debugging assistant. Analyze the provided stack\n"
    "trace, git diff, and current working directory (PWD).\n"
    "Your task is to identify which files, if any, you need to read to provide\n"
    "a complete diagnosis.\n"
    "Respond ONLY with a valid JSON array of strings, where each string is a\n"
    "file path relative to the PWD.\n"
    'Example: ["src/foo/bar.cc", "BUILD.gn"]\n'
    "If you do not need to read any files, respond with an empty array: [].\n"
    "\n"
    "PWD: {pwd}\n"
    "---\n"
    "STACK TRACE:\n"
    "{stack_trace}\n"
    "---\n"
    "GIT DIFF:\n"
    "{git_diff}\n"
    "---\n"
)

# Prompt 3 (Perform): Get analysis for verbosity 3 (log + diff + files).
_PROMPT_ANALYZE_FAILURE_V3_PERFORM = (
    "\nYou are an expert Fuchsia OS developer's assistant. Your task is to\n"
    "analyze a test failure and provide a concise, standardized debugging\n"
    "report. Format your entire response for a plain text terminal. Do not use\n"
    "any markdown.\n"
    "\n"
    "Your response MUST strictly follow this format, using these exact headers:\n"
    "\n"
    "## ROOT CAUSE ANALYSIS\n"
    "Analyze the stack trace, git diff, and any provided file contents to\n"
    "determine the most likely root cause. Clearly state the error type (e.g.,\n"
    "Null Pointer Exception, Race Condition, Assertion Failure) and explain the\n"
    "logic that leads to the failure. Keep the analysis as brief as possible,\n"
    "ideally under 4 lines, but expand if necessary to include all relevant\n"
    "context.\n"
    "\n"
    "## SUGGESTED FIX\n"
    "Provide a concise, best-practice code suggestion to resolve the issue.\n"
    "If the fix is uncertain, suggest the most logical next step for debugging\n"
    "(e.g., \"Add a log statement to check the value of 'my_var' before the\n"
    "call to 'do_thing()'\"). When providing a code suggestion, include the\n"
    "relevant code snippet with comments explaining the proposed change.\n"
    "\n"
    "IMPORTANT: Do not include the original stack trace, git diff, or file\n"
    "contents in your response. Your output must ONLY contain the analysis\n"
    "sections, starting with the '## ROOT CAUSE ANALYSIS' header.\n"
    "\n"
    "---\n"
    "CONTEXT\n"
    "---\n"
    "\n"
    "Stack Trace:\n"
    "{stack_trace}\n"
    "\n"
    "Git Diff:\n"
    "{diff_section}\n"
    "\n"
    "File Contents:\n"
    "{file_contents_context}\n"
)


@dataclass
class GeminiAnalysisResult:
    """A structured result from a call to the Gemini API."""

    text: str
    error: bool = False


def _blocking_gemini_call(
    api_key: str, gemini_model: str, data: bytes
) -> GeminiAnalysisResult:
    """synchronous function to make the web request."""
    logging.info("Making Gemini API call with data: %s", data.decode("utf-8"))
    url = f"https://generativelanguage.googleapis.com/v1beta/models/{gemini_model}:generateContent?key={api_key}"
    headers = {"Content-Type": "application/json"}
    req = request.Request(url, data=data, headers=headers, method="POST")
    try:
        with request.urlopen(req, timeout=45) as response:
            if response.status < 200 or response.status >= 300:
                error_result = GeminiAnalysisResult(
                    f"API Error: {response.status} {response.reason}",
                    error=True,
                )
                logging.error("Gemini API call failed: %s", error_result.text)
                return error_result
            response_body = response.read().decode("utf-8")
            logging.info("Gemini API response: %s", response_body)
            response_json = json.loads(response_body)

            candidates = response_json.get("candidates", [])
            if candidates:
                content = candidates[0].get("content", {})
                parts = content.get("parts", [])
                if parts:
                    success_result = GeminiAnalysisResult(
                        parts[0].get(
                            "text",
                            "Error: Could not extract text from response.",
                        )
                    )
                    logging.info(
                        "Successfully extracted text from Gemini response: %s",
                        success_result.text,
                    )
                    return success_result
            error_result = GeminiAnalysisResult(
                "Error: Unexpected response format.", error=True
            )
            logging.error("Failed to parse Gemini response: %s", response_body)
            return error_result
    except Exception as e:
        error_result = GeminiAnalysisResult(
            f"Error during API call: {e}", error=True
        )
        logging.error("Exception during Gemini API call: %s", e)
        return error_result


def _call_gemini_with_prompt(
    api_key: str, gemini_model: str, prompt: str, **kwargs: str
) -> GeminiAnalysisResult:
    """Formats a prompt, creates a payload, and calls the Gemini API."""
    formatted_prompt = prompt.format(**kwargs)
    payload = {"contents": [{"parts": [{"text": formatted_prompt}]}]}
    data = json.dumps(payload).encode("utf-8")
    return _blocking_gemini_call(api_key, gemini_model, data)


def _read_files_for_gemini(requested_files: list[str]) -> str:
    if not requested_files:
        return ""
    file_contents = {}
    logging.info("Reading files for Gemini: %s", requested_files)
    for file_path in requested_files:
        if not os.path.isfile(file_path):
            print(
                f"Gemini analysis warning: Path '{file_path}' is not a valid file.",
                file=sys.stderr,
            )
            logging.warning(
                "Warning: Path '%s' is not a valid file.", file_path
            )
            continue

        try:
            # Skip files larger than 1MB.
            if os.path.getsize(file_path) > 1_000_000:
                print(
                    f"Gemini analysis warning: File '{file_path}' is too large (>1MB), skipping.",
                    file=sys.stderr,
                )
                logging.warning(
                    "Warning: File '%s' is too large (>1MB), skipping.",
                    file_path,
                )
                continue

            # Read in binary mode to check for null bytes (binary indicator). This is a heuristic and may not always be accurate.
            with open(file_path, "rb") as f:
                raw_content = f.read()

            if b"\0" in raw_content:
                print(
                    f"Gemini analysis warning: File '{file_path}' appears to be binary, skipping.",
                    file=sys.stderr,
                )
                logging.warning(
                    "Warning: File '%s' appears to be binary, skipping.",
                    file_path,
                )
                continue

            # Decode as UTF-8 if not binary.
            content = raw_content.decode("utf-8", errors="replace")
            file_contents[file_path] = content
            logging.info("Content of %s:\n%s", file_path, content)
        except Exception as e:
            print(
                f"Gemini analysis warning: Could not read file at '{file_path}': {e}, skipping.",
                file=sys.stderr,
            )
            logging.warning(
                "Warning: Could not read file at '%s': %s, skipping.",
                file_path,
                e,
            )

    context_block = "\nADDITIONAL FILE CONTEXT:\n---\n"
    for path, content in file_contents.items():
        context_block += f"Content of file '{path}':\n{content}\n---\n"
    return context_block


def get_annotated_log(
    api_key: str,
    gemini_model: str,
    stack_trace: str,
) -> GeminiAnalysisResult:
    """Calls Gemini to get only the key lines for annotation."""
    return _call_gemini_with_prompt(
        api_key,
        gemini_model,
        _PROMPT_GET_KEY_LINES,
        stack_trace=stack_trace,
    )


def get_failure_analysis(
    api_key: str,
    gemini_model: str,
    stack_trace: str,
    git_diff: str | None,
    pwd: str,
    verbosity: int,
) -> GeminiAnalysisResult:
    """Gets the full failure analysis report (for v2 or v3)."""
    if verbosity == 2:
        return _call_gemini_with_prompt(
            api_key,
            gemini_model,
            _PROMPT_ANALYZE_FAILURE_V2,
            stack_trace=stack_trace,
            git_diff=git_diff or "No local changes.",
        )

    # Verbosity level 3
    file_list_result = _call_gemini_with_prompt(
        api_key,
        gemini_model,
        _PROMPT_VERBOSITY_3_ASK_FOR_FILES,
        pwd=pwd,
        stack_trace=stack_trace,
        git_diff=git_diff or "No local changes.",
    )

    if file_list_result.error:
        return file_list_result

    requested_files = []
    try:
        cleaned_response = (
            file_list_result.text.strip()
            .removeprefix("```json")
            .removesuffix("```")
            .strip()
        )
        parsed_json = json.loads(cleaned_response)
        if isinstance(parsed_json, list):
            requested_files = [str(item) for item in parsed_json]
    except (json.JSONDecodeError, TypeError):
        print(
            f"Could not parse file list from Gemini: {file_list_result.text}",
            file=sys.stderr,
        )

    file_contents_context = _read_files_for_gemini(requested_files)
    diff_section = (
        f"Recent code changes that might be related (git diff HEAD):\n---\n{git_diff}\n---\n"
        if git_diff
        else ""
    )

    return _call_gemini_with_prompt(
        api_key,
        gemini_model,
        _PROMPT_ANALYZE_FAILURE_V3_PERFORM,
        stack_trace=stack_trace,
        diff_section=diff_section,
        file_contents_context=file_contents_context,
    )


def _print_colorized_log(error_log: str, annotation_text: str) -> None:
    """Parses annotation JSON and prints the colorized log."""
    COLOR_RED = "\033[91m"  # red for primary
    COLOR_YELLOW = "\033[93m"  # yellow for secondary
    COLOR_RESET = "\033[0m"

    primary_line = ""
    secondary_lines = set()

    try:
        # clean up text in case of markdown or other noise
        cleaned_text = (
            annotation_text.strip()
            .removeprefix("```json")
            .removesuffix("```")
            .strip()
        )
        data = json.loads(cleaned_text)

        # get primary line and strip for comparison
        primary_line = data.get("primary_key_line", "").strip()

        # get secondary lines and strip them for comparison
        secondary_lines = set(
            s.strip() for s in data.get("secondary_key_lines", []) if s.strip()
        )
    except (json.JSONDecodeError, TypeError):
        logging.warning(
            "Could not parse JSON from annotation, falling back to raw text: %s",
            annotation_text,
        )
        # fallback: treat the whole response as the single primary line
        primary_line = annotation_text.strip()

    # print the original log, colorizing key lines
    for line in error_log.splitlines():
        stripped_line = line.strip()
        if stripped_line and stripped_line == primary_line:
            print(f"{COLOR_RED}{line}{COLOR_RESET}")
        elif stripped_line and stripped_line in secondary_lines:
            print(f"{COLOR_YELLOW}{line}{COLOR_RESET}")
        else:
            print(line)

    # if no new line add one
    if error_log and not error_log.endswith("\n"):
        print()


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Analyze test failures with Gemini."
    )
    parser.add_argument("--api-key", required=True, help="Gemini API key.")
    parser.add_argument(
        "--gemini-model",
        default="gemini-2.5-flash-lite",
        help="The Gemini model to use for the analysis.",
    )
    parser.add_argument(
        "--verbosity",
        type=int,
        default=2,
        choices=range(1, 4),
        help="Verbosity level (1-3).",
    )
    args = parser.parse_args()

    # set up logging to a persistent temporary file
    log_file = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="w",
            delete=False,  # persist the file on exit
            suffix=".log",
            prefix="gemini_analysis_",
            encoding="utf-8",
        ) as log_file_obj:
            log_file = log_file_obj.name

        logging.basicConfig(
            filename=log_file,
            level=logging.DEBUG,
            format="%(asctime)s - %(levelname)s - %(message)s",
            filemode="w",  # overwrite on each run
            force=True,
        )
        logging.info("Starting Gemini analysis with args: %s", args)
        logging.info("Logging to: %s", log_file)
    except Exception as e:
        print(f"Error setting up logging: {e}", file=sys.stderr)
        log_file = None  # continue without logging if setup fails

    # run git diff and capture the output
    git_diff = None
    if args.verbosity > 1:
        try:
            result = subprocess.run(
                ["git", "diff", "HEAD"],
                capture_output=True,
                text=True,
                check=False,
            )
            if result.stdout:
                git_diff = result.stdout
                logging.info("Git diff:\n%s", git_diff)
            if result.stderr:
                print("--- git diff stderr ---", file=sys.stderr)
                print(result.stderr, file=sys.stderr)
                logging.warning("Git diff stderr:\n%s", result.stderr)
        except FileNotFoundError:
            print("git command not found, skipping git diff.", file=sys.stderr)
            logging.warning("git command not found, skipping git diff.")
        except Exception as e:
            print(
                f"An error occurred while running git diff: {e}",
                file=sys.stderr,
            )
            logging.error("An error occurred while running git diff: %s", e)

    # read stdin for the error log
    error_log = sys.stdin.read()
    logging.info("Error log from stdin:\n%s", error_log)

    # get key lines stack trace first
    annotation = get_annotated_log(
        args.api_key,
        args.gemini_model,
        error_log,
    )

    if annotation.error:
        # if error print error log
        print(error_log, end="")
        print(f"\nGemini annotation failed: {annotation.text}", file=sys.stderr)
        if log_file:
            print(f"See {log_file} for details.", file=sys.stderr)
        sys.exit(1)

    # print colorized log
    _print_colorized_log(error_log, annotation.text)
    logging.info("Annotation output:\n%s", annotation.text)

    # get full analysis
    if args.verbosity > 1:
        # print the status message here, so user knows
        print("\nRunning Gemini analysis...", file=sys.stderr)

        pwd = os.getcwd()
        analysis = get_failure_analysis(
            args.api_key,
            args.gemini_model,
            error_log,
            git_diff,
            pwd,
            args.verbosity,
        )

        if analysis.error:
            print(f"Gemini analysis failed: {analysis.text}", file=sys.stderr)
            if log_file:
                print(f"See {log_file} for details.", file=sys.stderr)
            sys.exit(1)
        else:
            print("--- Gemini Failure Analysis ---")
            print(analysis.text)
            logging.info("Final analysis output:\n%s", analysis.text)
    else:
        # verbosity 1 is done, no further analysis needed
        logging.info("Analysis complete (annotation only).")


if __name__ == "__main__":
    main()

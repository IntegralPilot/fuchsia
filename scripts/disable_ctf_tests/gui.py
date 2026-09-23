# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import re
import sys
import tkinter as tk
import webbrowser
from tkinter import messagebox as mb
from tkinter import ttk
from typing import Any

import cli
import data_structure
import util


class GUI:
    def __init__(self) -> None:
        self.cli: cli.CLI | None = None

    def inform(self, out: util.Outputs) -> None:
        if not self.cli:
            self.cli = cli.CLI()
        self.cli.inform(out)

    def test_list(self, tests: data_structure.Tests) -> None:
        GUIWithTests(tests).run()

    def bail(self, failure_message: str) -> None:
        print(failure_message)
        sys.exit(1)


class GUIWithTests:
    def __init__(self, tests: data_structure.Tests) -> None:
        self.tests = tests
        self.cli: cli.CLI | None = None
        self.unsaved = False
        self.window = Window()
        self.tree = Tree(self)
        self.edit_pane = EditPane(self, self.tree)
        self.edit_pane.display("Select a test case")
        self.control_pane = ControlPane(self)
        self.tree.fill(self.select_test_suites())
        self.window.window.bind("<Control-s>", self.save)
        self.window.window.bind("<Control-q>", self.try_quit)

    def save(self, event: Any = None) -> None:
        self.tests.save()
        self.unsaved = False

    def run(self) -> None:
        self.window.mainloop()

    def select_test_suites(
        self, filter: str = "", disabled_only: bool = False
    ) -> list[data_structure.TestSuite]:
        test_list = self.tests.tests
        if filter == "" and not disabled_only:
            return test_list
        filtered_suites: list[data_structure.TestSuite] = []
        for t in test_list:
            if filter in t.compact_name and not disabled_only:
                filtered_suites.append(t)
            else:
                filtered_cases = []
                for c in t.cases:
                    if filter in c.name and not (
                        disabled_only and not c.disabled
                    ):
                        filtered_cases.append(c)
                if filtered_cases:
                    filtered_suites.append(
                        data_structure.TestSuite(
                            name=t.name, cases=filtered_cases
                        )
                    )
        return filtered_suites

    def set_unsaved(self) -> None:
        self.unsaved = True

    def try_quit(self, _: Any = None) -> None:
        if self.unsaved:
            response = mb.askyesnocancel(
                "Unsaved changes!",
                "Do you want to save before quitting?",
                default=mb.YES,
                icon=mb.QUESTION,
            )
            if response is None:
                return
            if response:
                self.save()
        sys.exit(0)

    def filter_tree(self, filter_string: str, disabled_only: bool) -> None:
        self.tree.fill(self.select_test_suites(filter_string, disabled_only))


DISPLAY_DISABLED = "  Disabled"
DISPLAY_ENABLED = "        -"


class Tree:
    def __init__(self, state: GUIWithTests) -> None:
        self.state = state
        self.cases_lookup: dict[str, data_structure.TestCase] = {}
        t = ttk.Treeview(columns=("disabled", "bug"))
        self.tree = t
        t.heading("#0", text="Name")
        t.heading("disabled", text="Disabled")
        t.heading("bug", text="Bug")
        t.column("#0", width=300)
        t.column("disabled", width=80, stretch=tk.NO)
        t.column("bug", width=140)
        t.bind("<<TreeviewSelect>>", self.selected)
        t.pack(fill=tk.BOTH, expand=True)

    def fill(self, tests: list[data_structure.TestSuite]) -> None:
        for item in self.tree.get_children():
            self.tree.delete(item)

        self.cases_lookup = {}
        for t in tests:
            t_tkid = self.tree.insert("", tk.END, text=t.compact_name)
            self.tree.item(t_tkid, open=True)
            for c in t.cases:
                case_id = self.tree.insert(
                    t_tkid,
                    tk.END,
                    text=c.name,
                    values=(self.disabled_string(c.disabled), c.bug),
                )
                self.cases_lookup[case_id] = c

    def selected_name_and_values(self) -> tuple[str, Any]:
        item_and_foo = self.tree.selection()
        if len(item_and_foo) != 1:
            return "", []
        item = item_and_foo[0]
        name = self.tree.item(item, "text")
        values = self.tree.item(item, "values")
        return name, values

    def selected(self, _: Any) -> None:
        self.tree.after_idle(self.deferred_selected)

    def deferred_selected(self) -> None:
        name, values = self.selected_name_and_values()
        if len(values) != 2:
            self.state.edit_pane.display(name)
        else:
            self.state.edit_pane.edit_tree_selection()

    def edit_values(self) -> tuple[str, bool, str]:
        name, values = self.selected_name_and_values()
        if len(values) != 2:
            return name, False, ""
        disabled_string, bug = values
        parent_item = self.tree.parent(self.tree.selection()[0])
        parent_name = self.tree.item(parent_item, "text")
        return f"{parent_name}:{name}", disabled_string == DISPLAY_DISABLED, bug

    def update(self, disabled: bool, bug: str) -> None:
        item = self.tree.selection()[0]
        self.tree.item(item, values=(self.disabled_string(disabled), bug))
        case = self.cases_lookup[item]
        case.disabled = disabled
        case.bug = bug
        self.state.set_unsaved()

    def disabled_string(self, disabled: bool) -> str:
        return DISPLAY_DISABLED if disabled else DISPLAY_ENABLED


class EditPane:
    def __init__(self, state: GUIWithTests, tree: Tree) -> None:
        self.frame = ttk.Frame(state.window.window)
        # subframe holds the 2nd line of the EditPane with several widgets in it.
        self.subframe = ttk.Frame(self.frame)
        self.disable_widget_actions = False
        self.label_contents = tk.StringVar(value="")
        self.label = ttk.Label(self.frame, textvariable=self.label_contents)
        # We want label above subframe
        self.label.pack(side="top", anchor="w")
        self.checkbox_value = tk.BooleanVar(value=False)
        self.checkbox = ttk.Checkbutton(
            self.subframe,
            text="Disable?",
            variable=self.checkbox_value,
            onvalue=True,
            offvalue=False,
            command=self.update,
        )
        self.checkbox.grid(row=0, column=0, padx=20, sticky="w")
        self.b = ttk.Label(self.subframe, text="Bug:")
        self.b.grid(row=0, column=1, padx=(20, 0), sticky="w")
        self.edit_value = tk.StringVar(value="")
        self.editor = ttk.Entry(
            self.subframe, textvariable=self.edit_value, width=40
        )
        self.editor.grid(row=0, column=2, padx=(0, 20), sticky="ew")
        self.edit_value.trace_add("write", self.update)
        self.clickable_url = ttk.Label(self.subframe, text="")
        self.clickable_url.grid(row=0, column=3, sticky="w")
        self.subframe.pack(
            side="bottom", fill=tk.X, expand=False, anchor="w", padx=20, pady=20
        )
        self.frame.pack(
            side="bottom", fill=tk.X, expand=False, anchor="w", padx=20, pady=20
        )
        self.tree = tree
        self.editing = False

    def update(self, _1: Any = None, _2: Any = None, _3: Any = None) -> None:
        if self.editing and not self.disable_widget_actions:
            bug = self.edit_value.get()
            self.update_clickable_url(bug)
            self.tree.update(self.checkbox_value.get(), bug)

    def done_with_programmatic_change(self) -> None:
        """Start treating UI updates as user actions.

        The program should not call this to balance
        start_programmatic_change(). Instead, start_programmatic_change() will
        queue this up to be called after Tkinter has drained all the events
        associated with the programmatic change."""

        self.disable_widget_actions = False

    def start_programmatic_change(self) -> None:
        """Don't treat UI updates as user actions.

        This is needed because Tkinter processes events in a strange order, and
        a checkbox may still be False after I've told it to be True. If I display
        a new test that's disabled, and the previous test was not disabled, then
        setting the checkbox to disabled might cause an update() call on the tree,
        which would check the checkbox and find disabled=False still.

        So this function should be called when the program is modifying the UI
        values. It will queue up setting disable_widget_actions to False after
        all the event-loop stuff has finished."""

        self.disable_widget_actions = True
        self.frame.after_idle(self.done_with_programmatic_change)

    def update_clickable_url(self, bug: str | None) -> None:
        def open_url_in_browser(url: str) -> None:
            try:
                webbrowser.open_new_tab(url)
            except webbrowser.Error as e:
                print(f"Could not open URL: {e}")

        if bug is not None:
            bug_text = re.search(r"\bb/(\d+)\b", bug)
            if bug_text:
                url = f"fxbug.dev/{bug_text.group(1)}"
                self.clickable_url.config(
                    text=url, foreground="blue", cursor="hand2"
                )
                # Python footgun: Beware of capturing the wrong version of the
                # variable in the lambda. In this case, it's OK to use
                # the latest, but if I were putting up several different
                # URLs I'd want to do something like:
                #   lambda event, bound_url_value=f"https://{url}": \
                #       open_url_in_browser(bound_url_value)
                self.clickable_url.bind(
                    "<Button-1>",
                    lambda event: open_url_in_browser(f"https://{url}"),
                )
                return
        self.clickable_url.config(text="", cursor="")
        self.clickable_url.unbind("<Button-1>")

    def display(self, name: str) -> None:
        self.start_programmatic_change()
        self.update_clickable_url(None)
        self.editing = False
        self.label_contents.set(name)
        self.checkbox.config(state="disabled")
        self.editor.config(state="disabled")
        self.b.config(foreground="gray")

    def edit_tree_selection(self) -> None:
        self.start_programmatic_change()
        text, disabled, bug = self.tree.edit_values()
        self.update_clickable_url(bug)
        self.label_contents.set(text)
        self.edit_value.set(bug)
        self.checkbox_value.set(disabled)
        self.checkbox.config(state="normal")
        self.editor.config(state="normal")
        self.b.config(foreground="black")
        self.editing = True


class ControlPane:
    def __init__(self, state: GUIWithTests) -> None:
        self.state = state
        self.frame = ttk.Frame(state.window.window)
        self.filter_label = ttk.Label(self.frame, text="Filter:")
        self.filter_label.pack(side="left", padx=(20, 0))
        self.filter_value = tk.StringVar(value="")
        self.filter_entry = ttk.Entry(
            self.frame, textvariable=self.filter_value
        )
        self.filter_entry.pack(side="left", padx=(0, 20))
        self.filter_value.trace_add("write", self.update)
        self.checkbox_value = tk.BooleanVar(value=False)
        self.checkbox = ttk.Checkbutton(
            self.frame,
            text="Disabled Only",
            variable=self.checkbox_value,
            onvalue=True,
            offvalue=False,
            command=self.update,
        )
        self.checkbox.pack(side="left", padx=20)
        self.save_button = ttk.Button(
            self.frame, text="Save ^S", command=self.state.save
        )
        self.save_button.pack(side="left", padx=20)
        self.quit_button = ttk.Button(
            self.frame, text="Quit ^Q", command=state.try_quit
        )
        self.quit_button.pack(side="left", padx=20)
        self.frame.pack(side="bottom", padx=20, pady=20)

    def update(self, *_) -> None:  # type: ignore
        self.state.filter_tree(
            self.filter_value.get(), self.checkbox_value.get()
        )


class Window:
    def __init__(self) -> None:
        w = tk.Tk()
        w.title("CTF Test Disabler")
        w.geometry("1000x600")
        w.update_idletasks()
        # w.geometry(f"800x{w.winfo_reqheight()}")
        self.window = w

    def mainloop(self) -> None:
        self.window.mainloop()

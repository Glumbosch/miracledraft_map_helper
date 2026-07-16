#!/usr/bin/env python3
"""Small Tkinter GUI for decoding Wonderdraft SVG wd:record metadata."""

from __future__ import annotations

import base64
import binascii
import re
from pathlib import Path
import sys
import tkinter as tk
from tkinter import filedialog, messagebox, ttk


WD_RECORD_PATTERN = re.compile(
    r"\bwd:record\s*=\s*([\"'])(.*?)\1",
    flags=re.DOTALL,
)


def extract_record_values(text: str) -> list[str]:
    """Return wd:record values from SVG/XML, or treat the input as one raw value."""
    matches = [match.group(2) for match in WD_RECORD_PATTERN.finditer(text)]
    if matches:
        return matches
    raw = text.strip().strip("\"'")
    return [raw] if raw else []


def decode_wd_record(encoded: str) -> str:
    """Decode unpadded URL-safe (or standard) Base64 into UTF-8 Godot text."""
    compact = "".join(encoded.split())
    if not compact:
        raise ValueError("The wd:record value is empty.")
    padded = compact + "=" * (-len(compact) % 4)
    try:
        decoded = base64.b64decode(padded, altchars=b"-_", validate=True)
    except (binascii.Error, ValueError) as error:
        raise ValueError(f"Invalid Base64 metadata: {error}") from error
    try:
        return decoded.decode("utf-8")
    except UnicodeDecodeError as error:
        raise ValueError("The decoded wd:record is not valid UTF-8.") from error


def decode_input(text: str) -> list[str]:
    """Decode every wd:record attribute found in an SVG or one pasted raw value."""
    values = extract_record_values(text)
    if not values:
        raise ValueError("Paste a wd:record value or SVG content first.")
    decoded = []
    for index, value in enumerate(values, start=1):
        try:
            decoded.append(decode_wd_record(value))
        except ValueError as error:
            if len(values) == 1:
                raise
            raise ValueError(f"Record {index}: {error}") from error
    return decoded


def format_decoded_records(records: list[str]) -> str:
    if len(records) == 1:
        return records[0]
    sections = []
    for index, record in enumerate(records, start=1):
        sections.append(f"----- wd:record {index} of {len(records)} -----\n{record}")
    return "\n\n".join(sections)


class DecoderApp:
    def __init__(self, root: tk.Tk) -> None:
        self.root = root
        self.root.title("Wonderdraft wd:record Decoder")
        self.root.minsize(720, 560)
        self.status = tk.StringVar(value="Paste a wd:record value or open an SVG file.")

        container = ttk.Frame(root, padding=12)
        container.grid(row=0, column=0, sticky="nsew")
        root.rowconfigure(0, weight=1)
        root.columnconfigure(0, weight=1)
        container.rowconfigure(1, weight=2)
        container.rowconfigure(4, weight=3)
        container.columnconfigure(0, weight=1)

        ttk.Label(
            container,
            text='Encoded input (raw value, wd:record="…" attribute, or complete SVG)',
        ).grid(row=0, column=0, sticky="w")
        self.input_text = self._text_area(container, row=1)

        input_buttons = ttk.Frame(container)
        input_buttons.grid(row=2, column=0, sticky="ew", pady=(8, 12))
        ttk.Button(input_buttons, text="Open SVG…", command=self.open_svg).pack(
            side="left"
        )
        ttk.Button(input_buttons, text="Paste", command=self.paste).pack(
            side="left", padx=(6, 0)
        )
        ttk.Button(input_buttons, text="Decode", command=self.decode).pack(
            side="left", padx=(6, 0)
        )
        ttk.Button(input_buttons, text="Clear", command=self.clear).pack(
            side="left", padx=(6, 0)
        )

        ttk.Label(container, text="Decoded Godot text").grid(
            row=3, column=0, sticky="w"
        )
        self.output_text = self._text_area(container, row=4)
        self.output_text.configure(state="disabled")

        output_buttons = ttk.Frame(container)
        output_buttons.grid(row=5, column=0, sticky="ew", pady=(8, 0))
        ttk.Button(output_buttons, text="Copy decoded", command=self.copy_output).pack(
            side="left"
        )
        ttk.Button(output_buttons, text="Save decoded…", command=self.save_output).pack(
            side="left", padx=(6, 0)
        )
        ttk.Label(output_buttons, textvariable=self.status).pack(
            side="right", padx=(12, 0)
        )

        root.bind("<Control-Return>", lambda _event: self.decode())
        root.bind("<Control-o>", lambda _event: self.open_svg())
        root.bind("<Control-s>", lambda _event: self.save_output())
        self.input_text.focus_set()

    @staticmethod
    def _text_area(parent: ttk.Frame, row: int) -> tk.Text:
        frame = ttk.Frame(parent)
        frame.grid(row=row, column=0, sticky="nsew", pady=(4, 0))
        frame.rowconfigure(0, weight=1)
        frame.columnconfigure(0, weight=1)
        text = tk.Text(frame, wrap="none", font="TkFixedFont", undo=True)
        vertical = ttk.Scrollbar(frame, orient="vertical", command=text.yview)
        horizontal = ttk.Scrollbar(frame, orient="horizontal", command=text.xview)
        text.configure(yscrollcommand=vertical.set, xscrollcommand=horizontal.set)
        text.grid(row=0, column=0, sticky="nsew")
        vertical.grid(row=0, column=1, sticky="ns")
        horizontal.grid(row=1, column=0, sticky="ew")
        return text

    def set_output(self, value: str) -> None:
        self.output_text.configure(state="normal")
        self.output_text.delete("1.0", "end")
        self.output_text.insert("1.0", value)
        self.output_text.configure(state="disabled")

    def decode(self) -> None:
        try:
            records = decode_input(self.input_text.get("1.0", "end"))
        except ValueError as error:
            self.status.set("Decode failed.")
            messagebox.showerror("Could not decode wd:record", str(error), parent=self.root)
            return
        self.set_output(format_decoded_records(records))
        noun = "record" if len(records) == 1 else "records"
        self.status.set(f"Decoded {len(records)} {noun}.")

    def open_svg(self) -> None:
        selected = filedialog.askopenfilename(
            parent=self.root,
            title="Open SVG",
            filetypes=(("SVG files", "*.svg"), ("All files", "*")),
        )
        if not selected:
            return
        try:
            content = Path(selected).read_text(encoding="utf-8")
        except (OSError, UnicodeError) as error:
            messagebox.showerror("Could not open SVG", str(error), parent=self.root)
            return
        self.input_text.delete("1.0", "end")
        self.input_text.insert("1.0", content)
        self.decode()

    def paste(self) -> None:
        try:
            clipboard = self.root.clipboard_get()
        except tk.TclError:
            self.status.set("The clipboard does not contain text.")
            return
        self.input_text.delete("1.0", "end")
        self.input_text.insert("1.0", clipboard)
        self.decode()

    def clear(self) -> None:
        self.input_text.delete("1.0", "end")
        self.set_output("")
        self.status.set("Cleared.")

    def copy_output(self) -> None:
        value = self.output_text.get("1.0", "end").strip()
        if not value:
            self.status.set("There is no decoded text to copy.")
            return
        self.root.clipboard_clear()
        self.root.clipboard_append(value)
        self.status.set("Decoded text copied.")

    def save_output(self) -> None:
        value = self.output_text.get("1.0", "end").strip()
        if not value:
            self.status.set("There is no decoded text to save.")
            return
        selected = filedialog.asksaveasfilename(
            parent=self.root,
            title="Save decoded Godot text",
            defaultextension=".txt",
            filetypes=(("Text files", "*.txt"), ("All files", "*")),
        )
        if not selected:
            return
        try:
            Path(selected).write_text(value + "\n", encoding="utf-8")
        except OSError as error:
            messagebox.showerror("Could not save decoded text", str(error), parent=self.root)
            return
        self.status.set(f"Saved {Path(selected).name}.")


def self_test() -> None:
    expected = '{\n"width": 18.0\n}'
    encoded = "ewoid2lkdGgiOiAxOC4wCn0"
    assert decode_wd_record(encoded) == expected
    assert decode_wd_record(encoded + "=") == expected
    assert decode_input(f'<path wd:record="{encoded}"/>') == [expected]
    assert format_decoded_records([expected]) == expected
    print("wd_record_decoder self-test passed")


def main() -> None:
    if "--self-test" in sys.argv:
        self_test()
        return
    root = tk.Tk()
    DecoderApp(root)
    root.mainloop()


if __name__ == "__main__":
    main()

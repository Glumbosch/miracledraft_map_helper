#!/usr/bin/env python3
"""Desktop editor for Wonderdraft .wonderdraft_map files.

Features:
- Open/decompress Wonderdraft Godot GCPF/FastLZ map files.
- Edit the complete Godot var2str-like map text.
- Export/import SVG interchange files containing labels, symbols, and paths.
- Resolve custom assets and default sprites from configurable folders.
- Export embedded Image objects as PNG and replace them from edited PNGs.
- Re-encode the Godot Variant tree and save a compressed .wonderdraft_map.

Requires Python 3.10+. Tkinter is normally included with Python.
Pillow is required for PNG import and preview: pip install pillow
"""
from __future__ import annotations

import array
import base64
import gc
import io
import json
import math
import os
import platform
import re
import shutil
import struct
import sys
import tempfile
import tkinter as tk
import urllib.parse
import xml.etree.ElementTree as ET
import zlib
from collections import Counter, OrderedDict
from dataclasses import dataclass
from pathlib import Path
from tkinter import filedialog, messagebox, ttk
from xml.sax.saxutils import escape, quoteattr
from typing import Any, BinaryIO, Callable, Iterator

try:
    from PIL import Image, ImageTk
except ImportError:
    Image = None
    ImageTk = None

# Reuse the proven decoder and PNG writer from the extraction utility.
try:
    from wonderdraft_extract import (
        GDValue,
        FormatError,
        decode_store_var_stream,
        decompress_gcpf,
        fastlz_decompress_block,
        image_object_info,
        to_godot_text,
        write_png,
    )
except ImportError as exc:
    raise SystemExit(
        "wonderdraft_extract.py must be in the same directory as this script"
    ) from exc

GCPF_MAGIC = b"GCPF"
TYPE_IDS = {
    "Nil": 0, "Bool": 1, "Int": 2, "Real": 3, "String": 4,
    "Vector2": 5, "Rect2": 6, "Vector3": 7, "Transform2D": 8,
    "Plane": 9, "Quat": 10, "AABB": 11, "Basis": 12,
    "Transform": 13, "Color": 14, "NodePath": 15, "RID": 16,
    "Object": 17, "Dictionary": 18, "Array": 19, "PoolByteArray": 20,
    "PoolIntArray": 21, "PoolRealArray": 22, "PoolStringArray": 23,
    "PoolVector2Array": 24, "PoolVector3Array": 25, "PoolColorArray": 26,
}
FLAG_64 = 1 << 16


# ------------------------- Low-memory binary storage -------------------------

@dataclass(frozen=True)
class DiskByteArray:
    """A byte array that remains in a temporary file instead of Python RAM."""

    path: Path
    offset: int
    length: int

    def __len__(self) -> int:
        return self.length

    def iter_chunks(self, chunk_size: int = 1024 * 1024) -> Iterator[bytes]:
        with self.path.open("rb", buffering=0) as handle:
            handle.seek(self.offset)
            remaining = self.length
            while remaining:
                chunk = handle.read(min(chunk_size, remaining))
                if not chunk:
                    raise FormatError(f"Unexpected end of disk-backed byte array in {self.path}")
                remaining -= len(chunk)
                yield chunk

    def read_slice(self, start: int, size: int) -> bytes:
        if start < 0 or size < 0 or start + size > self.length:
            raise ValueError("DiskByteArray slice is outside the stored range")
        with self.path.open("rb", buffering=0) as handle:
            handle.seek(self.offset + start)
            data = handle.read(size)
        if len(data) != size:
            raise FormatError(f"Unexpected end of disk-backed byte array in {self.path}")
        return data


ByteSource = bytes | bytearray | memoryview | DiskByteArray


def _source_length(source: ByteSource) -> int:
    return len(source)


def _source_chunks(source: ByteSource, chunk_size: int = 1024 * 1024) -> Iterator[bytes | memoryview]:
    if isinstance(source, DiskByteArray):
        yield from source.iter_chunks(chunk_size)
        return
    view = memoryview(source)
    for start in range(0, len(view), chunk_size):
        yield view[start:start + chunk_size]


def _source_slice(source: ByteSource, start: int, size: int) -> bytes | memoryview:
    if isinstance(source, DiskByteArray):
        return source.read_slice(start, size)
    return memoryview(source)[start:start + size]


def decompress_gcpf_path(
    source_path: Path,
    destination_path: Path,
    progress: Callable[[int, int], None] | None = None,
) -> dict[str, Any]:
    """Stream a GCPF file to disk, keeping only one compressed block in RAM."""
    file_size = source_path.stat().st_size
    with source_path.open("rb", buffering=1024 * 1024) as source:
        header = source.read(16)
        if len(header) != 16:
            raise FormatError("File is too small to be a GCPF container")
        magic, mode, block_size, uncompressed_size = struct.unpack("<4sIII", header)
        if magic != GCPF_MAGIC:
            raise FormatError(f"Invalid GCPF magic: {magic!r}")
        if mode != 0:
            raise FormatError(f"Unsupported GCPF compression mode {mode}; only FastLZ mode 0 is supported")
        if block_size <= 0:
            raise FormatError("Invalid GCPF block size")

        block_count = uncompressed_size // block_size + 1
        sizes = array.array("I")
        try:
            sizes.fromfile(source, block_count)
        except EOFError as exc:
            raise FormatError("Truncated GCPF block-size table") from exc
        if sys.byteorder != "little":
            sizes.byteswap()

        destination_path.parent.mkdir(parents=True, exist_ok=True)
        written = 0
        with destination_path.open("wb", buffering=1024 * 1024) as destination:
            report_step = max(1, block_count // 200)
            for index, compressed_size in enumerate(sizes):
                compressed = source.read(compressed_size)
                if len(compressed) != compressed_size:
                    raise FormatError(f"Truncated GCPF block {index}")
                expected = block_size
                if index == block_count - 1:
                    expected = uncompressed_size - block_size * (block_count - 1)
                block = fastlz_decompress_block(compressed, expected)
                if len(block) != expected:
                    raise FormatError(
                        f"GCPF block {index} decoded to {len(block)} bytes; expected {expected}"
                    )
                destination.write(block)
                written += len(block)
                if progress and (index % report_step == 0 or index + 1 == block_count):
                    progress(written, uncompressed_size)

        if source.read(4) != GCPF_MAGIC:
            raise FormatError("Missing trailing GCPF magic")
        if source.read(1):
            raise FormatError("Unexpected bytes after trailing GCPF magic")

    if written != uncompressed_size:
        raise FormatError("GCPF uncompressed-size mismatch")
    return {
        "container": "GCPF",
        "compression_mode": mode,
        "compression_name": "FastLZ",
        "block_size": block_size,
        "block_count": block_count,
        "uncompressed_size": uncompressed_size,
        "compressed_size": file_size,
    }


class Godot3VariantFileParser:
    """Decode a Godot 3 Variant directly from a seekable file.

    PoolByteArray payloads are represented by DiskByteArray references, so the
    three large Wonderdraft image buffers never become Python bytes objects.
    """

    TYPE_NAMES = {
        0: "Nil", 1: "Bool", 2: "Int", 3: "Real", 4: "String",
        5: "Vector2", 6: "Rect2", 7: "Vector3", 8: "Transform2D",
        9: "Plane", 10: "Quat", 11: "AABB", 12: "Basis",
        13: "Transform", 14: "Color", 15: "NodePath", 16: "RID",
        17: "Object", 18: "Dictionary", 19: "Array", 20: "PoolByteArray",
        21: "PoolIntArray", 22: "PoolRealArray", 23: "PoolStringArray",
        24: "PoolVector2Array", 25: "PoolVector3Array", 26: "PoolColorArray",
    }

    def __init__(self, handle: BinaryIO, backing_path: Path, payload_length: int):
        self.handle = handle
        self.backing_path = backing_path
        self.payload_start = handle.tell()
        self.payload_length = payload_length
        self.position = 0
        self.type_counts: Counter[str] = Counter()

    def _need(self, size: int) -> None:
        if self.position + size > self.payload_length:
            raise FormatError(
                f"Unexpected end of Godot Variant data at offset {self.position}; need {size} bytes"
            )

    def _read(self, size: int) -> bytes:
        self._need(size)
        data = self.handle.read(size)
        if len(data) != size:
            raise FormatError("Unexpected end of decompressed Variant file")
        self.position += size
        return data

    def _skip(self, size: int) -> None:
        self._need(size)
        self.handle.seek(size, os.SEEK_CUR)
        self.position += size

    def _u32(self) -> int:
        return struct.unpack("<I", self._read(4))[0]

    def _i32(self) -> int:
        return struct.unpack("<i", self._read(4))[0]

    def _i64(self) -> int:
        return struct.unpack("<q", self._read(8))[0]

    def _f32(self) -> float:
        return struct.unpack("<f", self._read(4))[0]

    def _f64(self) -> float:
        return struct.unpack("<d", self._read(8))[0]

    def _raw_string(self) -> str:
        length = self._u32()
        raw = self._read(length)
        padding = (-length) % 4
        if padding:
            self._skip(padding)
        try:
            return raw.decode("utf-8")
        except UnicodeDecodeError as exc:
            raise FormatError("Invalid UTF-8 string in Godot Variant") from exc

    def parse(self, depth: int = 0) -> Any:
        if depth > 100:
            raise FormatError("Godot Variant nesting is unreasonably deep")
        header_offset = self.position
        header = self._u32()
        type_id = header & 0xFF
        flags = header & ~0xFF
        name = self.TYPE_NAMES.get(type_id)
        if name is None:
            raise FormatError(f"Unknown Godot Variant type {type_id} at offset {header_offset}")
        self.type_counts[name] += 1

        if type_id == 0:
            return None
        if type_id == 1:
            return bool(self._u32())
        if type_id == 2:
            return self._i64() if flags & FLAG_64 else self._i32()
        if type_id == 3:
            return self._f64() if flags & FLAG_64 else self._f32()
        if type_id == 4:
            return self._raw_string()

        vector_lengths = {5: 2, 6: 4, 7: 3, 8: 6, 9: 4, 10: 4, 11: 6, 12: 9, 13: 12, 14: 4}
        if type_id in vector_lengths:
            return GDValue(name, tuple(self._f32() for _ in range(vector_lengths[type_id])))

        if type_id == 15:
            name_count_field = self._u32()
            if not (name_count_field & 0x80000000):
                raise FormatError("Old-format Godot NodePath is unsupported")
            name_count = name_count_field & 0x7FFFFFFF
            subname_count = self._u32()
            node_flags = self._u32()
            if node_flags & 2:
                subname_count += 1
            names = [self._raw_string() for _ in range(name_count)]
            subnames = [self._raw_string() for _ in range(subname_count)]
            return GDValue("NodePath", {"names": names, "subnames": subnames, "absolute": bool(node_flags & 1)})

        if type_id == 16:
            return GDValue("RID", None)

        if type_id == 17:
            if flags & FLAG_64:
                return GDValue("ObjectID", self._i64())
            class_name = self._raw_string()
            if class_name == "":
                return None
            property_count = self._u32()
            props: OrderedDict[str, Any] = OrderedDict()
            for _ in range(property_count):
                property_name = self._raw_string()
                props[property_name] = self.parse(depth + 1)
            return GDValue("Object", {"class": class_name, "properties": props})

        if type_id == 18:
            count = self._u32() & 0x7FFFFFFF
            result: OrderedDict[Any, Any] = OrderedDict()
            for _ in range(count):
                key = self.parse(depth + 1)
                result[key] = self.parse(depth + 1)
            return result

        if type_id == 19:
            count = self._u32() & 0x7FFFFFFF
            return [self.parse(depth + 1) for _ in range(count)]

        if type_id == 20:
            count = self._u32()
            self._need(count)
            absolute_offset = self.handle.tell()
            self._skip(count)
            padding = (-count) % 4
            if padding:
                self._skip(padding)
            return GDValue("PoolByteArray", DiskByteArray(self.backing_path, absolute_offset, count))

        if type_id == 21:
            count = self._u32()
            return GDValue("PoolIntArray", [self._i32() for _ in range(count)])
        if type_id == 22:
            count = self._u32()
            return GDValue("PoolRealArray", [self._f32() for _ in range(count)])
        if type_id == 23:
            count = self._u32()
            return GDValue("PoolStringArray", [self._raw_string() for _ in range(count)])
        if type_id == 24:
            count = self._u32()
            return GDValue("PoolVector2Array", [(self._f32(), self._f32()) for _ in range(count)])
        if type_id == 25:
            count = self._u32()
            return GDValue("PoolVector3Array", [(self._f32(), self._f32(), self._f32()) for _ in range(count)])
        if type_id == 26:
            count = self._u32()
            return GDValue("PoolColorArray", [tuple(self._f32() for _ in range(4)) for _ in range(count)])
        raise AssertionError("Unreachable")


def decode_store_var_file(path: Path) -> tuple[Any, dict[str, Any]]:
    total_size = path.stat().st_size
    with path.open("rb", buffering=1024 * 1024) as handle:
        prefix = handle.read(4)
        if len(prefix) != 4:
            raise FormatError("Decompressed data is too small to contain a Variant")
        payload_length = struct.unpack("<I", prefix)[0]
        if payload_length != total_size - 4:
            raise FormatError(
                f"Variant length prefix says {payload_length}, but {total_size - 4} bytes follow"
            )
        parser = Godot3VariantFileParser(handle, path, payload_length)
        result = parser.parse()
        if parser.position != payload_length:
            raise FormatError(
                f"Variant decoder consumed {parser.position} of {payload_length} bytes"
            )
    return result, {
        "variant_payload_size": payload_length,
        "variant_type_counts": dict(sorted(parser.type_counts.items())),
    }


def image_object_info(value: Any) -> tuple[int, int, str, bool, ByteSource] | None:
    """Like the extractor helper, but accepts disk-backed PoolByteArray data."""
    if not isinstance(value, GDValue) or value.type_name != "Object":
        return None
    if value.value.get("class") != "Image":
        return None
    properties = value.value.get("properties", {})
    image_data = properties.get("data")
    if not isinstance(image_data, dict):
        return None
    byte_array = image_data.get("data")
    if not isinstance(byte_array, GDValue) or byte_array.type_name != "PoolByteArray":
        return None
    source = byte_array.value
    if not isinstance(source, (bytes, bytearray, memoryview, DiskByteArray)):
        return None
    return (
        int(image_data["width"]), int(image_data["height"]), str(image_data["format"]),
        bool(image_data.get("mipmaps", False)), source,
    )


# --------------------------- Godot Variant encoding ---------------------------

def _pad4(data: bytes) -> bytes:
    return data + b"\x00" * ((-len(data)) % 4)


def _raw_string(value: str) -> bytes:
    raw = value.encode("utf-8")
    return struct.pack("<I", len(raw)) + _pad4(raw)


def _header(type_name: str, flags: int = 0) -> bytes:
    return struct.pack("<I", TYPE_IDS[type_name] | flags)


def encode_variant(value: Any) -> bytes:
    if value is None:
        return _header("Nil")
    if isinstance(value, bool):
        return _header("Bool") + struct.pack("<I", int(value))
    if isinstance(value, int):
        if -(2**31) <= value < 2**31:
            return _header("Int") + struct.pack("<i", value)
        return _header("Int", FLAG_64) + struct.pack("<q", value)
    if isinstance(value, float):
        return _header("Real") + struct.pack("<f", value)
    if isinstance(value, str):
        return _header("String") + _raw_string(value)

    if isinstance(value, list):
        return _header("Array") + struct.pack("<I", len(value)) + b"".join(
            encode_variant(item) for item in value
        )

    if isinstance(value, dict):
        parts = [_header("Dictionary"), struct.pack("<I", len(value))]
        for key, item in value.items():
            parts.append(encode_variant(key))
            parts.append(encode_variant(item))
        return b"".join(parts)

    if not isinstance(value, GDValue):
        raise FormatError(f"Cannot encode value of type {type(value).__name__}")

    name, data = value.type_name, value.value
    vector_lengths = {
        "Vector2": 2, "Rect2": 4, "Vector3": 3, "Transform2D": 6,
        "Plane": 4, "Quat": 4, "AABB": 6, "Basis": 9,
        "Transform": 12, "Color": 4,
    }
    if name in vector_lengths:
        vals = tuple(data)
        if len(vals) != vector_lengths[name]:
            raise FormatError(f"{name} needs {vector_lengths[name]} values")
        return _header(name) + struct.pack("<" + "f" * len(vals), *vals)

    if name == "Object":
        class_name = data["class"]
        props = data["properties"]
        parts = [_header("Object"), _raw_string(class_name), struct.pack("<I", len(props))]
        for prop_name, prop_value in props.items():
            parts.extend((_raw_string(str(prop_name)), encode_variant(prop_value)))
        return b"".join(parts)

    if name == "PoolByteArray":
        raw = bytes(data)
        return _header(name) + struct.pack("<I", len(raw)) + _pad4(raw)
    if name == "PoolIntArray":
        return _header(name) + struct.pack("<I", len(data)) + struct.pack(
            "<" + "i" * len(data), *data
        )
    if name == "PoolRealArray":
        return _header(name) + struct.pack("<I", len(data)) + struct.pack(
            "<" + "f" * len(data), *data
        )
    if name == "PoolStringArray":
        return _header(name) + struct.pack("<I", len(data)) + b"".join(
            _raw_string(v) for v in data
        )
    if name in {"PoolVector2Array", "PoolVector3Array", "PoolColorArray"}:
        components = {"PoolVector2Array": 2, "PoolVector3Array": 3, "PoolColorArray": 4}[name]
        flat = [component for item in data for component in item]
        return _header(name) + struct.pack("<I", len(data)) + struct.pack(
            "<" + "f" * (len(data) * components), *flat
        )
    if name == "RID":
        return _header(name)
    if name == "NodePath":
        names = data.get("names", [])
        subnames = data.get("subnames", [])
        flags = 1 if data.get("absolute") else 0
        parts = [
            _header(name), struct.pack("<I", 0x80000000 | len(names)),
            struct.pack("<I", len(subnames)), struct.pack("<I", flags),
        ]
        parts.extend(_raw_string(v) for v in names)
        parts.extend(_raw_string(v) for v in subnames)
        return b"".join(parts)
    raise FormatError(f"Unsupported GDValue type for encoding: {name}")


def encode_store_var_stream(value: Any) -> bytes:
    payload = encode_variant(value)
    return struct.pack("<I", len(payload)) + payload


def _raw_string_size(value: str) -> int:
    length = len(value.encode("utf-8"))
    return 4 + length + (-length) % 4


def variant_encoded_size(value: Any) -> int:
    """Return the binary Variant size without materializing its byte payload."""
    if value is None:
        return 4
    if isinstance(value, bool):
        return 8
    if isinstance(value, int):
        return 8 if -(2**31) <= value < 2**31 else 12
    if isinstance(value, float):
        return 8
    if isinstance(value, str):
        return 4 + _raw_string_size(value)
    if isinstance(value, list):
        return 8 + sum(variant_encoded_size(item) for item in value)
    if isinstance(value, dict):
        return 8 + sum(variant_encoded_size(k) + variant_encoded_size(v) for k, v in value.items())
    if not isinstance(value, GDValue):
        raise FormatError(f"Cannot encode value of type {type(value).__name__}")

    name, data = value.type_name, value.value
    vector_lengths = {
        "Vector2": 2, "Rect2": 4, "Vector3": 3, "Transform2D": 6,
        "Plane": 4, "Quat": 4, "AABB": 6, "Basis": 9,
        "Transform": 12, "Color": 4,
    }
    if name in vector_lengths:
        return 4 + 4 * vector_lengths[name]
    if name == "Object":
        props = data["properties"]
        return 4 + _raw_string_size(data["class"]) + 4 + sum(
            _raw_string_size(str(prop_name)) + variant_encoded_size(prop_value)
            for prop_name, prop_value in props.items()
        )
    if name == "PoolByteArray":
        length = _source_length(data)
        return 8 + length + (-length) % 4
    if name == "PoolIntArray" or name == "PoolRealArray":
        return 8 + 4 * len(data)
    if name == "PoolStringArray":
        return 8 + sum(_raw_string_size(v) for v in data)
    if name in {"PoolVector2Array", "PoolVector3Array", "PoolColorArray"}:
        components = {"PoolVector2Array": 2, "PoolVector3Array": 3, "PoolColorArray": 4}[name]
        return 8 + 4 * components * len(data)
    if name == "RID":
        return 4
    if name == "NodePath":
        names = data.get("names", [])
        subnames = data.get("subnames", [])
        return 16 + sum(_raw_string_size(v) for v in names) + sum(_raw_string_size(v) for v in subnames)
    raise FormatError(f"Unsupported GDValue type for encoding: {name}")


def _sink_write_raw_string(sink: Any, value: str) -> None:
    raw = value.encode("utf-8")
    sink.write(struct.pack("<I", len(raw)))
    sink.write(raw)
    padding = (-len(raw)) % 4
    if padding:
        sink.write(b"\x00" * padding)


def write_variant_stream(sink: Any, value: Any) -> None:
    """Write one Variant incrementally to a file-like sink."""
    if value is None:
        sink.write(_header("Nil")); return
    if isinstance(value, bool):
        sink.write(_header("Bool")); sink.write(struct.pack("<I", int(value))); return
    if isinstance(value, int):
        if -(2**31) <= value < 2**31:
            sink.write(_header("Int")); sink.write(struct.pack("<i", value))
        else:
            sink.write(_header("Int", FLAG_64)); sink.write(struct.pack("<q", value))
        return
    if isinstance(value, float):
        sink.write(_header("Real")); sink.write(struct.pack("<f", value)); return
    if isinstance(value, str):
        sink.write(_header("String")); _sink_write_raw_string(sink, value); return
    if isinstance(value, list):
        sink.write(_header("Array")); sink.write(struct.pack("<I", len(value)))
        for item in value:
            write_variant_stream(sink, item)
        return
    if isinstance(value, dict):
        sink.write(_header("Dictionary")); sink.write(struct.pack("<I", len(value)))
        for key, item in value.items():
            write_variant_stream(sink, key)
            write_variant_stream(sink, item)
        return
    if not isinstance(value, GDValue):
        raise FormatError(f"Cannot encode value of type {type(value).__name__}")

    name, data = value.type_name, value.value
    vector_lengths = {
        "Vector2": 2, "Rect2": 4, "Vector3": 3, "Transform2D": 6,
        "Plane": 4, "Quat": 4, "AABB": 6, "Basis": 9,
        "Transform": 12, "Color": 4,
    }
    if name in vector_lengths:
        vals = tuple(data)
        sink.write(_header(name)); sink.write(struct.pack("<" + "f" * len(vals), *vals)); return
    if name == "Object":
        props = data["properties"]
        sink.write(_header("Object")); _sink_write_raw_string(sink, data["class"])
        sink.write(struct.pack("<I", len(props)))
        for prop_name, prop_value in props.items():
            _sink_write_raw_string(sink, str(prop_name))
            write_variant_stream(sink, prop_value)
        return
    if name == "PoolByteArray":
        length = _source_length(data)
        sink.write(_header(name)); sink.write(struct.pack("<I", length))
        for chunk in _source_chunks(data):
            sink.write(chunk)
        padding = (-length) % 4
        if padding:
            sink.write(b"\x00" * padding)
        return
    if name == "PoolIntArray":
        sink.write(_header(name)); sink.write(struct.pack("<I", len(data)))
        for start in range(0, len(data), 16384):
            chunk = data[start:start + 16384]
            sink.write(struct.pack("<" + "i" * len(chunk), *chunk))
        return
    if name == "PoolRealArray":
        sink.write(_header(name)); sink.write(struct.pack("<I", len(data)))
        for start in range(0, len(data), 16384):
            chunk = data[start:start + 16384]
            sink.write(struct.pack("<" + "f" * len(chunk), *chunk))
        return
    if name == "PoolStringArray":
        sink.write(_header(name)); sink.write(struct.pack("<I", len(data)))
        for item in data:
            _sink_write_raw_string(sink, item)
        return
    if name in {"PoolVector2Array", "PoolVector3Array", "PoolColorArray"}:
        components = {"PoolVector2Array": 2, "PoolVector3Array": 3, "PoolColorArray": 4}[name]
        sink.write(_header(name)); sink.write(struct.pack("<I", len(data)))
        batch: list[float] = []
        for item in data:
            batch.extend(item)
            if len(batch) >= 16384 * components:
                sink.write(struct.pack("<" + "f" * len(batch), *batch)); batch.clear()
        if batch:
            sink.write(struct.pack("<" + "f" * len(batch), *batch))
        return
    if name == "RID":
        sink.write(_header(name)); return
    if name == "NodePath":
        names = data.get("names", [])
        subnames = data.get("subnames", [])
        flags = 1 if data.get("absolute") else 0
        sink.write(_header(name)); sink.write(struct.pack("<I", 0x80000000 | len(names)))
        sink.write(struct.pack("<I", len(subnames))); sink.write(struct.pack("<I", flags))
        for item in names:
            _sink_write_raw_string(sink, item)
        for item in subnames:
            _sink_write_raw_string(sink, item)
        return
    raise FormatError(f"Unsupported GDValue type for encoding: {name}")


class GCPFStreamingWriter:
    """File-like sink that compresses a Variant stream one GCPF block at a time."""

    def __init__(self, path: Path, uncompressed_size: int, block_size: int = 4096, *, compressed: bool = True):
        self.path = path
        self.uncompressed_size = uncompressed_size
        self.block_size = block_size
        self.compressed = compressed
        self.block_count = uncompressed_size // block_size + 1
        self.sizes = array.array("I")
        self.buffer = bytearray()
        self._block_cache: OrderedDict[bytes, bytes] = OrderedDict()
        self.written = 0
        self.handle = path.open("w+b", buffering=1024 * 1024)
        self.handle.write(GCPF_MAGIC + struct.pack("<III", 0, block_size, uncompressed_size))
        self.table_offset = self.handle.tell()
        self.data_offset = self.table_offset + 4 * self.block_count
        self.handle.seek(self.data_offset)

    def _emit(self, block: bytes) -> None:
        packed = self._block_cache.get(block)
        if packed is not None:
            self._block_cache.move_to_end(block)
        else:
            packed = fastlz_compress_block(block) if self.compressed else fastlz_literal_block(block)
            self._block_cache[block] = packed
            if len(self._block_cache) > 64:
                self._block_cache.popitem(last=False)
        self.handle.write(packed)
        self.sizes.append(len(packed))

    def write(self, data: bytes | bytearray | memoryview) -> int:
        view = memoryview(data)
        total = len(view)
        start = 0
        while start < total:
            needed = self.block_size - len(self.buffer)
            take = min(needed, total - start)
            self.buffer.extend(view[start:start + take])
            start += take
            self.written += take
            if len(self.buffer) == self.block_size:
                self._emit(bytes(self.buffer))
                self.buffer.clear()
        return total

    def finish(self) -> int:
        if self.written != self.uncompressed_size:
            raise FormatError(
                f"Streaming encoder wrote {self.written} bytes; expected {self.uncompressed_size}"
            )
        if self.buffer:
            self._emit(bytes(self.buffer)); self.buffer.clear()
        # Godot's block-count formula includes an empty final block when the
        # uncompressed size is exactly divisible by block_size.
        while len(self.sizes) < self.block_count:
            self._emit(b"")
        if len(self.sizes) != self.block_count:
            raise FormatError("Internal GCPF block-count mismatch")
        self.handle.write(GCPF_MAGIC)
        final_size = self.handle.tell()
        self.handle.seek(self.table_offset)
        if sys.byteorder == "little":
            self.sizes.tofile(self.handle)
        else:
            copy = array.array("I", self.sizes); copy.byteswap(); copy.tofile(self.handle)
        self.handle.flush()
        os.fsync(self.handle.fileno())
        self.handle.close()
        return final_size

    def abort(self) -> None:
        try:
            self.handle.close()
        finally:
            try:
                self.path.unlink()
            except OSError:
                pass


def write_wonderdraft_map_streaming(
    root: Any,
    destination: Path,
    block_size: int = 4096,
    *,
    compressed: bool = True,
) -> int:
    payload_size = variant_encoded_size(root)
    total_size = payload_size + 4
    destination.parent.mkdir(parents=True, exist_ok=True)
    temp_handle = tempfile.NamedTemporaryFile(
        prefix=destination.name + ".", suffix=".tmp", dir=destination.parent, delete=False
    )
    temp_path = Path(temp_handle.name)
    temp_handle.close()
    writer = GCPFStreamingWriter(temp_path, total_size, block_size, compressed=compressed)
    try:
        writer.write(struct.pack("<I", payload_size))
        write_variant_stream(writer, root)
        final_size = writer.finish()
        os.replace(temp_path, destination)
        return final_size
    except Exception:
        writer.abort()
        raise


# ----------------------------- GCPF compression ------------------------------

def fastlz_literal_block(data: bytes) -> bytes:
    """Create a valid FastLZ level-1 block using literal runs only.

    This compatibility mode performs no real compression. It remains useful as
    a fallback because its encoding is particularly simple and deterministic.
    """
    if not data:
        return b""
    out = bytearray()
    for start in range(0, len(data), 32):
        chunk = data[start:start + 32]
        out.append(len(chunk) - 1)
        out.extend(chunk)
    return bytes(out)


def fastlz_compress_block(data: bytes) -> bytes:
    """Compress one block using the FastLZ level-1 wire format.

    Wonderdraft/Godot uses independent blocks, normally 4096 bytes each.  A
    small greedy LZ77 matcher is sufficient here because the search window is
    only 8192 bytes and map image buffers usually contain long repeated runs.
    """
    if not data:
        return b""

    n = len(data)
    out = bytearray()
    literals = bytearray()
    # Recent locations for each three-byte sequence. The block is at most 4096
    # bytes in normal Wonderdraft files, but the encoder also supports larger
    # custom block sizes up to FastLZ level-1's 8192-byte distance limit.
    positions: dict[bytes, list[int]] = {}

    def flush_literals() -> None:
        nonlocal literals
        while literals:
            chunk = literals[:32]
            del literals[:32]
            out.append(len(chunk) - 1)
            out.extend(chunk)

    def remember(pos: int) -> None:
        if pos + 2 >= n:
            return
        key = data[pos:pos + 3]
        bucket = positions.setdefault(key, [])
        bucket.append(pos)
        # Limiting candidates keeps incompressible image data fast while still
        # retaining enough alternatives for useful matches.
        if len(bucket) > 64:
            del bucket[:-64]

    i = 0
    while i < n:
        best_len = 0
        best_distance = 0
        if i + 2 < n:
            key = data[i:i + 3]
            candidates = positions.get(key, ())
            max_len = min(264, n - i)  # maximum FastLZ level-1 match length
            for candidate in reversed(candidates):
                distance = i - candidate
                if distance > 8192:
                    break
                length = 3
                while length < max_len and data[candidate + length] == data[i + length]:
                    length += 1
                if length > best_len:
                    best_len = length
                    best_distance = distance
                    if length == max_len:
                        break

        # The first FastLZ control byte has its top three bits reserved for the
        # compression-level marker, so a block must begin with a literal token.
        can_emit_match = best_len >= 3 and (out or literals)
        if can_emit_match:
            flush_literals()
            distance_code = best_distance - 1
            if best_len <= 8:
                length_code = best_len - 2  # top bits 1..6
                out.append((length_code << 5) | (distance_code >> 8))
            else:
                out.append((7 << 5) | (distance_code >> 8))
                out.append(best_len - 9)
            out.append(distance_code & 0xFF)
            for pos in range(i, i + best_len):
                remember(pos)
            i += best_len
        else:
            literals.append(data[i])
            remember(i)
            i += 1
            if len(literals) == 32:
                flush_literals()

    flush_literals()
    return bytes(out)


def compress_gcpf(data: bytes, block_size: int = 4096, *, compressed: bool = True) -> bytes:
    """Create a Godot GCPF/FastLZ container.

    ``compressed=True`` uses real FastLZ matches. ``False`` uses literal-only
    blocks as a maximum-compatibility fallback.
    """
    block_count = len(data) // block_size + 1
    blocks = []
    for index in range(block_count):
        raw = data[index * block_size:(index + 1) * block_size]
        blocks.append(fastlz_compress_block(raw) if compressed else fastlz_literal_block(raw))
    header = GCPF_MAGIC + struct.pack("<III", 0, block_size, len(data))
    sizes = struct.pack("<" + "I" * len(blocks), *(len(b) for b in blocks))
    return header + sizes + b"".join(blocks) + GCPF_MAGIC


# ----------------------------- Godot text parser -----------------------------

@dataclass
class Token:
    kind: str
    value: str
    pos: int


class GodotTextParser:
    _number = re.compile(r"[-+]?(?:\d+\.\d*|\.\d+|\d+)(?:[eE][-+]?\d+)?")
    _ident = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")

    def __init__(self, text: str):
        self.text = text
        self.tokens = self._tokenize(text)
        self.index = 0

    @classmethod
    def _tokenize(cls, text: str) -> list[Token]:
        result: list[Token] = []
        i = 0
        while i < len(text):
            ch = text[i]
            if ch.isspace():
                i += 1
                continue
            if ch in "{}[]():,":
                result.append(Token(ch, ch, i)); i += 1; continue
            if ch == '"':
                start = i
                i += 1
                escaped = False
                while i < len(text):
                    if escaped:
                        escaped = False
                    elif text[i] == "\\":
                        escaped = True
                    elif text[i] == '"':
                        i += 1
                        break
                    i += 1
                else:
                    raise FormatError(f"Unterminated string at character {start}")
                result.append(Token("STRING", text[start:i], start)); continue
            m = cls._number.match(text, i)
            if m:
                result.append(Token("NUMBER", m.group(0), i)); i = m.end(); continue
            m = cls._ident.match(text, i)
            if m:
                result.append(Token("IDENT", m.group(0), i)); i = m.end(); continue
            raise FormatError(f"Unexpected character {ch!r} at character {i}")
        result.append(Token("EOF", "", len(text)))
        return result

    def peek(self, kind: str | None = None) -> Token | bool:
        token = self.tokens[self.index]
        return token.kind == kind if kind else token

    def take(self, kind: str) -> Token:
        token = self.tokens[self.index]
        if token.kind != kind:
            raise FormatError(f"Expected {kind}, found {token.kind} at character {token.pos}")
        self.index += 1
        return token

    def parse(self) -> Any:
        value = self.parse_value()
        self.take("EOF")
        return value

    def parse_value(self) -> Any:
        token = self.peek()
        assert isinstance(token, Token)
        if token.kind == "STRING":
            import json
            self.index += 1
            return json.loads(token.value)
        if token.kind == "NUMBER":
            self.index += 1
            lower = token.value.lower()
            return float(token.value) if any(c in lower for c in ".e") else int(token.value)
        if token.kind == "{":
            return self.parse_dict()
        if token.kind == "[":
            return self.parse_array()
        if token.kind == "IDENT":
            self.index += 1
            ident = token.value
            if ident == "true": return True
            if ident == "false": return False
            if ident == "null": return None
            if ident == "nan": return float("nan")
            if ident == "inf": return float("inf")
            self.take("(")
            args = self.parse_call_args()
            return self.make_constructor(ident, args, token.pos)
        raise FormatError(f"Unexpected token {token.kind} at character {token.pos}")

    def parse_dict(self) -> OrderedDict:
        self.take("{")
        out = OrderedDict()
        if self.peek("}"):
            self.take("}"); return out
        while True:
            key = self.parse_value()
            self.take(":")
            out[key] = self.parse_value()
            if self.peek(","):
                self.take(",")
                if self.peek("}"): break
            else:
                break
        self.take("}")
        return out

    def parse_array(self) -> list[Any]:
        self.take("[")
        out = []
        if self.peek("]"):
            self.take("]"); return out
        while True:
            out.append(self.parse_value())
            if self.peek(","):
                self.take(",")
                if self.peek("]"): break
            else:
                break
        self.take("]")
        return out

    def parse_call_args(self) -> list[Any]:
        args = []
        if self.peek(")"):
            self.take(")"); return args
        while True:
            args.append(self.parse_value())
            if self.peek(","):
                self.take(",")
            else:
                break
        self.take(")")
        return args

    @staticmethod
    def make_constructor(name: str, args: list[Any], pos: int) -> GDValue:
        fixed = {
            "Vector2": 2, "Rect2": 4, "Vector3": 3, "Transform2D": 6,
            "Plane": 4, "Quat": 4, "AABB": 6, "Basis": 9,
            "Transform": 12, "Color": 4,
        }
        if name in fixed:
            if len(args) != fixed[name]:
                raise FormatError(f"{name} requires {fixed[name]} arguments at character {pos}")
            return GDValue(name, tuple(float(v) for v in args))
        if name == "PoolByteArray":
            return GDValue(name, bytes(int(v) & 0xFF for v in args))
        if name == "PoolIntArray":
            return GDValue(name, [int(v) for v in args])
        if name == "PoolRealArray":
            return GDValue(name, [float(v) for v in args])
        if name == "PoolStringArray":
            return GDValue(name, [str(v) for v in args])
        components = {"PoolVector2Array": 2, "PoolVector3Array": 3, "PoolColorArray": 4}
        if name in components:
            n = components[name]
            if len(args) % n:
                raise FormatError(f"{name} argument count must be divisible by {n}")
            return GDValue(name, [tuple(float(x) for x in args[i:i+n]) for i in range(0, len(args), n)])
        raise FormatError(f"Unknown constructor {name!r} at character {pos}")


class LazyGodotTextParser:
    """Godot-text parser with one-token lookahead instead of a token list."""

    _number = GodotTextParser._number
    _ident = GodotTextParser._ident

    def __init__(self, text: str):
        self.text = text
        self.cursor = 0
        self.current = self._next_token()

    def _next_token(self) -> Token:
        text = self.text
        i = self.cursor
        while i < len(text) and text[i].isspace():
            i += 1
        if i >= len(text):
            self.cursor = i
            return Token("EOF", "", i)
        ch = text[i]
        if ch in "{}[]():,":
            self.cursor = i + 1
            return Token(ch, ch, i)
        if ch == '"':
            start = i
            i += 1
            escaped = False
            while i < len(text):
                if escaped:
                    escaped = False
                elif text[i] == "\\":
                    escaped = True
                elif text[i] == '"':
                    i += 1
                    self.cursor = i
                    return Token("STRING", text[start:i], start)
                i += 1
            raise FormatError(f"Unterminated string at character {start}")
        match = self._number.match(text, i)
        if match:
            self.cursor = match.end()
            return Token("NUMBER", match.group(0), i)
        match = self._ident.match(text, i)
        if match:
            self.cursor = match.end()
            return Token("IDENT", match.group(0), i)
        raise FormatError(f"Unexpected character {ch!r} at character {i}")

    def peek(self, kind: str | None = None) -> Token | bool:
        return self.current.kind == kind if kind else self.current

    def take(self, kind: str) -> Token:
        token = self.current
        if token.kind != kind:
            raise FormatError(f"Expected {kind}, found {token.kind} at character {token.pos}")
        self.current = self._next_token()
        return token

    def parse(self) -> Any:
        value = self.parse_value()
        self.take("EOF")
        return value

    def parse_value(self) -> Any:
        token = self.current
        if token.kind == "STRING":
            self.take("STRING")
            return json.loads(token.value)
        if token.kind == "NUMBER":
            self.take("NUMBER")
            lower = token.value.lower()
            return float(token.value) if any(c in lower for c in ".e") else int(token.value)
        if token.kind == "{":
            return self.parse_dict()
        if token.kind == "[":
            return self.parse_array()
        if token.kind == "IDENT":
            self.take("IDENT")
            ident = token.value
            if ident == "true": return True
            if ident == "false": return False
            if ident == "null": return None
            if ident == "nan": return float("nan")
            if ident == "inf": return float("inf")
            self.take("(")
            args = self.parse_call_args()
            return GodotTextParser.make_constructor(ident, args, token.pos)
        raise FormatError(f"Unexpected token {token.kind} at character {token.pos}")

    def parse_dict(self) -> OrderedDict:
        self.take("{")
        out = OrderedDict()
        if self.peek("}"):
            self.take("}"); return out
        while True:
            key = self.parse_value()
            self.take(":")
            out[key] = self.parse_value()
            if self.peek(","):
                self.take(",")
                if self.peek("}"):
                    break
            else:
                break
        self.take("}")
        return out

    def parse_array(self) -> list[Any]:
        self.take("[")
        out: list[Any] = []
        if self.peek("]"):
            self.take("]"); return out
        while True:
            out.append(self.parse_value())
            if self.peek(","):
                self.take(",")
                if self.peek("]"):
                    break
            else:
                break
        self.take("]")
        return out

    def parse_call_args(self) -> list[Any]:
        args: list[Any] = []
        if self.peek(")"):
            self.take(")"); return args
        while True:
            args.append(self.parse_value())
            if self.peek(","):
                self.take(",")
            else:
                break
        self.take(")")
        return args


def parse_godot_text(text: str) -> Any:
    return LazyGodotTextParser(text).parse()


# ------------------------------- Image helpers -------------------------------

def find_images(value: Any, path: tuple[str, ...] = ()) -> OrderedDict[str, GDValue]:
    found: OrderedDict[str, GDValue] = OrderedDict()
    if image_object_info(value) is not None:
        found[".".join(path)] = value
    elif isinstance(value, dict):
        for key, child in value.items():
            found.update(find_images(child, path + (str(key),)))
    elif isinstance(value, list):
        for idx, child in enumerate(value):
            found.update(find_images(child, path + (str(idx),)))
    elif isinstance(value, GDValue) and value.type_name == "Object":
        found.update(find_images(value.value["properties"], path + ("properties",)))
    return found


def replace_images_with_names(value: Any, images: OrderedDict[str, GDValue], path=()) -> Any:
    joined = ".".join(path)
    if joined in images and image_object_info(value) is not None:
        leaf = path[-1] if path else "image"
        return f".{leaf}.png"
    if isinstance(value, dict):
        return OrderedDict((k, replace_images_with_names(v, images, path + (str(k),))) for k, v in value.items())
    if isinstance(value, list):
        return [replace_images_with_names(v, images, path + (str(i),)) for i, v in enumerate(value)]
    return value


def restore_images(value: Any, images: OrderedDict[str, GDValue], path=()) -> Any:
    joined = ".".join(path)
    if joined in images:
        return images[joined]
    if isinstance(value, dict):
        return OrderedDict((k, restore_images(v, images, path + (str(k),))) for k, v in value.items())
    if isinstance(value, list):
        return [restore_images(v, images, path + (str(i),)) for i, v in enumerate(value)]
    return value


def png_to_image_object(
    path: Path,
    template: GDValue | None = None,
    backing_dir: Path | None = None,
) -> GDValue:
    if Image is None:
        raise FormatError("PNG import requires Pillow: pip install pillow")
    with Image.open(path) as img:
        img = img.convert("RGBA")
        width, height = img.size
        raw = img.tobytes()
    if template is not None:
        info = image_object_info(template)
        if info and (width, height) != info[:2]:
            raise FormatError(
                f"PNG is {width}x{height}; this image slot requires {info[0]}x{info[1]}"
            )
    source: ByteSource = raw
    if backing_dir is not None:
        backing_dir.mkdir(parents=True, exist_ok=True)
        handle = tempfile.NamedTemporaryFile(
            prefix="replacement_", suffix=".rgba", dir=backing_dir, delete=False
        )
        try:
            handle.write(raw)
            raw_path = Path(handle.name)
        finally:
            handle.close()
        source = DiskByteArray(raw_path, 0, len(raw))
        del raw
        gc.collect()
    data_dict = OrderedDict([
        ("width", width), ("height", height), ("mipmaps", False),
        ("format", "RGBA8"), ("data", GDValue("PoolByteArray", source)),
    ])
    return GDValue("Object", {
        "class": "Image",
        "properties": OrderedDict([("data", data_dict)]),
    })


def iter_godot_text(value: Any) -> Iterator[str]:
    """Yield Godot text in small pieces, avoiding one giant serialization string."""
    if isinstance(value, list):
        if not value:
            yield "[  ]"; return
        yield "[ "
        for index, item in enumerate(value):
            if index:
                yield ", "
            yield from iter_godot_text(item)
        yield " ]"
        return
    if isinstance(value, dict):
        if not value:
            yield "{\n}"; return
        items = list(value.items())
        if all(isinstance(key, str) for key, _ in items):
            items.sort(key=lambda item: item[0])
        yield "{\n"
        for index, (key, child) in enumerate(items):
            if index:
                yield ",\n"
            yield to_godot_text(key)
            yield ": "
            yield from iter_godot_text(child)
        yield "\n}"
        return
    # Editable map text never contains embedded PoolByteArray images. Other
    # scalar/constructor values are small enough to delegate to the proven formatter.
    yield to_godot_text(value)


def _png_write_chunk(handle: BinaryIO, chunk_type: bytes, payload: bytes | memoryview) -> None:
    handle.write(struct.pack(">I", len(payload)))
    handle.write(chunk_type)
    handle.write(payload)
    crc = zlib.crc32(chunk_type)
    crc = zlib.crc32(payload, crc)
    handle.write(struct.pack(">I", crc & 0xFFFFFFFF))


def write_png_streaming(
    path: Path,
    width: int,
    height: int,
    pixel_format: str,
    pixels: ByteSource,
    *,
    compression_level: int = 6,
) -> None:
    """Write a PNG row by row, using bounded memory for very large maps."""
    format_table = {
        "L8": (1, 0), "LA8": (2, 4), "RGB8": (3, 2), "RGBA8": (4, 6),
    }
    if pixel_format not in format_table:
        raise FormatError(
            f"Unsupported embedded Image format {pixel_format!r}; supported: {', '.join(format_table)}"
        )
    channels, color_type = format_table[pixel_format]
    row_size = width * channels
    base_size = row_size * height
    if _source_length(pixels) < base_size:
        raise FormatError(
            f"Image buffer has {_source_length(pixels)} bytes, but {width}x{height} {pixel_format} needs {base_size}"
        )

    path.parent.mkdir(parents=True, exist_ok=True)
    compressor = zlib.compressobj(compression_level)
    max_idat = 1024 * 1024
    pending = bytearray()

    def emit_compressed(data: bytes) -> None:
        pending.extend(data)
        while len(pending) >= max_idat:
            chunk = bytes(pending[:max_idat])
            del pending[:max_idat]
            _png_write_chunk(handle, b"IDAT", chunk)

    with path.open("wb", buffering=1024 * 1024) as handle:
        handle.write(b"\x89PNG\r\n\x1a\n")
        _png_write_chunk(
            handle, b"IHDR", struct.pack(">IIBBBBB", width, height, 8, color_type, 0, 0, 0)
        )
        if isinstance(pixels, DiskByteArray):
            with pixels.path.open("rb", buffering=1024 * 1024) as source:
                source.seek(pixels.offset)
                for _ in range(height):
                    row = source.read(row_size)
                    if len(row) != row_size:
                        raise FormatError("Unexpected end of disk-backed image while exporting PNG")
                    emit_compressed(compressor.compress(b"\x00"))
                    emit_compressed(compressor.compress(row))
        else:
            view = memoryview(pixels)
            for y in range(height):
                row = view[y * row_size:(y + 1) * row_size]
                emit_compressed(compressor.compress(b"\x00"))
                emit_compressed(compressor.compress(row))
        emit_compressed(compressor.flush())
        if pending:
            _png_write_chunk(handle, b"IDAT", bytes(pending))
        _png_write_chunk(handle, b"IEND", b"")


def make_image_thumbnail(width: int, height: int, pixel_format: str, pixels: ByteSource, max_size: int = 300):
    """Create a nearest-neighbour preview while holding only one source row."""
    if Image is None:
        return None
    modes = {"L8": ("L", 1), "LA8": ("LA", 2), "RGB8": ("RGB", 3), "RGBA8": ("RGBA", 4)}
    entry = modes.get(pixel_format)
    if not entry:
        return None
    mode, channels = entry
    scale = min(1.0, max_size / max(width, height))
    out_w = max(1, int(round(width * scale)))
    out_h = max(1, int(round(height * scale)))
    xs = [min(width - 1, int(x * width / out_w)) for x in range(out_w)]
    ys = [min(height - 1, int(y * height / out_h)) for y in range(out_h)]
    row_size = width * channels
    output = bytearray(out_w * out_h * channels)

    if isinstance(pixels, DiskByteArray):
        with pixels.path.open("rb", buffering=1024 * 1024) as source:
            for out_y, src_y in enumerate(ys):
                source.seek(pixels.offset + src_y * row_size)
                row = source.read(row_size)
                if len(row) != row_size:
                    raise FormatError("Unexpected end of disk-backed image while creating preview")
                base = out_y * out_w * channels
                for out_x, src_x in enumerate(xs):
                    src = src_x * channels
                    dst = base + out_x * channels
                    output[dst:dst + channels] = row[src:src + channels]
    else:
        view = memoryview(pixels)
        for out_y, src_y in enumerate(ys):
            row = view[src_y * row_size:(src_y + 1) * row_size]
            base = out_y * out_w * channels
            for out_x, src_x in enumerate(xs):
                src = src_x * channels
                dst = base + out_x * channels
                output[dst:dst + channels] = row[src:src + channels]
    return Image.frombytes(mode, (out_w, out_h), bytes(output))


# -------------------------- Settings and SVG support -------------------------

SCRIPT_DIR = Path(__file__).resolve().parent
CONFIG_PATH = SCRIPT_DIR / "wonderdraft_gui.config"
WD_NS = "urn:wonderdraft-map-editor"
SVG_NS = "http://www.w3.org/2000/svg"
XLINK_NS = "http://www.w3.org/1999/xlink"
ET.register_namespace("", SVG_NS)
ET.register_namespace("xlink", XLINK_NS)
ET.register_namespace("wd", WD_NS)

FALLBACK_TEXTURE = "res://sprites/symbols/custom_colors/s2_capital"
IMAGE_EXTENSIONS = (".png", ".webp", ".jpg", ".jpeg", ".svg")


def _looks_like_asset_root(path: Path | None) -> bool:
    if path is None or not path.is_dir():
        return False
    try:
        return any(p.suffix.lower() in IMAGE_EXTENSIONS for p in path.rglob("*"))
    except OSError:
        return False


def detect_custom_asset_folder() -> Path | None:
    candidates: list[Path] = []
    env = os.environ.get("WONDERDRAFT_ASSETS")
    if env:
        candidates.append(Path(env).expanduser())
    home = Path.home()
    system = platform.system().lower()
    if system == "windows":
        appdata = os.environ.get("APPDATA")
        if appdata:
            candidates.append(Path(appdata) / "Wonderdraft" / "assets")
    elif system == "darwin":
        candidates.append(home / "Library" / "Application Support" / "Wonderdraft" / "assets")
    else:
        candidates.extend([
            home / ".local" / "share" / "Wonderdraft" / "assets",
            home / ".var" / "app" / "com.wonderdraft.Wonderdraft" / "data" / "Wonderdraft" / "assets",
        ])
    candidates.extend([home / "Wonderdraft" / "assets", home / "wonderdraft" / "assets"])
    for candidate in candidates:
        if _looks_like_asset_root(candidate):
            return candidate.resolve()
    return None


def detect_default_asset_folder() -> Path | None:
    home = Path.home()
    candidates: list[Path] = [
        SCRIPT_DIR / "sprites",
        Path.cwd() / "sprites",
        SCRIPT_DIR.parent / "sprites",
        home / "code" / "wonderdraft_manipulator" / "sprites",
        home / "Documents" / "wonderdraftfiles" / "sprites",
        home / "documents" / "wonderdraftfiles" / "sprites",
    ]
    # Search a few likely development folders without walking the whole home tree.
    for parent in (home / "code", home / "projects", home / "Documents", home / "documents"):
        if parent.is_dir():
            try:
                candidates.extend(parent.glob("*/sprites"))
            except OSError:
                pass
    for candidate in candidates:
        if _looks_like_asset_root(candidate):
            return candidate.resolve()
    return None


def detect_cache_folder() -> Path:
    env = os.environ.get("WONDERDRAFT_GUI_CACHE")
    if env:
        return Path(env).expanduser()
    home = Path.home()
    system = platform.system().lower()
    if system == "windows":
        base = Path(os.environ.get("LOCALAPPDATA", home / "AppData" / "Local"))
        return base / "WonderdraftMapEditor" / "cache"
    if system == "darwin":
        return home / "Library" / "Caches" / "WonderdraftMapEditor"
    return Path(os.environ.get("XDG_CACHE_HOME", home / ".cache")) / "wonderdraft_gui"


def load_settings() -> dict[str, str]:
    settings = {"custom_asset_folder": "", "default_asset_folder": "", "cache_folder": ""}
    if CONFIG_PATH.is_file():
        try:
            raw = json.loads(CONFIG_PATH.read_text(encoding="utf-8"))
            if isinstance(raw, dict):
                for key in settings:
                    value = raw.get(key)
                    if isinstance(value, str):
                        settings[key] = value
        except Exception:
            # A damaged settings file must not prevent the editor from starting.
            pass
    if not settings["custom_asset_folder"]:
        found = detect_custom_asset_folder()
        if found:
            settings["custom_asset_folder"] = str(found)
    if not settings["default_asset_folder"]:
        found = detect_default_asset_folder()
        if found:
            settings["default_asset_folder"] = str(found)
    if not settings["cache_folder"]:
        settings["cache_folder"] = str(detect_cache_folder())
    return settings


def save_settings(settings: dict[str, str]) -> None:
    CONFIG_PATH.write_text(json.dumps(settings, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def _normalize_root(path_value: str, kind: str) -> Path | None:
    if not path_value.strip():
        return None
    path = Path(path_value).expanduser()
    if kind == "custom" and (path / "assets").is_dir() and not any(
        p.suffix.lower() in IMAGE_EXTENSIONS for p in path.glob("*")
    ):
        path = path / "assets"
    if kind == "default" and (path / "sprites").is_dir() and path.name != "sprites":
        path = path / "sprites"
    try:
        return path.resolve()
    except OSError:
        return path.absolute()


def _casefold_relative(path: Path) -> str:
    return path.as_posix().casefold()


@dataclass
class AssetInfo:
    texture: str
    path: Path
    width: float
    height: float
    base_radius: float


class AssetResolver:
    """Resolve Wonderdraft texture URIs to custom/default sprite files."""

    def __init__(self, custom_root: str = "", default_root: str = ""):
        self.custom_root = _normalize_root(custom_root, "custom")
        self.default_root = _normalize_root(default_root, "default")
        self._texture_to_path: dict[str, Path] = {}
        self._path_to_texture: dict[str, str] = {}
        self._metadata_files: dict[Path, dict[str, Any]] = {}
        self._indexed = False

    @property
    def custom_ready(self) -> bool:
        return bool(self.custom_root and self.custom_root.is_dir())

    @property
    def default_ready(self) -> bool:
        return bool(self.default_root and self.default_root.is_dir())

    def _index_root(self, root: Path, prefix: str) -> None:
        try:
            for path in root.rglob("*"):
                if not path.is_file():
                    continue
                if path.name == ".wonderdraft_symbols":
                    try:
                        parsed = json.loads(path.read_text(encoding="utf-8-sig"))
                        if isinstance(parsed, dict):
                            self._metadata_files[path.parent.resolve()] = parsed
                    except Exception:
                        pass
                    continue
                if path.suffix.lower() not in IMAGE_EXTENSIONS:
                    continue
                rel = path.relative_to(root).with_suffix("")
                texture = prefix + rel.as_posix()
                resolved = path.resolve()
                self._texture_to_path[texture.casefold()] = resolved
                self._path_to_texture[str(resolved).casefold()] = texture
        except OSError:
            pass

    def ensure_index(self) -> None:
        if self._indexed:
            return
        if self.custom_ready and self.custom_root:
            self._index_root(self.custom_root, "user://assets/")
        if self.default_ready and self.default_root:
            self._index_root(self.default_root, "res://sprites/")
        self._indexed = True

    def _candidate_exact(self, root: Path, rel: str) -> Path | None:
        rel_path = Path(urllib.parse.unquote(rel))
        candidate = root / rel_path
        if candidate.suffix.lower() in IMAGE_EXTENSIONS and candidate.is_file():
            return candidate.resolve()
        for ext in IMAGE_EXTENSIONS:
            p = Path(str(candidate) + ext)
            if p.is_file():
                return p.resolve()
        return None

    def _candidate_casefold(self, root: Path, rel: str) -> Path | None:
        """Resolve one path case-insensitively without indexing the whole asset tree."""
        parts = Path(urllib.parse.unquote(rel)).parts
        current = root
        for index, part in enumerate(parts):
            try:
                entries = {entry.name.casefold(): entry for entry in current.iterdir()}
            except OSError:
                return None
            match = entries.get(part.casefold())
            if match is None and index == len(parts) - 1:
                stem = Path(part).stem.casefold()
                match = next((entry for entry in entries.values()
                              if entry.is_file() and entry.suffix.lower() in IMAGE_EXTENSIONS
                              and entry.stem.casefold() == stem), None)
            if match is None:
                return None
            current = match
        if current.is_file() and current.suffix.lower() in IMAGE_EXTENSIONS:
            return current.resolve()
        if current.suffix:
            return None
        try:
            for ext in IMAGE_EXTENSIONS:
                candidate = current.with_suffix(ext)
                if candidate.is_file():
                    return candidate.resolve()
        except OSError:
            pass
        return None

    def resolve_texture(self, texture: str | None) -> Path | None:
        if not texture:
            return None
        if texture.startswith("user://assets/") and self.custom_root:
            rel = texture[len("user://assets/"):]
            return self._candidate_exact(self.custom_root, rel) or self._candidate_casefold(self.custom_root, rel)
        if texture.startswith("res://sprites/") and self.default_root:
            rel = texture[len("res://sprites/"):]
            return self._candidate_exact(self.default_root, rel) or self._candidate_casefold(self.default_root, rel)
        return None

    def texture_for_path(self, source: str, svg_dir: Path) -> str | None:
        if not source or source.startswith("data:"):
            return None
        source = urllib.parse.unquote(source)
        normalized = source.replace("\\", "/")
        # Inkscape often rewrites file references as paths relative to the SVG.
        # Recover Wonderdraft URIs from recognizable path suffixes even when the
        # SVG has moved to another directory or computer.
        custom_marker = "Wonderdraft/assets/"
        if custom_marker.casefold() in normalized.casefold():
            start = normalized.casefold().index(custom_marker.casefold()) + len(custom_marker)
            guessed = "user://assets/" + str(Path(normalized[start:]).with_suffix("")).replace("\\", "/")
            if self.resolve_texture(guessed):
                return guessed
        marker = "/sprites/"
        if marker in normalized.casefold():
            start = normalized.casefold().rindex(marker) + len(marker)
            guessed = "res://sprites/" + str(Path(normalized[start:]).with_suffix("")).replace("\\", "/")
            if self.resolve_texture(guessed):
                return guessed
        parsed = urllib.parse.urlparse(source)
        if parsed.scheme == "file":
            local = Path(urllib.parse.unquote(parsed.path))
            # Windows file URIs can begin with /C:/...
            if platform.system().lower() == "windows" and re.match(r"^/[A-Za-z]:", str(local)):
                local = Path(str(local)[1:])
        else:
            local = Path(source)
            if not local.is_absolute():
                local = svg_dir / local
        try:
            local = local.resolve()
        except OSError:
            local = local.absolute()
        for root, prefix in ((self.custom_root, "user://assets/"), (self.default_root, "res://sprites/")):
            if root is None:
                continue
            try:
                relative = local.relative_to(root)
            except ValueError:
                continue
            return prefix + relative.with_suffix("").as_posix()
        return None

    def _metadata_for_path(self, path: Path) -> dict[str, Any] | None:
        stem = path.stem.casefold()
        current = path.parent.resolve()
        roots = {r for r in (self.custom_root, self.default_root) if r is not None}
        while True:
            data = self._metadata_files.get(current)
            if data is None:
                metadata_path = current / ".wonderdraft_symbols"
                if metadata_path.is_file():
                    try:
                        parsed = json.loads(metadata_path.read_text(encoding="utf-8-sig"))
                        data = parsed if isinstance(parsed, dict) else {}
                    except Exception:
                        data = {}
                    self._metadata_files[current] = data
            if data:
                for key, value in data.items():
                    if str(key).casefold() == stem and isinstance(value, dict):
                        return value
                for key, value in data.items():
                    if str(key).casefold() == path.parent.name.casefold() and isinstance(value, dict):
                        return value
            if current in roots or current.parent == current:
                break
            current = current.parent
        return None

    @staticmethod
    def _image_dimensions(path: Path) -> tuple[float, float]:
        if path.suffix.lower() == ".svg":
            try:
                root = ET.parse(path).getroot()
                viewbox = root.get("viewBox")
                if viewbox:
                    vals = [float(v) for v in re.split(r"[ ,]+", viewbox.strip())]
                    if len(vals) == 4:
                        return abs(vals[2]), abs(vals[3])
                return _parse_svg_length(root.get("width", "0")), _parse_svg_length(root.get("height", "0"))
            except Exception:
                return 0.0, 0.0
        if Image is not None:
            try:
                with Image.open(path) as img:
                    return float(img.width), float(img.height)
            except Exception:
                pass
        # PNG dimensions can be read without Pillow.
        try:
            with path.open("rb") as handle:
                raw = handle.read(24)
            if raw.startswith(b"\x89PNG\r\n\x1a\n") and len(raw) >= 24:
                return tuple(map(float, struct.unpack(">II", raw[16:24])))
        except OSError:
            pass
        return 0.0, 0.0

    def asset_info(self, texture: str | None) -> AssetInfo | None:
        path = self.resolve_texture(texture)
        if not path or not texture:
            return None
        width, height = self._image_dimensions(path)

        # The radius stored in a Wonderdraft map is already a rendered pixel
        # radius.  A .wonderdraft_symbols radius is a pack/default-placement
        # hint, not a divisor for map-instance sizing.  Treating that metadata
        # value as the source image radius can magnify transparent padding and
        # move the visible sprite far away from its map position.
        base_radius = max(width, height) / 2.0 if max(width, height) > 0 else 1.0
        return AssetInfo(
            texture, path, width or base_radius * 2, height or base_radius * 2,
            base_radius,
        )


# Affine matrices use SVG's (a,b,c,d,e,f) convention.
Matrix = tuple[float, float, float, float, float, float]
IDENTITY: Matrix = (1.0, 0.0, 0.0, 1.0, 0.0, 0.0)


def _mat_mul(left: Matrix, right: Matrix) -> Matrix:
    a1, b1, c1, d1, e1, f1 = left
    a2, b2, c2, d2, e2, f2 = right
    return (
        a1 * a2 + c1 * b2,
        b1 * a2 + d1 * b2,
        a1 * c2 + c1 * d2,
        b1 * c2 + d1 * d2,
        a1 * e2 + c1 * f2 + e1,
        b1 * e2 + d1 * f2 + f1,
    )


def _mat_apply(matrix: Matrix, x: float, y: float) -> tuple[float, float]:
    a, b, c, d, e, f = matrix
    return a * x + c * y + e, b * x + d * y + f


def _matrix_scale_rotation(matrix: Matrix) -> tuple[float, float, float, bool]:
    a, b, c, d, _, _ = matrix
    sx = math.hypot(a, b)
    determinant = a * d - b * c
    sy = abs(determinant) / sx if sx else math.hypot(c, d)
    angle = math.atan2(b, a)
    return sx, sy, angle, determinant < 0


def _parse_transform(text: str | None) -> Matrix:
    if not text:
        return IDENTITY
    current = IDENTITY
    for name, args_text in re.findall(r"([A-Za-z]+)\s*\(([^)]*)\)", text):
        vals = [float(v) for v in re.findall(r"[-+]?(?:\d+\.\d*|\.\d+|\d+)(?:[eE][-+]?\d+)?", args_text)]
        name = name.lower()
        op = IDENTITY
        if name == "matrix" and len(vals) >= 6:
            op = tuple(vals[:6])  # type: ignore[assignment]
        elif name == "translate" and vals:
            op = (1, 0, 0, 1, vals[0], vals[1] if len(vals) > 1 else 0)
        elif name == "scale" and vals:
            op = (vals[0], 0, 0, vals[1] if len(vals) > 1 else vals[0], 0, 0)
        elif name == "rotate" and vals:
            angle = math.radians(vals[0])
            cos_a, sin_a = math.cos(angle), math.sin(angle)
            rotate = (cos_a, sin_a, -sin_a, cos_a, 0, 0)
            if len(vals) >= 3:
                cx, cy = vals[1], vals[2]
                op = _mat_mul(_mat_mul((1, 0, 0, 1, cx, cy), rotate), (1, 0, 0, 1, -cx, -cy))
            else:
                op = rotate
        elif name == "skewx" and vals:
            op = (1, 0, math.tan(math.radians(vals[0])), 1, 0, 0)
        elif name == "skewy" and vals:
            op = (1, math.tan(math.radians(vals[0])), 0, 1, 0, 0)
        current = _mat_mul(current, op)
    return current


def _parse_svg_length(value: str | None) -> float:
    if not value:
        return 0.0
    match = re.match(r"\s*([-+]?(?:\d+\.\d*|\.\d+|\d+)(?:[eE][-+]?\d+)?)\s*([A-Za-z%]*)", value)
    if not match:
        return 0.0
    number = float(match.group(1))
    unit = match.group(2).lower()
    factors = {"": 1.0, "px": 1.0, "mm": 96 / 25.4, "cm": 96 / 2.54, "in": 96.0, "pt": 96 / 72, "pc": 16.0}
    return number * factors.get(unit, 1.0)


def _style_map(element: ET.Element) -> dict[str, str]:
    style: dict[str, str] = {}
    for part in element.get("style", "").split(";"):
        if ":" in part:
            key, value = part.split(":", 1)
            style[key.strip()] = value.strip()
    for key in ("fill", "fill-opacity", "stroke", "stroke-opacity", "stroke-width", "font-family", "font-size", "text-anchor", "opacity", "filter", "dominant-baseline"):
        if element.get(key) is not None:
            style[key] = element.get(key, "")
    return style


def _clamp01(value: float) -> float:
    return max(0.0, min(1.0, float(value)))


def _color_tuple(value: Any, default=(1.0, 1.0, 1.0, 1.0)) -> tuple[float, float, float, float]:
    if isinstance(value, GDValue) and value.type_name == "Color" and len(value.value) == 4:
        return tuple(float(v) for v in value.value)  # type: ignore[return-value]
    return default


def _svg_color(value: Any) -> tuple[str, float]:
    # Godot's Color channels are expected to be nonlinear sRGB values. Therefore
    # they map directly to SVG/CSS sRGB bytes; no linear-light conversion is used.
    r, g, b, a = _color_tuple(value)
    return "#{:02x}{:02x}{:02x}".format(round(_clamp01(r) * 255), round(_clamp01(g) * 255), round(_clamp01(b) * 255)), _clamp01(a)


def _parse_css_color(value: str | None, opacity: float = 1.0, default=(1.0, 1.0, 1.0, 1.0)) -> GDValue:
    if not value or value in {"none", "transparent"}:
        return GDValue("Color", default)
    text = value.strip().lower()
    named = {"black": "#000000", "white": "#ffffff", "red": "#ff0000", "green": "#008000", "blue": "#0000ff"}
    text = named.get(text, text)
    r = g = b = 1.0
    alpha = opacity
    try:
        if text.startswith("#"):
            h = text[1:]
            if len(h) in (3, 4):
                h = "".join(ch * 2 for ch in h)
            if len(h) in (6, 8):
                r, g, b = (int(h[i:i+2], 16) / 255 for i in (0, 2, 4))
                if len(h) == 8:
                    alpha *= int(h[6:8], 16) / 255
        elif text.startswith("rgb"):
            vals = re.findall(r"[-+]?(?:\d+\.\d*|\.\d+|\d+)%?", text)
            parsed: list[float] = []
            for v in vals[:3]:
                parsed.append(float(v[:-1]) / 100 if v.endswith("%") else float(v) / 255)
            if len(parsed) == 3:
                r, g, b = parsed
            if len(vals) >= 4:
                alpha *= float(vals[3].rstrip("%")) / (100 if vals[3].endswith("%") else 1)
    except Exception:
        r, g, b, alpha = default
    return GDValue("Color", (_clamp01(r), _clamp01(g), _clamp01(b), _clamp01(alpha)))


def _vector2(value: Any, default=(0.0, 0.0)) -> tuple[float, float]:
    if isinstance(value, GDValue) and value.type_name == "Vector2" and len(value.value) == 2:
        return float(value.value[0]), float(value.value[1])
    if isinstance(value, (tuple, list)) and len(value) >= 2:
        return float(value[0]), float(value[1])
    return default


def _record_encode(value: Any) -> str:
    return base64.urlsafe_b64encode(to_godot_text(value).encode("utf-8")).decode("ascii")


def _record_decode(value: str | None) -> Any | None:
    if not value:
        return None
    try:
        return parse_godot_text(base64.urlsafe_b64decode(value.encode("ascii")).decode("utf-8"))
    except Exception:
        return None


def _rgba_png_bytes(width: int, height: int, fmt: str, pixels: bytes) -> bytes:
    if fmt != "RGBA8":
        if Image is None:
            raise FormatError(f"Embedding {fmt} images in SVG requires Pillow")
        modes = {"L8": "L", "LA8": "LA", "RGB8": "RGB", "RGBA8": "RGBA"}
        mode = modes.get(fmt)
        if not mode:
            raise FormatError(f"Unsupported image format {fmt}")
        image = Image.frombytes(mode, (width, height), pixels)
        buffer = io.BytesIO()
        image.convert("RGBA").save(buffer, format="PNG")
        return buffer.getvalue()
    stride = width * 4
    scanlines = b"".join(b"\x00" + pixels[y * stride:(y + 1) * stride] for y in range(height))

    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(scanlines, 9)) + chunk(b"IEND", b"")


def _find_image_by_leaf(images: OrderedDict[str, GDValue], leaf: str) -> GDValue | None:
    for key, image in images.items():
        if key.split(".")[-1] == leaf:
            return image
    return None


def _tag(element: ET.Element) -> str:
    return element.tag.rsplit("}", 1)[-1]


def _set_wd(element: ET.Element, key: str, value: Any) -> None:
    element.set(f"{{{WD_NS}}}{key}", str(value))


def _get_wd(element: ET.Element, key: str, default: str | None = None) -> str | None:
    return element.get(f"{{{WD_NS}}}{key}", element.get(f"data-wd-{key}", default))


def _parse_vector2_string(value: str) -> list[tuple[float, float]]:
    """Parse Wonderdraft's string-encoded ``[ Vector2(...), ... ]`` lists."""
    try:
        parsed = parse_godot_text(value)
        if isinstance(parsed, list) and parsed and all(
            isinstance(item, GDValue) and item.type_name == "Vector2"
            for item in parsed
        ):
            return [_vector2(item) for item in parsed]
    except Exception:
        pass

    # Be liberal when reading hand-edited files.
    matches = re.findall(
        r"Vector2\s*\(\s*([-+]?(?:\d+\.\d*|\.\d+|\d+)(?:[eE][-+]?\d+)?)\s*,\s*"
        r"([-+]?(?:\d+\.\d*|\.\d+|\d+)(?:[eE][-+]?\d+)?)\s*\)",
        value,
    )
    return [(float(x), float(y)) for x, y in matches]


def _find_points_slot(value: Any, path: tuple[Any, ...] = ()) -> tuple[tuple[Any, ...], list[tuple[float, float]], str] | None:
    last_key = str(path[-1]).lower() if path else ""
    if isinstance(value, str) and "point" in last_key:
        points = _parse_vector2_string(value)
        if len(points) >= 2:
            return path, points, "StringVector2List"
    if isinstance(value, GDValue) and value.type_name == "PoolVector2Array":
        return path, [(float(x), float(y)) for x, y in value.value], "PoolVector2Array"
    if isinstance(value, GDValue) and value.type_name == "PoolRealArray" and "point" in last_key and len(value.value) >= 4 and len(value.value) % 2 == 0:
        nums = [float(v) for v in value.value]
        return path, list(zip(nums[::2], nums[1::2])), "PoolRealArrayPoints"
    if isinstance(value, list) and value:
        if all(isinstance(v, GDValue) and v.type_name == "Vector2" for v in value):
            return path, [_vector2(v) for v in value], "Vector2List"
        if all(isinstance(v, (list, tuple)) and len(v) >= 2 for v in value):
            return path, [(float(v[0]), float(v[1])) for v in value], "TupleList"
        if "point" in last_key and all(isinstance(v, (int, float)) for v in value) and len(value) >= 4 and len(value) % 2 == 0:
            nums = [float(v) for v in value]
            return path, list(zip(nums[::2], nums[1::2])), "FlatNumberList"
    if isinstance(value, dict):
        preferred = [k for k in value if str(k).lower() in {"points", "vertices", "control_points", "curve_points"}]
        for key in preferred + [k for k in value if k not in preferred]:
            result = _find_points_slot(value[key], path + (key,))
            if result:
                return result
    return None


def _set_nested(value: Any, path: tuple[Any, ...], new_value: Any) -> None:
    target = value
    for key in path[:-1]:
        target = target[key]
    target[path[-1]] = new_value


def _replace_record_points(record: Any, points: list[tuple[float, float]], slot: tuple[Any, ...] | None = None, slot_type: str | None = None) -> bool:
    found = _find_points_slot(record) if slot is None else None
    if found:
        slot, _, slot_type = found
    if not slot:
        return False
    if slot_type == "PoolVector2Array":
        replacement = GDValue("PoolVector2Array", points)
    elif slot_type == "PoolRealArrayPoints":
        replacement = GDValue("PoolRealArray", [coordinate for point in points for coordinate in point])
    elif slot_type == "Vector2List":
        replacement = [GDValue("Vector2", p) for p in points]
    elif slot_type == "FlatNumberList":
        replacement = [coordinate for point in points for coordinate in point]
    elif slot_type == "StringVector2List":
        replacement = to_godot_text([GDValue("Vector2", point) for point in points])
    else:
        replacement = [list(p) for p in points]
    _set_nested(record, slot, replacement)
    return True


def _path_style(record: dict[str, Any], root: dict[str, Any]) -> tuple[GDValue, float]:
    color: Any = None
    for key in ("color", "path_color", "tint"):
        if key in record:
            color = record[key]
            break
    if color is None:
        color = root.get("theme", {}).get("path_color", GDValue("Color", (0.2, 0.1, 0.05, 1)))
    width = 3.0
    for key in ("width", "size", "radius", "stroke_width"):
        try:
            if key in record:
                width = float(record[key])
                break
        except (TypeError, ValueError):
            pass
    return color, width


def export_svg_file(root: dict[str, Any], images: OrderedDict[str, GDValue], destination: Path, resolver: AssetResolver) -> dict[str, int]:
    width = float(root.get("map_width", 512.0))
    height = float(root.get("map_height", 512.0))
    svg = ET.Element(f"{{{SVG_NS}}}svg", {
        "width": f"{width:g}px", "height": f"{height:g}px", "viewBox": f"0 0 {width:g} {height:g}", "version": "1.1",
    })
    _set_wd(svg, "format-version", "1")
    _set_wd(svg, "map-width", width)
    _set_wd(svg, "map-height", height)
    metadata = ET.SubElement(svg, f"{{{SVG_NS}}}metadata")
    metadata.text = "Wonderdraft Map Editor SVG interchange file"
    defs = ET.SubElement(svg, f"{{{SVG_NS}}}defs")

    bg_group = ET.SubElement(svg, f"{{{SVG_NS}}}g", {"id": "wonderdraft-mask-background"})
    mask = _find_image_by_leaf(images, "mask")
    if mask is not None:
        info = image_object_info(mask)
        if info:
            png = _rgba_png_bytes(info[0], info[1], info[2], info[4])
            bg = ET.SubElement(bg_group, f"{{{SVG_NS}}}image", {
                "x": "0", "y": "0", "width": f"{width:g}", "height": f"{height:g}",
                "preserveAspectRatio": "none", f"{{{XLINK_NS}}}href": "data:image/png;base64," + base64.b64encode(png).decode("ascii"),
            })
            _set_wd(bg, "kind", "background")
            _set_wd(bg, "image-key", "mask")

    paths_group = ET.SubElement(svg, f"{{{SVG_NS}}}g", {"id": "wonderdraft-paths"})
    exported_paths = 0
    for index, record in enumerate(root.get("paths", []) or []):
        if not isinstance(record, dict):
            continue
        found = _find_points_slot(record)
        if not found or len(found[1]) < 2:
            continue
        slot, points, slot_type = found
        path_position = _vector2(record.get("position"))
        absolute_points = [
            (x + path_position[0], y + path_position[1]) for x, y in points
        ]
        color, line_width = _path_style(record, root)
        stroke, opacity = _svg_color(color)
        el = ET.SubElement(paths_group, f"{{{SVG_NS}}}polyline", {
            "id": f"wonderdraft-path-{index}",
            "points": " ".join(f"{x:.6g},{y:.6g}" for x, y in absolute_points),
            "fill": "none", "stroke": stroke, "stroke-opacity": f"{opacity:.6g}",
            "stroke-width": f"{line_width:.6g}", "stroke-linecap": "round", "stroke-linejoin": "round",
        })
        _set_wd(el, "kind", "path")
        _set_wd(el, "record", _record_encode(record))
        _set_wd(el, "points-slot", json.dumps(list(slot), ensure_ascii=False))
        _set_wd(el, "points-type", slot_type)
        exported_paths += 1

    symbol_group = ET.SubElement(svg, f"{{{SVG_NS}}}g", {"id": "wonderdraft-symbols"})
    filters: dict[tuple[int, int, int], str] = {}
    exported_symbols = 0
    missing_symbols = 0
    for index, record in enumerate(root.get("symbols", []) or []):
        if not isinstance(record, dict):
            continue
        texture = str(record.get("texture", ""))
        position = _vector2(record.get("position"))
        # Wonderdraft's position is the map-space visual centre.  ``offset`` is
        # retained in the record, but adding it again here double-shifts assets.
        center = position
        radius = float(record.get("radius", 16.0))
        scale = _vector2(record.get("scale"), (1.0, 1.0))
        rotation = float(record.get("rotation", 0.0))
        mirror = bool(record.get("mirror", False))
        sample = record.get("sample", GDValue("Color", (1, 1, 1, 1)))
        sample_hex, sample_alpha = _svg_color(sample)
        asset = resolver.asset_info(texture)
        if asset:
            factor = radius / asset.base_radius if asset.base_radius else 1.0
            rendered_width = max(0.001, abs(asset.width * factor * scale[0]))
            rendered_height = max(0.001, abs(asset.height * factor * scale[1]))
            x = center[0] - rendered_width / 2
            y = center[1] - rendered_height / 2
            attrs = {
                "id": f"wonderdraft-symbol-{index}", "x": f"{x:.6g}", "y": f"{y:.6g}",
                "width": f"{rendered_width:.6g}", "height": f"{rendered_height:.6g}",
                "preserveAspectRatio": "none", f"{{{XLINK_NS}}}href": asset.path.as_uri(),
            }
            transforms: list[str] = []
            if mirror:
                transforms.append(f"translate({2 * center[0]:.6g} 0) scale(-1 1)")
            if rotation:
                transforms.append(f"rotate({math.degrees(rotation):.9g} {center[0]:.6g} {center[1]:.6g})")
            if transforms:
                attrs["transform"] = " ".join(transforms)
            rgb_key = tuple(round(_clamp01(v) * 255) for v in _color_tuple(sample)[:3])
            if rgb_key not in filters:
                filter_id = f"wonderdraft-color-{len(filters)}"
                filters[rgb_key] = filter_id
                filt = ET.SubElement(defs, f"{{{SVG_NS}}}filter", {
                    "id": filter_id, "x": "-0.1", "y": "-0.1", "width": "1.2", "height": "1.2",
                    "color-interpolation-filters": "sRGB",
                })
                ET.SubElement(filt, f"{{{SVG_NS}}}feFlood", {"flood-color": sample_hex, "result": "wdFlood"})
                ET.SubElement(filt, f"{{{SVG_NS}}}feComposite", {"in": "wdFlood", "in2": "SourceAlpha", "operator": "in", "result": "wdColor"})
                ET.SubElement(filt, f"{{{SVG_NS}}}feBlend", {"in": "SourceGraphic", "in2": "wdColor", "mode": "multiply", "result": "wdBlend"})
                ET.SubElement(filt, f"{{{SVG_NS}}}feComposite", {"in": "wdBlend", "in2": "SourceAlpha", "operator": "in"})
            attrs["filter"] = f"url(#{filters[rgb_key]})"
            attrs["opacity"] = f"{sample_alpha:.6g}"
            el = ET.SubElement(symbol_group, f"{{{SVG_NS}}}image", attrs)
            _set_wd(el, "asset-path", str(asset.path))
            _set_wd(el, "export-width", rendered_width)
            _set_wd(el, "export-height", rendered_height)
        else:
            missing_symbols += 1
            el = ET.SubElement(symbol_group, f"{{{SVG_NS}}}circle", {
                "id": f"wonderdraft-symbol-{index}", "cx": f"{center[0]:.6g}", "cy": f"{center[1]:.6g}",
                "r": f"{max(1.0, radius * max(abs(scale[0]), abs(scale[1]))):.6g}",
                "fill": sample_hex, "fill-opacity": f"{sample_alpha:.6g}", "stroke": "#ff00ff", "stroke-width": "1",
            })
            _set_wd(el, "fallback", "circle")
            _set_wd(el, "export-width", radius * 2 * abs(scale[0]))
            _set_wd(el, "export-height", radius * 2 * abs(scale[1]))
        _set_wd(el, "kind", "symbol")
        _set_wd(el, "record", _record_encode(record))
        _set_wd(el, "texture", texture)
        _set_wd(el, "sample", ",".join(f"{v:.9g}" for v in _color_tuple(sample)))
        exported_symbols += 1

    label_group = ET.SubElement(svg, f"{{{SVG_NS}}}g", {"id": "wonderdraft-labels"})
    exported_labels = 0
    for index, record in enumerate(root.get("labels", []) or []):
        if not isinstance(record, dict):
            continue
        x, y = _vector2(record.get("position"))
        size = float(record.get("size", 24.0))
        rotation = float(record.get("rotation", 0.0))
        align = int(record.get("align", 1))
        anchors = {0: "start", 1: "middle", 2: "end"}
        fill, fill_opacity = _svg_color(record.get("color", GDValue("Color", (0, 0, 0, 1))))
        outline, outline_opacity = _svg_color(record.get("outline_color", GDValue("Color", (1, 1, 1, 0))))
        outline_size = float(record.get("outline_size", 0.0))
        attrs = {
            "id": f"wonderdraft-label-{index}", "x": f"{x:.6g}", "y": f"{y:.6g}",
            "font-family": str(record.get("font", "sans-serif")), "font-size": f"{size:.6g}px",
            "text-anchor": anchors.get(align, "middle"), "dominant-baseline": "central",
            "fill": fill, "fill-opacity": f"{fill_opacity:.6g}",
        }
        if outline_size > 0 and outline_opacity > 0:
            attrs.update({"stroke": outline, "stroke-opacity": f"{outline_opacity:.6g}", "stroke-width": f"{outline_size * 2:.6g}", "paint-order": "stroke fill"})
        if rotation:
            attrs["transform"] = f"rotate({math.degrees(rotation):.9g} {x:.6g} {y:.6g})"
        el = ET.SubElement(label_group, f"{{{SVG_NS}}}text", attrs)
        el.text = str(record.get("text", ""))
        _set_wd(el, "kind", "label")
        _set_wd(el, "record", _record_encode(record))
        exported_labels += 1

    try:
        ET.indent(svg, space="  ")
    except AttributeError:
        pass
    tree = ET.ElementTree(svg)
    destination.parent.mkdir(parents=True, exist_ok=True)
    tree.write(destination, encoding="utf-8", xml_declaration=True)
    return {"labels": exported_labels, "symbols": exported_symbols, "paths": exported_paths, "missing_symbols": missing_symbols}



def _xml_attrs(attributes: dict[str, Any]) -> str:
    return "".join(f" {name}={quoteattr(str(value))}" for name, value in attributes.items())


def _write_xml(handle: BinaryIO, text: str) -> None:
    handle.write(text.encode("utf-8"))


def _stream_base64_file(source: Path, output: BinaryIO, chunk_size: int = 1024 * 1024) -> None:
    """Write base64 without ever creating the complete encoded string."""
    remainder = b""
    with source.open("rb", buffering=1024 * 1024) as handle:
        while True:
            chunk = handle.read(chunk_size)
            if not chunk:
                break
            chunk = remainder + chunk
            usable = len(chunk) - (len(chunk) % 3)
            if usable:
                output.write(base64.b64encode(chunk[:usable]))
            remainder = chunk[usable:]
    if remainder:
        output.write(base64.b64encode(remainder))


def export_svg_file(
    root: dict[str, Any],
    images: OrderedDict[str, GDValue],
    destination: Path,
    resolver: AssetResolver,
    *,
    embed_background: bool = False,
) -> dict[str, Any]:
    """Stream an SVG and its mask PNG to disk with bounded memory.

    By default the mask is stored as ``<svg-name>.mask.png`` and referenced by
    the SVG. Embedding remains available, but is streamed through a temporary
    PNG so even very large maps do not create multi-gigabyte Python strings.
    """
    width = float(root.get("map_width", 512.0))
    height = float(root.get("map_height", 512.0))
    destination.parent.mkdir(parents=True, exist_ok=True)

    mask_png: Path | None = None
    temporary_mask = False
    mask = _find_image_by_leaf(images, "mask")
    if mask is not None:
        info = image_object_info(mask)
        if info:
            if embed_background:
                temp = tempfile.NamedTemporaryFile(
                    prefix=destination.stem + ".mask.", suffix=".png",
                    dir=destination.parent, delete=False,
                )
                mask_png = Path(temp.name)
                temp.close()
                temporary_mask = True
            else:
                mask_png = destination.with_name(destination.stem + ".mask.png")
            write_png_streaming(mask_png, info[0], info[1], info[2], info[4])

    symbols_data = root.get("symbols", []) or []
    filters: dict[tuple[int, int, int], str] = {}
    for record in symbols_data:
        if not isinstance(record, dict):
            continue
        sample = record.get("sample", GDValue("Color", (1, 1, 1, 1)))
        rgb_key = tuple(round(_clamp01(v) * 255) for v in _color_tuple(sample)[:3])
        filters.setdefault(rgb_key, f"wonderdraft-color-{len(filters)}")

    exported_paths = 0
    exported_symbols = 0
    exported_labels = 0
    missing_symbols = 0

    try:
        with destination.open("wb", buffering=1024 * 1024) as handle:
            _write_xml(handle, '<?xml version="1.0" encoding="utf-8"?>\n')
            root_attrs = {
                "xmlns": SVG_NS, "xmlns:xlink": XLINK_NS, "xmlns:wd": WD_NS,
                "width": f"{width:g}px", "height": f"{height:g}px",
                "viewBox": f"0 0 {width:g} {height:g}", "version": "1.1",
                "wd:format-version": "1", "wd:map-width": f"{width:g}",
                "wd:map-height": f"{height:g}",
            }
            _write_xml(handle, f"<svg{_xml_attrs(root_attrs)}>\n")
            _write_xml(handle, "  <metadata>Wonderdraft Map Editor SVG interchange file</metadata>\n")
            _write_xml(handle, "  <defs>\n")
            for rgb_key, filter_id in filters.items():
                sample_hex = "#{:02x}{:02x}{:02x}".format(*rgb_key)
                _write_xml(handle, f'    <filter{_xml_attrs({"id": filter_id, "x": "-0.1", "y": "-0.1", "width": "1.2", "height": "1.2", "color-interpolation-filters": "sRGB"})}>\n')
                _write_xml(handle, f'      <feFlood{_xml_attrs({"flood-color": sample_hex, "result": "wdFlood"})}/>\n')
                _write_xml(handle, '      <feComposite in="wdFlood" in2="SourceAlpha" operator="in" result="wdColor"/>\n')
                _write_xml(handle, '      <feBlend in="SourceGraphic" in2="wdColor" mode="multiply" result="wdBlend"/>\n')
                _write_xml(handle, '      <feComposite in="wdBlend" in2="SourceAlpha" operator="in"/>\n')
                _write_xml(handle, "    </filter>\n")
            _write_xml(handle, "  </defs>\n")

            _write_xml(handle, '  <g id="wonderdraft-mask-background">\n')
            if mask_png is not None:
                attrs = {
                    "x": "0", "y": "0", "width": f"{width:g}", "height": f"{height:g}",
                    "preserveAspectRatio": "none", "wd:kind": "background", "wd:image-key": "mask",
                }
                if embed_background:
                    _write_xml(handle, f"    <image{_xml_attrs(attrs)} xlink:href=\"data:image/png;base64,")
                    _stream_base64_file(mask_png, handle)
                    _write_xml(handle, '"/>\n')
                else:
                    attrs["xlink:href"] = urllib.parse.quote(mask_png.name)
                    _write_xml(handle, f"    <image{_xml_attrs(attrs)}/>\n")
            _write_xml(handle, "  </g>\n")

            _write_xml(handle, '  <g id="wonderdraft-paths">\n')
            for index, record in enumerate(root.get("paths", []) or []):
                if not isinstance(record, dict):
                    continue
                found = _find_points_slot(record)
                if not found or len(found[1]) < 2:
                    continue
                slot, points, slot_type = found
                path_position = _vector2(record.get("position"))
                absolute_points = [(x + path_position[0], y + path_position[1]) for x, y in points]
                color, line_width = _path_style(record, root)
                stroke, opacity = _svg_color(color)
                attrs = {
                    "id": f"wonderdraft-path-{index}",
                    "points": " ".join(f"{x:.6g},{y:.6g}" for x, y in absolute_points),
                    "fill": "none", "stroke": stroke, "stroke-opacity": f"{opacity:.6g}",
                    "stroke-width": f"{line_width:.6g}", "stroke-linecap": "round",
                    "stroke-linejoin": "round", "wd:kind": "path",
                    "wd:record": _record_encode(record),
                    "wd:points-slot": json.dumps(list(slot), ensure_ascii=False),
                    "wd:points-type": slot_type,
                }
                _write_xml(handle, f"    <polyline{_xml_attrs(attrs)}/>\n")
                exported_paths += 1
            _write_xml(handle, "  </g>\n")

            _write_xml(handle, '  <g id="wonderdraft-symbols">\n')
            for index, record in enumerate(symbols_data):
                if not isinstance(record, dict):
                    continue
                texture = str(record.get("texture", ""))
                center = _vector2(record.get("position"))
                radius = float(record.get("radius", 16.0))
                scale = _vector2(record.get("scale"), (1.0, 1.0))
                rotation = float(record.get("rotation", 0.0))
                mirror = bool(record.get("mirror", False))
                sample = record.get("sample", GDValue("Color", (1, 1, 1, 1)))
                sample_hex, sample_alpha = _svg_color(sample)
                rgb_key = tuple(round(_clamp01(v) * 255) for v in _color_tuple(sample)[:3])
                common = {
                    "id": f"wonderdraft-symbol-{index}", "wd:kind": "symbol",
                    "wd:record": _record_encode(record), "wd:texture": texture,
                    "wd:sample": ",".join(f"{v:.9g}" for v in _color_tuple(sample)),
                }
                asset = resolver.asset_info(texture)
                if asset:
                    factor = radius / asset.base_radius if asset.base_radius else 1.0
                    rendered_width = max(0.001, abs(asset.width * factor * scale[0]))
                    rendered_height = max(0.001, abs(asset.height * factor * scale[1]))
                    x = center[0] - rendered_width / 2
                    y = center[1] - rendered_height / 2
                    attrs = {
                        **common, "x": f"{x:.6g}", "y": f"{y:.6g}",
                        "width": f"{rendered_width:.6g}", "height": f"{rendered_height:.6g}",
                        "preserveAspectRatio": "none", "xlink:href": asset.path.as_uri(),
                        "filter": f"url(#{filters[rgb_key]})", "opacity": f"{sample_alpha:.6g}",
                        "wd:asset-path": str(asset.path), "wd:export-width": f"{rendered_width:.9g}",
                        "wd:export-height": f"{rendered_height:.9g}",
                    }
                    transforms: list[str] = []
                    if mirror:
                        transforms.append(f"translate({2 * center[0]:.6g} 0) scale(-1 1)")
                    if rotation:
                        transforms.append(f"rotate({math.degrees(rotation):.9g} {center[0]:.6g} {center[1]:.6g})")
                    if transforms:
                        attrs["transform"] = " ".join(transforms)
                    _write_xml(handle, f"    <image{_xml_attrs(attrs)}/>\n")
                else:
                    missing_symbols += 1
                    attrs = {
                        **common, "cx": f"{center[0]:.6g}", "cy": f"{center[1]:.6g}",
                        "r": f"{max(1.0, radius * max(abs(scale[0]), abs(scale[1]))):.6g}",
                        "fill": sample_hex, "fill-opacity": f"{sample_alpha:.6g}",
                        "stroke": "#ff00ff", "stroke-width": "1", "wd:fallback": "circle",
                        "wd:export-width": f"{radius * 2 * abs(scale[0]):.9g}",
                        "wd:export-height": f"{radius * 2 * abs(scale[1]):.9g}",
                    }
                    _write_xml(handle, f"    <circle{_xml_attrs(attrs)}/>\n")
                exported_symbols += 1
            _write_xml(handle, "  </g>\n")

            _write_xml(handle, '  <g id="wonderdraft-labels">\n')
            anchors = {0: "start", 1: "middle", 2: "end"}
            for index, record in enumerate(root.get("labels", []) or []):
                if not isinstance(record, dict):
                    continue
                x, y = _vector2(record.get("position"))
                size = float(record.get("size", 24.0))
                rotation = float(record.get("rotation", 0.0))
                align = int(record.get("align", 1))
                fill, fill_opacity = _svg_color(record.get("color", GDValue("Color", (0, 0, 0, 1))))
                outline, outline_opacity = _svg_color(record.get("outline_color", GDValue("Color", (1, 1, 1, 0))))
                outline_size = float(record.get("outline_size", 0.0))
                attrs = {
                    "id": f"wonderdraft-label-{index}", "x": f"{x:.6g}", "y": f"{y:.6g}",
                    "font-family": str(record.get("font", "sans-serif")), "font-size": f"{size:.6g}px",
                    "text-anchor": anchors.get(align, "middle"), "dominant-baseline": "central",
                    "fill": fill, "fill-opacity": f"{fill_opacity:.6g}",
                    "wd:kind": "label", "wd:record": _record_encode(record),
                }
                if outline_size > 0 and outline_opacity > 0:
                    attrs.update({
                        "stroke": outline, "stroke-opacity": f"{outline_opacity:.6g}",
                        "stroke-width": f"{outline_size * 2:.6g}", "paint-order": "stroke fill",
                    })
                if rotation:
                    attrs["transform"] = f"rotate({math.degrees(rotation):.9g} {x:.6g} {y:.6g})"
                _write_xml(handle, f"    <text{_xml_attrs(attrs)}>{escape(str(record.get('text', '')))}</text>\n")
                exported_labels += 1
            _write_xml(handle, "  </g>\n</svg>\n")
    finally:
        if temporary_mask and mask_png is not None:
            try:
                mask_png.unlink()
            except OSError:
                pass

    return {
        "labels": exported_labels, "symbols": exported_symbols,
        "paths": exported_paths, "missing_symbols": missing_symbols,
        "background": "embedded" if embed_background and mask_png else (mask_png.name if mask_png else "none"),
    }


def _element_text(element: ET.Element) -> str:
    return "".join(element.itertext()).strip()


def _float_attr(element: ET.Element, name: str, default=0.0) -> float:
    value = element.get(name)
    if value is None:
        return default
    values = re.findall(r"[-+]?(?:\d+\.\d*|\.\d+|\d+)(?:[eE][-+]?\d+)?", value)
    return float(values[0]) if values else default


def _filter_colors(root: ET.Element) -> dict[str, str]:
    result: dict[str, str] = {}
    for element in root.iter():
        if _tag(element) != "filter" or not element.get("id"):
            continue
        color = None
        for child in element.iter():
            if _tag(child) == "feFlood":
                color = child.get("flood-color") or _style_map(child).get("flood-color")
                if color:
                    break
        if color:
            result[element.get("id", "")] = color
    return result


def _style_color(element: ET.Element, filters: dict[str, str], kind="fill", default="#ffffff") -> GDValue:
    style = _style_map(element)
    value = style.get(kind, default)
    opacity = float(style.get(f"{kind}-opacity", "1") or 1) * float(style.get("opacity", "1") or 1)
    if kind == "fill" and (not value or value == "none"):
        filter_ref = style.get("filter", "")
        match = re.search(r"url\(#([^)]*)\)", filter_ref)
        if match and match.group(1) in filters:
            value = filters[match.group(1)]
    return _parse_css_color(value, opacity)


def _svg_root_matrix(svg: ET.Element, map_width: float, map_height: float) -> Matrix:
    viewbox = svg.get("viewBox")
    if viewbox:
        vals = [float(v) for v in re.findall(r"[-+]?(?:\d+\.\d*|\.\d+|\d+)(?:[eE][-+]?\d+)?", viewbox)]
        if len(vals) == 4 and vals[2] and vals[3]:
            return (map_width / vals[2], 0, 0, map_height / vals[3], -vals[0] * map_width / vals[2], -vals[1] * map_height / vals[3])
    width = _parse_svg_length(svg.get("width")) or map_width
    height = _parse_svg_length(svg.get("height")) or map_height
    return (map_width / width, 0, 0, map_height / height, 0, 0)


def _walk_visible(element: ET.Element, parent_matrix: Matrix = IDENTITY):
    local = _parse_transform(element.get("transform"))
    combined = _mat_mul(parent_matrix, local)
    tag = _tag(element)
    if tag not in {"svg", "g", "defs", "metadata", "namedview"}:
        yield element, combined
    if tag not in {"defs", "metadata"}:
        for child in list(element):
            yield from _walk_visible(child, combined)


def _transformed_rect(element: ET.Element, matrix: Matrix) -> tuple[tuple[float, float], float, float, float, bool]:
    x, y = _float_attr(element, "x"), _float_attr(element, "y")
    width, height = _float_attr(element, "width"), _float_attr(element, "height")
    p0 = _mat_apply(matrix, x, y)
    px = _mat_apply(matrix, x + width, y)
    py = _mat_apply(matrix, x, y + height)
    center = _mat_apply(matrix, x + width / 2, y + height / 2)
    rendered_width = math.dist(p0, px)
    rendered_height = math.dist(p0, py)
    _, _, angle, mirrored = _matrix_scale_rotation(matrix)
    return center, rendered_width, rendered_height, angle, mirrored


def _default_label(root: dict[str, Any]) -> OrderedDict:
    labels = root.get("labels") or []
    if labels and isinstance(labels[0], dict):
        result = OrderedDict(labels[0])
    else:
        city = root.get("theme", {}).get("label_presets", {}).get("City", {})
        result = OrderedDict([
            ("align", 1), ("color", city.get("font_color", GDValue("Color", (0.15, 0.08, 0.03, 1)))),
            ("curve", 0.0), ("extra_spacing_char", 0), ("font", city.get("font_name", "sans-serif")),
            ("glow_color", GDValue("Color", (1, 1, 1, 1))), ("glow_size", 0),
            ("outline_color", city.get("font_outline_color", GDValue("Color", (1, 1, 1, 0)))),
            ("outline_size", int(float(city.get("font_outline_width", 0)))),
            ("position", GDValue("Vector2", (0, 0))), ("rotation", 0.0),
            ("size", int(float(city.get("font_size", 24)))), ("text", ""), ("z_index", 0),
        ])
    return result


def _import_label(element: ET.Element, matrix: Matrix, root: dict[str, Any], filters: dict[str, str]) -> dict[str, Any]:
    record = _record_decode(_get_wd(element, "record"))
    if not isinstance(record, dict):
        record = _default_label(root)
    style = _style_map(element)
    first_span = next((child for child in element.iter() if child is not element and _tag(child) in {"tspan", "textPath"}), None)
    if first_span is not None:
        style.update(_style_map(first_span))
    coordinate_source = element
    if first_span is not None and (element.get("x") is None or element.get("y") is None):
        coordinate_source = first_span
    x = _float_attr(coordinate_source, "x")
    y = _float_attr(coordinate_source, "y")
    x, y = _mat_apply(matrix, x, y)
    sx, sy, angle, _ = _matrix_scale_rotation(matrix)
    size = _parse_svg_length(style.get("font-size", element.get("font-size", "24"))) * math.sqrt(max(0.000001, sx * sy))
    baseline = style.get("dominant-baseline", "")
    if baseline not in {"middle", "central"}:
        # SVG text normally stores a baseline; Wonderdraft stores the visual center.
        y -= size * 0.26
    record["position"] = GDValue("Vector2", (x, y))
    record["rotation"] = angle
    record["size"] = max(1, int(round(size)))
    family = style.get("font-family", element.get("font-family", str(record.get("font", "sans-serif"))))
    record["font"] = family.strip("'\"").split(",")[0].strip()
    anchor = style.get("text-anchor", "middle")
    record["align"] = {"start": 0, "middle": 1, "end": 2}.get(anchor, 1)
    record["text"] = _element_text(element)
    record["color"] = _style_color(element, filters, "fill", "#000000")
    stroke = style.get("stroke")
    if stroke and stroke != "none":
        record["outline_color"] = _style_color(element, filters, "stroke", "#ffffff")
        stroke_width = _parse_svg_length(style.get("stroke-width", "0")) * math.sqrt(max(0.000001, sx * sy))
        record["outline_size"] = max(0, int(round(stroke_width / 2)))
    else:
        record["outline_size"] = 0
    return record


def _default_symbol(position=(0.0, 0.0), radius=16.0, texture=FALLBACK_TEXTURE, sample=None) -> OrderedDict:
    return OrderedDict([
        ("custom_color_mode", None), ("custom_colors", None), ("mirror", False),
        ("offset", GDValue("Vector2", (0, 0))), ("outline_color", GDValue("Color", (1, 1, 1, 1))),
        ("outline_width", 0), ("position", GDValue("Vector2", position)), ("radius", float(radius)),
        ("rotation", 0.0), ("sample", sample or GDValue("Color", (1, 1, 1, 1))),
        ("scale", GDValue("Vector2", (1, 1))), ("texture", texture),
        ("type", "symbol"), ("z_index", 0),
    ])


def _infer_symbol_type(texture: str) -> str:
    lower = texture.casefold()
    if "mountain" in lower or "hill" in lower or "rock" in lower:
        return "mountain"
    if "tree" in lower or "forest" in lower:
        return "tree"
    return "symbol"


def _image_href(element: ET.Element) -> str:
    return element.get(f"{{{XLINK_NS}}}href", element.get("href", ""))


def _update_custom_pack_lists(root: dict[str, Any], symbols: list[dict[str, Any]]) -> None:
    packs = list(root.get("included_packs", []) or [])
    seen = {str(p) for p in packs}
    for symbol in symbols:
        texture = str(symbol.get("texture", ""))
        if texture.startswith("user://assets/"):
            rest = texture[len("user://assets/"):]
            pack = rest.split("/", 1)[0]
            if pack and pack not in seen:
                packs.append(pack)
                seen.add(pack)
    root["included_packs"] = packs


def _import_symbol(element: ET.Element, matrix: Matrix, root: dict[str, Any], svg_path: Path, resolver: AssetResolver, filters: dict[str, str]) -> dict[str, Any]:
    tag = _tag(element)
    original = _record_decode(_get_wd(element, "record"))
    record = OrderedDict(original) if isinstance(original, dict) else None
    href = _image_href(element) if tag == "image" else ""
    mapped_texture = resolver.texture_for_path(href, svg_path.parent) if href else None
    if tag == "image":
        center, rendered_width, rendered_height, angle, mirrored = _transformed_rect(element, matrix)
    else:
        cx, cy, r = _float_attr(element, "cx"), _float_attr(element, "cy"), _float_attr(element, "r", 1)
        center = _mat_apply(matrix, cx, cy)
        sx, sy, angle, mirrored = _matrix_scale_rotation(matrix)
        rendered_width, rendered_height = 2 * r * sx, 2 * r * sy

    sample = None
    sample_attr = _get_wd(element, "sample")
    if sample_attr:
        try:
            vals = [float(v) for v in sample_attr.split(",")]
            if len(vals) == 4:
                sample = GDValue("Color", tuple(vals))
        except Exception:
            pass
    if sample is None:
        style = _style_map(element)
        filter_ref = style.get("filter", "")
        filter_match = re.search(r"url\(#([^)]*)\)", filter_ref)
        if filter_match and filter_match.group(1) in filters:
            opacity = float(style.get("opacity", "1") or 1)
            sample = _parse_css_color(filters[filter_match.group(1)], opacity)
        else:
            sample = _style_color(element, filters, "fill", "#ffffff")

    if record is None:
        texture = mapped_texture or FALLBACK_TEXTURE
        asset = resolver.asset_info(texture)
        if asset:
            ratio_x = rendered_width / asset.width if asset.width else 1.0
            ratio_y = rendered_height / asset.height if asset.height else 1.0
            max_ratio = max(ratio_x, ratio_y, 1e-6)
            radius = asset.base_radius * max_ratio
            scale = (ratio_x / max_ratio, ratio_y / max_ratio)
        else:
            radius = max(rendered_width, rendered_height) / 2
            scale = (rendered_width / max(1e-6, radius * 2), rendered_height / max(1e-6, radius * 2))
        record = _default_symbol(center, radius, texture, sample)
        record["scale"] = GDValue("Vector2", scale)
        record["type"] = _infer_symbol_type(texture)
    else:
        texture = str(record.get("texture", FALLBACK_TEXTURE))
        # A referenced external image must map to a configured asset. Unknown
        # replacement images become the documented stock-capital fallback.
        if tag == "image":
            if mapped_texture:
                texture = mapped_texture
            elif href and not href.startswith("data:"):
                texture = FALLBACK_TEXTURE
        record["texture"] = texture
        old_width = float(_get_wd(element, "export-width", "0") or 0)
        old_height = float(_get_wd(element, "export-height", "0") or 0)
        old_scale = _vector2(record.get("scale"), (1, 1))
        old_radius = float(record.get("radius", 16.0))
        if old_width > 0 and old_height > 0:
            ratio_x = rendered_width / old_width
            ratio_y = rendered_height / old_height
            uniform = math.sqrt(max(1e-12, ratio_x * ratio_y))
            record["radius"] = old_radius * uniform
            record["scale"] = GDValue("Vector2", (old_scale[0] * ratio_x / uniform, old_scale[1] * ratio_y / uniform))
        elif tag == "circle":
            record["radius"] = max(rendered_width, rendered_height) / 2
        record["position"] = GDValue("Vector2", center)
    record["rotation"] = angle
    record["mirror"] = bool(mirrored)
    record["sample"] = sample
    return record


def _parse_points_attr(value: str) -> list[tuple[float, float]]:
    nums = [float(v) for v in re.findall(r"[-+]?(?:\d+\.\d*|\.\d+|\d+)(?:[eE][-+]?\d+)?", value)]
    return list(zip(nums[::2], nums[1::2]))


def _sample_cubic(p0, p1, p2, p3, count=8):
    out = []
    for i in range(1, count + 1):
        t = i / count
        mt = 1 - t
        out.append((mt**3*p0[0] + 3*mt*mt*t*p1[0] + 3*mt*t*t*p2[0] + t**3*p3[0], mt**3*p0[1] + 3*mt*mt*t*p1[1] + 3*mt*t*t*p2[1] + t**3*p3[1]))
    return out


def _sample_quadratic(p0, p1, p2, count=8):
    out = []
    for i in range(1, count + 1):
        t = i / count
        mt = 1 - t
        out.append((mt*mt*p0[0] + 2*mt*t*p1[0] + t*t*p2[0], mt*mt*p0[1] + 2*mt*t*p1[1] + t*t*p2[1]))
    return out


def _parse_path_d(d: str) -> list[tuple[float, float]]:
    tokens = re.findall(r"[A-Za-z]|[-+]?(?:\d+\.\d*|\.\d+|\d+)(?:[eE][-+]?\d+)?", d)
    i = 0
    command = ""
    current = (0.0, 0.0)
    start = current
    last_control = current
    points: list[tuple[float, float]] = []
    arg_counts = {"M": 2, "L": 2, "H": 1, "V": 1, "C": 6, "S": 4, "Q": 4, "T": 2, "A": 7, "Z": 0}
    while i < len(tokens):
        if tokens[i].isalpha():
            command = tokens[i]
            i += 1
        if not command:
            break
        upper = command.upper()
        relative = command.islower()
        if upper == "Z":
            points.append(start)
            current = start
            command = ""
            continue
        needed = arg_counts.get(upper)
        if needed is None or i + needed > len(tokens):
            break
        vals = [float(v) for v in tokens[i:i+needed]]
        i += needed
        def point(x, y):
            return (x + current[0], y + current[1]) if relative else (x, y)
        if upper == "M":
            current = point(vals[0], vals[1]); start = current; points.append(current); command = "l" if relative else "L"
        elif upper == "L":
            current = point(vals[0], vals[1]); points.append(current)
        elif upper == "H":
            current = (current[0] + vals[0], current[1]) if relative else (vals[0], current[1]); points.append(current)
        elif upper == "V":
            current = (current[0], current[1] + vals[0]) if relative else (current[0], vals[0]); points.append(current)
        elif upper == "C":
            p1, p2, p3 = point(vals[0], vals[1]), point(vals[2], vals[3]), point(vals[4], vals[5])
            points.extend(_sample_cubic(current, p1, p2, p3)); current = p3; last_control = p2
        elif upper == "S":
            p1 = (2*current[0]-last_control[0], 2*current[1]-last_control[1])
            p2, p3 = point(vals[0], vals[1]), point(vals[2], vals[3])
            points.extend(_sample_cubic(current, p1, p2, p3)); current = p3; last_control = p2
        elif upper == "Q":
            p1, p2 = point(vals[0], vals[1]), point(vals[2], vals[3])
            points.extend(_sample_quadratic(current, p1, p2)); current = p2; last_control = p1
        elif upper == "T":
            p1 = (2*current[0]-last_control[0], 2*current[1]-last_control[1])
            p2 = point(vals[0], vals[1]); points.extend(_sample_quadratic(current, p1, p2)); current = p2; last_control = p1
        elif upper == "A":
            # Arc flattening is intentionally conservative: retain the endpoint.
            current = point(vals[5], vals[6]); points.append(current)
    return points


def _element_points(element: ET.Element, matrix: Matrix) -> list[tuple[float, float]]:
    tag = _tag(element)
    if tag in {"polyline", "polygon"}:
        points = _parse_points_attr(element.get("points", ""))
    elif tag == "line":
        points = [(_float_attr(element, "x1"), _float_attr(element, "y1")), (_float_attr(element, "x2"), _float_attr(element, "y2"))]
    elif tag == "path":
        points = _parse_path_d(element.get("d", ""))
    else:
        points = []
    return [_mat_apply(matrix, *point) for point in points]


def _default_path_record(points: list[tuple[float, float]], root: dict[str, Any], element: ET.Element, filters: dict[str, str]) -> OrderedDict:
    # Version-15 Wonderdraft maps store path control points as a Godot-text
    # string rather than a PoolVector2Array.  This fuller template imports
    # ordinary Inkscape strokes as paths Wonderdraft can edit again.
    style = _style_map(element)
    color = _style_color(element, filters, "stroke", "#4f3016")
    width = _parse_svg_length(style.get("stroke-width", "3"))
    point_text = to_godot_text([GDValue("Vector2", point) for point in points])
    return OrderedDict([
        ("color", color),
        ("noise_seed", 0),
        ("points", point_text),
        ("position", GDValue("Vector2", (0, 0))),
        ("roughness", 0.33),
        ("straight", False),
        ("style", "res://textures/paths/path_blended"),
        ("width", float(width)),
        ("z_index", 0),
    ])


def _import_path(element: ET.Element, matrix: Matrix, root: dict[str, Any], filters: dict[str, str]) -> dict[str, Any] | None:
    points = _element_points(element, matrix)
    if len(points) < 2:
        return None
    record = _record_decode(_get_wd(element, "record"))
    if not isinstance(record, dict):
        return _default_path_record(points, root, element, filters)
    slot = None
    try:
        raw_slot = _get_wd(element, "points-slot")
        if raw_slot:
            slot = tuple(json.loads(raw_slot))
    except Exception:
        slot = None
    slot_type = _get_wd(element, "points-type")
    # Exported SVG points are absolute map coordinates.  Wonderdraft records
    # may store them relative to a separate path ``position``.
    path_position = _vector2(record.get("position"))
    local_points = [
        (x - path_position[0], y - path_position[1]) for x, y in points
    ]
    if not _replace_record_points(record, local_points, slot, slot_type):
        return None
    style = _style_map(element)
    if "color" in record and style.get("stroke"):
        record["color"] = _style_color(element, filters, "stroke", "#4f3016")
    for key in ("width", "size", "radius", "stroke_width"):
        if key in record and style.get("stroke-width"):
            record[key] = _parse_svg_length(style["stroke-width"])
            break
    return record


def import_svg_file(root: dict[str, Any], source: Path, resolver: AssetResolver) -> tuple[dict[str, Any], dict[str, int | list[str]]]:
    svg = ET.parse(source).getroot()
    map_width = float(root.get("map_width", _get_wd(svg, "map-width", "512") or 512))
    map_height = float(root.get("map_height", _get_wd(svg, "map-height", "512") or 512))
    root_matrix = _svg_root_matrix(svg, map_width, map_height)
    filters = _filter_colors(svg)
    labels: list[dict[str, Any]] = []
    symbols: list[dict[str, Any]] = []
    paths: list[dict[str, Any]] = []
    warnings: list[str] = []

    for element, matrix in _walk_visible(svg, root_matrix):
        tag = _tag(element)
        kind = _get_wd(element, "kind")
        if kind == "background":
            continue
        if kind == "label" or (not kind and tag == "text"):
            labels.append(_import_label(element, matrix, root, filters))
            continue
        if kind == "symbol" or (not kind and tag in {"image", "circle", "ellipse"}):
            if tag == "image" and not kind:
                # Ignore a full-page raster used as an editing backdrop.
                center, rw, rh, _, _ = _transformed_rect(element, matrix)
                if rw >= map_width * 0.9 and rh >= map_height * 0.9:
                    continue
            try:
                symbols.append(_import_symbol(element, matrix, root, source, resolver, filters))
            except Exception as exc:
                warnings.append(f"Skipped symbol {element.get('id', '')}: {exc}")
            continue
        if kind == "path" or (not kind and tag in {"path", "polyline", "polygon", "line"} and _style_map(element).get("fill", "none") == "none"):
            imported = _import_path(element, matrix, root, filters)
            if imported:
                paths.append(imported)

    group_ids = {element.get("id") for element in svg.iter() if _tag(element) == "g"}
    replace_labels = bool(labels) or "wonderdraft-labels" in group_ids
    replace_symbols = bool(symbols) or "wonderdraft-symbols" in group_ids
    replace_paths = bool(paths) or "wonderdraft-paths" in group_ids
    if replace_labels:
        root["labels"] = labels
    if replace_symbols:
        root["symbols"] = symbols
        _update_custom_pack_lists(root, symbols)
    if replace_paths:
        root["paths"] = paths
    return root, {"labels": len(labels), "symbols": len(symbols), "paths": len(paths), "warnings": warnings}


# ----------------------------------- GUI -------------------------------------

class WonderdraftEditor(tk.Tk):
    def __init__(self):
        super().__init__()
        self.title("Wonderdraft Map Editor — low-memory SVG edition")
        self.geometry("1240x820")
        self.minsize(940, 620)
        self.current_path: Path | None = None
        self.block_size = 4096
        self.images: OrderedDict[str, GDValue] = OrderedDict()
        self.preview_ref = None
        self._map_temp: tempfile.TemporaryDirectory[str] | None = None
        self.use_compression = tk.BooleanVar(value=True)
        self.embed_svg_background = tk.BooleanVar(value=False)
        self.verify_saved_map = tk.BooleanVar(value=True)
        self.settings = load_settings()
        self.asset_resolver = AssetResolver(self.settings["custom_asset_folder"], self.settings["default_asset_folder"])
        self._build_ui()
        self.protocol("WM_DELETE_WINDOW", self.close_editor)
        self.after(50, self._persist_detected_settings)

    def _persist_detected_settings(self):
        if any(self.settings.values()):
            try:
                save_settings(self.settings)
            except OSError:
                pass

    def _build_ui(self):
        toolbar = ttk.Frame(self, padding=6)
        toolbar.pack(fill="x")
        for label, command in [
            ("Open map", self.open_map), ("Validate text", self.validate_text),
            ("Save map as…", self.save_map), ("Export SVG…", self.export_svg),
            ("Import SVG…", self.import_svg), ("Export all PNGs", self.export_all_images),
            ("Asset folders…", self.configure_assets),
        ]:
            ttk.Button(toolbar, text=label, command=command).pack(side="left", padx=3)
        ttk.Checkbutton(toolbar, text="Compress saved map", variable=self.use_compression).pack(side="left", padx=(12, 3))
        ttk.Checkbutton(toolbar, text="Verify save", variable=self.verify_saved_map).pack(side="left", padx=3)
        ttk.Checkbutton(
            toolbar, text="Embed mask in SVG", variable=self.embed_svg_background
        ).pack(side="left", padx=3)
        self.status = tk.StringVar(value="Open a .wonderdraft_map file")
        ttk.Label(toolbar, textvariable=self.status).pack(side="right", padx=8)

        paned = ttk.Panedwindow(self, orient="horizontal")
        paned.pack(fill="both", expand=True)
        left = ttk.Frame(paned, padding=(6, 0, 3, 6))
        right = ttk.Frame(paned, padding=(3, 0, 6, 6))
        paned.add(left, weight=4); paned.add(right, weight=1)

        ttk.Label(left, text="Map data (Godot text syntax)").pack(anchor="w")
        text_frame = ttk.Frame(left)
        text_frame.pack(fill="both", expand=True)
        self.text = tk.Text(text_frame, wrap="none", undo=True, maxundo=100, font=("TkFixedFont", 10))
        ybar = ttk.Scrollbar(text_frame, orient="vertical", command=self.text.yview)
        xbar = ttk.Scrollbar(text_frame, orient="horizontal", command=self.text.xview)
        self.text.configure(yscrollcommand=ybar.set, xscrollcommand=xbar.set)
        self.text.grid(row=0, column=0, sticky="nsew"); ybar.grid(row=0, column=1, sticky="ns")
        xbar.grid(row=1, column=0, sticky="ew")
        text_frame.rowconfigure(0, weight=1); text_frame.columnconfigure(0, weight=1)

        ttk.Label(right, text="Embedded images").pack(anchor="w")
        self.image_list = tk.Listbox(right, exportselection=False, height=8)
        self.image_list.pack(fill="x", pady=(4, 6))
        self.image_list.bind("<<ListboxSelect>>", lambda _e: self.show_preview())
        buttons = ttk.Frame(right); buttons.pack(fill="x")
        ttk.Button(buttons, text="Export PNG", command=self.export_selected_image).pack(side="left", expand=True, fill="x", padx=(0, 2))
        ttk.Button(buttons, text="Replace PNG", command=self.replace_selected_image).pack(side="left", expand=True, fill="x", padx=(2, 0))
        self.image_info = tk.StringVar(value="")
        ttk.Label(right, textvariable=self.image_info, wraplength=280).pack(anchor="w", pady=6)
        self.asset_info_var = tk.StringVar(value=self._asset_status_text())
        ttk.Label(right, textvariable=self.asset_info_var, wraplength=280).pack(anchor="w", pady=(0, 6))
        self.preview = ttk.Label(right, anchor="center")
        self.preview.pack(fill="both", expand=True)

    def _asset_status_text(self) -> str:
        custom = self.settings.get("custom_asset_folder") or "not configured"
        default = self.settings.get("default_asset_folder") or "not configured"
        cache = self.settings.get("cache_folder") or str(detect_cache_folder())
        return f"Custom assets: {custom}\nDefault sprites: {default}\nDisk cache: {cache}"

    def configure_assets(self):
        dialog = tk.Toplevel(self)
        dialog.title("Wonderdraft asset folders")
        dialog.transient(self)
        dialog.grab_set()
        dialog.columnconfigure(1, weight=1)
        custom_var = tk.StringVar(value=self.settings.get("custom_asset_folder", ""))
        default_var = tk.StringVar(value=self.settings.get("default_asset_folder", ""))
        cache_var = tk.StringVar(value=self.settings.get("cache_folder", str(detect_cache_folder())))

        def choose(variable: tk.StringVar):
            chosen = filedialog.askdirectory(parent=dialog, initialdir=variable.get() or str(Path.home()))
            if chosen:
                variable.set(chosen)

        ttk.Label(dialog, text="Custom asset folder:").grid(row=0, column=0, sticky="w", padx=8, pady=(10, 4))
        ttk.Entry(dialog, textvariable=custom_var, width=70).grid(row=0, column=1, sticky="ew", padx=4, pady=(10, 4))
        ttk.Button(dialog, text="Browse…", command=lambda: choose(custom_var)).grid(row=0, column=2, padx=8, pady=(10, 4))
        ttk.Label(dialog, text="Default sprites folder:").grid(row=1, column=0, sticky="w", padx=8, pady=4)
        ttk.Entry(dialog, textvariable=default_var, width=70).grid(row=1, column=1, sticky="ew", padx=4, pady=4)
        ttk.Button(dialog, text="Browse…", command=lambda: choose(default_var)).grid(row=1, column=2, padx=8, pady=4)
        ttk.Label(dialog, text="Disk cache folder:").grid(row=2, column=0, sticky="w", padx=8, pady=4)
        ttk.Entry(dialog, textvariable=cache_var, width=70).grid(row=2, column=1, sticky="ew", padx=4, pady=4)
        ttk.Button(dialog, text="Browse…", command=lambda: choose(cache_var)).grid(row=2, column=2, padx=8, pady=4)
        ttk.Label(
            dialog,
            text="The default folder should be the extracted PCK's sprites directory.\n"
                 "The disk cache must have room for the uncompressed map (often several GiB).",
            justify="left",
        ).grid(row=3, column=0, columnspan=3, sticky="w", padx=8, pady=8)
        buttons = ttk.Frame(dialog); buttons.grid(row=4, column=0, columnspan=3, sticky="e", padx=8, pady=10)

        def auto_detect():
            custom = detect_custom_asset_folder()
            default = detect_default_asset_folder()
            if custom: custom_var.set(str(custom))
            if default: default_var.set(str(default))
            cache_var.set(str(detect_cache_folder()))

        def apply():
            self.settings = {
                "custom_asset_folder": custom_var.get().strip(),
                "default_asset_folder": default_var.get().strip(),
                "cache_folder": cache_var.get().strip() or str(detect_cache_folder()),
            }
            try:
                Path(self.settings["cache_folder"]).expanduser().mkdir(parents=True, exist_ok=True)
            except OSError as exc:
                messagebox.showerror("Settings", f"Could not create cache folder:\n{exc}", parent=dialog)
                return
            try:
                save_settings(self.settings)
            except OSError as exc:
                messagebox.showerror("Settings", f"Could not write {CONFIG_PATH}:\n{exc}", parent=dialog)
                return
            self.asset_resolver = AssetResolver(self.settings["custom_asset_folder"], self.settings["default_asset_folder"])
            self.asset_info_var.set(self._asset_status_text())
            self.status.set(f"Saved settings to {CONFIG_PATH.name}")
            dialog.destroy()

        ttk.Button(buttons, text="Auto-detect", command=auto_detect).pack(side="left", padx=4)
        ttk.Button(buttons, text="Cancel", command=dialog.destroy).pack(side="left", padx=4)
        ttk.Button(buttons, text="Save", command=apply).pack(side="left", padx=4)

    def close_editor(self):
        temp = self._map_temp
        self._map_temp = None
        self.images.clear()
        self.preview_ref = None
        if temp is not None:
            try:
                temp.cleanup()
            except OSError:
                pass
        self.destroy()

    def _status_progress(self, action: str, done: int, total: int):
        percent = (100.0 * done / total) if total else 100.0
        self.status.set(f"{action}: {percent:5.1f}% — {done / (1024**2):,.1f} / {total / (1024**2):,.1f} MiB")
        self.update_idletasks()

    def _set_text_root(self, root: dict[str, Any]):
        self.text.configure(undo=False)
        self.text.delete("1.0", "end")
        buffer: list[str] = []
        buffered = 0
        for piece in iter_godot_text(root):
            buffer.append(piece)
            buffered += len(piece)
            if buffered >= 1024 * 1024:
                self.text.insert("end", "".join(buffer))
                buffer.clear(); buffered = 0
                self.update_idletasks()
        buffer.append("\n")
        self.text.insert("end", "".join(buffer))
        self.text.edit_reset()
        self.text.configure(undo=True)

    def open_map(self):
        filename = filedialog.askopenfilename(
            filetypes=[("Wonderdraft map", "*.wonderdraft_map"), ("All files", "*")]
        )
        if not filename:
            return
        new_temp: tempfile.TemporaryDirectory[str] | None = None
        try:
            self.status.set("Creating disk-backed map cache…")
            self.update_idletasks()
            cache_root = Path(self.settings.get("cache_folder") or detect_cache_folder()).expanduser()
            cache_root.mkdir(parents=True, exist_ok=True)
            with Path(filename).open("rb") as source_header:
                header = source_header.read(16)
            if len(header) != 16 or header[:4] != GCPF_MAGIC:
                raise FormatError("The selected file is not a valid GCPF Wonderdraft map")
            required_cache = struct.unpack("<I", header[12:16])[0]
            free_cache = shutil.disk_usage(cache_root).free
            if free_cache < required_cache + 64 * 1024 * 1024:
                raise FormatError(
                    f"The disk cache needs about {required_cache / (1024**3):.2f} GiB, but only "
                    f"{free_cache / (1024**3):.2f} GiB is free in {cache_root}."
                )
            new_temp = tempfile.TemporaryDirectory(prefix="wonderdraft_gui_", dir=cache_root)
            stream_path = Path(new_temp.name) / "map.variant"
            meta = decompress_gcpf_path(
                Path(filename), stream_path,
                lambda done, total: self._status_progress("Decompressing to disk", done, total),
            )
            self.status.set("Decoding map metadata…")
            self.update_idletasks()
            root, _ = decode_store_var_file(stream_path)
            if not isinstance(root, dict):
                raise FormatError("The map root is not a Godot Dictionary")
            images = find_images(root)
            editable = replace_images_with_names(root, images)
            self._set_text_root(editable)

            old_temp = self._map_temp
            self._map_temp = new_temp
            new_temp = None
            self.current_path = Path(filename)
            self.block_size = int(meta.get("block_size", 4096))
            self.images = images
            self.image_list.delete(0, "end")
            for name in self.images:
                self.image_list.insert("end", name)
            self.preview_ref = None
            self.preview.configure(image="", text="")
            if self.images:
                self.image_list.selection_set(0)
                self.show_preview()
            if old_temp is not None:
                old_temp.cleanup()
            raw_gib = int(meta.get("uncompressed_size", 0)) / (1024**3)
            self.status.set(
                f"Loaded {self.current_path.name} — {len(self.images)} disk-backed images; "
                f"{raw_gib:,.2f} GiB cache, image pixels not held in RAM"
            )
            del root, editable
            gc.collect()
        except Exception as exc:
            if new_temp is not None:
                new_temp.cleanup()
            messagebox.showerror("Open failed", str(exc))
            self.status.set("Open failed")

    def validate_text(self, quiet=False):
        try:
            parsed = parse_godot_text(self.text.get("1.0", "end"))
            if not isinstance(parsed, dict):
                raise FormatError("Root value must be a Dictionary")
            if not quiet: messagebox.showinfo("Valid", "The map text is syntactically valid.")
            self.status.set("Text validated successfully")
            return parsed
        except Exception as exc:
            if not quiet: messagebox.showerror("Invalid map text", str(exc))
            self.status.set("Validation failed")
            return None

    def _replace_text_root(self, root: dict[str, Any]):
        self._set_text_root(root)

    def save_map(self):
        parsed = self.validate_text(quiet=True)
        if parsed is None:
            return
        filename = filedialog.asksaveasfilename(
            defaultextension=".wonderdraft_map",
            initialfile=(self.current_path.stem + "_edited.wonderdraft_map") if self.current_path else "edited.wonderdraft_map",
            filetypes=[("Wonderdraft map", "*.wonderdraft_map")],
        )
        if not filename:
            return
        try:
            self.status.set("Streaming map to compressed output…")
            self.update_idletasks()
            root = restore_images(parsed, self.images)
            destination = Path(filename)
            packed_size = write_wonderdraft_map_streaming(
                root, destination, self.block_size, compressed=self.use_compression.get()
            )
            verified = False
            if self.verify_saved_map.get():
                self.status.set("Verifying saved map with low-memory decoder…")
                self.update_idletasks()
                cache_root = Path(self.settings.get("cache_folder") or detect_cache_folder()).expanduser()
                cache_root.mkdir(parents=True, exist_ok=True)
                with tempfile.TemporaryDirectory(prefix="wonderdraft_verify_", dir=cache_root) as verify_dir:
                    verify_path = Path(verify_dir) / "verify.variant"
                    decompress_gcpf_path(destination, verify_path)
                    check_root, _ = decode_store_var_file(verify_path)
                    if not isinstance(check_root, dict):
                        raise FormatError("Verification produced a non-dictionary root")
                    del check_root
                verified = True
            mode = "FastLZ compressed" if self.use_compression.get() else "literal-only compatibility"
            verification = "; verified" if verified else ""
            self.status.set(f"Saved {destination.name} ({packed_size:,} bytes, {mode}{verification})")
            messagebox.showinfo(
                "Saved",
                f"Created:\n{filename}\n\nSize: {packed_size:,} bytes\nMode: {mode}\n"
                + ("The file passed a low-memory decode verification." if verified else "Verification was disabled."),
            )
            del root, parsed
            gc.collect()
        except Exception as exc:
            messagebox.showerror("Save failed", str(exc))
            self.status.set("Save failed")

    def export_svg(self):
        parsed = self.validate_text(quiet=True)
        if parsed is None:
            return
        filename = filedialog.asksaveasfilename(
            defaultextension=".svg",
            initialfile=(self.current_path.stem + ".svg") if self.current_path else "wonderdraft_map.svg",
            filetypes=[("SVG", "*.svg")],
        )
        if not filename:
            return
        try:
            mode = "embedded mask" if self.embed_svg_background.get() else "external mask PNG"
            self.status.set(f"Streaming SVG export ({mode})…")
            self.update_idletasks()
            summary = export_svg_file(
                parsed, self.images, Path(filename), self.asset_resolver,
                embed_background=self.embed_svg_background.get(),
            )
            self.status.set(
                f"Exported SVG: {summary['labels']} labels, {summary['symbols']} symbols, "
                f"{summary['paths']} paths; background {summary['background']}"
            )
            detail = ""
            if summary["missing_symbols"]:
                detail = f"\n\n{summary['missing_symbols']} missing sprites were represented by magenta-outlined SVG circles."
            if not self.embed_svg_background.get() and summary.get("background") not in {"none", None}:
                detail += f"\n\nThe mask is stored beside the SVG as {summary['background']}. Keep both files together."
            messagebox.showinfo(
                "SVG exported",
                f"Created:\n{filename}\n\nLabels: {summary['labels']}\nSymbols: {summary['symbols']}\n"
                f"Paths: {summary['paths']}\nBackground: {summary['background']}{detail}",
            )
            del parsed
            gc.collect()
        except Exception as exc:
            messagebox.showerror("SVG export failed", str(exc))
            self.status.set("SVG export failed")

    def import_svg(self):
        parsed = self.validate_text(quiet=True)
        if parsed is None: return
        filename = filedialog.askopenfilename(filetypes=[("SVG", "*.svg"), ("All files", "*")])
        if not filename: return
        try:
            updated, summary = import_svg_file(parsed, Path(filename), self.asset_resolver)
            self._replace_text_root(updated)
            warning_text = ""
            warnings = summary.get("warnings", [])
            if warnings:
                warning_text = "\n\nWarnings:\n" + "\n".join(str(w) for w in warnings[:10])
            self.status.set(f"Imported SVG: {summary['labels']} labels, {summary['symbols']} symbols, {summary['paths']} paths")
            messagebox.showinfo("SVG imported", f"Updated the editable map data.\n\nLabels: {summary['labels']}\nSymbols: {summary['symbols']}\nPaths: {summary['paths']}{warning_text}")
        except Exception as exc:
            messagebox.showerror("SVG import failed", str(exc))

    def selected_image_key(self) -> str | None:
        sel = self.image_list.curselection()
        return self.image_list.get(sel[0]) if sel else None

    def export_selected_image(self):
        key = self.selected_image_key()
        if not key:
            return
        info = image_object_info(self.images[key])
        if not info:
            return
        filename = filedialog.asksaveasfilename(
            defaultextension=".png", initialfile=f".{key.split('.')[-1]}.png",
            filetypes=[("PNG", "*.png")],
        )
        if not filename:
            return
        try:
            self.status.set(f"Streaming {key} to PNG…")
            self.update_idletasks()
            write_png_streaming(Path(filename), info[0], info[1], info[2], info[4])
            self.status.set(f"Exported {key}")
        except Exception as exc:
            messagebox.showerror("Export failed", str(exc))

    def export_all_images(self):
        if not self.images:
            return
        directory = filedialog.askdirectory()
        if not directory:
            return
        try:
            exported = 0
            for index, (key, image_obj) in enumerate(self.images.items(), start=1):
                info = image_object_info(image_obj)
                if info:
                    self.status.set(f"Exporting image {index}/{len(self.images)}: {key}")
                    self.update_idletasks()
                    write_png_streaming(
                        Path(directory) / f".{key.split('.')[-1]}.png",
                        info[0], info[1], info[2], info[4],
                    )
                    exported += 1
            self.status.set(f"Exported {exported} PNG files with bounded memory")
        except Exception as exc:
            messagebox.showerror("Export failed", str(exc))

    def replace_selected_image(self):
        key = self.selected_image_key()
        if not key:
            return
        filename = filedialog.askopenfilename(
            filetypes=[("PNG", "*.png"), ("Images", "*.png;*.jpg;*.jpeg;*.webp")]
        )
        if not filename:
            return
        try:
            backing_dir = Path(self._map_temp.name) if self._map_temp is not None else None
            self.images[key] = png_to_image_object(
                Path(filename), self.images[key], backing_dir=backing_dir
            )
            self.show_preview()
            self.status.set(f"Replaced {key} with {Path(filename).name}; raw pixels are disk-backed")
        except Exception as exc:
            messagebox.showerror("Replace failed", str(exc))

    def show_preview(self):
        key = self.selected_image_key()
        if not key:
            return
        info = image_object_info(self.images[key])
        if not info:
            return
        width, height, fmt, mipmaps, pixels = info
        storage = "disk-backed" if isinstance(pixels, DiskByteArray) else "in memory"
        self.image_info.set(
            f"{key}\n{width} × {height}, {fmt}, {_source_length(pixels):,} raw bytes\n{storage}"
        )
        if Image is None or ImageTk is None:
            self.preview.configure(image="", text="Install Pillow for preview")
            return
        try:
            img = make_image_thumbnail(width, height, fmt, pixels, 300)
            if img is None:
                self.preview.configure(image="", text=f"Preview unsupported for {fmt}")
                return
            self.preview_ref = ImageTk.PhotoImage(img)
            self.preview.configure(image=self.preview_ref, text="")
        except Exception as exc:
            self.preview.configure(image="", text=f"Preview failed: {exc}")



def main() -> int:
    app = WonderdraftEditor()
    app.mainloop()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

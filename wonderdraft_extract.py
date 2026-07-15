#!/usr/bin/env python3
"""Extract data and embedded images from Wonderdraft .wonderdraft_map files.

The format used by the tested Wonderdraft map is:

1. A Godot GCPF compressed-file container.
2. Compression mode 0 (FastLZ), split into independent blocks.
3. A Godot 3 Variant stream as written by File.store_var(..., true).
4. Embedded Godot Image objects whose pixel data is stored in PoolByteArray.

This script is intentionally dependency-free and supports the Godot 3 Variant
value types used by Wonderdraft map files.
"""

from __future__ import annotations

import argparse
import json
import math
import re
import struct
import sys
import zlib
from collections import Counter, OrderedDict
from dataclasses import dataclass
from pathlib import Path
from typing import Any


GCPF_MAGIC = b"GCPF"
FASTLZ_MODE = 0
ENCODE_FLAG_64 = 1 << 16
ENCODE_FLAG_OBJECT_AS_ID = 1 << 16


class FormatError(ValueError):
    """Raised when a file does not match the expected format."""


@dataclass
class GDValue:
    """A tagged Godot value that has no direct JSON/Python equivalent."""

    type_name: str
    value: Any


# ---------------------------------------------------------------------------
# FastLZ decompression
# ---------------------------------------------------------------------------

def fastlz_decompress_block(source: bytes, max_output: int) -> bytes:
    """Decompress one independent FastLZ block (levels 1 and 2)."""
    if not source:
        return b""

    level = (source[0] >> 5) + 1
    if level not in (1, 2):
        raise FormatError(f"Unsupported FastLZ level {level}")

    ip = 0
    ip_limit = len(source)
    ip_bound = ip_limit - 2
    output = bytearray()

    ctrl = source[ip] & 31
    ip += 1

    def copy_match(reference: int, length: int) -> None:
        if reference < 0:
            raise FormatError("Invalid FastLZ backward reference")
        if len(output) + length > max_output:
            raise FormatError("FastLZ block expands beyond its declared size")
        # Byte-by-byte copying is required because LZ matches may overlap.
        for _ in range(length):
            output.append(output[reference])
            reference += 1

    while True:
        if ctrl >= 32:
            length = (ctrl >> 5) - 1
            offset = (ctrl & 31) << 8
            reference = len(output) - offset - 1

            if level == 1:
                if length == 6:
                    if ip > ip_bound:
                        raise FormatError("Truncated FastLZ length")
                    length += source[ip]
                    ip += 1

                if ip >= ip_limit:
                    raise FormatError("Truncated FastLZ offset")
                reference -= source[ip]
                ip += 1
                length += 3

            else:  # FastLZ level 2
                if length == 6:
                    while True:
                        if ip > ip_bound:
                            raise FormatError("Truncated FastLZ extended length")
                        code = source[ip]
                        ip += 1
                        length += code
                        if code != 255:
                            break

                if ip >= ip_limit:
                    raise FormatError("Truncated FastLZ offset")
                code = source[ip]
                ip += 1
                reference -= code
                length += 3

                if code == 255 and offset == (31 << 8):
                    if ip >= ip_bound:
                        raise FormatError("Truncated FastLZ far-distance match")
                    offset = (source[ip] << 8) + source[ip + 1]
                    ip += 2
                    reference = len(output) - offset - 8191 - 1

            copy_match(reference, length)

        else:
            literal_count = ctrl + 1
            if ip + literal_count > ip_limit:
                raise FormatError("Truncated FastLZ literal")
            if len(output) + literal_count > max_output:
                raise FormatError("FastLZ literal exceeds declared block size")
            output.extend(source[ip : ip + literal_count])
            ip += literal_count

        if (level == 1 and ip > ip_bound) or (level == 2 and ip >= ip_limit):
            break

        ctrl = source[ip]
        ip += 1

    return bytes(output)


def decompress_gcpf(blob: bytes) -> tuple[bytes, dict[str, Any]]:
    """Decompress a Godot GCPF container and return data plus header metadata."""
    if len(blob) < 20:
        raise FormatError("File is too small to be a GCPF container")

    magic, mode, block_size, uncompressed_size = struct.unpack_from("<4sIII", blob, 0)
    if magic != GCPF_MAGIC:
        raise FormatError(f"Invalid GCPF magic: {magic!r}")
    if mode != FASTLZ_MODE:
        raise FormatError(
            f"Unsupported GCPF compression mode {mode}; this extractor currently supports FastLZ mode 0"
        )
    if block_size <= 0:
        raise FormatError("Invalid GCPF block size")

    block_count = uncompressed_size // block_size + 1
    table_end = 16 + 4 * block_count
    if table_end > len(blob):
        raise FormatError("Truncated GCPF block-size table")

    compressed_sizes = struct.unpack_from(f"<{block_count}I", blob, 16)
    position = table_end
    output_parts: list[bytes] = []

    for index, compressed_size in enumerate(compressed_sizes):
        end = position + compressed_size
        if end > len(blob):
            raise FormatError(f"Truncated GCPF block {index}")
        compressed = blob[position:end]
        position = end

        expected = block_size
        if index == block_count - 1:
            expected = uncompressed_size - block_size * (block_count - 1)

        decompressed = fastlz_decompress_block(compressed, expected)
        if len(decompressed) != expected:
            raise FormatError(
                f"GCPF block {index} decoded to {len(decompressed)} bytes; expected {expected}"
            )
        output_parts.append(decompressed)

    if blob[position : position + 4] != GCPF_MAGIC:
        raise FormatError("Missing trailing GCPF magic")
    if position + 4 != len(blob):
        raise FormatError("Unexpected bytes after trailing GCPF magic")

    output = b"".join(output_parts)
    if len(output) != uncompressed_size:
        raise FormatError("GCPF uncompressed-size mismatch")

    metadata = {
        "container": "GCPF",
        "compression_mode": mode,
        "compression_name": "FastLZ",
        "block_size": block_size,
        "block_count": block_count,
        "uncompressed_size": uncompressed_size,
        "compressed_block_sizes": list(compressed_sizes),
    }
    return output, metadata


# ---------------------------------------------------------------------------
# Godot 3 Variant decoding
# ---------------------------------------------------------------------------

class Godot3VariantParser:
    TYPE_NAMES = {
        0: "Nil",
        1: "Bool",
        2: "Int",
        3: "Real",
        4: "String",
        5: "Vector2",
        6: "Rect2",
        7: "Vector3",
        8: "Transform2D",
        9: "Plane",
        10: "Quat",
        11: "AABB",
        12: "Basis",
        13: "Transform",
        14: "Color",
        15: "NodePath",
        16: "RID",
        17: "Object",
        18: "Dictionary",
        19: "Array",
        20: "PoolByteArray",
        21: "PoolIntArray",
        22: "PoolRealArray",
        23: "PoolStringArray",
        24: "PoolVector2Array",
        25: "PoolVector3Array",
        26: "PoolColorArray",
    }

    def __init__(self, data: bytes):
        self.data = data
        self.position = 0
        self.type_counts: Counter[str] = Counter()

    def _need(self, size: int) -> None:
        if self.position + size > len(self.data):
            raise FormatError(
                f"Unexpected end of Godot Variant data at offset {self.position}; need {size} bytes"
            )

    def _u32(self) -> int:
        self._need(4)
        value = struct.unpack_from("<I", self.data, self.position)[0]
        self.position += 4
        return value

    def _i32(self) -> int:
        self._need(4)
        value = struct.unpack_from("<i", self.data, self.position)[0]
        self.position += 4
        return value

    def _i64(self) -> int:
        self._need(8)
        value = struct.unpack_from("<q", self.data, self.position)[0]
        self.position += 8
        return value

    def _f32(self) -> float:
        self._need(4)
        value = struct.unpack_from("<f", self.data, self.position)[0]
        self.position += 4
        return value

    def _f64(self) -> float:
        self._need(8)
        value = struct.unpack_from("<d", self.data, self.position)[0]
        self.position += 8
        return value

    def _raw_string(self) -> str:
        length = self._u32()
        self._need(length)
        raw = self.data[self.position : self.position + length]
        self.position += length
        self.position += (-length) % 4
        try:
            return raw.decode("utf-8")
        except UnicodeDecodeError as exc:
            raise FormatError(f"Invalid UTF-8 string at offset {self.position - length}") from exc

    def parse(self, depth: int = 0) -> Any:
        if depth > 100:
            raise FormatError("Godot Variant nesting is unreasonably deep")

        header_offset = self.position
        header = self._u32()
        type_id = header & 0xFF
        flags = header & ~0xFF

        type_name = self.TYPE_NAMES.get(type_id)
        if type_name is None:
            raise FormatError(
                f"Unknown Godot Variant type {type_id} at offset {header_offset}"
            )
        self.type_counts[type_name] += 1

        if type_id == 0:
            return None
        if type_id == 1:
            return bool(self._u32())
        if type_id == 2:
            return self._i64() if flags & ENCODE_FLAG_64 else self._i32()
        if type_id == 3:
            return self._f64() if flags & ENCODE_FLAG_64 else self._f32()
        if type_id == 4:
            return self._raw_string()

        # Godot 3 math values use 32-bit components in the default build.
        if type_id == 5:
            return GDValue("Vector2", (self._f32(), self._f32()))
        if type_id == 6:
            return GDValue("Rect2", tuple(self._f32() for _ in range(4)))
        if type_id == 7:
            return GDValue("Vector3", tuple(self._f32() for _ in range(3)))
        if type_id == 8:
            return GDValue("Transform2D", tuple(self._f32() for _ in range(6)))
        if type_id == 9:
            return GDValue("Plane", tuple(self._f32() for _ in range(4)))
        if type_id == 10:
            return GDValue("Quat", tuple(self._f32() for _ in range(4)))
        if type_id == 11:
            return GDValue("AABB", tuple(self._f32() for _ in range(6)))
        if type_id == 12:
            return GDValue("Basis", tuple(self._f32() for _ in range(9)))
        if type_id == 13:
            return GDValue("Transform", tuple(self._f32() for _ in range(12)))
        if type_id == 14:
            return GDValue("Color", tuple(self._f32() for _ in range(4)))

        if type_id == 15:
            name_count_field = self._u32()
            if not (name_count_field & 0x80000000):
                raise FormatError("Old-format Godot NodePath is unsupported")
            name_count = name_count_field & 0x7FFFFFFF
            subname_count = self._u32()
            node_flags = self._u32()
            if node_flags & 2:  # obsolete property-separate flag
                subname_count += 1
            names = [self._raw_string() for _ in range(name_count)]
            subnames = [self._raw_string() for _ in range(subname_count)]
            return GDValue(
                "NodePath",
                {"names": names, "subnames": subnames, "absolute": bool(node_flags & 1)},
            )

        if type_id == 16:
            return GDValue("RID", None)

        if type_id == 17:
            if flags & ENCODE_FLAG_OBJECT_AS_ID:
                return GDValue("ObjectID", self._i64())

            class_name = self._raw_string()
            if class_name == "":
                return None

            property_count = self._u32()
            properties: OrderedDict[str, Any] = OrderedDict()
            for _ in range(property_count):
                property_name = self._raw_string()
                properties[property_name] = self.parse(depth + 1)
            return GDValue(
                "Object", {"class": class_name, "properties": properties}
            )

        if type_id == 18:
            count = self._u32() & 0x7FFFFFFF
            result: OrderedDict[Any, Any] = OrderedDict()
            for _ in range(count):
                key = self.parse(depth + 1)
                value = self.parse(depth + 1)
                try:
                    result[key] = value
                except TypeError as exc:
                    raise FormatError("Encountered an unhashable Godot Dictionary key") from exc
            return result

        if type_id == 19:
            count = self._u32() & 0x7FFFFFFF
            return [self.parse(depth + 1) for _ in range(count)]

        if type_id == 20:
            count = self._u32()
            self._need(count)
            value = self.data[self.position : self.position + count]
            self.position += count
            self.position += (-count) % 4
            return GDValue("PoolByteArray", value)

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
            return GDValue(
                "PoolVector2Array",
                [(self._f32(), self._f32()) for _ in range(count)],
            )

        if type_id == 25:
            count = self._u32()
            return GDValue(
                "PoolVector3Array",
                [(self._f32(), self._f32(), self._f32()) for _ in range(count)],
            )

        if type_id == 26:
            count = self._u32()
            return GDValue(
                "PoolColorArray",
                [tuple(self._f32() for _ in range(4)) for _ in range(count)],
            )

        raise AssertionError("Unreachable")


def decode_store_var_stream(data: bytes) -> tuple[Any, dict[str, Any]]:
    """Decode the four-byte-length-prefixed payload written by File.store_var."""
    if len(data) < 8:
        raise FormatError("Decompressed data is too small to contain a Variant")

    payload_length = struct.unpack_from("<I", data, 0)[0]
    if payload_length != len(data) - 4:
        raise FormatError(
            f"Variant length prefix says {payload_length}, but {len(data) - 4} bytes follow"
        )

    parser = Godot3VariantParser(data[4:])
    result = parser.parse()
    if parser.position != payload_length:
        raise FormatError(
            f"Variant decoder consumed {parser.position} of {payload_length} bytes"
        )

    return result, {
        "variant_payload_size": payload_length,
        "variant_type_counts": dict(sorted(parser.type_counts.items())),
    }


# ---------------------------------------------------------------------------
# Image extraction and serialization
# ---------------------------------------------------------------------------

def _png_chunk(chunk_type: bytes, payload: bytes) -> bytes:
    return (
        struct.pack(">I", len(payload))
        + chunk_type
        + payload
        + struct.pack(">I", zlib.crc32(chunk_type + payload) & 0xFFFFFFFF)
    )


def write_png(path: Path, width: int, height: int, pixel_format: str, pixels: bytes) -> None:
    """Write uncompressed Godot Image pixel bytes as a conventional PNG."""
    format_table = {
        "L8": (1, 0),       # channels, PNG grayscale
        "LA8": (2, 4),     # grayscale + alpha
        "RGB8": (3, 2),    # truecolor
        "RGBA8": (4, 6),   # truecolor + alpha
    }
    if pixel_format not in format_table:
        raise FormatError(
            f"Unsupported embedded Image format {pixel_format!r}; supported: {', '.join(format_table)}"
        )

    channels, color_type = format_table[pixel_format]
    base_size = width * height * channels
    if len(pixels) < base_size:
        raise FormatError(
            f"Image buffer has {len(pixels)} bytes, but {width}x{height} {pixel_format} needs {base_size}"
        )

    # Ignore additional mipmap levels and export the full-resolution base image.
    pixels = pixels[:base_size]
    stride = width * channels
    scanlines = b"".join(
        b"\x00" + pixels[y * stride : (y + 1) * stride] for y in range(height)
    )

    png = bytearray(b"\x89PNG\r\n\x1a\n")
    png += _png_chunk(
        b"IHDR", struct.pack(">IIBBBBB", width, height, 8, color_type, 0, 0, 0)
    )
    png += _png_chunk(b"IDAT", zlib.compress(scanlines, 9))
    png += _png_chunk(b"IEND", b"")
    path.write_bytes(png)


def image_object_info(value: Any) -> tuple[int, int, str, bool, bytes] | None:
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

    return (
        int(image_data["width"]),
        int(image_data["height"]),
        str(image_data["format"]),
        bool(image_data.get("mipmaps", False)),
        byte_array.value,
    )


def safe_component(value: str) -> str:
    component = re.sub(r"[^A-Za-z0-9_.-]+", "_", value).strip("._")
    return component or "image"


def extract_images(value: Any, output_dir: Path, path_parts: tuple[str, ...] = ()) -> tuple[Any, list[dict[str, Any]]]:
    """Recursively extract Godot Image objects and replace them with PNG paths."""
    image = image_object_info(value)
    if image is not None:
        width, height, pixel_format, mipmaps, pixels = image

        if len(path_parts) == 1:
            # Match Wonderdraft's own sidecar naming convention.
            filename = f".{safe_component(path_parts[0])}.png"
        else:
            joined = ".".join(safe_component(part) for part in path_parts) or "image"
            filename = f".{joined}.png"

        destination = output_dir / filename
        write_png(destination, width, height, pixel_format, pixels)
        record = {
            "path": ".".join(path_parts),
            "filename": filename,
            "width": width,
            "height": height,
            "format": pixel_format,
            "mipmaps": mipmaps,
            "raw_byte_count": len(pixels),
        }
        return filename, [record]

    if isinstance(value, dict):
        result: OrderedDict[Any, Any] = OrderedDict()
        images: list[dict[str, Any]] = []
        for key, child in value.items():
            replacement, child_images = extract_images(
                child, output_dir, path_parts + (str(key),)
            )
            result[key] = replacement
            images.extend(child_images)
        return result, images

    if isinstance(value, list):
        result_list: list[Any] = []
        images: list[dict[str, Any]] = []
        for index, child in enumerate(value):
            replacement, child_images = extract_images(
                child, output_dir, path_parts + (str(index),)
            )
            result_list.append(replacement)
            images.extend(child_images)
        return result_list, images

    if isinstance(value, GDValue) and value.type_name == "Object":
        # Preserve non-Image objects in tagged form.
        properties, images = extract_images(
            value.value["properties"], output_dir, path_parts + ("properties",)
        )
        return GDValue(
            "Object", {"class": value.value["class"], "properties": properties}
        ), images

    return value, []


def format_number(value: float | int, force_float_marker: bool = False) -> str:
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, int):
        return str(value)
    if math.isnan(value):
        return "nan"
    if math.isinf(value):
        return "inf" if value > 0 else "-inf"
    # Godot's var2str output is close to six significant digits for these files.
    result = format(value, ".6g")
    if result == "-0":
        result = "0"
    # Standalone REAL Variants retain a decimal marker, while components of
    # Vector/Color/PoolRealArray values are printed without one when integral.
    if force_float_marker and "e" not in result.lower() and "." not in result:
        result += ".0"
    return result


def godot_string(value: str) -> str:
    # JSON escaping matches the quoted string syntax used by Godot's var2str.
    return json.dumps(value, ensure_ascii=False)


def to_godot_text(value: Any) -> str:
    """Render Python/GDValue data in Godot's var2str-like text syntax."""
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, int):
        return str(value)
    if isinstance(value, float):
        return format_number(value, force_float_marker=True)
    if isinstance(value, str):
        return godot_string(value)

    if isinstance(value, GDValue):
        name = value.type_name
        if name in {
            "Vector2", "Rect2", "Vector3", "Transform2D", "Plane", "Quat",
            "AABB", "Basis", "Transform", "Color"
        }:
            return f"{name}( " + ", ".join(format_number(v) for v in value.value) + " )"
        if name in {
            "PoolIntArray", "PoolRealArray", "PoolStringArray",
            "PoolVector2Array", "PoolVector3Array", "PoolColorArray"
        }:
            flat: list[Any] = []
            for element in value.value:
                if isinstance(element, tuple):
                    flat.extend(element)
                else:
                    flat.append(element)
            formatted = []
            for item in flat:
                if isinstance(item, str):
                    formatted.append(godot_string(item))
                else:
                    formatted.append(format_number(item))
            return f"{name}( " + ", ".join(formatted) + " )"
        if name == "PoolByteArray":
            return "PoolByteArray( " + ", ".join(str(v) for v in value.value) + " )"
        if name == "Object":
            return to_godot_text(
                OrderedDict(
                    [
                        ("__class__", value.value["class"]),
                        ("properties", value.value["properties"]),
                    ]
                )
            )
        return to_godot_text({"__type__": name, "value": value.value})

    if isinstance(value, list):
        if not value:
            return "[  ]"
        return "[ " + ", ".join(to_godot_text(item) for item in value) + " ]"

    if isinstance(value, dict):
        if not value:
            return "{\n}"
        # Godot's dictionary text output is sorted by key for string-keyed maps.
        items = list(value.items())
        if all(isinstance(key, str) for key, _ in items):
            items.sort(key=lambda item: item[0])
        lines = [f"{to_godot_text(key)}: {to_godot_text(child)}" for key, child in items]
        return "{\n" + ",\n".join(lines) + "\n}"

    raise TypeError(f"Cannot serialize value of type {type(value).__name__}")


def to_json_compatible(value: Any) -> Any:
    if isinstance(value, GDValue):
        if value.type_name == "PoolByteArray":
            return {
                "__godot_type__": value.type_name,
                "length": len(value.value),
                "hex_preview": value.value[:32].hex(),
            }
        return {
            "__godot_type__": value.type_name,
            "value": to_json_compatible(value.value),
        }
    if isinstance(value, dict):
        return {str(key): to_json_compatible(child) for key, child in value.items()}
    if isinstance(value, list):
        return [to_json_compatible(child) for child in value]
    if isinstance(value, tuple):
        return [to_json_compatible(child) for child in value]
    return value


# ---------------------------------------------------------------------------
# Command-line interface
# ---------------------------------------------------------------------------

def extract_map(input_path: Path, output_dir: Path, save_raw: bool = True) -> dict[str, Any]:
    blob = input_path.read_bytes()
    decompressed, container_metadata = decompress_gcpf(blob)
    decoded, variant_metadata = decode_store_var_stream(decompressed)

    output_dir.mkdir(parents=True, exist_ok=True)
    stem = input_path.stem

    replaced, images = extract_images(decoded, output_dir)

    text_path = output_dir / f"{stem}.txt"
    text_path.write_text(to_godot_text(replaced) + "\n", encoding="utf-8")

    json_path = output_dir / f"{stem}.decoded.json"
    json_path.write_text(
        json.dumps(to_json_compatible(replaced), ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )

    raw_path: Path | None = None
    if save_raw:
        raw_path = output_dir / f"{stem}.variant.bin"
        raw_path.write_bytes(decompressed)

    report = {
        "input_file": input_path.name,
        "input_size": len(blob),
        **container_metadata,
        **variant_metadata,
        "top_level_keys": list(decoded.keys()) if isinstance(decoded, dict) else None,
        "images": images,
        "outputs": {
            "text": text_path.name,
            "json": json_path.name,
            "raw_variant": raw_path.name if raw_path else None,
        },
    }

    report_path = output_dir / f"{stem}.report.json"
    report_path.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    report["outputs"]["report"] = report_path.name
    return report


def build_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Extract Godot Variant data and embedded PNGs from a Wonderdraft map"
    )
    parser.add_argument("input", type=Path, help="Path to a .wonderdraft_map file")
    parser.add_argument(
        "-o",
        "--output",
        type=Path,
        help="Output directory (default: <input stem>_extracted next to the input)",
    )
    parser.add_argument(
        "--no-raw",
        action="store_true",
        help="Do not save the decompressed length-prefixed Variant stream",
    )
    return parser


def main() -> int:
    args = build_arg_parser().parse_args()
    input_path: Path = args.input
    output_dir: Path = args.output or input_path.with_name(f"{input_path.stem}_extracted")

    try:
        report = extract_map(input_path, output_dir, save_raw=not args.no_raw)
    except (OSError, FormatError, KeyError, TypeError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1

    print(f"Extracted {input_path} -> {output_dir}")
    print(f"Compression: {report['compression_name']} in {report['block_count']} blocks")
    print(f"Decoded Variant bytes: {report['variant_payload_size']}")
    print(f"Embedded images: {len(report['images'])}")
    for image in report["images"]:
        print(
            f"  {image['path']}: {image['width']}x{image['height']} "
            f"{image['format']} -> {image['filename']}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

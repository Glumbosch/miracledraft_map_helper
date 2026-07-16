#!/usr/bin/env sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
binary=${WONDERDRAFT_EDITOR_BINARY:-"$project_dir/wonderdraft-editor"}

if [ ! -x "$binary" ]; then
    binary="$project_dir/target/release/wonderdraft-editor"
fi

if [ ! -x "$binary" ]; then
    echo "Release executable not found: $binary" >&2
    echo "Build it first with: cargo build --release, or run this script from an extracted Linux release archive." >&2
    exit 1
fi

data_home=${XDG_DATA_HOME:-"$HOME/.local/share"}
app_dir="$data_home/wonderdraft-map-extractor"
applications_dir="$data_home/applications"
pixmaps_dir="$data_home/pixmaps"
scalable_icons_dir="$data_home/icons/hicolor/scalable/apps"
desktop_file="$applications_dir/wonderdraft-map-extractor.desktop"

install -d "$app_dir/bin" "$applications_dir" "$pixmaps_dir" "$scalable_icons_dir"
install -m 755 "$binary" "$app_dir/bin/wonderdraft-editor"
if [ -f "$project_dir/wonderdraft_font_names.txt" ]; then
    install -m 644 "$project_dir/wonderdraft_font_names.txt" \
        "$app_dir/wonderdraft_font_names.txt"
fi
install -m 644 "$project_dir/wonderdraft_map_extractor.png" \
    "$pixmaps_dir/wonderdraft-map-extractor.png"
install -m 644 "$project_dir/wonderdraft_map_extractor.svg" \
    "$scalable_icons_dir/wonderdraft-map-extractor.svg"

escaped_exec=$(printf '%s' "$app_dir/bin/wonderdraft-editor" | sed 's/[&|]/\\&/g')
escaped_path=$(printf '%s' "$app_dir" | sed 's/[&|]/\\&/g')
sed \
    -e "s|^Exec=.*|Exec=env GDK_BACKEND=x11 \"$escaped_exec\"|" \
    -e "s|^Path=.*|Path=$escaped_path|" \
    -e 's|^Icon=.*|Icon=wonderdraft-map-extractor|' \
    "$project_dir/wonderdraft-map-extractor.desktop" >"$desktop_file"
chmod 644 "$desktop_file"

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$applications_dir" >/dev/null 2>&1 || true
fi

echo "Installed Wonderdraft Map Extractor launcher:"
echo "  $desktop_file"

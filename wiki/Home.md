# Miracledraft Map Helper Wiki

Miracledraft Map Helper is an unofficial desktop tool for inspecting and
converting `.wonderdraft_map` files. Its core workflow is to export editable map
layers as SVG, edit them in a vector editor, import the changes, and save a new
Wonderdraft map.

> Always keep an untouched backup. This is an experimental tool, and edited
> maps should be tested in Wonderdraft before they are relied upon.

## Documentation

- [Getting Started](Getting-Started)
- [Functions and Keyboard Shortcuts](Functions-and-Keyboard-Shortcuts)
- [SVG Round Trip](SVG-Round-Trip)
- [Editing SVG for Import](Editing-SVG-for-Import)
- [Translation Settings](translation%20settings)
- [Inkarnate and CSV Import](Inkarnate-and-CSV-Import)
- [Wonderdraft Map File Format](wonderdraft_map-fileformat)
- [Map-data Text Syntax](Map-Data-Text-Syntax)
- [Settings and Troubleshooting](Settings-and-Troubleshooting)

## What the editor can change

- Complete decoded map data in Godot text syntax
- Roads and paths
- Symbols, including placement, rotation, mirroring, outline, and custom color
- Labels, including font mapping, outline, and glow
- Territories and their editable point lists
- The embedded `ground`, `mask`, `water_tint`, and other detected images

The project does not distribute Wonderdraft or its assets. Buy Wonderdraft from
[wonderdraft.net](https://www.wonderdraft.net/) to create and use compatible
maps.

## Screenshots

### Main editor

![Miracledraft Map Helper main window](https://raw.githubusercontent.com/Glumbosch/miracledraft_map_helper/main/screenshots/mainwindow.jpg)

### Before editing

![Original Wonderdraft map beside the helper](https://raw.githubusercontent.com/Glumbosch/miracledraft_map_helper/main/screenshots/load_pre_edit.jpg)

### Editing the SVG in Inkscape

![Editing an exported road in Inkscape](https://raw.githubusercontent.com/Glumbosch/miracledraft_map_helper/main/screenshots/in_inkscape.jpg)

### After importing the edited SVG

![Edited SVG content imported back into Wonderdraft](https://raw.githubusercontent.com/Glumbosch/miracledraft_map_helper/main/screenshots/after_edit.jpg)

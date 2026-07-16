# SVG Round Trip

The SVG workflow is designed for viewing Wonderdraft boxes and editing roads,
paths, symbols, labels, and territories in applications such as Inkscape.

For the exact group and attribute rules used by the importer, see
[Editing SVG for Import](Editing-SVG-for-Import).

## Workflow

1. Open a `.wonderdraft_map` and keep an untouched backup.
2. Enable only the layers you need under **SVG export layers**.
3. Enable **Embed mask in SVG**, **Embed boxes in SVG**, and/or **Embed symbols
   in SVG** if the SVG must be portable to another folder or computer.
4. Click **Export SVG…**.
5. Edit the SVG while preserving `wd:*` metadata attributes.
6. Click **Import SVG…** and choose the edited SVG.
7. Review the import count in the status/dialog, then click **Save map as…**.
8. Open the new map in Wonderdraft and verify it visually.

## Example

Original map and decoded data:

![Original Wonderdraft map beside the editor](https://raw.githubusercontent.com/Glumbosch/wonderdraft_map_extractor/main/screenshots/load_pre_edit.jpg)

Editing a road/path in Inkscape:

![Editing an exported road in Inkscape](https://raw.githubusercontent.com/Glumbosch/wonderdraft_map_extractor/main/screenshots/in_inkscape.jpg)

Saved map after importing the SVG:

![Edited SVG content imported back into Wonderdraft](https://raw.githubusercontent.com/Glumbosch/wonderdraft_map_extractor/main/screenshots/after_edit.jpg)

## Round-trip behavior

- Export groups are Inkscape layers whose `id` and `inkscape:label` use the
  `wonderdraft-*` names shown in the Layers panel.
- `wd:*` attributes retain the original Wonderdraft records. Supported
  untagged elements are also imported when placed in the matching Wonderdraft
  layer.
- Boxes export to the `wonderdraft-boxes` layer as embedded or linked PNG
  images stretched to their `margin_left`, `margin_top`, `margin_right`, and
  `margin_bottom` rectangle. Nine-patch cropping and border reconstruction are
  intentionally not applied. Box SVG geometry is export-only; edit box records
  through the map-data **Boxes** jump section.
- Roads and territory areas export as SVG `<path>` elements for convenient node
  editing. Edited path endpoints become Wonderdraft point lists on import.
- Wonderdraft road styles are represented with SVG strokes, dash arrays,
  outlines, or fill-only pattern geometry. The directional style uses
  repeating chevrons scaled at 50 pixels per Wonderdraft width unit.
- Repeated embedded symbols share one image definition and use `<use>` clones.
- Symbol custom-color modes, transparency, rotation, mirroring, and outlines are
  represented with SVG transforms and reusable filters.
- Labels retain mapped font family, style, weight, outlines, and glow where the
  source data provides them.
- New untagged text in the labels layer uses the theme's Town preset as its
  fallback, while its SVG text, position, size, and color are imported.
- Territory opacity and supported solid, dashed, gradient, and dark-dot borders
  are represented in SVG.

## Missing symbols

Built-in textures resolve below **Default sprites folder**. Custom pack textures
resolve below **Custom asset folder**. Run the setup wizard to extract core
sprites and locate Wonderdraft assets, then export again. The export summary
reports missing sprites.

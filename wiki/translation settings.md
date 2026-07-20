# Translation Settings

**Render SVG…** and **Render from CSV…** create a renderer window with two
columns. Select a source SVG class or Inkscape layer on the left; configure how
that selection becomes Wonderdraft data on the right. Each row has its own
category and settings, so one source file can create several kinds of map data.

![Landmass translation settings with label options](https://raw.githubusercontent.com/Glumbosch/miracledraft_map_helper/main/screenshots/translation%20settings%20landmass%20and%20label.jpg)

## Common controls

- **Category** chooses the type of Wonderdraft output.
- **Use this layer/class as tracing image** rasterizes the selected source into
  Wonderdraft's trace overlay. Use this for a reference layer such as the
  Inkarnate `Preview` layer; only the first selected tracing row is used.
- **Name attribute** is used for names where the selected category needs one.
  `map:svgname` is the usual MapSVG-compatible default.
- **Create label** adds a label alongside each created symbol or other named
  element. For the **label** category, SVG `<text>` content is the label text.
- **Prepend class** prefixes generated label text with the source class name.

## Categories

| Category | Result |
| --- | --- |
| **invisible** | Ignores the selected source when creating the map. It can still be used as the tracing image. |
| **label** | Creates Wonderdraft labels from SVG `<text>` elements. **Match SVG text style** keeps the source font, size, color, opacity, and anchor when possible. |
| **symbol** | Places the chosen Wonderdraft symbol at each source point. Use **Symbol gallery…** to select a resolved built-in or custom asset. |
| **path** | Creates Wonderdraft roads or paths. Choose a path style, color, width, and roughness. |
| **territory** | Creates filled territory shapes with a selectable border style, color, and width. |
| **ground** | Rasterizes the selection into the map's `ground` paint layer. Fill and border overrides are optional. |
| **water_tint** | Rasterizes the selection into the water-tint image layer. |
| **landmass** | Paints the selected geometry black onto `mask.png`, making it land. The width control sets the black mask border width. |
| **fill with land** | Fills the entire map mask with land; the selected source geometry is intentionally ignored. |
| **freshwater** | Paints freshwater after landmass rendering. It is visible only where land exists; a fill becomes red mask paint and a positive border width produces a red border. |

## Labels and landmass

The label controls choose a Wonderdraft font preset or installed font, size,
alignment, fill, outline, and offsets. The landmass category has no vector
style controls because it is rendered into the map mask instead.

## Paths and territories

Choose the closest Wonderdraft path or border style, then set its color and
width. Paths also expose **Roughness**. Territory rows use the same controls
but select a border style instead of a path style.

![Path translation settings](https://raw.githubusercontent.com/Glumbosch/miracledraft_map_helper/main/screenshots/translation%20settings%20path.jpg)

## Save reusable settings

Use **Save settings JSON…** in the renderer after configuring the rows. The
file stores the class/layer mappings and the render setup, so it can be loaded
for another SVG with the same source naming.

//! Create a new Wonderdraft map from ordinary SVG classes or Inkscape layers.
//!
//! Vector records are translated directly. Raster layers are rendered by a
//! small external Linux SVG renderer so the application does not have to ship
//! a second browser/SVG engine.

use crate::{ByteSource, Error, Result, Value, assets::Resolver, godot_text, variant};
use image::{RgbaImage, imageops};
use quick_xml::{Reader, events::Event};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

type Matrix = [f64; 6];
const IDENTITY: Matrix = [1., 0., 0., 1., 0., 0.];

pub const LABEL_FONT_PRESETS: &[&str] = &[
    "Aladin",
    "Barlow Condensed",
    "Bilbo",
    "Cinzel Decorative",
    "East Sea Dokdo",
    "Fredericka the Great",
    "Gentium Book Basic Bold",
    "IM FELL DW Pica",
    "IM FELL English Italic",
    "Katibeh",
    "Lancelot",
    "Marko One",
    "Merriweather",
    "Metamorphous",
    "Uncial Antiqua",
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Category {
    Symbol,
    Path,
    Ground,
    WaterTint,
    Territory,
    Landmass,
    FillWithLand,
    Freshwater,
    #[default]
    Invisible,
}

impl Category {
    pub const ALL: [Self; 9] = [
        Self::Symbol,
        Self::Path,
        Self::Ground,
        Self::WaterTint,
        Self::Territory,
        Self::Landmass,
        Self::FillWithLand,
        Self::Freshwater,
        Self::Invisible,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Symbol => "symbol",
            Self::Path => "path",
            Self::Ground => "ground",
            Self::WaterTint => "water_tint",
            Self::Territory => "territory",
            Self::Landmass => "landmass",
            Self::FillWithLand => "fill with land",
            Self::Freshwater => "freshwater",
            Self::Invisible => "invisible",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LabelSettings {
    pub enabled: bool,
    #[serde(default)]
    pub prepend_class: bool,
    pub size: f32,
    pub font: String,
    pub color: [u8; 4],
    pub outline_color: [u8; 4],
    pub outline: f32,
    #[serde(default)]
    pub align: i32,
    pub offset_x: f32,
    pub offset_y: f32,
}
impl Default for LabelSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            prepend_class: false,
            size: 24.,
            font: "Gentium Book Basic Bold".into(),
            color: [0, 0, 0, 255],
            outline_color: [255, 255, 255, 255],
            outline: 0.,
            align: 0,
            offset_x: 0.,
            offset_y: 0.,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClassSettings {
    pub class_name: String,
    pub category: Category,
    pub name_attribute: String,
    pub label: LabelSettings,
    pub symbol: String,
    pub symbol_scale: f32,
    pub tint: [u8; 4],
    #[serde(default = "default_custom_colors")]
    pub custom_colors: [[u8; 4]; 3],
    pub path_style: String,
    pub path_color: [u8; 4],
    pub width: f32,
    #[serde(default)]
    pub roughness: f32,
    pub fill_override: Option<[u8; 4]>,
    #[serde(default)]
    pub no_fill_override: bool,
    pub border_override: Option<[u8; 4]>,
    pub border_width_override: Option<f32>,
}
impl ClassSettings {
    fn new(class_name: String, name_attribute: &str) -> Self {
        Self {
            class_name,
            category: Category::Invisible,
            name_attribute: name_attribute.into(),
            label: LabelSettings::default(),
            symbol: String::new(),
            symbol_scale: 1.,
            tint: [255; 4],
            custom_colors: default_custom_colors(),
            path_style: "res://textures/paths/path_blended".into(),
            path_color: [0, 0, 0, 255],
            width: 4.,
            roughness: 0.,
            fill_override: None,
            no_fill_override: false,
            border_override: None,
            border_width_override: None,
        }
    }
}

fn default_custom_colors() -> [[u8; 4]; 3] {
    [[3, 37, 119, 222], [38, 191, 66, 227], [138, 30, 30, 208]]
}

#[derive(Clone, Debug)]
pub struct Document {
    pub source: PathBuf,
    pub width: u32,
    pub height: u32,
    pub classes: Vec<String>,
    pub layer_fallback: bool,
    source_svg: String,
    layer_ids: HashMap<String, Vec<String>>,
    instances: Vec<Instance>,
}

impl Document {
    /// Bounds of all coordinates across every class/layer in document pixels.
    pub fn data_bounds(&self) -> Option<(f64, f64, f64, f64)> {
        let mut points = self.instances.iter().flat_map(|instance| {
            if instance.points.is_empty() {
                vec![instance.center]
            } else {
                instance.points.clone()
            }
        });
        let first = points.next()?;
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (first.0, first.1, first.0, first.1);
        for (x, y) in points {
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
        Some((
            min_x,
            min_y,
            (max_x - min_x).max(1.0),
            (max_y - min_y).max(1.0),
        ))
    }

    /// Returns the point geometry used by the vector importer for a class.
    /// This is deliberately a copy: callers use it for previews only.
    pub fn class_geometry(&self, class_name: &str) -> Vec<Vec<(f64, f64)>> {
        self.instances
            .iter()
            .filter(|instance| instance.class_name == class_name)
            .map(|instance| {
                if instance.points.is_empty() {
                    vec![instance.center]
                } else {
                    instance.points.clone()
                }
            })
            .collect()
    }

    /// Makes an output document from selected classes and a source viewport.
    pub fn cropped_for_render(
        &self,
        classes: &HashSet<String>,
        viewport: (f64, f64, f64, f64),
        width: u32,
        height: u32,
    ) -> Self {
        let (x, y, viewport_width, viewport_height) = viewport;
        let inside = |point: (f64, f64)| {
            point.0 >= x
                && point.0 <= x + viewport_width
                && point.1 >= y
                && point.1 <= y + viewport_height
        };
        let scale_x = width as f64 / viewport_width.max(f64::EPSILON);
        let scale_y = height as f64 / viewport_height.max(f64::EPSILON);
        let transform = |point: (f64, f64)| ((point.0 - x) * scale_x, (point.1 - y) * scale_y);
        let mut document = self.clone();
        document.width = width;
        document.height = height;
        // Keep rasterized categories (ground, water, landmass, freshwater) in
        // the same source viewport as vector records. The original SVG is a
        // nested document so its authored coordinate system remains intact.
        document.source_svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="{x} {y} {viewport_width} {viewport_height}" overflow="hidden">{}</svg>"#,
            self.source_svg
        );
        document.classes.retain(|class| classes.contains(class));
        document.instances = self
            .instances
            .iter()
            .filter_map(|instance| {
                if !classes.contains(&instance.class_name) {
                    return None;
                }
                let source_points = if instance.points.is_empty() {
                    vec![instance.center]
                } else {
                    instance.points.clone()
                };
                let points = source_points
                    .into_iter()
                    .filter(|point| inside(*point))
                    .map(transform)
                    .collect::<Vec<_>>();
                if points.is_empty() || (instance.has_nodes && points.len() < 2) {
                    return None;
                }
                let mut instance = instance.clone();
                instance.center = transform(instance.center);
                instance.label_anchor = instance
                    .label_anchor
                    .filter(|point| inside(*point))
                    .map(transform);
                instance.points = points;
                Some(instance)
            })
            .collect();
        document
    }
}

#[derive(Clone, Debug)]
struct Instance {
    class_name: String,
    attrs: Vec<(String, String)>,
    matrix: Matrix,
    points: Vec<(f64, f64)>,
    center: (f64, f64),
    label_anchor: Option<(f64, f64)>,
    has_fill: bool,
    has_nodes: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableEncoding {
    Utf8,
    Windows1252,
    Latin1,
    Utf16Le,
    Utf16Be,
}

impl TableEncoding {
    pub const ALL: [Self; 5] = [
        Self::Utf8,
        Self::Windows1252,
        Self::Latin1,
        Self::Utf16Le,
        Self::Utf16Be,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Utf8 => "UTF-8",
            Self::Windows1252 => "Windows-1252",
            Self::Latin1 => "ISO-8859-1",
            Self::Utf16Le => "UTF-16 LE",
            Self::Utf16Be => "UTF-16 BE",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableDelimiter {
    Auto,
    Tab,
    Comma,
    Semicolon,
    Pipe,
}

impl TableDelimiter {
    pub const ALL: [Self; 5] = [
        Self::Auto,
        Self::Tab,
        Self::Comma,
        Self::Semicolon,
        Self::Pipe,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto detect",
            Self::Tab => "Tab",
            Self::Comma => "Comma",
            Self::Semicolon => "Semicolon",
            Self::Pipe => "Pipe",
        }
    }
    fn character(self) -> Option<char> {
        match self {
            Self::Auto => None,
            Self::Tab => Some('\t'),
            Self::Comma => Some(','),
            Self::Semicolon => Some(';'),
            Self::Pipe => Some('|'),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct TableData {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub delimiter: char,
}

#[derive(Clone, Debug, Default)]
pub struct TableColumns {
    pub tag: Option<usize>,
    pub id: Option<usize>,
    pub name: Option<usize>,
    pub class_name: Option<usize>,
    pub fill: Option<usize>,
    pub stroke: Option<usize>,
    pub stroke_width: Option<usize>,
    pub coordinates: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct TableOptions {
    pub columns: TableColumns,
    pub relative_after_first: bool,
    pub source_x: f64,
    pub source_y: f64,
    pub source_width: f64,
    pub source_height: f64,
    pub map_width: u32,
    pub map_height: u32,
}

pub fn read_table(
    path: &Path,
    encoding: TableEncoding,
    delimiter: TableDelimiter,
) -> Result<TableData> {
    let bytes = fs::read(path).map_err(|error| Error::format(error.to_string()))?;
    let raw = decode_table(&bytes, encoding)?;
    let first_line = raw
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    let delimiter = delimiter
        .character()
        .unwrap_or_else(|| detect_delimiter(first_line));
    let mut records = raw
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| delimited_fields(line, delimiter));
    let headers = records
        .next()
        .ok_or_else(|| Error::format("the table is empty"))?;
    let rows = records.collect();
    Ok(TableData {
        headers,
        rows,
        delimiter,
    })
}

pub fn auto_table_columns(headers: &[String]) -> TableColumns {
    let find = |aliases: &[&str]| {
        headers.iter().position(|header| {
            let normalized = normalize_header(header);
            aliases.iter().any(|alias| normalized == *alias)
        })
    };
    TableColumns {
        tag: find(&["tag", "element", "element_type"]),
        id: find(&["id", "element_id"]),
        name: find(&["mapsvg_name", "mapsvgname", "name", "label"]),
        class_name: find(&["class", "class_name", "classname", "layer"]),
        fill: find(&["fill", "fill_color", "fillcolor"]),
        stroke: find(&["stroke", "stroke_color", "strokecolor"]),
        stroke_width: find(&["stroke_width", "strokewidth", "width"]),
        coordinates: find(&["coordinates", "coordinate", "coords", "points"]),
    }
}

pub fn table_bounds(
    table: &TableData,
    columns: &TableColumns,
    relative_after_first: bool,
) -> Result<(f64, f64, f64, f64)> {
    let coordinate_column = columns
        .coordinates
        .ok_or_else(|| Error::format("assign a Coordinates column"))?;
    let tag_column = columns.tag;
    let mut all = Vec::new();
    for row in &table.rows {
        let tag = table_value(row, tag_column).unwrap_or("path");
        let Some(raw) = row.get(coordinate_column) else {
            continue;
        };
        let parsed = parse_coordinate_pairs(raw)?;
        all.extend(expand_table_points(tag, &parsed, relative_after_first));
    }
    if all.is_empty() {
        return Err(Error::format("no coordinate pairs were found"));
    }
    let min_x = all
        .iter()
        .map(|point| point.0)
        .fold(f64::INFINITY, f64::min);
    let max_x = all
        .iter()
        .map(|point| point.0)
        .fold(f64::NEG_INFINITY, f64::max);
    let min_y = all
        .iter()
        .map(|point| point.1)
        .fold(f64::INFINITY, f64::min);
    let max_y = all
        .iter()
        .map(|point| point.1)
        .fold(f64::NEG_INFINITY, f64::max);
    Ok((
        min_x,
        min_y,
        (max_x - min_x).max(1.),
        (max_y - min_y).max(1.),
    ))
}

pub fn analyze_table(path: &Path, table: &TableData, options: &TableOptions) -> Result<Document> {
    let class_column = options
        .columns
        .class_name
        .ok_or_else(|| Error::format("assign a Class column"))?;
    let coordinate_column = options
        .columns
        .coordinates
        .ok_or_else(|| Error::format("assign a Coordinates column"))?;
    if options.map_width == 0 || options.map_height == 0 {
        return Err(Error::format(
            "map width and height must be greater than zero",
        ));
    }
    if options.source_width <= 0. || options.source_height <= 0. {
        return Err(Error::format(
            "source width and height must be greater than zero",
        ));
    }

    let transform = |(x, y): (f64, f64)| {
        (
            (x - options.source_x) * options.map_width as f64 / options.source_width,
            (y - options.source_y) * options.map_height as f64 / options.source_height,
        )
    };
    let mut classes = HashSet::new();
    let mut instances = Vec::new();
    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:mapsvg="http://www.garetien.de" width="{}" height="{}" viewBox="0 0 {} {}">"#,
        options.map_width, options.map_height, options.map_width, options.map_height
    );
    for row in &table.rows {
        let Some(class_value) = row.get(class_column).map(|value| value.trim()) else {
            continue;
        };
        if class_value.is_empty() {
            continue;
        }
        let raw_coordinates = row.get(coordinate_column).map_or("", String::as_str);
        let parsed = parse_coordinate_pairs(raw_coordinates)?;
        let tag = table_value(row, options.columns.tag).unwrap_or("path");
        let points = expand_table_points(tag, &parsed, options.relative_after_first)
            .into_iter()
            .map(transform)
            .collect::<Vec<_>>();
        if points.is_empty() {
            continue;
        }
        let fill = table_value(row, options.columns.fill).unwrap_or("").trim();
        let stroke = table_value(row, options.columns.stroke)
            .unwrap_or("")
            .trim();
        let stroke_width = table_value(row, options.columns.stroke_width)
            .unwrap_or("")
            .trim();
        let name = table_value(row, options.columns.name).unwrap_or("").trim();
        let id = table_value(row, options.columns.id).unwrap_or("").trim();
        let has_fill = !fill.is_empty() && !fill.eq_ignore_ascii_case("none");
        let has_nodes = matches!(
            tag.to_ascii_lowercase().as_str(),
            "path" | "polyline" | "polygon" | "line"
        );
        let center = if points.len() == 1 {
            points[0]
        } else {
            bounds_center(&points)
        };
        let mut attrs = vec![("class".into(), class_value.into())];
        if !id.is_empty() {
            attrs.push(("id".into(), id.into()));
        }
        if !name.is_empty() {
            attrs.push(("mapsvg:name".into(), name.into()));
        }
        if !fill.is_empty() {
            attrs.push(("fill".into(), fill.into()));
        }
        if !stroke.is_empty() {
            attrs.push(("stroke".into(), stroke.into()));
        }
        if !stroke_width.is_empty() {
            attrs.push(("stroke-width".into(), stroke_width.into()));
        }
        for class_name in class_value.split_whitespace() {
            classes.insert(class_name.to_owned());
            instances.push(Instance {
                class_name: class_name.to_owned(),
                attrs: attrs.clone(),
                matrix: IDENTITY,
                points: points.clone(),
                center,
                label_anchor: None,
                has_fill,
                has_nodes,
            });
        }
        append_table_svg_element(
            &mut svg,
            tag,
            class_value,
            id,
            name,
            fill,
            stroke,
            stroke_width,
            &points,
            has_fill,
        );
    }
    svg.push_str("</svg>");
    let mut classes = classes.into_iter().collect::<Vec<_>>();
    classes.sort_by_key(|value| value.to_lowercase());
    if classes.is_empty() {
        return Err(Error::format(
            "no rows with a class and coordinates were found",
        ));
    }
    Ok(Document {
        source: path.to_owned(),
        width: options.map_width,
        height: options.map_height,
        classes,
        layer_fallback: false,
        source_svg: svg,
        layer_ids: HashMap::new(),
        instances,
    })
}

#[derive(Clone)]
struct ParseFrame {
    matrix: Matrix,
    layer: Option<String>,
    classes: Vec<String>,
    symbol: Option<String>,
    hidden: bool,
}

pub fn analyze(path: &Path) -> Result<Document> {
    let raw = fs::read_to_string(path).map_err(|e| Error::format(e.to_string()))?;
    let mut reader = Reader::from_str(&raw);
    let mut stack = vec![ParseFrame {
        matrix: IDENTITY,
        layer: None,
        classes: Vec::new(),
        symbol: None,
        hidden: false,
    }];
    let mut class_names = HashSet::new();
    let mut layers = HashSet::new();
    let mut layer_ids: HashMap<String, Vec<String>> = HashMap::new();
    let mut pending = Vec::new();
    let mut width = None;
    let mut height = None;
    let mut view_box = None;
    let mut root_svg_seen = false;
    let mut symbol_bounds: HashMap<String, Vec<(f64, f64)>> = HashMap::new();
    loop {
        let xml_event = reader
            .read_event()
            .map_err(|e| Error::format(e.to_string()))?;
        match &xml_event {
            Event::Start(event) | Event::Empty(event) => {
                let is_container = matches!(&xml_event, Event::Start(_));
                let tag = String::from_utf8_lossy(event.local_name().as_ref()).into_owned();
                let attrs = attributes(event);
                let is_root_svg = tag == "svg" && !root_svg_seen;
                if is_root_svg {
                    root_svg_seen = true;
                    width = attribute(&attrs, "width").and_then(parse_length);
                    height = attribute(&attrs, "height").and_then(parse_length);
                    view_box = attribute(&attrs, "viewBox").and_then(parse_view_box);
                }
                let parent = stack.last().cloned().unwrap_or(ParseFrame {
                    matrix: IDENTITY,
                    layer: None,
                    classes: Vec::new(),
                    symbol: None,
                    hidden: false,
                });
                let element_transform = parse_transform(attribute(&attrs, "transform"));
                let nested_viewport = if tag == "svg" && !is_root_svg {
                    svg_viewport_matrix(&attrs)
                } else {
                    IDENTITY
                };
                let matrix = multiply(parent.matrix, multiply(element_transform, nested_viewport));
                let hidden = parent.hidden || is_invisible(&attrs);
                let is_layer = attribute(&attrs, "groupmode").is_some_and(|value| value == "layer");
                let own_layer = is_layer.then(|| {
                    attribute(&attrs, "label")
                        .or_else(|| attribute(&attrs, "id"))
                        .unwrap_or("unnamed layer")
                        .to_owned()
                });
                if let (Some(layer_name), Some(id)) = (&own_layer, attribute(&attrs, "id")) {
                    layer_ids
                        .entry(layer_name.clone())
                        .or_default()
                        .push(id.to_owned());
                }
                let layer = own_layer.or(parent.layer);
                if let Some(layer) = &layer {
                    layers.insert(layer.clone());
                }
                let own_classes = attribute(&attrs, "class")
                    .map(|value| {
                        value
                            .split_whitespace()
                            .map(str::to_owned)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                class_names.extend(own_classes.iter().cloned());
                let mut active_classes = parent.classes;
                for class_name in own_classes {
                    if !active_classes.contains(&class_name) {
                        active_classes.push(class_name);
                    }
                }
                let current_symbol = if tag == "symbol" {
                    attribute(&attrs, "id").map(str::to_owned)
                } else {
                    parent.symbol
                };
                if tag == "symbol"
                    && let Some(id) = &current_symbol
                    && let Some((x, y, w, h)) =
                        attribute(&attrs, "viewBox").and_then(parse_view_box)
                {
                    symbol_bounds
                        .entry(id.clone())
                        .or_default()
                        .extend([(x, y), (x + w, y + h)]);
                }
                if is_drawable(&tag) && !hidden {
                    if let Some(id) = &current_symbol {
                        let definition_points = local_points(&tag, &attrs, &HashMap::new())
                            .into_iter()
                            .map(|(x, y)| apply(matrix, x, y));
                        symbol_bounds
                            .entry(id.clone())
                            .or_default()
                            .extend(definition_points);
                    }
                    pending.push((tag, attrs, matrix, layer.clone(), active_classes.clone()));
                }
                if is_container {
                    stack.push(ParseFrame {
                        matrix,
                        layer,
                        classes: active_classes,
                        symbol: current_symbol,
                        hidden,
                    });
                }
            }
            Event::End(_) => {
                if stack.len() > 1 {
                    stack.pop();
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    let (vx, vy, vw, vh) =
        view_box.unwrap_or((0., 0., width.unwrap_or(512.), height.unwrap_or(512.)));
    let width = width.unwrap_or(vw).round().max(1.) as u32;
    let height = height.unwrap_or(vh).round().max(1.) as u32;
    let viewport_point = |(x, y): (f64, f64)| {
        (
            (x - vx) * width as f64 / vw.max(f64::EPSILON),
            (y - vy) * height as f64 / vh.max(f64::EPSILON),
        )
    };
    let layer_fallback = class_names.is_empty();
    let mut instances = Vec::new();
    let symbol_centers = symbol_bounds
        .into_iter()
        .filter_map(|(id, points)| (!points.is_empty()).then(|| (id, bounds_center(&points))))
        .collect();
    for (tag, attrs, matrix, layer, active_classes) in pending {
        let assigned = if layer_fallback {
            layer.into_iter().collect::<Vec<_>>()
        } else {
            active_classes
        };
        for class_name in assigned {
            let local = local_points(&tag, &attrs, &symbol_centers);
            let points = local
                .into_iter()
                .map(|(x, y)| viewport_point(apply(matrix, x, y)))
                .collect::<Vec<_>>();
            let center = if points.is_empty() {
                viewport_point(apply(matrix, 0., 0.))
            } else {
                bounds_center(&points)
            };
            // MapSVG town labels are positioned at this local point by the
            // reference Python script. Applying the complete accumulated
            // matrix also accounts for transformed parent groups and nested
            // SVG viewports.
            let label_anchor = (tag == "use").then(|| viewport_point(apply(matrix, 18., 13.)));
            let has_fill =
                presentation(&attrs, "fill").is_some_and(|fill| !fill.eq_ignore_ascii_case("none"));
            instances.push(Instance {
                class_name,
                attrs: attrs.clone(),
                matrix,
                points,
                center,
                label_anchor,
                has_fill,
                has_nodes: matches!(tag.as_str(), "path" | "polyline" | "polygon" | "line"),
            });
        }
    }
    let mut classes: Vec<String> = if layer_fallback {
        layers.into_iter().collect()
    } else {
        class_names.into_iter().collect()
    };
    classes.sort_by_key(|v| v.to_lowercase());
    Ok(Document {
        source: path.to_owned(),
        width,
        height,
        classes,
        layer_fallback,
        source_svg: raw,
        layer_ids,
        instances,
    })
}

pub fn default_settings(document: &Document) -> Vec<ClassSettings> {
    document
        .classes
        .iter()
        .cloned()
        .map(|name| ClassSettings::new(name, "map:svgname"))
        .collect()
}

pub fn save_csv(path: &Path, settings: &[ClassSettings]) -> Result<()> {
    let mut out = String::from("class,settings_json\n");
    for row in settings {
        let json = serde_json::to_string(row).map_err(|e| Error::format(e.to_string()))?;
        out.push_str(&csv_escape(&row.class_name));
        out.push(',');
        out.push_str(&csv_escape(&json));
        out.push('\n');
    }
    fs::write(path, out).map_err(|e| Error::format(e.to_string()))
}

pub fn load_csv(path: &Path, settings: &mut [ClassSettings]) -> Result<usize> {
    let raw = fs::read_to_string(path).map_err(|e| Error::format(e.to_string()))?;
    let mut loaded = 0;
    for line in raw.lines().skip(1) {
        let fields = csv_fields(line);
        if fields.len() != 2 {
            continue;
        }
        let Ok(row) = serde_json::from_str::<ClassSettings>(&fields[1]) else {
            continue;
        };
        if let Some(existing) = settings
            .iter_mut()
            .find(|item| item.class_name == fields[0])
        {
            *existing = row;
            loaded += 1;
        }
    }
    Ok(loaded)
}

#[derive(Clone, Debug, Default)]
pub struct RenderSummary {
    pub symbols: usize,
    pub paths: usize,
    pub territories: usize,
    pub labels: usize,
}

pub fn render(
    document: &Document,
    settings: &[ClassSettings],
    resolver: &Resolver,
    destination: &Path,
    compressed: bool,
) -> Result<RenderSummary> {
    let mut mask = initial_mask(document.width, document.height, settings);
    let mut ground = RgbaImage::new(document.width, document.height);
    let mut water = RgbaImage::new(document.width, document.height);
    let mut symbols = Vec::new();
    let mut paths = Vec::new();
    let mut territories = Vec::new();
    let mut labels = Vec::new();
    let source = document.source_svg.clone();
    let temp = crate::images::temp_cache_dir(&std::env::temp_dir())?;
    // Land first, freshwater second, regardless of class ordering.
    for category in [
        Category::Ground,
        Category::WaterTint,
        Category::Landmass,
        Category::Freshwater,
    ] {
        for row in settings.iter().filter(|row| row.category == category) {
            let css = raster_css(document, row);
            let filtered = inject_style(&source, &css)?;
            let svg_path = temp.join(format!("{}.svg", safe_name(&row.class_name)));
            let png_path = temp.join(format!("{}.png", safe_name(&row.class_name)));
            fs::write(&svg_path, filtered).map_err(|e| Error::format(e.to_string()))?;
            render_external(&svg_path, &png_path, document.width, document.height)?;
            let overlay = image::open(&png_path)?.to_rgba8();
            match category {
                Category::Ground => imageops::overlay(&mut ground, &overlay, 0, 0),
                Category::WaterTint => imageops::overlay(&mut water, &overlay, 0, 0),
                _ => imageops::overlay(&mut mask, &overlay, 0, 0),
            }
        }
    }
    let by_class =
        document
            .instances
            .iter()
            .fold(HashMap::<&str, Vec<&Instance>>::new(), |mut map, item| {
                map.entry(&item.class_name).or_default().push(item);
                map
            });
    for row in settings {
        let instances = by_class
            .get(row.class_name.as_str())
            .cloned()
            .unwrap_or_default();
        for instance in instances {
            let first = instance.points.first().copied().unwrap_or(instance.center);
            let label_position = if let Some(anchor) = instance.label_anchor {
                anchor
            } else if instance.has_fill {
                geometric_center(&instance.points)
            } else {
                first
            };
            match row.category {
                Category::Symbol if !row.symbol.is_empty() => {
                    for position in symbol_positions(instance, document.width, document.height) {
                        symbols.push(symbol_record_at(row, instance, position, resolver));
                    }
                }
                Category::Path if intersects(&instance.points, document.width, document.height) => {
                    paths.push(path_record(row, instance))
                }
                Category::Territory
                    if intersects(&instance.points, document.width, document.height) =>
                {
                    territories.push(territory_record(row, instance))
                }
                _ => {}
            }
            if row.category != Category::Invisible && row.label.enabled {
                if let Some(text) = instance_name(instance, &row.name_attribute) {
                    labels.push(label_record(row, label_text(row, &text), label_position));
                }
            }
        }
    }
    let mut root = base_map(document.width, document.height);
    root.set("mask", image_value(mask));
    root.set("ground", image_value(ground));
    root.set("water_tint", image_value(water));
    root.set("symbols", Value::Array(symbols));
    root.set("paths", Value::Array(paths));
    root.set("labels", Value::Array(labels));
    let mut territory_data = Value::dict();
    territory_data.set("territories", Value::Array(territories));
    root.set("territories", territory_data);
    let summary = RenderSummary {
        symbols: root
            .get("symbols")
            .and_then(Value::as_array)
            .map_or(0, |v| v.len()),
        paths: root
            .get("paths")
            .and_then(Value::as_array)
            .map_or(0, |v| v.len()),
        territories: root
            .get("territories")
            .and_then(|v| v.get("territories"))
            .and_then(Value::as_array)
            .map_or(0, |v| v.len()),
        labels: root
            .get("labels")
            .and_then(Value::as_array)
            .map_or(0, |v| v.len()),
    };
    variant::save_map(&root, destination, 4096, compressed)?;
    let _ = fs::remove_dir_all(temp);
    Ok(summary)
}

fn initial_mask(width: u32, height: u32, settings: &[ClassSettings]) -> RgbaImage {
    if settings
        .iter()
        .any(|row| row.category == Category::FillWithLand)
    {
        RgbaImage::from_pixel(width, height, image::Rgba([0, 0, 0, 255]))
    } else {
        RgbaImage::new(width, height)
    }
}

fn base_map(width: u32, height: u32) -> Value {
    let mut root = Value::dict();
    root.set("version", Value::Int(15));
    root.set("map_width", Value::Real(width as f64));
    root.set("map_height", Value::Real(height as f64));
    root.set("boxes", Value::Array(vec![]));
    root.set("windroses", Value::Array(vec![]));
    root.set("included_packs", Value::Array(vec![]));
    root.set("included_default_packs", Value::Array(vec![]));
    root.set(
        "frame",
        dict(&[
            ("enabled", Value::Bool(false)),
            ("size", Value::Real(1.)),
            ("texture", Value::String("celtic_dotted".into())),
            ("tint", color([255; 4])),
        ]),
    );
    root.set("grid", Value::Nil);
    root.set("trace", Value::Nil);
    root.set("scale", Value::Nil);
    root.set("sharpen_labels", Value::Bool(false));
    root.set("load_default_symbols", Value::Bool(true));
    root.set("load_default_not_symbols", Value::Bool(true));
    root.set(
        "layers",
        dict(&[
            ("enabled", Value::Bool(false)),
            (
                "names",
                Value::Array(
                    [
                        "+5", "+4", "+3", "+2", "+1", "Default", "-1", "-2", "-3", "-4", "-5",
                    ]
                    .into_iter()
                    .map(|v| Value::String(v.into()))
                    .collect(),
                ),
            ),
            ("lock", Value::Array(vec![Value::Bool(false); 11])),
            ("visibility", Value::Array(vec![Value::Bool(true); 11])),
        ]),
    );
    root.set("water_flip_x", Value::Bool(false));
    root.set("water_flip_y", Value::Bool(false));
    root.set("water_level", Value::Real(0.5));
    root.set("water_offset", vec2(0., 0.));
    root.set("water_stain", Value::Real(0.));
    let mut theme = Value::dict();
    theme.set("coastal_fx_distance", Value::Real(24.3));
    theme.set("coastline_color", color([48, 67, 1, 123]));
    theme.set("coastline_style", Value::Int(0));
    theme.set(
        "color_grading",
        dict(&[
            (
                "colors",
                Value::PoolVectors {
                    kind: "PoolColorArray".into(),
                    components: 4,
                    values: vec![vec![0.262, 0.165, 0.041, 1.], vec![1., 1., 1., 1.]],
                },
            ),
            ("offsets", Value::PoolRealArray(vec![0., 1.])),
            ("strength", Value::Real(0.)),
        ]),
    );
    theme.set("ground_texture", Value::String("Worn".into()));
    theme.set("water_texture", Value::String("Worn".into()));
    theme.set(
        "ground_color_names",
        Value::Array((0..6).map(|_| Value::String(String::new())).collect()),
    );
    theme.set(
        "ground_colors",
        Value::Array(
            [
                [190, 189, 142, 255],
                [181, 190, 142, 255],
                [228, 228, 211, 255],
                [220, 209, 150, 255],
                [255, 173, 0, 255],
                [255, 0, 0, 255],
            ]
            .into_iter()
            .map(color)
            .collect(),
        ),
    );
    theme.set(
        "water_color_names",
        Value::Array((0..5).map(|_| Value::String(String::new())).collect()),
    );
    theme.set(
        "water_colors",
        Value::Array(
            [
                [182, 184, 146, 255],
                [130, 155, 132, 255],
                [156, 167, 135, 255],
                [138, 159, 121, 255],
                [253, 0, 255, 255],
            ]
            .into_iter()
            .map(color)
            .collect(),
        ),
    );
    theme.set("water_hue", Value::Real(-0.25));
    theme.set("water_saturation", Value::Real(-0.15));
    theme.set("water_value", Value::Real(0.04));
    theme.set("vignette_strength", Value::Real(0.));
    theme.set("randomize_water_mirror", Value::Bool(true));
    theme.set("randomize_water_offsets", Value::Bool(true));
    theme.set("landmass_outline_color", color([67, 42, 10, 255]));
    theme.set("landmass_outline_blend", Value::Real(1.));
    theme.set("freshwater_color", color([41, 91, 51, 140]));
    theme.set("freshwater_outline_color", color([45, 29, 0, 255]));
    theme.set("path_color", color([218, 113, 10, 255]));
    theme.set("windrose_color", color([216, 185, 62, 136]));
    theme.set(
        "symbol_custom_colors",
        Value::Array(vec![
            color([118, 40, 3, 222]),
            color([223, 209, 161, 227]),
            color([0, 0, 0, 124]),
        ]),
    );
    let preset = |font_size: f64, font_color: [u8; 4], outline: [u8; 4], width: f64| {
        dict(&[
            ("font_name", Value::String("East Sea Dokdo".into())),
            ("font_size", Value::Real(font_size)),
            ("font_color", color(font_color)),
            ("font_outline_color", color(outline)),
            ("font_outline_width", Value::Real(width)),
        ])
    };
    theme.set(
        "label_presets",
        dict(&[
            (
                "City",
                preset(48., [0, 0, 0, 255], [229, 218, 168, 255], 2.),
            ),
            (
                "Town",
                preset(32., [0, 0, 0, 255], [229, 218, 168, 194], 2.),
            ),
            (
                "Water",
                preset(72., [216, 185, 62, 136], [0, 0, 0, 255], 1.),
            ),
        ]),
    );
    root.set("theme", theme);
    root
}

fn image_value(image: RgbaImage) -> Value {
    let (width, height) = image.dimensions();
    Value::Object {
        class: "Image".into(),
        properties: vec![(
            "data".into(),
            dict(&[
                ("width", Value::Int(width as i64)),
                ("height", Value::Int(height as i64)),
                ("mipmaps", Value::Bool(false)),
                ("format", Value::String("RGBA8".into())),
                (
                    "data",
                    Value::PoolByteArray(ByteSource::Memory(image.into_raw())),
                ),
            ]),
        )],
    }
}
fn symbol_record_at(
    row: &ClassSettings,
    instance: &Instance,
    position: (f64, f64),
    resolver: &Resolver,
) -> Value {
    let mut record = Value::dict();
    let asset = resolver.asset_info(&row.symbol);
    let (_, _, angle, mirror) = matrix_parts(instance.matrix);
    record.set("position", vec2(position.0, position.1));
    record.set(
        "scale",
        vec2(row.symbol_scale as f64, row.symbol_scale as f64),
    );
    record.set("rotation", Value::Real(angle));
    record.set("mirror", Value::Bool(mirror));
    record.set("texture", Value::String(row.symbol.clone()));
    record.set("type", Value::String("symbol".into()));
    record.set("z_index", Value::Int(0));
    record.set(
        "offset",
        vec2(
            asset.as_ref().map_or(0., |a| a.offset_x),
            asset.as_ref().map_or(0., |a| a.offset_y),
        ),
    );
    record.set("outline_width", Value::Real(0.));
    record.set("outline_color", color([255; 4]));
    record.set(
        "radius",
        asset
            .as_ref()
            .map_or(Value::Nil, |asset| Value::Real(asset.base_radius)),
    );
    // Wonderdraft expects these keys on every symbol, including normal stamps.
    // Draw modes only replace the values that they actively use.
    record.set("custom_color_mode", Value::Nil);
    record.set("custom_colors", Value::Nil);
    record.set("sample", Value::Nil);
    match asset.as_ref().map(|asset| asset.draw_mode.as_str()) {
        Some("sample_color") => {
            record.set("sample", color(row.tint));
        }
        Some("custom_colors") => {
            record.set("custom_color_mode", Value::Int(1));
            record.set(
                "custom_colors",
                Value::Array(row.custom_colors.into_iter().map(color).collect()),
            );
        }
        _ => {}
    }
    record
}
fn path_record(row: &ClassSettings, instance: &Instance) -> Value {
    let mut r = Value::dict();
    r.set("points", points_string(&instance.points));
    r.set("position", vec2(0., 0.));
    r.set("width", Value::Real(row.width as f64));
    r.set("color", color(row.path_color));
    r.set("style", Value::String(row.path_style.clone()));
    r.set("roughness", Value::Real(row.roughness as f64));
    r.set("straight", Value::Bool(false));
    r.set("z_index", Value::Int(0));
    r
}

fn label_text(row: &ClassSettings, attribute_value: &str) -> String {
    if row.label.prepend_class {
        format!("{}: {attribute_value}", row.class_name)
    } else {
        attribute_value.to_owned()
    }
}
fn territory_record(row: &ClassSettings, instance: &Instance) -> Value {
    let mut r = Value::dict();
    r.set("points", points_string(&instance.points));
    r.set("position", vec2(0., 0.));
    r.set("width", Value::Real(row.width as f64));
    r.set("color", color(row.path_color));
    r.set("opacity", Value::Real(row.path_color[3] as f64 / 255.));
    r.set(
        "style",
        Value::String(if row.path_style.is_empty() {
            "res://textures/borders/border_solid".into()
        } else {
            row.path_style.clone()
        }),
    );
    r.set("smoothing", Value::Real(0.2));
    r.set("z_index", Value::Int(0));
    r
}
fn label_record(row: &ClassSettings, text: String, position: (f64, f64)) -> Value {
    let s = &row.label;
    let mut r = Value::dict();
    r.set("text", Value::String(text));
    r.set(
        "position",
        vec2(
            position.0 + s.offset_x as f64,
            position.1 + s.offset_y as f64,
        ),
    );
    r.set("font", Value::String(s.font.clone()));
    r.set("size", Value::Int(s.size.round() as i64));
    r.set("color", color(s.color));
    r.set("outline_color", color(s.outline_color));
    r.set("outline_size", Value::Real(s.outline as f64));
    r.set("align", Value::Int(s.align.clamp(0, 2) as i64));
    r.set("curve", Value::Real(0.));
    r.set("rotation", Value::Real(0.));
    r.set("extra_spacing_char", Value::Int(0));
    r.set("glow_color", color([255; 4]));
    r.set("glow_size", Value::Int(0));
    r.set("z_index", Value::Int(0));
    r
}

fn raster_css(document: &Document, row: &ClassSettings) -> String {
    let target = if document.layer_fallback {
        let mut selectors = vec![format!(
            "[inkscape\\:label=\"{}\"]",
            css_string(&row.class_name)
        )];
        selectors.extend(
            document
                .layer_ids
                .get(&row.class_name)
                .into_iter()
                .flatten()
                .map(|id| format!("#{}", css_ident(id))),
        );
        selectors.join(",")
    } else {
        format!("[class~=\"{}\"]", css_string(&row.class_name))
    };
    let descendants = format!("{target}, {target} *");
    let mut style = format!(
        "*{{visibility:hidden!important}} defs,defs *{{visibility:visible!important}} {descendants}{{visibility:visible!important}}"
    );
    match row.category {
        Category::Landmass => style.push_str(&format!(" {descendants}{{fill:#000!important;stroke:#000!important;stroke-width:{}px!important}}", row.width)),
        Category::Freshwater => {
            style.push_str(&format!(" {target}:not([fill=\"none\"]):not([style*=\"fill:none\"]), {target} *:not([fill=\"none\"]):not([style*=\"fill:none\"]){{fill:#ff0000!important}} {target}:not([stroke=\"none\"]):not([style*=\"stroke:none\"]), {target} *:not([stroke=\"none\"]):not([style*=\"stroke:none\"]){{stroke:#ff0000!important}}"));
            if row.no_fill_override { style.push_str(&format!(" {descendants}{{fill:none!important}}")); }
            else if let Some(value) = row.fill_override { style.push_str(&format!(" {descendants}{{fill:{}!important}}", hex(value))); }
            if let Some(value) = row.border_override { style.push_str(&format!(" {descendants}{{stroke:{}!important}}", hex(value))); }
            if let Some(value) = row.border_width_override { style.push_str(&format!(" {descendants}{{stroke-width:{value}px!important}}")); }
        }
        Category::Ground | Category::WaterTint => {
            if row.no_fill_override { style.push_str(&format!(" {descendants}{{fill:none!important}}")); }
            else if let Some(value)=row.fill_override { style.push_str(&format!(" {descendants}{{fill:{}!important}}", hex(value))); }
            if let Some(value)=row.border_override { style.push_str(&format!(" {descendants}{{stroke:{}!important}}", hex(value))); }
            if let Some(value)=row.border_width_override { style.push_str(&format!(" {descendants}{{stroke-width:{value}px!important}}")); }
        }
        _ => {}
    }
    // The isolation rule above makes the selected layer visible. Re-apply
    // authored display:none afterwards. Visibility:hidden is intentionally
    // not treated as a filter: MapSVG uses it for level-of-detail metadata.
    style.push_str(" [display=\"none\"],[display=\"none\"] *,[style*=\"display:none\"],[style*=\"display:none\"] * ,[style*=\"display: none\"],[style*=\"display: none\"] *{display:none!important}");
    style
}
fn render_external(source: &Path, output: &Path, width: u32, height: u32) -> Result<()> {
    let candidates = [
        "resvg",
        "rsvg-convert",
        "inkscape",
        "google-chrome",
        "chromium",
        "chromium-browser",
    ];
    for tool in candidates {
        if command_exists(tool) {
            let status = match tool {
                "resvg" => Command::new(tool)
                    .arg(source)
                    .arg(output)
                    .arg("--width")
                    .arg(width.to_string())
                    .arg("--height")
                    .arg(height.to_string())
                    .status(),
                "rsvg-convert" => Command::new(tool)
                    .arg("-w")
                    .arg(width.to_string())
                    .arg("-h")
                    .arg(height.to_string())
                    .arg("-o")
                    .arg(output)
                    .arg(source)
                    .status(),
                "inkscape" => Command::new(tool)
                    .arg("--export-type=png")
                    .arg(format!("--export-filename={}", output.display()))
                    .arg(format!("--export-width={width}"))
                    .arg(format!("--export-height={height}"))
                    .arg(source)
                    .status(),
                _ => Command::new(tool)
                    .args([
                        "--headless=new",
                        "--no-sandbox",
                        "--disable-gpu",
                        "--hide-scrollbars",
                        "--default-background-color=00000000",
                    ])
                    .arg(format!("--window-size={width},{height}"))
                    .arg(format!("--screenshot={}", output.display()))
                    .arg(format!(
                        "file://{}",
                        source
                            .canonicalize()
                            .unwrap_or_else(|_| source.to_owned())
                            .display()
                    ))
                    .status(),
            };
            if status.is_ok_and(|status| status.success()) && output.is_file() {
                return Ok(());
            }
        }
    }
    Err(Error::format(
        "SVG rasterization failed. Install one of: resvg, rsvg-convert (librsvg), or Inkscape. On Linux, a Chrome/Chromium executable is also supported.",
    ))
}

/// Rasterize an SVG asset for the configuration-window preview with the same
/// external renderer used for map paint layers.
pub fn render_preview(source: &Path, output: &Path, width: u32, height: u32) -> Result<()> {
    render_external(source, output, width, height)
}
fn command_exists(tool: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg("command -v \"$1\" >/dev/null 2>&1")
        .arg("sh")
        .arg(tool)
        .status()
        .is_ok_and(|s| s.success())
}
fn inject_style(source: &str, css: &str) -> Result<String> {
    let at = source
        .rfind("</svg>")
        .ok_or_else(|| Error::format("SVG has no closing </svg> element"))?;
    let mut out = String::with_capacity(source.len() + css.len() + 30);
    out.push_str(&source[..at]);
    out.push_str("<style>");
    out.push_str(css);
    out.push_str("</style>");
    out.push_str(&source[at..]);
    Ok(out)
}

fn attributes(event: &quick_xml::events::BytesStart<'_>) -> Vec<(String, String)> {
    event
        .attributes()
        .flatten()
        .map(|a| {
            (
                String::from_utf8_lossy(a.key.as_ref()).into_owned(),
                a.unescape_value().unwrap_or_default().into_owned(),
            )
        })
        .collect()
}
fn attribute<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find_map(|(k, v)| (k == name || k.ends_with(&format!(":{name}"))).then_some(v.as_str()))
}
fn presentation<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attribute(attrs, "style")
        .and_then(|s| {
            s.split(';').find_map(|d| {
                let (k, v) = d.split_once(':')?;
                (k.trim() == name).then_some(v.trim())
            })
        })
        .or_else(|| attribute(attrs, name))
}
fn is_invisible(attrs: &[(String, String)]) -> bool {
    presentation(attrs, "display")
        .and_then(|value| value.split_whitespace().next())
        .is_some_and(|value| value.eq_ignore_ascii_case("none"))
}
fn is_drawable(tag: &str) -> bool {
    matches!(
        tag,
        "path" | "polyline" | "polygon" | "line" | "rect" | "circle" | "ellipse" | "use" | "image"
    )
}
fn parse_length(value: &str) -> Option<f64> {
    value
        .trim()
        .trim_end_matches(|c: char| c.is_ascii_alphabetic() || c == '%')
        .parse()
        .ok()
}
fn parse_view_box(value: &str) -> Option<(f64, f64, f64, f64)> {
    let v = value
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|v| !v.is_empty())
        .filter_map(|v| v.parse().ok())
        .collect::<Vec<_>>();
    (v.len() == 4).then(|| (v[0], v[1], v[2], v[3]))
}
/// Matrix from a nested SVG's viewBox coordinate system into its parent.
/// SVGs commonly use a small page-sized outer viewport around a large map
/// coordinate system, so this conversion must happen before child transforms.
fn svg_viewport_matrix(attrs: &[(String, String)]) -> Matrix {
    let value = |name, default| {
        attribute(attrs, name)
            .and_then(parse_length)
            .unwrap_or(default)
    };
    let x = value("x", 0.);
    let y = value("y", 0.);
    let Some((vx, vy, vw, vh)) = attribute(attrs, "viewBox").and_then(parse_view_box) else {
        return [1., 0., 0., 1., x, y];
    };
    let width = value("width", vw);
    let height = value("height", vh);
    let sx = width / vw.max(f64::EPSILON);
    let sy = height / vh.max(f64::EPSILON);
    if attribute(attrs, "preserveAspectRatio").is_some_and(|value| value.contains("none")) {
        return [sx, 0., 0., sy, x - vx * sx, y - vy * sy];
    }
    let scale = sx.min(sy);
    [
        scale,
        0.,
        0.,
        scale,
        x + (width - vw * scale) / 2. - vx * scale,
        y + (height - vh * scale) / 2. - vy * scale,
    ]
}
fn multiply(l: Matrix, r: Matrix) -> Matrix {
    let [a, b, c, d, e, f] = l;
    let [g, h, i, j, k, m] = r;
    [
        a * g + c * h,
        b * g + d * h,
        a * i + c * j,
        b * i + d * j,
        a * k + c * m + e,
        b * k + d * m + f,
    ]
}
fn apply(m: Matrix, x: f64, y: f64) -> (f64, f64) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}
fn parse_transform(value: Option<&str>) -> Matrix {
    let Some(mut text) = value else {
        return IDENTITY;
    };
    let mut result = IDENTITY;
    while let Some(open) = text.find('(') {
        let name = text[..open].split_whitespace().last().unwrap_or("");
        let Some(close) = text[open + 1..].find(')') else {
            break;
        };
        let args = text[open + 1..open + 1 + close]
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|v| !v.is_empty())
            .filter_map(|v| v.parse::<f64>().ok())
            .collect::<Vec<_>>();
        let m = match name {
            "matrix" if args.len() >= 6 => [args[0], args[1], args[2], args[3], args[4], args[5]],
            "translate" if !args.is_empty() => {
                [1., 0., 0., 1., args[0], *args.get(1).unwrap_or(&0.)]
            }
            "scale" if !args.is_empty() => {
                [args[0], 0., 0., *args.get(1).unwrap_or(&args[0]), 0., 0.]
            }
            "rotate" if !args.is_empty() => {
                let r = args[0].to_radians();
                let (s, c) = r.sin_cos();
                let rot = [c, s, -s, c, 0., 0.];
                if args.len() >= 3 {
                    multiply(
                        multiply([1., 0., 0., 1., args[1], args[2]], rot),
                        [1., 0., 0., 1., -args[1], -args[2]],
                    )
                } else {
                    rot
                }
            }
            _ => IDENTITY,
        };
        result = multiply(result, m);
        text = &text[open + close + 2..];
    }
    result
}
fn number_pairs(data: &str) -> Vec<(f64, f64)> {
    let mut nums = Vec::new();
    let mut token = String::new();
    for ch in data.chars().chain(std::iter::once(' ')) {
        if ch.is_ascii_digit() || matches!(ch, '.' | '-' | '+' | 'e' | 'E') {
            token.push(ch)
        } else if !token.is_empty() {
            if let Ok(v) = token.parse() {
                nums.push(v)
            }
            token.clear()
        }
    }
    nums.chunks_exact(2).map(|v| (v[0], v[1])).collect()
}
fn local_points(
    tag: &str,
    attrs: &[(String, String)],
    symbols: &HashMap<String, (f64, f64)>,
) -> Vec<(f64, f64)> {
    let f = |n: &str, d: f64| attribute(attrs, n).and_then(parse_length).unwrap_or(d);
    match tag {
        "path" => crate::svg::path_endpoints(attribute(attrs, "d").unwrap_or("")),
        "polyline" | "polygon" => number_pairs(attribute(attrs, "points").unwrap_or("")),
        "line" => vec![(f("x1", 0.), f("y1", 0.)), (f("x2", 0.), f("y2", 0.))],
        "rect" | "image" => {
            let (x, y, w, h) = (f("x", 0.), f("y", 0.), f("width", 0.), f("height", 0.));
            vec![(x, y), (x + w, y + h)]
        }
        "circle" => {
            let (x, y, r) = (f("cx", 0.), f("cy", 0.), f("r", 0.));
            vec![(x - r, y - r), (x + r, y + r)]
        }
        "ellipse" => {
            let (x, y, rx, ry) = (f("cx", 0.), f("cy", 0.), f("rx", 0.), f("ry", 0.));
            vec![(x - rx, y - ry), (x + rx, y + ry)]
        }
        "use" => {
            let x = f("x", 0.);
            let y = f("y", 0.);
            let center = attribute(attrs, "href")
                .and_then(|v| v.strip_prefix('#'))
                .and_then(|id| symbols.get(id))
                .copied()
                .unwrap_or((0., 0.));
            vec![(x + center.0, y + center.1)]
        }
        _ => vec![],
    }
}
fn bounds_center(points: &[(f64, f64)]) -> (f64, f64) {
    let (minx, maxx, miny, maxy) = points.iter().fold(
        (
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ),
        |(a, b, c, d), (x, y)| (a.min(*x), b.max(*x), c.min(*y), d.max(*y)),
    );
    ((minx + maxx) / 2., (miny + maxy) / 2.)
}
fn geometric_center(points: &[(f64, f64)]) -> (f64, f64) {
    if points.len() < 3 {
        return points.first().copied().unwrap_or((0., 0.));
    }
    let mut twice_area = 0.;
    let mut x_sum = 0.;
    let mut y_sum = 0.;
    for index in 0..points.len() {
        let (x1, y1) = points[index];
        let (x2, y2) = points[(index + 1) % points.len()];
        let cross = x1 * y2 - x2 * y1;
        twice_area += cross;
        x_sum += (x1 + x2) * cross;
        y_sum += (y1 + y2) * cross;
    }
    if twice_area.abs() < 1e-9 {
        bounds_center(points)
    } else {
        (x_sum / (3. * twice_area), y_sum / (3. * twice_area))
    }
}
fn matrix_parts(m: Matrix) -> (f64, f64, f64, bool) {
    (
        m[0].hypot(m[1]),
        m[2].hypot(m[3]),
        m[1].atan2(m[0]),
        m[0] * m[3] - m[1] * m[2] < 0.,
    )
}
/// Point-like elements receive one symbol at their centre. For SVG path-like
/// geometry, each distinct visible node becomes a separate symbol placement.
fn symbol_positions(instance: &Instance, width: u32, height: u32) -> Vec<(f64, f64)> {
    if !instance.has_nodes {
        return inside(instance.center, width, height)
            .then_some(instance.center)
            .into_iter()
            .collect();
    }
    let mut positions = Vec::new();
    for point in instance.points.iter().copied() {
        if inside(point, width, height)
            && !positions.iter().any(|existing: &(f64, f64)| {
                (existing.0 - point.0).abs() < 1e-6 && (existing.1 - point.1).abs() < 1e-6
            })
        {
            positions.push(point);
        }
    }
    positions
}
fn inside((x, y): (f64, f64), w: u32, h: u32) -> bool {
    x >= 0. && y >= 0. && x <= w as f64 && y <= h as f64
}
fn intersects(points: &[(f64, f64)], w: u32, h: u32) -> bool {
    if points.is_empty() {
        return false;
    }
    let c = bounds_center(points);
    let minx = points.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
    let maxx = points.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
    let miny = points.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let maxy = points.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
    inside(c, w, h) || (maxx >= 0. && minx <= w as f64 && maxy >= 0. && miny <= h as f64)
}
fn instance_name(instance: &Instance, attribute_name: &str) -> Option<String> {
    let wanted = attribute_name.trim();
    attribute(&instance.attrs, wanted)
        .or_else(|| {
            wanted
                .eq_ignore_ascii_case("map:svgname")
                .then(|| attribute(&instance.attrs, "mapsvg:name"))
                .flatten()
        })
        .or_else(|| {
            wanted
                .strip_prefix("map:")
                .and_then(|local| attribute(&instance.attrs, local))
        })
        .or_else(|| attribute(&instance.attrs, "name"))
        .map(str::to_owned)
}
fn vec2(x: f64, y: f64) -> Value {
    Value::Vector {
        kind: "Vector2".into(),
        values: vec![x as f32, y as f32],
    }
}
fn color(v: [u8; 4]) -> Value {
    Value::Vector {
        kind: "Color".into(),
        values: v.into_iter().map(|v| v as f32 / 255.).collect(),
    }
}
fn dict(entries: &[(&str, Value)]) -> Value {
    Value::Dictionary(
        entries
            .iter()
            .map(|(k, v)| (Value::String((*k).into()), v.clone()))
            .collect(),
    )
}
fn points_string(points: &[(f64, f64)]) -> Value {
    let a = Value::Array(points.iter().map(|(x, y)| vec2(*x, *y)).collect());
    Value::String(godot_text::format(&a))
}
fn hex(v: [u8; 4]) -> String {
    format!("#{:02x}{:02x}{:02x}{:02x}", v[0], v[1], v[2], v[3])
}
fn csv_escape(v: &str) -> String {
    format!("\"{}\"", v.replace('"', "\"\""))
}
fn decode_table(bytes: &[u8], encoding: TableEncoding) -> Result<String> {
    match encoding {
        TableEncoding::Utf8 => String::from_utf8(
            bytes
                .strip_prefix(&[0xef, 0xbb, 0xbf])
                .unwrap_or(bytes)
                .to_vec(),
        )
        .map_err(|error| Error::format(format!("invalid UTF-8: {error}"))),
        TableEncoding::Latin1 => Ok(bytes.iter().map(|byte| char::from(*byte)).collect()),
        TableEncoding::Windows1252 => Ok(bytes.iter().map(|byte| windows_1252(*byte)).collect()),
        TableEncoding::Utf16Le | TableEncoding::Utf16Be => {
            let bytes = match encoding {
                TableEncoding::Utf16Le => bytes.strip_prefix(&[0xff, 0xfe]).unwrap_or(bytes),
                TableEncoding::Utf16Be => bytes.strip_prefix(&[0xfe, 0xff]).unwrap_or(bytes),
                _ => bytes,
            };
            if bytes.len() % 2 != 0 {
                return Err(Error::format("UTF-16 input has an odd byte count"));
            }
            let words = bytes.chunks_exact(2).map(|pair| match encoding {
                TableEncoding::Utf16Le => u16::from_le_bytes([pair[0], pair[1]]),
                TableEncoding::Utf16Be => u16::from_be_bytes([pair[0], pair[1]]),
                _ => unreachable!(),
            });
            String::from_utf16(&words.collect::<Vec<_>>())
                .map_err(|error| Error::format(format!("invalid UTF-16: {error}")))
        }
    }
}
fn windows_1252(byte: u8) -> char {
    const SPECIAL: [char; 32] = [
        '\u{20ac}', '\u{0081}', '\u{201a}', '\u{0192}', '\u{201e}', '\u{2026}', '\u{2020}',
        '\u{2021}', '\u{02c6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{008d}',
        '\u{017d}', '\u{008f}', '\u{0090}', '\u{2018}', '\u{2019}', '\u{201c}', '\u{201d}',
        '\u{2022}', '\u{2013}', '\u{2014}', '\u{02dc}', '\u{2122}', '\u{0161}', '\u{203a}',
        '\u{0153}', '\u{009d}', '\u{017e}', '\u{0178}',
    ];
    if (0x80..=0x9f).contains(&byte) {
        SPECIAL[(byte - 0x80) as usize]
    } else {
        char::from(byte)
    }
}
fn detect_delimiter(line: &str) -> char {
    ['\t', ',', ';', '|']
        .into_iter()
        .max_by_key(|delimiter| delimiter_count(line, *delimiter))
        .unwrap_or('\t')
}
fn delimiter_count(line: &str, delimiter: char) -> usize {
    let mut quoted = false;
    let mut count = 0;
    for character in line.chars() {
        if character == '"' {
            quoted = !quoted;
        } else if character == delimiter && !quoted {
            count += 1;
        }
    }
    count
}
fn delimited_fields(line: &str, delimiter: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '"' if quoted && chars.peek() == Some(&'"') => {
                current.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            value if value == delimiter && !quoted => out.push(std::mem::take(&mut current)),
            _ => current.push(character),
        }
    }
    out.push(current);
    out
}
fn normalize_header(header: &str) -> String {
    header
        .trim()
        .to_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_owned()
}
fn table_value(row: &[String], column: Option<usize>) -> Option<&str> {
    column.and_then(|index| row.get(index)).map(String::as_str)
}
fn parse_coordinate_pairs(raw: &str) -> Result<Vec<(f64, f64)>> {
    // Coordinate cells commonly arrive as one tuple for point features, or
    // as a comma-separated sequence of tuples for paths. Prefer the tuple
    // boundaries so numbers in any surrounding text cannot become points.
    let mut tuples = Vec::new();
    let mut tuple_start = None;
    for (index, character) in raw.char_indices() {
        match character {
            '(' => tuple_start = Some(index + character.len_utf8()),
            ')' => {
                if let Some(start) = tuple_start.take() {
                    let values = parse_coordinate_numbers(&raw[start..index])?;
                    if values.len() != 2 {
                        return Err(Error::format(
                            "coordinate tuples must contain exactly two numbers",
                        ));
                    }
                    tuples.push((values[0], values[1]));
                }
            }
            _ => {}
        }
    }
    if tuple_start.is_some() {
        return Err(Error::format(
            "coordinate tuple is missing a closing parenthesis",
        ));
    }
    if !tuples.is_empty() {
        return Ok(tuples);
    }

    let numbers = parse_coordinate_numbers(raw)?;
    if numbers.len() % 2 != 0 {
        return Err(Error::format("coordinates contain an unmatched number"));
    }
    Ok(numbers
        .chunks_exact(2)
        .map(|pair| (pair[0], pair[1]))
        .collect())
}
fn parse_coordinate_numbers(raw: &str) -> Result<Vec<f64>> {
    let mut numbers = Vec::new();
    let mut current = String::new();
    for character in raw.chars().chain(std::iter::once(' ')) {
        if character.is_ascii_digit() || matches!(character, '-' | '+' | '.' | 'e' | 'E') {
            current.push(character);
        } else if !current.is_empty() {
            numbers.push(
                current
                    .parse::<f64>()
                    .map_err(|_| Error::format(format!("invalid coordinate number: {current}")))?,
            );
            current.clear();
        }
    }
    Ok(numbers)
}
fn expand_table_points(
    tag: &str,
    coordinates: &[(f64, f64)],
    relative_after_first: bool,
) -> Vec<(f64, f64)> {
    let path_like = matches!(
        tag.trim().to_ascii_lowercase().as_str(),
        "path" | "polyline" | "polygon" | "line"
    );
    if relative_after_first && path_like && coordinates.len() > 1 {
        let origin = coordinates[0];
        coordinates[1..]
            .iter()
            .map(|point| (origin.0 + point.0, origin.1 + point.1))
            .collect()
    } else {
        coordinates.to_vec()
    }
}
#[allow(clippy::too_many_arguments)]
fn append_table_svg_element(
    svg: &mut String,
    tag: &str,
    class_name: &str,
    id: &str,
    name: &str,
    fill: &str,
    stroke: &str,
    stroke_width: &str,
    points: &[(f64, f64)],
    has_fill: bool,
) {
    let attributes = format!(
        r#" class="{}" id="{}" mapsvg:name="{}" fill="{}" stroke="{}" stroke-width="{}""#,
        xml_escape(class_name),
        xml_escape(id),
        xml_escape(name),
        xml_escape(if fill.is_empty() { "none" } else { fill }),
        xml_escape(if stroke.is_empty() { "none" } else { stroke }),
        xml_escape(if stroke_width.is_empty() {
            "0"
        } else {
            stroke_width
        }),
    );
    if points.len() == 1 {
        svg.push_str(&format!(
            r#"<circle{attributes} cx="{}" cy="{}" r="0.5"/>"#,
            points[0].0, points[0].1
        ));
        return;
    }
    let points = points
        .iter()
        .map(|(x, y)| format!("{x},{y}"))
        .collect::<Vec<_>>()
        .join(" ");
    // A CSV's element/tag column is authoritative. In particular, paths and
    // polylines must remain open even when their source row contains a fill
    // value; treating every filled row as a polygon closes freshwater lines.
    let tag = match tag.trim().to_ascii_lowercase().as_str() {
        "polygon" if has_fill => "polygon",
        _ => "polyline",
    };
    svg.push_str(&format!(r#"<{tag}{attributes} points="{points}"/>"#));
}
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
fn csv_fields(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' if quoted && chars.peek() == Some(&'"') => {
                current.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => out.push(std::mem::take(&mut current)),
            _ => current.push(ch),
        }
    }
    out.push(current);
    out
}
fn css_string(v: &str) -> String {
    v.replace('\\', "\\\\").replace('"', "\\\"")
}
fn css_ident(v: &str) -> String {
    v.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}
fn safe_name(v: &str) -> String {
    css_ident(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;
    #[test]
    fn finds_classes_transforms_and_viewport() {
        let dir = std::env::temp_dir().join("svg-render-analysis-test.svg");
        fs::write(&dir,r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="80"><g transform="translate(10 20)"><path class="river road" d="M -20,10 L 120,10"/></g></svg>"#).unwrap();
        let d = analyze(&dir).unwrap();
        assert_eq!(d.classes, vec!["river", "road"]);
        assert_eq!((d.width, d.height), (100, 80));
        assert!(intersects(&d.instances[0].points, 100, 80));
        let _ = fs::remove_file(dir);
    }
    #[test]
    fn layers_are_fallback_only_without_classes() {
        let p = std::env::temp_dir().join("svg-render-layer-test.svg");
        fs::write(&p,r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:inkscape="http://www.inkscape.org/namespaces/inkscape" viewBox="0 0 20 20"><g inkscape:groupmode="layer" inkscape:label="Rivers"><path d="M0 0L20 20"/></g></svg>"#).unwrap();
        let d = analyze(&p).unwrap();
        assert!(d.layer_fallback);
        assert_eq!(d.classes, vec!["Rivers"]);
        let _ = fs::remove_file(p);
    }
    #[test]
    fn group_classes_apply_to_contained_shapes() {
        let p = std::env::temp_dir().join("svg-render-group-class-test.svg");
        fs::write(&p,r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20"><g class="rivers"><path d="M0 0L20 20"/></g></svg>"#).unwrap();
        let d = analyze(&p).unwrap();
        assert_eq!(
            d.instances
                .iter()
                .filter(|item| item.class_name == "rivers")
                .count(),
            1
        );
        let _ = fs::remove_file(p);
    }
    #[test]
    fn selected_symbol_is_placed_at_each_distinct_path_node() {
        let p = std::env::temp_dir().join("svg-render-symbol-nodes.svg");
        fs::write(
            &p,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><path class="route" d="M10 10 L50 10 L50 50 Z"/><circle class="marker" cx="75" cy="75" r="3"/></svg>"#,
        )
        .unwrap();
        let document = analyze(&p).unwrap();
        let route = document
            .instances
            .iter()
            .find(|instance| instance.class_name == "route")
            .unwrap();
        assert_eq!(symbol_positions(route, 100, 100).len(), 3);
        let marker = document
            .instances
            .iter()
            .find(|instance| instance.class_name == "marker")
            .unwrap();
        assert_eq!(symbol_positions(marker, 100, 100), vec![(75., 75.)]);
        let _ = fs::remove_file(p);
    }
    #[test]
    fn nested_svg_viewbox_applies_before_child_transforms() {
        let p = std::env::temp_dir().join("svg-render-nested-viewport.svg");
        fs::write(
            &p,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="840" height="770" viewBox="0 0 120 110"><svg x="3" y="3" width="94" height="94" viewBox="1000 2000 100 100"><circle class="place" cx="1050" cy="2050" r="1"/></svg></svg>"#,
        )
        .unwrap();
        let document = analyze(&p).unwrap();
        let center = document.instances[0].center;
        assert!((center.0 - 350.).abs() < 0.01, "{center:?}");
        assert!((center.1 - 350.).abs() < 0.01, "{center:?}");
        let _ = fs::remove_file(p);
    }
    #[test]
    fn use_label_anchor_matches_python_local_offset_through_parent_matrix() {
        let p = std::env::temp_dir().join("svg-render-use-label-anchor.svg");
        fs::write(
            &p,
            r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="300" height="300"><defs><symbol id="town"><rect x="0" y="0" width="15" height="15"/></symbol></defs><g transform="matrix(2,0,0,2,10,20)"><use class="places" name="Town" xlink:href="#town" transform="matrix(3,0,0,4,5,6)"/></g></svg>"##,
        )
        .unwrap();
        let document = analyze(&p).unwrap();
        let place = document
            .instances
            .iter()
            .find(|instance| instance.class_name == "places")
            .unwrap();
        // Python writes child-local (18, 13); the parent then transforms that
        // to (128, 136) in the root viewport.
        assert_eq!(place.label_anchor, Some((128., 136.)));
        let _ = fs::remove_file(p);
    }
    #[test]
    fn labels_can_prepend_class_and_paths_default_to_zero_roughness() {
        let mut row = ClassSettings::new("town".into(), "name");
        assert_eq!(row.label.font, "Gentium Book Basic Bold");
        assert_eq!(row.label.outline, 0.);
        assert_eq!(row.label.align, 0);
        assert!(LABEL_FONT_PRESETS.contains(&"Aladin"));
        assert!(LABEL_FONT_PRESETS.contains(&"Uncial Antiqua"));
        assert_eq!(row.roughness, 0.);
        assert_eq!(label_text(&row, "Oakrest"), "Oakrest");
        row.label.prepend_class = true;
        assert_eq!(label_text(&row, "Oakrest"), "town: Oakrest");
        let record = label_record(&row, "Oakrest".into(), (0., 0.));
        assert_eq!(record.get("align").and_then(Value::as_f64), Some(0.));
    }
    #[test]
    fn fill_with_land_category_initializes_the_whole_mask() {
        let mut row = ClassSettings::new("background".into(), "name");
        row.category = Category::FillWithLand;
        let mask = initial_mask(3, 2, &[row]);
        assert!(
            mask.pixels()
                .all(|pixel| *pixel == image::Rgba([0, 0, 0, 255]))
        );

        let empty_mask = initial_mask(2, 1, &[]);
        assert!(empty_mask.pixels().all(|pixel| pixel.0 == [0, 0, 0, 0]));
    }
    #[test]
    fn tabular_import_maps_headers_relative_points_and_encoding() {
        let path = std::env::temp_dir().join("svg-render-table.tsv");
        fs::write(
            &path,
            "tag\tid\tmapsvg_name\tclass\tfill\tstroke\tstroke_width\tcoordinates\npath\tp1\tWäldchen\tWald\t#008000\t\t\t[(100, 200), (10, -20), (30, 40)]\n",
        )
        .unwrap();
        let table = read_table(&path, TableEncoding::Utf8, TableDelimiter::Auto).unwrap();
        assert_eq!(table.delimiter, '\t');
        let columns = auto_table_columns(&table.headers);
        assert_eq!(columns.name, Some(2));
        assert_eq!(
            table_bounds(&table, &columns, true).unwrap(),
            (110., 180., 20., 60.)
        );
        let document = analyze_table(
            &path,
            &table,
            &TableOptions {
                columns,
                relative_after_first: true,
                source_x: 100.,
                source_y: 100.,
                source_width: 100.,
                source_height: 200.,
                map_width: 1000,
                map_height: 1000,
            },
        )
        .unwrap();
        assert_eq!(document.classes, vec!["Wald"]);
        assert_eq!(
            document.instances[0].points,
            vec![(100., 400.), (300., 700.)]
        );
        assert!(document.source_svg.contains("Wäldchen"));
        assert!(document.source_svg.contains("<polyline"));
        assert!(!document.source_svg.contains("<polygon"));
        let _ = fs::remove_file(path);
    }
    #[test]
    fn tabular_import_accepts_single_coordinate_tuples_for_point_features() {
        let path = std::env::temp_dir().join("svg-render-single-coordinate-tuples.tsv");
        fs::write(
            &path,
            "tag\tid\tname\tclass\tcoordinates\nuse\tuse5328\tStoerrebrandt-Kolleg\tAkademie\t(2488.2, 792.0)\nuse\tuse5603\tSchwert und Stab\tAkademie\t(2623.9, 1242.1)\npath\tfoob\thurteweg\tWeg\t(5579.1, 2485.2),(5579.1, 2485.2)\nuse\tuse6765\tGarten des Ruhms Frans\tAkademie\t(1343.2, 1542.4)\n",
        )
        .unwrap();
        let table = read_table(&path, TableEncoding::Utf8, TableDelimiter::Auto).unwrap();
        let columns = auto_table_columns(&table.headers);
        assert_eq!(table.delimiter, '\t');
        assert_eq!(table_bounds(&table, &columns, false).unwrap().0, 1343.2);
        let document = analyze_table(
            &path,
            &table,
            &TableOptions {
                columns,
                relative_after_first: false,
                source_x: 0.,
                source_y: 0.,
                source_width: 6000.,
                source_height: 3000.,
                map_width: 6000,
                map_height: 3000,
            },
        )
        .unwrap();
        assert_eq!(document.instances.len(), 4);
        assert_eq!(document.instances[0].points, vec![(2488.2, 792.0)]);
        assert_eq!(document.instances[2].points.len(), 2);
        let _ = fs::remove_file(path);
    }
    #[test]
    fn display_none_svg_elements_are_not_routed_or_rasterized() {
        let p = std::env::temp_dir().join("svg-render-invisible-elements.svg");
        fs::write(
            &p,
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20"><g class="places"><circle cx="4" cy="4" r="2"/><circle cx="8" cy="8" r="2" style="display: none"/><g visibility="hidden"><circle cx="12" cy="12" r="2"/></g></g></svg>"#,
        )
        .unwrap();
        let document = analyze(&p).unwrap();
        assert_eq!(
            document
                .instances
                .iter()
                .filter(|item| item.class_name == "places")
                .count(),
            2
        );
        let mut row = ClassSettings::new("places".into(), "map:svgname");
        row.category = Category::Ground;
        let css = raster_css(&document, &row);
        assert!(css.contains("display:none!important"));
        assert!(!css.contains("[visibility=\"hidden\"]"));
        let _ = fs::remove_file(p);
    }
    #[test]
    fn layermap_layers_keep_all_visible_symbols_and_paint_layers() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("testfiles");
        let source = fixtures.join("layermap.svg");
        let document = analyze(&source).unwrap();
        assert!(document.layer_fallback);
        let visible_places = document
            .instances
            .iter()
            .filter(|item| item.class_name == "places")
            .filter(|item| inside(item.center, document.width, document.height))
            .count();
        assert_eq!(visible_places, 5, "{:#?}", document.instances);

        let mut settings = default_settings(&document);
        assert_eq!(
            load_csv(
                &fixtures.join("layermap_render_settings.csv"),
                &mut settings
            )
            .unwrap(),
            7
        );
        for (class_name, category) in [
            ("ground_painting", Category::Ground),
            ("land", Category::Landmass),
            ("places", Category::Symbol),
            ("rivvers", Category::Freshwater),
            ("streets", Category::Path),
            ("water_paiting", Category::WaterTint),
        ] {
            assert!(
                settings
                    .iter()
                    .any(|row| row.class_name == class_name && row.category == category)
            );
        }
        for (class_name, category, layer_id) in [
            ("land", Category::Landmass, "layer5"),
            ("rivvers", Category::Freshwater, "layer6"),
        ] {
            let mut row = ClassSettings::new(class_name.into(), "map:svgname");
            row.category = category;
            assert!(
                raster_css(&document, &row).contains(&format!("#{layer_id}")),
                "{class_name} must target its actual Inkscape layer id"
            );
        }

        let expected = crate::godot_text::parse(
            &fs::read_to_string(fixtures.join("layermap_map_data.txt")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            expected.get("map_width").and_then(Value::as_f64),
            Some(1000.)
        );
        assert_eq!(
            expected.get("map_height").and_then(Value::as_f64),
            Some(1000.)
        );
        for (key, filename) in [
            ("ground", ".ground.png"),
            ("mask", ".mask.png"),
            ("water_tint", ".water_tint.png"),
        ] {
            assert_eq!(expected.get(key).and_then(Value::as_str), Some(filename));
        }
        assert_eq!(
            expected
                .get("symbols")
                .and_then(Value::as_array)
                .map_or(0, |symbols| symbols.len()),
            5
        );
        assert_eq!(
            expected
                .get("paths")
                .and_then(Value::as_array)
                .map_or(0, |paths| paths.len()),
            1
        );
        assert_eq!(
            expected
                .get("labels")
                .and_then(Value::as_array)
                .map_or(0, |labels| labels.len()),
            1
        );
        for filename in [
            "layermap.ground.png",
            "layermap.mask.png",
            "layermap.water_tint.png",
        ] {
            let image = image::open(fixtures.join(filename)).unwrap().to_rgba8();
            assert_eq!(image.dimensions(), (1000, 1000));
            assert!(
                image.pixels().any(|pixel| pixel[3] != 0),
                "{filename} is empty"
            );
        }
    }
    #[test]
    fn fullmap_attempt2_routes_transformed_nested_svg_symbols() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("testfiles");
        let document = analyze(&fixtures.join("fullmap_attempt2.svg")).unwrap();
        assert_eq!((document.width, document.height), (840, 770));
        let mut settings = default_settings(&document);
        assert_eq!(
            load_csv(
                &fixtures.join("fullmap_attempt2_render_settings.csv"),
                &mut settings
            )
            .unwrap(),
            58
        );
        let symbol_classes = settings
            .iter()
            .filter(|row| row.category == Category::Symbol)
            .map(|row| row.class_name.as_str())
            .collect::<HashSet<_>>();
        let visible_symbols = document
            .instances
            .iter()
            .filter(|instance| symbol_classes.contains(instance.class_name.as_str()))
            .filter(|instance| inside(instance.center, document.width, document.height))
            .count();
        assert!(
            visible_symbols > 0,
            "no transformed symbols were in the viewport"
        );
    }
    #[test]
    fn settings_csv_round_trip() {
        let p = std::env::temp_dir().join("svg-render-settings.csv");
        let mut rows = vec![ClassSettings::new("town, big".into(), "mapsvg:name")];
        rows[0].category = Category::Symbol;
        save_csv(&p, &rows).unwrap();
        let mut target = vec![ClassSettings::new("town, big".into(), "x")];
        assert_eq!(load_csv(&p, &mut target).unwrap(), 1);
        assert_eq!(target[0].category, Category::Symbol);
        let _ = fs::remove_file(p);
    }
    #[test]
    fn symbol_records_follow_draw_mode_metadata() {
        let root =
            std::env::temp_dir().join(format!("svg-render-draw-mode-{}", std::process::id()));
        let symbols = root.join("sprites/symbols/test");
        fs::create_dir_all(&symbols).unwrap();
        fs::write(symbols.join("custom.png"), b"placeholder").unwrap();
        fs::write(
            symbols.join(".wonderdraft_symbols"),
            r#"{"custom":{"radius":64,"offset_x":0,"offset_y":0,"draw_mode":"custom_colors"}}"#,
        )
        .unwrap();
        let resolver = Resolver::new(&Settings {
            default_asset_folder: root.join("sprites").to_string_lossy().into_owned(),
            ..Settings::default()
        });
        let mut row = ClassSettings::new("town".into(), "mapsvg:name");
        row.symbol = "res://sprites/symbols/test/custom".into();
        row.custom_colors = [[1, 2, 3, 255], [4, 5, 6, 255], [7, 8, 9, 255]];
        let instance = Instance {
            class_name: "town".into(),
            attrs: vec![],
            matrix: IDENTITY,
            points: vec![],
            center: (10., 20.),
            label_anchor: None,
            has_fill: false,
            has_nodes: false,
        };
        let record = symbol_record_at(&row, &instance, instance.center, &resolver);
        assert!(matches!(
            record.get("custom_color_mode"),
            Some(Value::Int(1))
        ));
        assert!(
            matches!(record.get("custom_colors"), Some(Value::Array(colors)) if colors.len() == 3)
        );
        assert!(matches!(record.get("sample"), Some(Value::Nil)));

        row.symbol = "res://sprites/symbols/test/normal".into();
        let normal = symbol_record_at(&row, &instance, instance.center, &resolver);
        assert!(matches!(normal.get("custom_color_mode"), Some(Value::Nil)));
        assert!(matches!(normal.get("custom_colors"), Some(Value::Nil)));
        assert!(matches!(normal.get("sample"), Some(Value::Nil)));
        assert!(matches!(normal.get("outline_width"), Some(Value::Real(width)) if *width == 0.0));
        let _ = fs::remove_dir_all(root);
    }
}

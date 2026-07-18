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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Category {
    Symbol,
    Path,
    Ground,
    WaterTint,
    Territory,
    Landmass,
    Freshwater,
    #[default]
    Invisible,
}

impl Category {
    pub const ALL: [Self; 8] = [
        Self::Symbol,
        Self::Path,
        Self::Ground,
        Self::WaterTint,
        Self::Territory,
        Self::Landmass,
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
            Self::Freshwater => "freshwater",
            Self::Invisible => "invisible",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LabelSettings {
    pub enabled: bool,
    pub size: f32,
    pub font: String,
    pub color: [u8; 4],
    pub outline_color: [u8; 4],
    pub outline: f32,
    pub offset_x: f32,
    pub offset_y: f32,
}
impl Default for LabelSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            size: 24.,
            font: "East Sea Dokdo".into(),
            color: [0, 0, 0, 255],
            outline_color: [255, 255, 255, 255],
            outline: 2.,
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
    pub path_style: String,
    pub path_color: [u8; 4],
    pub width: f32,
    pub fill_override: Option<[u8; 4]>,
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
            path_style: "res://textures/paths/path_blended".into(),
            path_color: [0, 0, 0, 255],
            width: 4.,
            fill_override: None,
            border_override: None,
            border_width_override: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Document {
    pub source: PathBuf,
    pub width: u32,
    pub height: u32,
    pub classes: Vec<String>,
    pub layer_fallback: bool,
    instances: Vec<Instance>,
}

#[derive(Clone, Debug)]
struct Instance {
    class_name: String,
    attrs: Vec<(String, String)>,
    matrix: Matrix,
    points: Vec<(f64, f64)>,
    center: (f64, f64),
    has_fill: bool,
}

#[derive(Clone)]
struct ParseFrame {
    matrix: Matrix,
    layer: Option<String>,
    classes: Vec<String>,
    symbol: Option<String>,
}

pub fn analyze(path: &Path) -> Result<Document> {
    let raw = fs::read_to_string(path).map_err(|e| Error::format(e.to_string()))?;
    let mut reader = Reader::from_str(&raw);
    let mut stack = vec![ParseFrame {
        matrix: IDENTITY,
        layer: None,
        classes: Vec::new(),
        symbol: None,
    }];
    let mut class_names = HashSet::new();
    let mut layers = HashSet::new();
    let mut pending = Vec::new();
    let mut width = None;
    let mut height = None;
    let mut view_box = None;
    let mut symbol_bounds: HashMap<String, Vec<(f64, f64)>> = HashMap::new();
    loop {
        let event = reader
            .read_event()
            .map_err(|e| Error::format(e.to_string()))?;
        match event {
            Event::Start(ref event) | Event::Empty(ref event) => {
                let tag = String::from_utf8_lossy(event.local_name().as_ref()).into_owned();
                let attrs = attributes(event);
                if tag == "svg" && width.is_none() {
                    width = attribute(&attrs, "width").and_then(parse_length);
                    height = attribute(&attrs, "height").and_then(parse_length);
                    view_box = attribute(&attrs, "viewBox").and_then(parse_view_box);
                }
                let parent = stack.last().cloned().unwrap_or(ParseFrame {
                    matrix: IDENTITY,
                    layer: None,
                    classes: Vec::new(),
                    symbol: None,
                });
                let matrix = multiply(
                    parent.matrix,
                    parse_transform(attribute(&attrs, "transform")),
                );
                let is_layer = attribute(&attrs, "groupmode").is_some_and(|value| value == "layer");
                let own_layer = is_layer.then(|| {
                    attribute(&attrs, "label")
                        .or_else(|| attribute(&attrs, "id"))
                        .unwrap_or("unnamed layer")
                        .to_owned()
                });
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
                if is_drawable(&tag) {
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
                if matches!(event, quick_xml::events::BytesStart { .. }) {
                    stack.push(ParseFrame {
                        matrix,
                        layer,
                        classes: active_classes,
                        symbol: current_symbol,
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
            let has_fill =
                presentation(&attrs, "fill").is_some_and(|fill| !fill.eq_ignore_ascii_case("none"));
            instances.push(Instance {
                class_name,
                attrs: attrs.clone(),
                matrix,
                points,
                center,
                has_fill,
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
    let mut mask = RgbaImage::new(document.width, document.height);
    let mut ground = RgbaImage::new(document.width, document.height);
    let mut water = RgbaImage::new(document.width, document.height);
    let mut symbols = Vec::new();
    let mut paths = Vec::new();
    let mut territories = Vec::new();
    let mut labels = Vec::new();
    let source = fs::read_to_string(&document.source).map_err(|e| Error::format(e.to_string()))?;
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
            let label_position = if instance.has_fill {
                geometric_center(&instance.points)
            } else {
                first
            };
            match row.category {
                Category::Symbol
                    if !row.symbol.is_empty()
                        && inside(instance.center, document.width, document.height) =>
                {
                    symbols.push(symbol_record(row, instance, resolver))
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
                    labels.push(label_record(row, text, label_position));
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
fn symbol_record(row: &ClassSettings, instance: &Instance, resolver: &Resolver) -> Value {
    let mut record = Value::dict();
    let asset = resolver.asset_info(&row.symbol);
    let (_, _, angle, mirror) = matrix_parts(instance.matrix);
    record.set("position", vec2(instance.center.0, instance.center.1));
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
    record.set("sample", color(row.tint));
    record
}
fn path_record(row: &ClassSettings, instance: &Instance) -> Value {
    let mut r = Value::dict();
    r.set("points", points_string(&instance.points));
    r.set("position", vec2(0., 0.));
    r.set("width", Value::Real(row.width as f64));
    r.set("color", color(row.path_color));
    r.set("style", Value::String(row.path_style.clone()));
    r.set("roughness", Value::Real(0.33));
    r.set("straight", Value::Bool(false));
    r.set("z_index", Value::Int(0));
    r
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
    r.set("align", Value::Int(1));
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
        format!(
            "[inkscape\\:label=\"{}\"],#{}",
            css_string(&row.class_name),
            css_ident(&row.class_name)
        )
    } else {
        format!("[class~=\"{}\"]", css_string(&row.class_name))
    };
    let descendants = format!("{target}, {target} *");
    let mut style = format!(
        "*{{visibility:hidden!important}} defs,defs *{{visibility:visible!important}} {descendants}{{visibility:visible!important}}"
    );
    match row.category {
        Category::Landmass => style.push_str(&format!(" {descendants}{{fill:#000!important;stroke:#000!important;stroke-width:{}px!important}}", row.width)),
        Category::Freshwater => style.push_str(&format!(" {target}:not([fill=\"none\"]):not([style*=\"fill:none\"]), {target} *:not([fill=\"none\"]):not([style*=\"fill:none\"]){{fill:#ff0000!important}} {target}:not([stroke=\"none\"]):not([style*=\"stroke:none\"]), {target} *:not([stroke=\"none\"]):not([style*=\"stroke:none\"]){{stroke:#ff0000!important}}")),
        Category::Ground | Category::WaterTint => {
            if let Some(value)=row.fill_override { style.push_str(&format!(" {descendants}{{fill:{}!important}}", hex(value))); }
            if let Some(value)=row.border_override { style.push_str(&format!(" {descendants}{{stroke:{}!important}}", hex(value))); }
            if let Some(value)=row.border_width_override { style.push_str(&format!(" {descendants}{{stroke-width:{value}px!important}}")); }
        }
        _ => {}
    }
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
}

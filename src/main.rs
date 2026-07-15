#![allow(clippy::collapsible_if)]

use eframe::egui;
use std::{fs, path::PathBuf};
use wonderdraft_editor::{
    Error, Result, Value,
    assets::Resolver,
    gcpf, godot_text,
    images::{self, Images},
    settings::{self, Settings},
    svg,
    value::image_info,
    variant,
};

struct App {
    text: String,
    root_path: Option<PathBuf>,
    cache_dir: PathBuf,
    images: Images,
    selected: usize,
    preview: Option<egui::TextureHandle>,
    status: String,
    compressed: bool,
    verify: bool,
    embed_bg: bool,
    settings: Settings,
    settings_open: bool,
}
impl Default for App {
    fn default() -> Self {
        let settings = settings::load();
        Self {
            text: String::new(),
            root_path: None,
            cache_dir: PathBuf::from(&settings.cache_folder),
            images: Vec::new(),
            selected: 0,
            preview: None,
            status: "Open a .wonderdraft_map file".into(),
            compressed: true,
            verify: true,
            embed_bg: false,
            settings,
            settings_open: false,
        }
    }
}

fn dialog(title: &str, message: &str, error: bool) {
    let level = if error {
        rfd::MessageLevel::Error
    } else {
        rfd::MessageLevel::Info
    };
    rfd::MessageDialog::new()
        .set_title(title)
        .set_description(message)
        .set_level(level)
        .show();
}
impl App {
    fn fail(&mut self, title: &str, e: impl std::fmt::Display) {
        self.status = title.into();
        dialog(title, &e.to_string(), true)
    }
    fn parse(&self) -> Result<Value> {
        let v = godot_text::parse(&self.text)?;
        if !matches!(v, Value::Dictionary(_)) {
            return Err(Error::format("root value must be a Dictionary"));
        }
        Ok(v)
    }
    fn open(&mut self, ctx: &egui::Context) -> Result<()> {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Wonderdraft map", &["wonderdraft_map"])
            .pick_file()
        else {
            return Ok(());
        };
        fs::create_dir_all(&self.cache_dir).map_err(|e| Error::format(e.to_string()))?;
        let cache = images::temp_cache_dir(&self.cache_dir)?;
        let variant_path = cache.join("map.variant");
        self.status = "Decompressing map to disk…".into();
        ctx.request_repaint();
        let meta = gcpf::decompress_file(&path, &variant_path, |_, _| {})?;
        self.status = "Decoding Godot Variant…".into();
        let root = variant::decode_file(&variant_path)?;
        if !matches!(root, Value::Dictionary(_)) {
            return Err(Error::format("map root is not a Godot Dictionary"));
        }
        self.images = images::find(&root);
        self.text = godot_text::format(&images::placeholders(&root, &self.images));
        self.root_path = Some(path.clone());
        self.cache_dir = cache;
        self.selected = 0;
        self.preview = None;
        self.status = format!(
            "Loaded {} — {} disk-backed images, {:.2} GiB cache",
            path.file_name().unwrap_or_default().to_string_lossy(),
            self.images.len(),
            meta.uncompressed_size as f64 / 1024f64.powi(3)
        );
        self.update_preview(ctx)?;
        Ok(())
    }
    fn validate(&mut self) {
        match self.parse() {
            Ok(_) => {
                self.status = "Text validated successfully".into();
                dialog("Valid", "The map text is syntactically valid.", false)
            }
            Err(e) => self.fail("Invalid map text", e),
        }
    }
    fn save(&mut self) -> Result<()> {
        let editable = self.parse()?;
        let root = images::restore(&editable, &self.images);
        let name = self
            .root_path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|s| format!("{}_edited.wonderdraft_map", s.to_string_lossy()))
            .unwrap_or_else(|| "edited.wonderdraft_map".into());
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(&name)
            .add_filter("Wonderdraft map", &["wonderdraft_map"])
            .save_file()
        else {
            return Ok(());
        };
        self.status = "Streaming map to GCPF output…".into();
        let size = variant::save_map(&root, &path, 4096, self.compressed)?;
        if self.verify {
            let verify_dir = images::temp_cache_dir(&settings::default_cache())?;
            let raw = verify_dir.join("verify.variant");
            gcpf::decompress_file(&path, &raw, |_, _| {})?;
            let check = variant::decode_file(&raw)?;
            if !matches!(check, Value::Dictionary(_)) {
                return Err(Error::format("verification produced a non-dictionary root"));
            }
            let _ = fs::remove_dir_all(verify_dir);
        }
        self.status = format!(
            "Saved {} ({size} bytes{})",
            path.file_name().unwrap_or_default().to_string_lossy(),
            if self.verify { "; verified" } else { "" }
        );
        dialog("Saved", &self.status, false);
        Ok(())
    }
    fn export_svg(&mut self) -> Result<()> {
        let root = self.parse()?;
        let name = self
            .root_path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|s| format!("{}.svg", s.to_string_lossy()))
            .unwrap_or_else(|| "wonderdraft_map.svg".into());
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(&name)
            .add_filter("SVG", &["svg"])
            .save_file()
        else {
            return Ok(());
        };
        let s = svg::export(
            &root,
            &self.images,
            &path,
            &Resolver::new(&self.settings),
            self.embed_bg,
        )?;
        self.status = format!(
            "Exported SVG: {} labels, {} symbols, {} paths",
            s.labels, s.symbols, s.paths
        );
        dialog(
            "SVG exported",
            &format!(
                "{}\nMissing sprites: {}\nBackground: {}",
                self.status, s.missing_symbols, s.background
            ),
            false,
        );
        Ok(())
    }
    fn import_svg(&mut self) -> Result<()> {
        let mut root = self.parse()?;
        let Some(path) = rfd::FileDialog::new()
            .add_filter("SVG", &["svg"])
            .pick_file()
        else {
            return Ok(());
        };
        let s = svg::import(&mut root, &path, &Resolver::new(&self.settings))?;
        self.text = godot_text::format(&root);
        self.status = format!(
            "Imported SVG: {} labels, {} symbols, {} paths",
            s.labels, s.symbols, s.paths
        );
        dialog("SVG imported", &self.status, false);
        Ok(())
    }
    fn export_selected(&mut self) -> Result<()> {
        let Some((key, value)) = self.images.get(self.selected) else {
            return Ok(());
        };
        let info = image_info(value).ok_or_else(|| Error::format("invalid embedded image"))?;
        let leaf = key.split('.').next_back().unwrap_or("image");
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(format!(".{leaf}.png"))
            .add_filter("PNG", &["png"])
            .save_file()
        else {
            return Ok(());
        };
        images::export_png(&path, &info)?;
        self.status = format!("Exported {key}");
        Ok(())
    }
    fn export_all(&mut self) -> Result<()> {
        let Some(dir) = rfd::FileDialog::new().pick_folder() else {
            return Ok(());
        };
        for (key, value) in &self.images {
            if let Some(info) = image_info(value) {
                let leaf = key.split('.').next_back().unwrap_or("image");
                images::export_png(&dir.join(format!(".{leaf}.png")), &info)?;
            }
        }
        self.status = format!("Exported {} PNG files", self.images.len());
        Ok(())
    }
    fn replace_selected(&mut self, ctx: &egui::Context) -> Result<()> {
        if self.images.get(self.selected).is_none() {
            return Ok(());
        }
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Images", &["png", "jpg", "jpeg", "webp"])
            .pick_file()
        else {
            return Ok(());
        };
        let old = &self.images[self.selected].1;
        let value = images::import_image(&path, old, &self.cache_dir)?;
        self.images[self.selected].1 = value;
        self.preview = None;
        self.update_preview(ctx)?;
        self.status = format!("Replaced {}", self.images[self.selected].0);
        Ok(())
    }
    fn update_preview(&mut self, ctx: &egui::Context) -> Result<()> {
        self.preview = None;
        let Some((key, v)) = self.images.get(self.selected) else {
            return Ok(());
        };
        let Some(info) = image_info(v) else {
            return Ok(());
        };
        let (w, h, rgba) = images::thumbnail(&info, 300)?;
        let image = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
        self.preview = Some(ctx.load_texture(key, image, egui::TextureOptions::NEAREST));
        Ok(())
    }
    fn set_selected(&mut self, i: usize, ctx: &egui::Context) {
        if self.selected != i {
            self.selected = i;
            if let Err(e) = self.update_preview(ctx) {
                self.fail("Preview failed", e);
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, root_ui: &mut egui::Ui, _: &mut eframe::Frame) {
        let ctx = root_ui.ctx().clone();
        egui::Panel::top("toolbar").show(root_ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.button("Open map").clicked() {
                    if let Err(e) = self.open(&ctx) {
                        self.fail("Open failed", e)
                    }
                }
                if ui.button("Validate text").clicked() {
                    self.validate()
                }
                if ui.button("Save map as…").clicked() {
                    if let Err(e) = self.save() {
                        self.fail("Save failed", e)
                    }
                }
                if ui.button("Export SVG…").clicked() {
                    if let Err(e) = self.export_svg() {
                        self.fail("SVG export failed", e)
                    }
                }
                if ui.button("Import SVG…").clicked() {
                    if let Err(e) = self.import_svg() {
                        self.fail("SVG import failed", e)
                    }
                }
                if ui.button("Export all PNGs").clicked() {
                    if let Err(e) = self.export_all() {
                        self.fail("Export failed", e)
                    }
                }
                if ui.button("Asset folders…").clicked() {
                    self.settings_open = true;
                }
                ui.checkbox(&mut self.compressed, "Compress saved map");
                ui.checkbox(&mut self.verify, "Verify save");
                ui.checkbox(&mut self.embed_bg, "Embed mask in SVG");
            });
            ui.label(&self.status);
        });
        egui::Panel::right("images")
            .default_size(300.)
            .show(root_ui, |ui| {
                ui.heading("Embedded images");
                let mut chosen = None;
                for (i, (key, _)) in self.images.iter().enumerate() {
                    if ui.selectable_label(self.selected == i, key).clicked() {
                        chosen = Some(i);
                    }
                }
                if let Some(i) = chosen {
                    self.set_selected(i, &ctx);
                }
                ui.horizontal(|ui| {
                    if ui.button("Export PNG").clicked() {
                        if let Err(e) = self.export_selected() {
                            self.fail("Export failed", e)
                        }
                    }
                    if ui.button("Replace PNG").clicked() {
                        if let Err(e) = self.replace_selected(&ctx) {
                            self.fail("Replace failed", e)
                        }
                    }
                });
                if let Some((key, v)) = self.images.get(self.selected) {
                    if let Some(info) = image_info(v) {
                        ui.label(format!(
                            "{key}\n{} × {}, {}, {} raw bytes\n{}",
                            info.width,
                            info.height,
                            info.format,
                            info.pixels.len(),
                            if matches!(info.pixels, wonderdraft_editor::ByteSource::File { .. }) {
                                "disk-backed"
                            } else {
                                "in memory"
                            }
                        ));
                    }
                }
                if let Some(texture) = &self.preview {
                    ui.add(egui::Image::new(texture).max_size(egui::vec2(300., 300.)));
                }
                ui.separator();
                ui.label(format!(
                    "Custom assets: {}\nDefault sprites: {}\nDisk cache: {}",
                    empty(&self.settings.custom_asset_folder),
                    empty(&self.settings.default_asset_folder),
                    self.settings.cache_folder
                ));
            });
        egui::CentralPanel::default().show(root_ui, |ui| {
            ui.label("Map data (Godot text syntax)");
            egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut self.text)
                        .font(egui::TextStyle::Monospace)
                        .desired_width(f32::INFINITY)
                        .desired_rows(40),
                );
            });
        });
        if self.settings_open {
            let mut open = self.settings_open;
            let mut close = false;
            egui::Window::new("Wonderdraft asset folders")
                .open(&mut open)
                .show(&ctx, |ui| {
                    ui.label("Custom asset folder");
                    ui.text_edit_singleline(&mut self.settings.custom_asset_folder);
                    if ui.button("Browse custom…").clicked() {
                        if let Some(p) = rfd::FileDialog::new().pick_folder() {
                            self.settings.custom_asset_folder = p.to_string_lossy().into_owned();
                        }
                    }
                    ui.label("Default sprites folder");
                    ui.text_edit_singleline(&mut self.settings.default_asset_folder);
                    if ui.button("Browse sprites…").clicked() {
                        if let Some(p) = rfd::FileDialog::new().pick_folder() {
                            self.settings.default_asset_folder = p.to_string_lossy().into_owned();
                        }
                    }
                    ui.label("Disk cache folder");
                    ui.text_edit_singleline(&mut self.settings.cache_folder);
                    if ui.button("Browse cache…").clicked() {
                        if let Some(p) = rfd::FileDialog::new().pick_folder() {
                            self.settings.cache_folder = p.to_string_lossy().into_owned();
                        }
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            match settings::save(&self.settings) {
                                Ok(()) => {
                                    self.cache_dir = PathBuf::from(&self.settings.cache_folder);
                                    self.status = "Settings saved".into();
                                    close = true
                                }
                                Err(e) => self.fail("Settings failed", e),
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                });
            self.settings_open = open && !close;
        }
    }
}
fn empty(s: &str) -> &str {
    if s.is_empty() { "not configured" } else { s }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1240., 820.])
            .with_min_inner_size([940., 620.]),
        ..Default::default()
    };
    eframe::run_native(
        "Wonderdraft Map Editor — Rust SVG edition",
        options,
        Box::new(|_| Ok(Box::new(App::default()))),
    )
}

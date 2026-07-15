#![allow(clippy::collapsible_if)]

use eframe::egui;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
};
use wonderdraft_editor::{
    Error, Result, Value,
    assets::Resolver,
    gcpf, godot_text,
    images::{self, BinaryBlobs, Images},
    settings::{self, Settings},
    svg,
    value::image_info,
    variant,
};

struct App {
    text: String,
    root_path: Option<PathBuf>,
    cache_dir: PathBuf,
    work_dir: Option<PathBuf>,
    images: Images,
    binary_blobs: BinaryBlobs,
    selected: usize,
    preview: Option<egui::TextureHandle>,
    status: String,
    compressed: bool,
    verify: bool,
    embed_bg: bool,
    export_background: bool,
    export_paths: bool,
    export_symbols: bool,
    export_labels: bool,
    settings: Settings,
    settings_open: bool,
    pending_dialog: Option<Receiver<DialogSelection>>,
    map_load: Option<Receiver<Result<LoadedMap>>>,
}

struct LoadedMap {
    path: PathBuf,
    work_dir: PathBuf,
    text: String,
    images: Images,
    binary_blobs: BinaryBlobs,
    uncompressed_size: u32,
}

enum DialogAction {
    OpenMap,
    SaveMap { file_name: String },
    ExportSvg { file_name: String },
    ImportSvg,
    ExportImage { index: usize, file_name: String },
    ExportAllImages,
    ReplaceImage { index: usize },
    CustomAssets,
    DefaultSprites,
    CacheFolder,
}

struct DialogSelection {
    action: DialogAction,
    path: Option<PathBuf>,
}
impl Default for App {
    fn default() -> Self {
        let settings = settings::load();
        Self {
            text: String::new(),
            root_path: None,
            cache_dir: PathBuf::from(&settings.cache_folder),
            work_dir: None,
            images: Vec::new(),
            binary_blobs: Vec::new(),
            selected: 0,
            preview: None,
            status: "Open a .wonderdraft_map file".into(),
            compressed: true,
            verify: true,
            embed_bg: false,
            export_background: true,
            export_paths: true,
            export_symbols: true,
            export_labels: true,
            settings,
            settings_open: false,
            pending_dialog: None,
            map_load: None,
        }
    }
}

fn dialog(title: &str, message: &str, error: bool) {
    let title = title.to_owned();
    let message = message.to_owned();
    let level = if error {
        rfd::MessageLevel::Error
    } else {
        rfd::MessageLevel::Info
    };
    std::thread::spawn(move || {
        rfd::MessageDialog::new()
            .set_title(title)
            .set_description(message)
            .set_level(level)
            .show();
    });
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
    fn load_map(path: PathBuf, cache_root: PathBuf) -> Result<LoadedMap> {
        fs::create_dir_all(&cache_root).map_err(|e| Error::format(e.to_string()))?;
        let cache = images::temp_cache_dir(&cache_root)?;
        let variant_path = cache.join("map.variant");
        let meta = gcpf::decompress_file(&path, &variant_path, |_, _| {})?;
        let root = variant::decode_file(&variant_path)?;
        if !matches!(root, Value::Dictionary(_)) {
            return Err(Error::format("map root is not a Godot Dictionary"));
        }
        let prepared = images::prepare(&root);
        Ok(LoadedMap {
            path,
            work_dir: cache,
            text: godot_text::format(&prepared.editable),
            images: prepared.images,
            binary_blobs: prepared.binary_blobs,
            uncompressed_size: meta.uncompressed_size,
        })
    }

    fn begin_dialog(&mut self, action: DialogAction, ctx: &egui::Context) {
        if self.pending_dialog.is_some() || self.map_load.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let ctx = ctx.clone();
        self.pending_dialog = Some(receiver);
        self.status = "File chooser is open…".into();
        std::thread::spawn(move || {
            let path = match &action {
                DialogAction::OpenMap => rfd::FileDialog::new()
                    .add_filter("Wonderdraft map", &["wonderdraft_map"])
                    .pick_file(),
                DialogAction::SaveMap { file_name } => rfd::FileDialog::new()
                    .set_file_name(file_name)
                    .add_filter("Wonderdraft map", &["wonderdraft_map"])
                    .save_file(),
                DialogAction::ExportSvg { file_name } => rfd::FileDialog::new()
                    .set_file_name(file_name)
                    .add_filter("SVG", &["svg"])
                    .save_file(),
                DialogAction::ImportSvg => rfd::FileDialog::new()
                    .add_filter("SVG", &["svg"])
                    .pick_file(),
                DialogAction::ExportImage { file_name, .. } => rfd::FileDialog::new()
                    .set_file_name(file_name)
                    .add_filter("PNG", &["png"])
                    .save_file(),
                DialogAction::ExportAllImages
                | DialogAction::CustomAssets
                | DialogAction::DefaultSprites
                | DialogAction::CacheFolder => rfd::FileDialog::new().pick_folder(),
                DialogAction::ReplaceImage { .. } => rfd::FileDialog::new()
                    .add_filter("Images", &["png", "jpg", "jpeg", "webp"])
                    .pick_file(),
            };
            let _ = sender.send(DialogSelection { action, path });
            ctx.request_repaint();
        });
    }

    fn begin_map_load(&mut self, path: PathBuf, ctx: &egui::Context) {
        if self.map_load.is_some() {
            self.status = "A map is already loading".into();
            return;
        }
        if !is_wonderdraft_map(&path) {
            self.fail(
                "Unsupported file",
                format!(
                    "Drop or select a .wonderdraft_map file, not {}",
                    path.display()
                ),
            );
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let ctx = ctx.clone();
        let cache_root = self.cache_dir.clone();
        self.map_load = Some(receiver);
        self.status = format!(
            "Loading {}…",
            path.file_name().unwrap_or_default().to_string_lossy()
        );
        std::thread::spawn(move || {
            let result = Self::load_map(path, cache_root);
            let _ = sender.send(result);
            ctx.request_repaint();
        });
    }

    fn finish_map_load(&mut self, loaded: LoadedMap, ctx: &egui::Context) -> Result<()> {
        if let Some(old) = self.work_dir.replace(loaded.work_dir)
            && old != self.cache_dir
        {
            let _ = fs::remove_dir_all(old);
        }
        self.images = loaded.images;
        self.binary_blobs = loaded.binary_blobs;
        self.text = loaded.text;
        self.root_path = Some(loaded.path.clone());
        self.selected = 0;
        self.preview = None;
        self.status = format!(
            "Loaded {} — {} disk-backed images, {:.2} GiB cache",
            loaded
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy(),
            self.images.len(),
            loaded.uncompressed_size as f64 / 1024f64.powi(3)
        );
        self.update_preview(ctx)?;
        Ok(())
    }

    fn poll_background_work(&mut self, ctx: &egui::Context) {
        if let Some(receiver) = self.pending_dialog.take() {
            match receiver.try_recv() {
                Ok(DialogSelection { action, path }) => {
                    if let Some(path) = path {
                        self.handle_dialog_selection(action, path, ctx);
                    } else {
                        self.status = "File selection cancelled".into();
                    }
                }
                Err(TryRecvError::Empty) => self.pending_dialog = Some(receiver),
                Err(TryRecvError::Disconnected) => {
                    self.fail("File chooser failed", "File picker stopped unexpectedly")
                }
            }
        }
        if let Some(receiver) = self.map_load.take() {
            match receiver.try_recv() {
                Ok(Ok(loaded)) => {
                    if let Err(error) = self.finish_map_load(loaded, ctx) {
                        self.fail("Open failed", error);
                    }
                }
                Ok(Err(error)) => self.fail("Open failed", error),
                Err(TryRecvError::Empty) => self.map_load = Some(receiver),
                Err(TryRecvError::Disconnected) => {
                    self.fail("Open failed", "Map loader stopped unexpectedly")
                }
            }
        }
    }

    fn handle_dialog_selection(
        &mut self,
        action: DialogAction,
        path: PathBuf,
        ctx: &egui::Context,
    ) {
        let result = match action {
            DialogAction::OpenMap => {
                self.begin_map_load(path, ctx);
                return;
            }
            DialogAction::SaveMap { .. } => self.save_to(&path),
            DialogAction::ExportSvg { .. } => self.export_svg_to(&path),
            DialogAction::ImportSvg => self.import_svg_from(&path),
            DialogAction::ExportImage { index, .. } => self.export_image_to(index, &path),
            DialogAction::ExportAllImages => self.export_all_to(&path),
            DialogAction::ReplaceImage { index } => self.replace_image_from(index, &path, ctx),
            DialogAction::CustomAssets => {
                self.settings.custom_asset_folder = path.to_string_lossy().into_owned();
                return;
            }
            DialogAction::DefaultSprites => {
                self.settings.default_asset_folder = path.to_string_lossy().into_owned();
                return;
            }
            DialogAction::CacheFolder => {
                self.settings.cache_folder = path.to_string_lossy().into_owned();
                return;
            }
        };
        if let Err(error) = result {
            self.fail("Operation failed", error);
        }
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
    fn save_to(&mut self, path: &Path) -> Result<()> {
        let editable = self.parse()?;
        let root = images::restore_external(&editable, &self.images, &self.binary_blobs);
        self.status = "Streaming map to GCPF output…".into();
        let size = variant::save_map(&root, path, 4096, self.compressed)?;
        if self.verify {
            let verify_dir = images::temp_cache_dir(&settings::default_cache())?;
            let raw = verify_dir.join("verify.variant");
            gcpf::decompress_file(path, &raw, |_, _| {})?;
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
    fn export_svg_to(&mut self, path: &Path) -> Result<()> {
        let root = self.parse()?;
        let options = svg::ExportOptions {
            background: self.export_background,
            paths: self.export_paths,
            symbols: self.export_symbols,
            labels: self.export_labels,
            embed_background: self.embed_bg,
        };
        let s = svg::export(
            &root,
            &self.images,
            path,
            &Resolver::new(&self.settings),
            options,
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
    fn import_svg_from(&mut self, path: &Path) -> Result<()> {
        let mut root = self.parse()?;
        let s = svg::import(&mut root, path, &Resolver::new(&self.settings))?;
        self.text = godot_text::format(&root);
        self.status = format!(
            "Imported SVG: {} labels, {} symbols, {} paths",
            s.labels, s.symbols, s.paths
        );
        dialog("SVG imported", &self.status, false);
        Ok(())
    }
    fn export_image_to(&mut self, index: usize, path: &Path) -> Result<()> {
        let Some((key, value)) = self.images.get(index) else {
            return Ok(());
        };
        let info = image_info(value).ok_or_else(|| Error::format("invalid embedded image"))?;
        images::export_png(path, &info)?;
        self.status = format!("Exported {key}");
        Ok(())
    }
    fn export_all_to(&mut self, dir: &Path) -> Result<()> {
        for (key, value) in &self.images {
            if let Some(info) = image_info(value) {
                let leaf = key.split('.').next_back().unwrap_or("image");
                images::export_png(&dir.join(format!(".{leaf}.png")), &info)?;
            }
        }
        self.status = format!("Exported {} PNG files", self.images.len());
        Ok(())
    }
    fn replace_image_from(&mut self, index: usize, path: &Path, ctx: &egui::Context) -> Result<()> {
        if self.images.get(index).is_none() {
            return Ok(());
        }
        let old = &self.images[index].1;
        let replacement_cache = self.work_dir.as_deref().unwrap_or(&self.cache_dir);
        let value = images::import_image(path, old, replacement_cache)?;
        self.images[index].1 = value;
        self.selected = index;
        self.preview = None;
        self.update_preview(ctx)?;
        self.status = format!("Replaced {}", self.images[index].0);
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
        self.poll_background_work(&ctx);

        let dropped_paths = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .filter_map(|file| file.path.clone())
                .collect::<Vec<_>>()
        });
        if self.pending_dialog.is_some() || self.map_load.is_some() {
            if !dropped_paths.is_empty() {
                self.status =
                    "Finish the current file operation before dropping another map".into();
            }
        } else if let Some(path) = dropped_paths
            .into_iter()
            .find(|path| is_wonderdraft_map(path))
        {
            self.begin_map_load(path, &ctx);
        } else if ctx.input(|input| !input.raw.dropped_files.is_empty()) {
            self.fail(
                "Unsupported file",
                "Drop a .wonderdraft_map file to open it",
            );
        }

        let busy = self.pending_dialog.is_some() || self.map_load.is_some();
        egui::Panel::top("toolbar").show(root_ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(!busy, egui::Button::new("Open map"))
                    .clicked()
                {
                    self.begin_dialog(DialogAction::OpenMap, &ctx);
                }
                if ui.button("Validate text").clicked() {
                    self.validate()
                }
                if ui.button("Save map as…").clicked() {
                    let file_name = self
                        .root_path
                        .as_ref()
                        .and_then(|path| path.file_stem())
                        .map(|stem| format!("{}_edited.wonderdraft_map", stem.to_string_lossy()))
                        .unwrap_or_else(|| "edited.wonderdraft_map".into());
                    self.begin_dialog(DialogAction::SaveMap { file_name }, &ctx);
                }
                if ui.button("Export SVG…").clicked() {
                    let file_name = self
                        .root_path
                        .as_ref()
                        .and_then(|path| path.file_stem())
                        .map(|stem| format!("{}.svg", stem.to_string_lossy()))
                        .unwrap_or_else(|| "wonderdraft_map.svg".into());
                    self.begin_dialog(DialogAction::ExportSvg { file_name }, &ctx);
                }
                if ui.button("Import SVG…").clicked() {
                    self.begin_dialog(DialogAction::ImportSvg, &ctx);
                }
                if ui.button("Export all PNGs").clicked() {
                    self.begin_dialog(DialogAction::ExportAllImages, &ctx);
                }
                if ui.button("Asset folders…").clicked() {
                    self.settings_open = true;
                }
                ui.checkbox(&mut self.compressed, "Compress saved map");
                ui.checkbox(&mut self.verify, "Verify save");
                ui.checkbox(&mut self.embed_bg, "Embed mask in SVG");
            });
            if busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(if self.pending_dialog.is_some() {
                        "File chooser is open — the editor remains responsive"
                    } else {
                        "Loading map in the background"
                    });
                });
            }
            ui.horizontal_wrapped(|ui| {
                ui.label("SVG export layers:");
                ui.checkbox(&mut self.export_background, "Background mask");
                ui.checkbox(&mut self.export_paths, "Roads / paths");
                ui.checkbox(&mut self.export_symbols, "Symbols");
                ui.checkbox(&mut self.export_labels, "Labels");
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
                        if let Some((key, _)) = self.images.get(self.selected) {
                            let leaf = key.split('.').next_back().unwrap_or("image");
                            self.begin_dialog(
                                DialogAction::ExportImage {
                                    index: self.selected,
                                    file_name: format!(".{leaf}.png"),
                                },
                                &ctx,
                            );
                        }
                    }
                    if ui.button("Replace PNG").clicked() {
                        if self.images.get(self.selected).is_some() {
                            self.begin_dialog(
                                DialogAction::ReplaceImage {
                                    index: self.selected,
                                },
                                &ctx,
                            );
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
                        self.begin_dialog(DialogAction::CustomAssets, &ctx);
                    }
                    ui.label("Default sprites folder");
                    ui.text_edit_singleline(&mut self.settings.default_asset_folder);
                    if ui.button("Browse sprites…").clicked() {
                        self.begin_dialog(DialogAction::DefaultSprites, &ctx);
                    }
                    ui.label("Disk cache folder");
                    ui.text_edit_singleline(&mut self.settings.cache_folder);
                    if ui.button("Browse cache…").clicked() {
                        self.begin_dialog(DialogAction::CacheFolder, &ctx);
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

        if ctx.input(|input| !input.raw.hovered_files.is_empty()) {
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("file-drop-overlay"),
            ));
            let rect = ctx.content_rect();
            painter.rect_filled(rect, 8.0, egui::Color32::from_black_alpha(190));
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Drop a .wonderdraft_map file to open it",
                egui::FontId::proportional(28.0),
                egui::Color32::WHITE,
            );
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        if let Some(work_dir) = self.work_dir.take() {
            let _ = fs::remove_dir_all(work_dir);
        }
    }
}

fn is_wonderdraft_map(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("wonderdraft_map"))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_loader_accepts_dropped_map_paths() {
        let base = std::env::temp_dir().join(format!(
            "wonderdraft-background-load-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let map = base.join("DROP.WONDERDRAFT_MAP");
        let root = Value::Dictionary(vec![(Value::String("map_width".into()), Value::Int(512))]);
        variant::save_map(&root, &map, 64, true).unwrap();
        assert!(is_wonderdraft_map(&map));

        let cache = base.join("cache");
        let loaded = std::thread::spawn(move || App::load_map(map, cache))
            .join()
            .unwrap()
            .unwrap();
        assert!(loaded.text.contains("map_width"));
        assert_eq!(
            loaded.uncompressed_size as u64,
            fs::metadata(loaded.work_dir.join("map.variant"))
                .unwrap()
                .len()
        );
        let _ = fs::remove_dir_all(base);
    }
}

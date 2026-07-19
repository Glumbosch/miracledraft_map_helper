#![allow(clippy::collapsible_if)]

use eframe::egui;
use miracledraft_map_helper::{
    Error, Result, Value,
    assets::Resolver,
    fonts, gcpf, godot_text,
    images::{self, BinaryBlobs, Images},
    pck,
    settings::{self, Settings, WonderdraftConfig},
    svg, svg_render,
    value::image_info,
    variant,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
};

const APP_NAME: &str = "Miracledraft Map Helper";
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const BUILD_TIME: &str = env!("MIRACLEDRAFT_BUILD_TIME");
const DRAW_MODE_ICONS: &[(&str, &str, &[u8])] = &[
    (
        "normal",
        "rubber-stamp.svg",
        include_bytes!("../app_assets/icons/rubber-stamp.svg"),
    ),
    (
        "sample_color",
        "brush.svg",
        include_bytes!("../app_assets/icons/brush.svg"),
    ),
    (
        "custom_colors",
        "palette.svg",
        include_bytes!("../app_assets/icons/palette.svg"),
    ),
];

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
    embed_boxes: bool,
    embed_symbols: bool,
    export_territories: bool,
    export_background: bool,
    export_boxes: bool,
    export_paths: bool,
    export_symbols: bool,
    export_labels: bool,
    settings: Settings,
    settings_backup: Option<Settings>,
    wonderdraft_config: WonderdraftConfig,
    settings_open: bool,
    setup_wizard_open: bool,
    setup_step: usize,
    cache_size_bytes: Option<u64>,
    pending_dialog: Option<Receiver<DialogSelection>>,
    dialog_directory: Option<PathBuf>,
    overwrite_prompt: Option<PathBuf>,
    map_load: Option<Receiver<Result<LoadedMap>>>,
    pck_extraction: Option<Receiver<Result<pck::Extraction>>>,
    font_installation: Option<Receiver<Result<fonts::Installation>>>,
    install_core_fonts: bool,
    install_custom_fonts: bool,
    choose_fonts_individually: bool,
    font_choices: Vec<FontChoice>,
    font_list_loaded: bool,
    font_discovery_error: Option<String>,
    search_open: bool,
    search_query: String,
    search_from: usize,
    focus_search: bool,
    pending_text_selection: Option<(usize, usize)>,
    section_choice: usize,
    csv_importer: Option<CsvImporterWindow>,
    svg_renderer: Option<SvgRendererWindow>,
    svg_render_job: Option<Receiver<(PathBuf, Result<svg_render::RenderSummary>)>>,
}

struct SvgRendererWindow {
    document: svg_render::Document,
    rows: Vec<svg_render::ClassSettings>,
    selected: usize,
    assets: Vec<miracledraft_map_helper::assets::AssetInfo>,
    path_styles: Vec<PathStyleInfo>,
    preview: Option<egui::TextureHandle>,
    preview_texture: String,
    preview_background: usize,
    created_map: Option<PathBuf>,
    gallery_open: bool,
    gallery_scale: f32,
    gallery_background: usize,
    gallery_search: String,
    gallery_textures: HashMap<String, egui::TextureHandle>,
    path_style_textures: HashMap<String, egui::TextureHandle>,
    draw_mode_icons: HashMap<String, egui::TextureHandle>,
    focused_last_frame: bool,
    render_settings_open: bool,
    render_settings: RenderImportSettings,
    full_preview_open: bool,
}

#[derive(Clone, Serialize, Deserialize)]
struct RenderImportSettings {
    map_width: u32,
    map_height: u32,
    selected_layers: Vec<bool>,
    source_x: f64,
    source_y: f64,
    source_width: f64,
    source_height: f64,
    preview_layer: Option<usize>,
    #[serde(skip)]
    selection_drag: Option<SelectionDrag>,
}

#[derive(Serialize, Deserialize)]
struct SavedRenderSettings {
    version: u32,
    rows: Vec<svg_render::ClassSettings>,
    render_settings: RenderImportSettings,
}

#[derive(Clone, Copy)]
struct SelectionDrag {
    handle: SelectionHandle,
    pointer: (f64, f64),
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[derive(Clone, Copy)]
enum SelectionHandle {
    Move,
    NorthWest,
    NorthEast,
    SouthWest,
    SouthEast,
}

struct CsvImporterWindow {
    path: PathBuf,
    encoding: svg_render::TableEncoding,
    delimiter: svg_render::TableDelimiter,
    table: svg_render::TableData,
    columns: svg_render::TableColumns,
    relative_after_first: bool,
    source_x: f64,
    source_y: f64,
    source_width: f64,
    source_height: f64,
    map_width: u32,
    map_height: u32,
    error: Option<String>,
}

#[derive(Clone)]
struct PathStyleInfo {
    texture: String,
    path: PathBuf,
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
    RenderSvg,
    RenderCsv,
    LoadRenderSettings,
    SaveRenderSettings { file_name: String },
    SaveRenderedMap { file_name: String },
    ExportMapData { file_name: String },
    ExportImage { index: usize, file_name: String },
    ExportAllImages,
    ReplaceImage { index: usize },
    CustomAssets,
    DefaultSprites,
    CacheFolder,
    WonderdraftFolder,
    WonderdraftPck,
}

struct DialogSelection {
    action: DialogAction,
    path: Option<PathBuf>,
}

struct FontChoice {
    candidate: fonts::Candidate,
    selected: bool,
}

impl Default for App {
    fn default() -> Self {
        let mut settings = settings::load();
        let mut repaired_core_assets = false;
        if !configured_directory(&settings.default_asset_folder)
            && let Some(core_sprites) = miracledraft_map_helper::assets::bundled_core_sprites()
        {
            settings.default_asset_folder = core_sprites.to_string_lossy().into_owned();
            repaired_core_assets = true;
        }
        let wonderdraft_folder = settings::find_wonderdraft_folder(&settings.wonderdraft_folder);
        let setup_wizard_open = !settings.setup_completed;
        let wonderdraft_config = wonderdraft_folder
            .as_deref()
            .and_then(|folder| {
                settings.wonderdraft_folder = folder.to_string_lossy().into_owned();
                settings::read_wonderdraft_config(folder).ok()
            })
            .unwrap_or_default();
        if settings.auto_locate_custom_assets
            && let Some(folder) = wonderdraft_folder.as_deref()
        {
            settings.custom_asset_folder =
                settings::custom_assets_folder(folder, &wonderdraft_config)
                    .to_string_lossy()
                    .into_owned();
        }
        let cache_dir = PathBuf::from(&settings.cache_folder);
        let cache_size_bytes = settings::directory_size(&cache_dir).ok();
        if repaired_core_assets {
            let _ = settings::save(&settings);
        }
        Self {
            text: String::new(),
            root_path: None,
            cache_dir,
            work_dir: None,
            images: Vec::new(),
            binary_blobs: Vec::new(),
            selected: 0,
            preview: None,
            status: "Open a .wonderdraft_map file".into(),
            compressed: true,
            verify: true,
            embed_bg: false,
            embed_boxes: false,
            embed_symbols: false,
            export_territories: true,
            export_background: true,
            export_boxes: true,
            export_paths: true,
            export_symbols: true,
            export_labels: true,
            settings,
            settings_backup: None,
            wonderdraft_config,
            settings_open: false,
            setup_wizard_open,
            setup_step: 0,
            cache_size_bytes,
            pending_dialog: None,
            dialog_directory: None,
            overwrite_prompt: None,
            map_load: None,
            pck_extraction: None,
            font_installation: None,
            install_core_fonts: true,
            install_custom_fonts: true,
            choose_fonts_individually: false,
            font_choices: Vec::new(),
            font_list_loaded: false,
            font_discovery_error: None,
            search_open: false,
            search_query: String::new(),
            search_from: 0,
            focus_search: false,
            pending_text_selection: None,
            section_choice: 0,
            csv_importer: None,
            svg_renderer: None,
            svg_render_job: None,
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
        let open_directory = self
            .dialog_directory
            .take()
            .or_else(|| self.wonderdraft_config.last_directory.clone());
        let wonderdraft_directory = if self.settings.wonderdraft_folder.trim().is_empty() {
            settings::default_wonderdraft_folder()
        } else {
            PathBuf::from(&self.settings.wonderdraft_folder)
        };
        let wonderdraft_pack_directory = pck::default_pack_directory_hint();
        self.pending_dialog = Some(receiver);
        self.status = "File chooser is open…".into();
        std::thread::spawn(move || {
            let path = match &action {
                DialogAction::OpenMap => {
                    let mut picker =
                        rfd::FileDialog::new().add_filter("Wonderdraft map", &["wonderdraft_map"]);
                    if let Some(directory) = open_directory {
                        picker = picker.set_directory(directory);
                    }
                    picker.pick_file()
                }
                DialogAction::SaveMap { file_name } => rfd::FileDialog::new()
                    .set_directory(open_directory.clone().unwrap_or_default())
                    .set_file_name(file_name)
                    .add_filter("Wonderdraft map", &["wonderdraft_map"])
                    .save_file(),
                DialogAction::ExportSvg { file_name } => rfd::FileDialog::new()
                    .set_directory(open_directory.clone().unwrap_or_default())
                    .set_file_name(file_name)
                    .add_filter("SVG", &["svg"])
                    .save_file(),
                DialogAction::ImportSvg => rfd::FileDialog::new()
                    .set_directory(open_directory.clone().unwrap_or_default())
                    .add_filter("SVG", &["svg"])
                    .pick_file(),
                DialogAction::RenderSvg => rfd::FileDialog::new()
                    .set_directory(open_directory.clone().unwrap_or_default())
                    .add_filter("SVG", &["svg"])
                    .pick_file(),
                DialogAction::RenderCsv => rfd::FileDialog::new()
                    .set_directory(open_directory.clone().unwrap_or_default())
                    .add_filter("Delimited table", &["csv", "tsv", "txt"])
                    .pick_file(),
                DialogAction::LoadRenderSettings => rfd::FileDialog::new()
                    .set_directory(open_directory.clone().unwrap_or_default())
                    .add_filter("Render settings JSON", &["json"])
                    .pick_file(),
                DialogAction::SaveRenderSettings { file_name } => rfd::FileDialog::new()
                    .set_directory(open_directory.clone().unwrap_or_default())
                    .set_file_name(file_name)
                    .add_filter("Render settings JSON", &["json"])
                    .save_file(),
                DialogAction::SaveRenderedMap { file_name } => rfd::FileDialog::new()
                    .set_directory(open_directory.clone().unwrap_or_default())
                    .set_file_name(file_name)
                    .add_filter("Wonderdraft map", &["wonderdraft_map"])
                    .save_file(),
                DialogAction::ExportMapData { file_name } => rfd::FileDialog::new()
                    .set_directory(open_directory.clone().unwrap_or_default())
                    .set_file_name(file_name)
                    .add_filter("Text", &["txt"])
                    .save_file(),
                DialogAction::ExportImage { file_name, .. } => rfd::FileDialog::new()
                    .set_file_name(file_name)
                    .add_filter("PNG", &["png"])
                    .save_file(),
                DialogAction::ExportAllImages
                | DialogAction::CustomAssets
                | DialogAction::DefaultSprites
                | DialogAction::CacheFolder => rfd::FileDialog::new().pick_folder(),
                DialogAction::WonderdraftFolder => rfd::FileDialog::new()
                    .set_directory(wonderdraft_directory)
                    .pick_folder(),
                DialogAction::WonderdraftPck => {
                    let mut picker = rfd::FileDialog::new()
                        .set_file_name("Wonderdraft.pck")
                        .add_filter("Wonderdraft.pck", &["pck"]);
                    if let Some(directory) = wonderdraft_pack_directory {
                        picker = picker.set_directory(directory);
                    }
                    picker.pick_file()
                }
                DialogAction::ReplaceImage { .. } => rfd::FileDialog::new()
                    .set_directory(open_directory.clone().unwrap_or_default())
                    .add_filter("Images", &["png", "jpg", "jpeg", "webp"])
                    .pick_file(),
            };
            let _ = sender.send(DialogSelection { action, path });
            ctx.request_repaint();
        });
    }

    fn begin_dialog_in_directory(
        &mut self,
        action: DialogAction,
        directory: PathBuf,
        ctx: &egui::Context,
    ) {
        self.dialog_directory = Some(directory);
        self.begin_dialog(action, ctx);
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
        settings::remember_recent_map(&mut self.settings, &loaded.path);
        let _ = settings::save(&self.settings);
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
        if let Some(receiver) = self.pck_extraction.take() {
            match receiver.try_recv() {
                Ok(Ok(extracted)) => {
                    self.settings.default_asset_folder =
                        extracted.sprites_dir.to_string_lossy().into_owned();
                    self.font_list_loaded = false;
                    match settings::save(&self.settings) {
                        Ok(()) => {
                            self.status = format!(
                                "Extracted {} files ({} images renamed) and configured core sprites",
                                extracted.file_count, extracted.renamed_images
                            );
                            dialog(
                                "Core assets extracted",
                                &format!(
                                    "{}\n\nDefault sprites folder:\n{}",
                                    self.status,
                                    extracted.sprites_dir.display()
                                ),
                                false,
                            );
                        }
                        Err(error) => {
                            self.fail("Assets extracted, but settings could not be saved", error)
                        }
                    }
                }
                Ok(Err(error)) => self.fail("Core asset extraction failed", error),
                Err(TryRecvError::Empty) => self.pck_extraction = Some(receiver),
                Err(TryRecvError::Disconnected) => self.fail(
                    "Core asset extraction failed",
                    "Extractor stopped unexpectedly",
                ),
            }
        }
        if let Some(receiver) = self.font_installation.take() {
            match receiver.try_recv() {
                Ok(Ok(installed)) => {
                    self.status = format!(
                        "Wonderdraft fonts: {} installed, {} already present, {} conflicts",
                        installed.installed, installed.already_installed, installed.conflicts
                    );
                    let mut message = format!(
                        "{} font files found\n{} newly installed\n{} already installed\n{} same-name conflicts left unchanged\n\nUser font folder:\n{}",
                        installed.discovered,
                        installed.installed,
                        installed.already_installed,
                        installed.conflicts,
                        installed.destination.display()
                    );
                    if !installed.warnings.is_empty() {
                        message.push_str("\n\nNotes:\n");
                        message.push_str(&installed.warnings.join("\n"));
                    }
                    dialog("Wonderdraft fonts installed", &message, false);
                }
                Ok(Err(error)) => self.fail("Wonderdraft font installation failed", error),
                Err(TryRecvError::Empty) => self.font_installation = Some(receiver),
                Err(TryRecvError::Disconnected) => self.fail(
                    "Wonderdraft font installation failed",
                    "Font installer stopped unexpectedly",
                ),
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
            DialogAction::RenderSvg => self.open_svg_renderer(path, ctx),
            DialogAction::RenderCsv => self.open_csv_importer(path),
            DialogAction::LoadRenderSettings => self.load_svg_render_settings(&path),
            DialogAction::SaveRenderSettings { .. } => self.save_svg_render_settings(&path),
            DialogAction::SaveRenderedMap { .. } => {
                self.begin_svg_render(path, ctx);
                return;
            }
            DialogAction::ExportMapData { .. } => self.export_map_data_to(&path),
            DialogAction::ExportImage { index, .. } => self.export_image_to(index, &path),
            DialogAction::ExportAllImages => self.export_all_to(&path),
            DialogAction::ReplaceImage { index } => self.replace_image_from(index, &path, ctx),
            DialogAction::CustomAssets => {
                self.settings.custom_asset_folder = path.to_string_lossy().into_owned();
                self.settings.auto_locate_custom_assets = false;
                self.font_list_loaded = false;
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
            DialogAction::WonderdraftFolder => {
                self.settings.wonderdraft_folder = path.to_string_lossy().into_owned();
                match self.reload_wonderdraft_config() {
                    Ok(()) => {
                        if !self.settings_open {
                            if let Err(error) = settings::save(&self.settings) {
                                self.fail(
                                    "Wonderdraft folder found, but settings could not be saved",
                                    error,
                                );
                                return;
                            }
                        }
                        self.status = "Wonderdraft config.ini loaded".into();
                    }
                    Err(error) => self.fail("Wonderdraft config.ini was not found", error),
                }
                return;
            }
            DialogAction::WonderdraftPck => {
                self.begin_pck_extraction(path, ctx);
                return;
            }
        };
        if let Err(error) = result {
            self.fail("Operation failed", error);
        }
    }
    fn reload_wonderdraft_config(&mut self) -> Result<()> {
        let folder = settings::find_wonderdraft_folder(&self.settings.wonderdraft_folder)
            .ok_or_else(|| {
                Error::format(format!(
                    "Choose the Wonderdraft user-data folder containing config.ini. The current setting is: {}",
                    empty(&self.settings.wonderdraft_folder)
                ))
            })?;
        let config = settings::read_wonderdraft_config(&folder)?;
        self.settings.wonderdraft_folder = folder.to_string_lossy().into_owned();
        if self.settings.auto_locate_custom_assets {
            self.settings.custom_asset_folder = settings::custom_assets_folder(&folder, &config)
                .to_string_lossy()
                .into_owned();
            self.font_list_loaded = false;
        }
        self.wonderdraft_config = config;
        Ok(())
    }
    fn refresh_cache_size(&mut self) {
        self.cache_size_bytes = settings::directory_size(&self.cache_dir).ok();
    }
    fn clear_cache(&mut self) {
        match settings::clear_cache(&self.cache_dir, self.work_dir.as_deref()) {
            Ok(()) => {
                self.refresh_cache_size();
                self.status = if self.work_dir.is_some() {
                    "Cleared stale cache data; the open map's active cache was preserved".into()
                } else {
                    "Cache cleared".into()
                };
            }
            Err(error) => self.fail("Could not clear cache", error),
        }
    }
    fn begin_core_asset_setup(&mut self, ctx: &egui::Context) {
        if let Some(pack) = pck::find_default_pack() {
            self.begin_pck_extraction(pack, ctx);
        } else {
            self.status =
                "Wonderdraft.pck was not found in a default location; choose it now…".into();
            self.begin_dialog(DialogAction::WonderdraftPck, ctx);
        }
    }
    fn begin_pck_extraction(&mut self, pack: PathBuf, ctx: &egui::Context) {
        if self.pck_extraction.is_some() {
            return;
        }
        let output = pck::default_output_dir();
        let (sender, receiver) = mpsc::channel();
        let repaint = ctx.clone();
        self.pck_extraction = Some(receiver);
        self.status = format!("Extracting {} to {}…", pack.display(), output.display());
        std::thread::spawn(move || {
            let result = pck::extract(&pack, &output);
            let _ = sender.send(result);
            repaint.request_repaint();
        });
    }
    fn refresh_font_choices(&mut self) {
        let core = fonts::source_dir();
        let custom = self.settings.custom_asset_folder.trim();
        let custom = (!custom.is_empty()).then(|| PathBuf::from(custom));
        let previous = std::mem::take(&mut self.font_choices);
        match fonts::discover(Some(&core), custom.as_deref()) {
            Ok(candidates) => {
                let mapping_result = fonts::update_name_mapping(&candidates);
                self.font_choices = candidates
                    .into_iter()
                    .map(|candidate| {
                        let selected = previous
                            .iter()
                            .find(|choice| choice.candidate.path == candidate.path)
                            .map(|choice| choice.selected)
                            .unwrap_or(true);
                        FontChoice {
                            candidate,
                            selected,
                        }
                    })
                    .collect();
                self.font_discovery_error = mapping_result.err().map(|error| error.to_string());
            }
            Err(error) => {
                self.font_discovery_error = Some(error.to_string());
            }
        }
        self.font_list_loaded = true;
    }
    fn selected_font_paths(&self) -> Vec<PathBuf> {
        self.font_choices
            .iter()
            .filter(|choice| match choice.candidate.origin {
                fonts::Origin::Core => self.install_core_fonts,
                fonts::Origin::Custom => self.install_custom_fonts,
            })
            .filter(|choice| !self.choose_fonts_individually || choice.selected)
            .map(|choice| choice.candidate.path.clone())
            .collect()
    }
    fn begin_font_installation(&mut self, ctx: &egui::Context) {
        if self.font_installation.is_some() {
            return;
        }
        if !self.font_list_loaded {
            self.refresh_font_choices();
        }
        let selected = self.selected_font_paths();
        if selected.is_empty() {
            self.status = "No fonts selected; choose a font source or skip this step".into();
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let repaint = ctx.clone();
        self.font_installation = Some(receiver);
        self.status = format!("Installing {} selected Wonderdraft fonts…", selected.len());
        std::thread::spawn(move || {
            let result = fonts::install_selected(&selected);
            let _ = sender.send(result);
            repaint.request_repaint();
        });
    }
    fn complete_setup(&mut self) -> Result<()> {
        if self.settings.cache_folder.trim().is_empty() {
            self.settings.cache_folder = settings::default_cache().to_string_lossy().into_owned();
        }
        let cache = PathBuf::from(self.settings.cache_folder.trim());
        fs::create_dir_all(&cache).map_err(|error| Error::format(error.to_string()))?;

        if settings::find_wonderdraft_folder(&self.settings.wonderdraft_folder).is_some() {
            self.reload_wonderdraft_config()?;
        }
        let indexed = Resolver::new(&self.settings)
            .rebuild_symbol_database()?
            .len();
        self.settings.setup_completed = true;
        settings::save(&self.settings)?;
        self.cache_dir = cache;
        self.refresh_cache_size();
        self.status = format!(
            "Setup complete — symbol database contains {indexed} assets; open or drop a .wonderdraft_map file"
        );
        Ok(())
    }
    fn show_setup_wizard(&mut self, ctx: &egui::Context) {
        let mut open = self.setup_wizard_open;
        let mut finish = false;
        let mut skip_fonts = false;
        egui::Window::new("Miracledraft Map Helper setup")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(620.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(format!("Step {} of 5", self.setup_step + 1));
                ui.separator();
                match self.setup_step {
                    0 => {
                        ui.heading("Welcome");
                        ui.label(
                            "This wizard connects the editor to Wonderdraft and prepares its core assets for map and SVG editing.",
                        );
                        ui.add_space(8.0);
                        ui.label("The wizard will help you:");
                        ui.label("• locate Wonderdraft's user-data folder and custom assets");
                        ui.label("• locate and extract Wonderdraft.pck");
                        ui.label("• optionally install the extracted Wonderdraft fonts");
                        ui.label("• choose a writable disk-cache folder");
                        ui.add_space(8.0);
                        ui.small("No maps are changed during setup. Core files are only extracted after you choose that action.");
                    }
                    1 => {
                        ui.heading("Wonderdraft user data");
                        ui.label("This folder contains config.ini. It supplies recent maps and the custom-assets location.");
                        ui.add_space(6.0);
                        let configured = settings::find_wonderdraft_folder(
                            &self.settings.wonderdraft_folder,
                        );
                        if let Some(folder) = configured {
                            ui.colored_label(
                                egui::Color32::from_rgb(70, 170, 90),
                                format!("Found: {}", folder.display()),
                            );
                        } else {
                            ui.colored_label(
                                egui::Color32::YELLOW,
                                "Not found. Choose the folder containing config.ini, or continue without Wonderdraft integration.",
                            );
                        }
                        ui.horizontal(|ui| {
                            let response = ui.add(
                                egui::TextEdit::singleline(
                                    &mut self.settings.wonderdraft_folder,
                                )
                                .desired_width(470.0),
                            );
                            if response.changed() {
                                self.font_list_loaded = false;
                            }
                            if ui
                                .add_enabled(
                                    self.pending_dialog.is_none(),
                                    egui::Button::new("Browse…"),
                                )
                                .clicked()
                            {
                                self.begin_dialog(DialogAction::WonderdraftFolder, ctx);
                            }
                        });
                        ui.checkbox(
                            &mut self.settings.auto_locate_custom_assets,
                            "Use config.ini to locate custom assets automatically",
                        );
                        if !self.settings.custom_asset_folder.trim().is_empty() {
                            ui.small(format!(
                                "Custom assets: {}",
                                self.settings.custom_asset_folder
                            ));
                        }
                    }
                    2 => {
                        ui.heading("Wonderdraft core sprites");
                        ui.label("The editor extracts Wonderdraft.pck into wonderdraft_files and configures its sprites folder.");
                        ui.add_space(6.0);
                        if configured_directory(&self.settings.default_asset_folder) {
                            ui.colored_label(
                                egui::Color32::from_rgb(70, 170, 90),
                                format!(
                                    "Core sprites ready: {}",
                                    self.settings.default_asset_folder
                                ),
                            );
                        } else if let Some(pack) = pck::find_default_pack() {
                            ui.colored_label(
                                egui::Color32::from_rgb(70, 170, 90),
                                format!("Found: {}", pack.display()),
                            );
                            if ui
                                .add_enabled(
                                    self.pending_dialog.is_none()
                                        && self.pck_extraction.is_none(),
                                    egui::Button::new("Extract detected Wonderdraft.pck"),
                                )
                                .clicked()
                            {
                                self.begin_pck_extraction(pack, ctx);
                            }
                        } else {
                            ui.colored_label(
                                egui::Color32::YELLOW,
                                "Wonderdraft.pck was not found in a standard installation folder.",
                            );
                        }
                        if self.pck_extraction.is_some() {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label("Extracting core assets in the background…");
                            });
                        }
                        let choose_pck = ui.add_enabled(
                            self.pending_dialog.is_none() && self.pck_extraction.is_none(),
                            egui::Button::new("Choose Wonderdraft.pck…"),
                        );
                        if choose_pck.clicked() {
                            self.begin_dialog(DialogAction::WonderdraftPck, ctx);
                        }
                        if let Some(path) = dropped_path_over(ui, &choose_pck, |path| {
                            is_extension(path, &["pck"])
                        }) {
                            self.begin_pck_extraction(path, ctx);
                        }
                        ui.small(format!(
                            "Extraction destination: {}",
                            pck::default_output_dir().display()
                        ));
                    }
                    3 => {
                        if !self.font_list_loaded {
                            self.refresh_font_choices();
                        }
                        ui.heading("Wonderdraft fonts");
                        ui.label(
                            "Optionally install core fonts, fonts supplied by custom asset packs, or selected fonts only. Administrator access is not required.",
                        );
                        ui.add_space(6.0);
                        match fonts::user_fonts_dir() {
                            Ok(destination) => {
                                ui.label(format!("User font folder: {}", destination.display()));
                            }
                            Err(error) => {
                                ui.colored_label(egui::Color32::YELLOW, error.to_string());
                            }
                        }
                        ui.label(format!(
                            "Editable font-name map: {}",
                            fonts::name_mapping_path().display()
                        ));
                        if ui.button("Rescan fonts and update name map").clicked() {
                            self.refresh_font_choices();
                        }
                        if let Some(error) = &self.font_discovery_error {
                            ui.colored_label(
                                egui::Color32::YELLOW,
                                format!("Font discovery failed: {error}"),
                            );
                        }
                        ui.add_space(6.0);
                        let core_count = self
                            .font_choices
                            .iter()
                            .filter(|choice| choice.candidate.origin == fonts::Origin::Core)
                            .count();
                        let custom_count = self
                            .font_choices
                            .iter()
                            .filter(|choice| choice.candidate.origin == fonts::Origin::Custom)
                            .count();
                        ui.add_enabled_ui(core_count > 0, |ui| {
                            ui.checkbox(
                                &mut self.install_core_fonts,
                                format!("Core asset fonts ({core_count})"),
                            );
                        });
                        ui.add_enabled_ui(custom_count > 0, |ui| {
                            ui.checkbox(
                                &mut self.install_custom_fonts,
                                format!("Custom asset-pack fonts ({custom_count})"),
                            );
                        });
                        if core_count == 0 {
                            ui.small(
                                "No core fonts found in wonderdraft_files/fonts/. Extract Wonderdraft.pck to make them available.",
                            );
                        }
                        if custom_count == 0 {
                            ui.small(
                                "No custom fonts found below folders named fonts in the configured custom-assets directory.",
                            );
                        }
                        ui.checkbox(
                            &mut self.choose_fonts_individually,
                            "Choose fonts individually",
                        );
                        if self.choose_fonts_individually {
                            ui.horizontal(|ui| {
                                if ui.button("Select all enabled fonts").clicked() {
                                    for choice in &mut self.font_choices {
                                        let enabled = match choice.candidate.origin {
                                            fonts::Origin::Core => self.install_core_fonts,
                                            fonts::Origin::Custom => self.install_custom_fonts,
                                        };
                                        if enabled {
                                            choice.selected = true;
                                        }
                                    }
                                }
                                if ui.button("Clear enabled fonts").clicked() {
                                    for choice in &mut self.font_choices {
                                        let enabled = match choice.candidate.origin {
                                            fonts::Origin::Core => self.install_core_fonts,
                                            fonts::Origin::Custom => self.install_custom_fonts,
                                        };
                                        if enabled {
                                            choice.selected = false;
                                        }
                                    }
                                }
                            });
                            egui::ScrollArea::vertical()
                                .id_salt("setup-font-selection")
                                .max_height(190.0)
                                .show(ui, |ui| {
                                    for choice in &mut self.font_choices {
                                        let (enabled, source) = match choice.candidate.origin {
                                            fonts::Origin::Core => {
                                                (self.install_core_fonts, "Core")
                                            }
                                            fonts::Origin::Custom => {
                                                (self.install_custom_fonts, "Custom")
                                            }
                                        };
                                        let file_name = choice
                                            .candidate
                                            .path
                                            .file_name()
                                            .unwrap_or_default()
                                            .to_string_lossy();
                                        ui.add_enabled(
                                            enabled,
                                            egui::Checkbox::new(
                                                &mut choice.selected,
                                                format!("[{source}] {file_name}"),
                                            ),
                                        )
                                        .on_hover_text(choice.candidate.path.display().to_string());
                                    }
                                });
                        }
                        let selected_count = self.selected_font_paths().len();
                        ui.small(format!("{selected_count} fonts selected for installation"));
                        if ui
                            .add_enabled(
                                selected_count > 0
                                    && self.pck_extraction.is_none()
                                    && self.font_installation.is_none(),
                                egui::Button::new(format!(
                                    "Install {selected_count} fonts for this user"
                                )),
                            )
                            .clicked()
                        {
                            self.begin_font_installation(ctx);
                        }
                        if self.font_installation.is_some() {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label("Installing fonts and refreshing the font registry…");
                            });
                        }
                        if ui
                            .add_enabled(
                                self.font_installation.is_none(),
                                egui::Button::new("Skip font installation"),
                            )
                            .clicked()
                        {
                            skip_fonts = true;
                        }
                        ui.small(
                            "Fonts that are already installed are skipped. A different existing font with the same filename is preserved and reported.",
                        );
                    }
                    _ => {
                        ui.heading("Cache and summary");
                        ui.label("Disk cache folder");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.settings.cache_folder)
                                    .desired_width(470.0),
                            );
                            if ui
                                .add_enabled(
                                    self.pending_dialog.is_none(),
                                    egui::Button::new("Browse…"),
                                )
                                .clicked()
                            {
                                self.begin_dialog(DialogAction::CacheFolder, ctx);
                            }
                        });
                        ui.checkbox(
                            &mut self.settings.clear_cache_on_exit,
                            "Clear the cache when the program exits",
                        );
                        ui.add_space(8.0);
                        ui.label(format!(
                            "Wonderdraft config: {}",
                            if settings::find_wonderdraft_folder(
                                &self.settings.wonderdraft_folder
                            )
                            .is_some()
                            {
                                "ready"
                            } else {
                                "not configured (optional)"
                            }
                        ));
                        ui.label(format!(
                            "Custom assets: {}",
                            if configured_directory(&self.settings.custom_asset_folder) {
                                "ready"
                            } else {
                                "not found (optional)"
                            }
                        ));
                        ui.label(format!(
                            "Core sprites: {}",
                            if configured_directory(&self.settings.default_asset_folder) {
                                "ready"
                            } else {
                                "not extracted (can be configured later)"
                            }
                        ));
                        ui.label(format!(
                            "Wonderdraft fonts: {}",
                            if fonts::source_dir().is_dir() {
                                "available to install from the previous step"
                            } else {
                                "not extracted (optional)"
                            }
                        ));
                        ui.small("You can run this wizard again from Settings at any time.");
                    }
                }

                ui.separator();
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(self.setup_step > 0, egui::Button::new("Back"))
                        .clicked()
                    {
                        self.setup_step -= 1;
                    }
                    if self.setup_step < 4 {
                        if ui.button("Next").clicked() {
                            self.setup_step += 1;
                        }
                    } else if ui
                        .add_enabled(
                            self.pending_dialog.is_none()
                                && self.pck_extraction.is_none()
                                && self.font_installation.is_none(),
                            egui::Button::new("Finish setup"),
                        )
                        .clicked()
                    {
                        finish = true;
                    }
                });
            });

        if finish {
            match self.complete_setup() {
                Ok(()) => open = false,
                Err(error) => self.fail("Setup could not be saved", error),
            }
        }
        if skip_fonts {
            self.setup_step = 4;
        }
        self.setup_wizard_open = open;
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
    fn export_map_data_to(&mut self, path: &Path) -> Result<()> {
        fs::write(path, &self.text).map_err(|error| Error::format(error.to_string()))?;
        self.status = format!("Exported map data text to {}", path.display());
        Ok(())
    }
    fn queue_text_selection(&mut self, byte_start: usize, byte_len: usize) {
        self.pending_text_selection = Some((byte_start, byte_start.saturating_add(byte_len)));
    }
    fn jump_to_section(&mut self, marker: &str, label: &str) {
        if let Some(byte_start) = self.text.find(marker) {
            self.queue_text_selection(byte_start, marker.len());
            self.status = format!("Jumped to {label}");
        } else {
            self.status = format!("The loaded map has no {label} section");
        }
    }
    fn find_next(&mut self) {
        if self.search_query.is_empty() {
            self.status = "Enter text to find".into();
            return;
        }
        let mut start = self.search_from.min(self.text.len());
        while !self.text.is_char_boundary(start) {
            start -= 1;
        }
        let found = self.text[start..]
            .find(&self.search_query)
            .map(|offset| start + offset)
            .or_else(|| self.text[..start].find(&self.search_query));
        if let Some(byte_start) = found {
            let query_len = self.search_query.len();
            self.queue_text_selection(byte_start, query_len);
            self.search_from = byte_start.saturating_add(query_len);
            self.status = format!("Found ‘{}’", self.search_query);
        } else {
            self.status = format!("‘{}’ was not found", self.search_query);
        }
    }
    fn remove_off_canvas_symbols(&mut self) {
        let mut root = match self.parse() {
            Ok(root) => root,
            Err(error) => {
                self.fail("Could not inspect symbols", error);
                return;
            }
        };
        let removed = remove_off_canvas_symbols(&mut root, &Resolver::new(&self.settings));
        self.text = godot_text::format(&root);
        self.status = if removed == 1 {
            "Removed 1 completely off-canvas symbol".into()
        } else {
            format!("Removed {removed} completely off-canvas symbols")
        };
    }
    fn save_to(&mut self, path: &Path) -> Result<()> {
        let editable = self.parse()?;
        let root = images::restore_external(&editable, &self.images, &self.binary_blobs);
        self.status = "Streaming map to GCPF output…".into();
        let size = variant::save_map(&root, path, 4096, self.compressed)?;
        if self.verify {
            let verify_dir = images::temp_cache_dir(&self.cache_dir)?;
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
            boxes: self.export_boxes,
            paths: self.export_paths,
            symbols: self.export_symbols,
            labels: self.export_labels,
            territories: self.export_territories,
            embed_background: self.embed_bg,
            embed_boxes: self.embed_boxes,
            embed_symbols: self.embed_symbols,
        };
        let s = svg::export(
            &root,
            &self.images,
            path,
            &Resolver::new(&self.settings),
            options,
        )?;
        self.status = format!(
            "Exported SVG: {} boxes, {} labels, {} symbols, {} paths, {} territories",
            s.boxes, s.labels, s.symbols, s.paths, s.territories
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
            "Imported SVG: {} labels, {} symbols, {} paths, {} territories",
            s.labels, s.symbols, s.paths, s.territories
        );
        dialog("SVG imported", &self.status, false);
        Ok(())
    }
    fn open_svg_renderer(&mut self, path: PathBuf, ctx: &egui::Context) -> Result<()> {
        let document = svg_render::analyze(&path)?;
        let class_count = document.classes.len();
        let fallback = document.layer_fallback;
        self.open_render_document(document, ctx)?;
        self.status = if fallback {
            format!("SVG has no classes; found {class_count} Inkscape layers")
        } else {
            format!("Found {class_count} SVG classes")
        };
        Ok(())
    }
    fn open_render_document(
        &mut self,
        document: svg_render::Document,
        ctx: &egui::Context,
    ) -> Result<()> {
        let rows = svg_render::default_settings(&document);
        let (source_x, source_y, source_width, source_height) = document.data_bounds().unwrap_or((
            0.0,
            0.0,
            document.width as f64,
            document.height as f64,
        ));
        let render_settings = RenderImportSettings {
            map_width: document.width,
            map_height: document.height,
            selected_layers: vec![true; rows.len()],
            source_x,
            source_y,
            source_width,
            source_height,
            preview_layer: None,
            selection_drag: None,
        };
        let resolver = Resolver::new(&self.settings);
        let assets = resolver.symbol_database()?;
        self.svg_renderer = Some(SvgRendererWindow {
            document,
            rows,
            selected: 0,
            path_styles: path_styles(&resolver),
            assets,
            preview: None,
            preview_texture: String::new(),
            preview_background: 2,
            created_map: None,
            gallery_open: false,
            gallery_scale: 1.0,
            gallery_background: 2,
            gallery_search: String::new(),
            gallery_textures: HashMap::new(),
            path_style_textures: HashMap::new(),
            draw_mode_icons: load_draw_mode_icons(&self.cache_dir, ctx),
            focused_last_frame: false,
            render_settings_open: false,
            render_settings,
            full_preview_open: false,
        });
        Ok(())
    }
    fn open_csv_importer(&mut self, path: PathBuf) -> Result<()> {
        let delimiter = svg_render::TableDelimiter::Auto;
        let (encoding, table) =
            match svg_render::read_table(&path, svg_render::TableEncoding::Utf8, delimiter) {
                Ok(table) => (svg_render::TableEncoding::Utf8, table),
                Err(_) => (
                    svg_render::TableEncoding::Windows1252,
                    svg_render::read_table(
                        &path,
                        svg_render::TableEncoding::Windows1252,
                        delimiter,
                    )?,
                ),
            };
        let columns = svg_render::auto_table_columns(&table.headers);
        let bounds = svg_render::table_bounds(&table, &columns, true);
        let (source_x, source_y, source_width, source_height) =
            bounds.as_ref().copied().unwrap_or((0., 0., 1., 1.));
        let (map_width, map_height) = table_default_map_size(source_width, source_height);
        self.csv_importer = Some(CsvImporterWindow {
            path,
            encoding,
            delimiter,
            table,
            columns,
            relative_after_first: true,
            source_x,
            source_y,
            source_width,
            source_height,
            map_width,
            map_height,
            error: bounds.err().map(|error| error.to_string()),
        });
        self.status = "CSV table loaded; assign its columns and coordinate space".into();
        Ok(())
    }
    fn load_svg_render_settings(&mut self, path: &Path) -> Result<()> {
        let window = self
            .svg_renderer
            .as_mut()
            .ok_or_else(|| Error::format("no SVG renderer is open"))?;
        let source = fs::read_to_string(path).map_err(|error| Error::format(error.to_string()))?;
        let saved: SavedRenderSettings =
            serde_json::from_str(&source).map_err(|error| Error::format(error.to_string()))?;
        let selected_by_class = saved
            .rows
            .iter()
            .zip(&saved.render_settings.selected_layers)
            .map(|(row, selected)| (row.class_name.as_str(), *selected))
            .collect::<HashMap<_, _>>();
        let mut loaded = 0;
        for row in &mut window.rows {
            if let Some(saved_row) = saved
                .rows
                .iter()
                .find(|saved| saved.class_name == row.class_name)
            {
                *row = saved_row.clone();
                loaded += 1;
            }
        }
        let mut render_settings = saved.render_settings;
        render_settings.selected_layers = window
            .rows
            .iter()
            .map(|row| {
                selected_by_class
                    .get(row.class_name.as_str())
                    .copied()
                    .unwrap_or(true)
            })
            .collect();
        render_settings.selection_drag = None;
        window.render_settings = render_settings;
        window.preview = None;
        window.preview_texture.clear();
        self.status = format!("Loaded JSON settings for {loaded} SVG classes");
        Ok(())
    }
    fn save_svg_render_settings(&mut self, path: &Path) -> Result<()> {
        let window = self
            .svg_renderer
            .as_ref()
            .ok_or_else(|| Error::format("no SVG renderer is open"))?;
        let saved = SavedRenderSettings {
            version: 1,
            rows: window.rows.clone(),
            render_settings: window.render_settings.clone(),
        };
        let json = serde_json::to_string_pretty(&saved)
            .map_err(|error| Error::format(error.to_string()))?;
        fs::write(path, json).map_err(|error| Error::format(error.to_string()))?;
        self.status = format!("Saved render settings JSON to {}", path.display());
        Ok(())
    }
    fn rebuild_symbol_database(&mut self) -> Result<usize> {
        let assets = Resolver::new(&self.settings).rebuild_symbol_database()?;
        let count = assets.len();
        if let Some(window) = self.svg_renderer.as_mut() {
            window.assets = assets;
            window.gallery_textures.clear();
            window.preview = None;
            window.preview_texture.clear();
        }
        self.status = format!("Rebuilt symbol database with {count} assets");
        Ok(count)
    }
    fn begin_svg_render(&mut self, path: PathBuf, ctx: &egui::Context) {
        if self.svg_render_job.is_some() {
            return;
        }
        let Some(window) = self.svg_renderer.as_ref() else {
            return;
        };
        let selected_classes = window
            .rows
            .iter()
            .zip(&window.render_settings.selected_layers)
            .filter(|(_, selected)| **selected)
            .map(|(row, _)| row.class_name.clone())
            .collect::<std::collections::HashSet<_>>();
        let document = window.document.cropped_for_render(
            &selected_classes,
            (
                window.render_settings.source_x,
                window.render_settings.source_y,
                window.render_settings.source_width,
                window.render_settings.source_height,
            ),
            window.render_settings.map_width,
            window.render_settings.map_height,
        );
        let rows = window
            .rows
            .iter()
            .zip(&window.render_settings.selected_layers)
            .filter(|(_, selected)| **selected)
            .map(|(row, _)| row.clone())
            .collect::<Vec<_>>();
        let resolver = Resolver::new(&self.settings);
        let compressed = self.compressed;
        let (sender, receiver) = mpsc::channel();
        let repaint = ctx.clone();
        let result_path = path.clone();
        std::thread::spawn(move || {
            let result = svg_render::render(&document, &rows, &resolver, &path, compressed);
            let _ = sender.send((result_path, result));
            repaint.request_repaint();
        });
        self.svg_render_job = Some(receiver);
        self.status = "Rendering imported classes and creating a new Wonderdraft map…".into();
    }
    fn poll_svg_render(&mut self) {
        let Some(receiver) = self.svg_render_job.take() else {
            return;
        };
        match receiver.try_recv() {
            Ok((path, Ok(summary))) => {
                if let Some(window) = self.svg_renderer.as_mut() {
                    window.created_map = Some(path.clone());
                }
                self.status = format!(
                    "Created {}: {} symbols, {} paths, {} territories, {} labels",
                    path.display(),
                    summary.symbols,
                    summary.paths,
                    summary.territories,
                    summary.labels
                );
            }
            Ok((_, Err(error))) => self.fail("Could not create Wonderdraft map", error),
            Err(TryRecvError::Empty) => self.svg_render_job = Some(receiver),
            Err(TryRecvError::Disconnected) => self.fail(
                "Could not create Wonderdraft map",
                "Render worker stopped unexpectedly",
            ),
        }
    }
    fn update_svg_symbol_preview(&mut self, ctx: &egui::Context) {
        let Some(window) = self.svg_renderer.as_mut() else {
            return;
        };
        let Some(row) = window.rows.get(window.selected) else {
            return;
        };
        if row.symbol == window.preview_texture {
            return;
        }
        window.preview_texture = row.symbol.clone();
        window.preview = None;
        let Some(asset) = window
            .assets
            .iter()
            .find(|asset| asset.texture == row.symbol)
        else {
            return;
        };
        let image = image::open(&asset.path).or_else(|original_error| {
            let is_svg = asset
                .path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("svg"));
            if !is_svg {
                return Err(original_error);
            }
            let preview_path = self
                .cache_dir
                .join(format!("svg_symbol_preview_{}.png", window.selected));
            let _ = fs::remove_file(&preview_path);
            svg_render::render_preview(&asset.path, &preview_path, 256, 256)
                .map_err(|_| original_error)?;
            image::open(preview_path)
        });
        let Ok(image) = image else { return };
        let rgba = image.to_rgba8();
        let size = [rgba.width() as usize, rgba.height() as usize];
        let color_image = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
        window.preview = Some(ctx.load_texture(
            format!("svg-render-preview:{}", row.symbol),
            color_image,
            egui::TextureOptions::LINEAR,
        ));
    }
    fn show_symbol_gallery(&mut self, ctx: &egui::Context) {
        let Some(window) = self.svg_renderer.as_mut() else {
            return;
        };
        if !window.gallery_open {
            return;
        }
        let mut selected_texture = None;
        let still_open = ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("svg-symbol-gallery-window"),
            egui::ViewportBuilder::default()
                .with_title("Symbol gallery")
                .with_inner_size([1040.0, 760.0])
                .with_min_inner_size([620.0, 440.0]),
            |ui, _| {
                if ui.input(|input| input.viewport().close_requested()) {
                    return false;
                }
                let focused = ui.input(|input| input.focused);
                if focused && !window.focused_last_frame {
                    // Raising the parent first, then this child viewport, keeps
                    // the main editor immediately behind the render window.
                    ui.ctx()
                        .send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                window.focused_last_frame = focused;
                ui.horizontal(|ui| {
                    ui.label(format!(
                        "{} available core, pack, and custom symbols",
                        window.assets.len()
                    ));
                    ui.separator();
                    ui.label("Find");
                    ui.text_edit_singleline(&mut window.gallery_search);
                    ui.separator();
                    ui.label("Tile scale");
                    ui.add(
                        egui::Slider::new(&mut window.gallery_scale, 0.5..=2.0).show_value(true),
                    );
                });
                ui.horizontal(|ui| {
                    ui.label("Background");
                    ui.selectable_value(&mut window.gallery_background, 0, "Black");
                    ui.selectable_value(&mut window.gallery_background, 1, "White");
                    ui.selectable_value(&mut window.gallery_background, 2, "Checkerboard");
                    ui.small("Double-click a symbol to select it.");
                });
                ui.separator();
                let query = window.gallery_search.to_lowercase();
                let assets = window
                    .assets
                    .iter()
                    .filter(|asset| {
                        query.is_empty() || asset.texture.to_lowercase().contains(&query)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                let tile = 116.0 * window.gallery_scale;
                let columns = (ui.available_width() / tile).floor().max(1.0) as usize;
                egui::ScrollArea::vertical()
                    .id_salt("symbol-gallery-scroll")
                    .show(ui, |ui| {
                        egui::Grid::new("symbol-gallery-grid")
                            .num_columns(columns)
                            .spacing(egui::vec2(6.0, 8.0))
                            .show(ui, |ui| {
                                for row in assets.chunks(columns) {
                                    for asset in row {
                                        let (rect, response) = ui.allocate_exact_size(
                                            egui::vec2(tile, tile + 30.0),
                                            egui::Sense::click(),
                                        );
                                        let image_rect = egui::Rect::from_min_size(
                                            rect.min,
                                            egui::vec2(tile, tile),
                                        );
                                        let is_selected = window
                                            .rows
                                            .get(window.selected)
                                            .is_some_and(|row| row.symbol == asset.texture);
                                        paint_preview_background(
                                            ui.painter(),
                                            image_rect,
                                            window.gallery_background,
                                        );
                                        if let Some(texture) = gallery_texture(window, asset, ctx) {
                                            let padding = (tile * 0.08).max(4.0);
                                            let bounds = image_rect.shrink(padding);
                                            ui.painter().image(
                                                texture.id(),
                                                fit_rect(bounds, texture.size_vec2()),
                                                egui::Rect::from_min_max(
                                                    egui::pos2(0.0, 0.0),
                                                    egui::pos2(1.0, 1.0),
                                                ),
                                                egui::Color32::WHITE,
                                            );
                                        } else {
                                            ui.painter().text(
                                                image_rect.center(),
                                                egui::Align2::CENTER_CENTER,
                                                "Preview\nunavailable",
                                                egui::FontId::proportional(12.0),
                                                egui::Color32::GRAY,
                                            );
                                        }
                                        if let Some(icon) =
                                            window.draw_mode_icons.get(&asset.draw_mode)
                                        {
                                            let icon_rect = egui::Rect::from_min_size(
                                                image_rect.min + egui::vec2(4.0, 4.0),
                                                egui::vec2(22.0, 22.0),
                                            );
                                            ui.painter().rect_filled(
                                                icon_rect.expand(2.0),
                                                3.0,
                                                egui::Color32::from_black_alpha(180),
                                            );
                                            ui.painter().image(
                                                icon.id(),
                                                icon_rect,
                                                egui::Rect::from_min_max(
                                                    egui::pos2(0.0, 0.0),
                                                    egui::pos2(1.0, 1.0),
                                                ),
                                                egui::Color32::WHITE,
                                            );
                                        }
                                        let label = asset
                                            .texture
                                            .rsplit('/')
                                            .next()
                                            .unwrap_or(&asset.texture);
                                        ui.painter().text(
                                            egui::pos2(rect.center().x, rect.max.y - 2.0),
                                            egui::Align2::CENTER_BOTTOM,
                                            truncate_label(label, 19),
                                            egui::FontId::proportional(12.0),
                                            ui.visuals().text_color(),
                                        );
                                        if is_selected {
                                            ui.painter().rect_stroke(
                                                rect.shrink(1.0),
                                                4.0,
                                                egui::Stroke::new(
                                                    3.0,
                                                    egui::Color32::from_rgb(80, 190, 255),
                                                ),
                                                egui::StrokeKind::Inside,
                                            );
                                            ui.painter().text(
                                                rect.min + egui::vec2(6.0, 5.0),
                                                egui::Align2::LEFT_TOP,
                                                "Selected",
                                                egui::FontId::proportional(12.0),
                                                egui::Color32::from_rgb(80, 190, 255),
                                            );
                                        }
                                        if response.clicked() {
                                            selected_texture = Some(asset.texture.clone());
                                        }
                                        if response.double_clicked() {
                                            window.gallery_open = false;
                                        }
                                        response.on_hover_text(&asset.texture);
                                    }
                                    ui.end_row();
                                }
                            });
                    });
                true
            },
        );
        if !still_open {
            window.gallery_open = false;
        }
        if let Some(texture) = selected_texture {
            if let Some(row) = window.rows.get_mut(window.selected) {
                row.symbol = texture;
            }
            self.update_svg_symbol_preview(ctx);
        }
    }
    fn show_full_render_preview(&mut self, ctx: &egui::Context) {
        let Some(window) = self.svg_renderer.as_mut() else {
            return;
        };
        if !window.full_preview_open {
            return;
        }
        let source_bounds = window.document.data_bounds().unwrap_or((
            0.0,
            0.0,
            window.document.width as f64,
            window.document.height as f64,
        ));
        let still_open = ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("svg-render-full-preview-window"),
            egui::ViewportBuilder::default()
                .with_title("Full render preview — 1 coordinate = 1 pixel")
                .with_inner_size([1000.0, 760.0])
                .with_min_inner_size([500.0, 360.0]),
            |ui, _| {
                if ui.input(|input| input.viewport().close_requested()) {
                    return false;
                }
                let class_name = window
                    .render_settings
                    .preview_layer
                    .filter(|index| {
                        window.render_settings.selected_layers.get(*index) == Some(&true)
                    })
                    .and_then(|index| window.rows.get(index))
                    .map(|row| row.class_name.clone());
                if let Some(class_name) = class_name {
                    egui::ScrollArea::both()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            paint_source_viewport_preview(
                                ui,
                                &window.document,
                                &class_name,
                                source_bounds,
                                &mut window.render_settings,
                                true,
                            );
                        });
                } else {
                    ui.label("Choose an imported preview layer in Render Settings first.");
                }
                true
            },
        );
        if !still_open {
            window.full_preview_open = false;
        }
    }

    fn show_csv_importer(&mut self, ctx: &egui::Context) {
        let Some(window) = self.csv_importer.as_mut() else {
            return;
        };
        let mut reload = false;
        let mut update_bounds = false;
        let mut continue_to_renderer = false;
        let still_open = ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("csv-render-import-window"),
            egui::ViewportBuilder::default()
                .with_title("Render from CSV — column mapping")
                .with_inner_size([980.0, 760.0])
                .with_min_inner_size([720.0, 520.0]),
            |ui, _| {
                if ui.input(|input| input.viewport().close_requested()) {
                    return false;
                }
                ui.label(format!("Source: {}", window.path.display()));
                ui.horizontal(|ui| {
                    let old_encoding = window.encoding;
                    egui::ComboBox::from_label("Encoding")
                        .selected_text(window.encoding.label())
                        .show_ui(ui, |ui| {
                            for encoding in svg_render::TableEncoding::ALL {
                                ui.selectable_value(
                                    &mut window.encoding,
                                    encoding,
                                    encoding.label(),
                                );
                            }
                        });
                    let old_delimiter = window.delimiter;
                    egui::ComboBox::from_label("Delimiter")
                        .selected_text(window.delimiter.label())
                        .show_ui(ui, |ui| {
                            for delimiter in svg_render::TableDelimiter::ALL {
                                ui.selectable_value(
                                    &mut window.delimiter,
                                    delimiter,
                                    delimiter.label(),
                                );
                            }
                        });
                    reload = old_encoding != window.encoding || old_delimiter != window.delimiter;
                    ui.label(format!(
                        "{} columns, {} data rows",
                        window.table.headers.len(),
                        window.table.rows.len()
                    ));
                });
                if let Some(error) = &window.error {
                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                }
                ui.separator();
                ui.heading("Assign table columns");
                egui::Grid::new("csv-column-mapping")
                    .num_columns(4)
                    .spacing([18.0, 5.0])
                    .show(ui, |ui| {
                        update_bounds |= table_column_combo(ui, "Tag", &window.table.headers, &mut window.columns.tag);
                        update_bounds |= table_column_combo(ui, "ID", &window.table.headers, &mut window.columns.id);
                        ui.end_row();
                        update_bounds |= table_column_combo(ui, "Name / label", &window.table.headers, &mut window.columns.name);
                        update_bounds |= table_column_combo(ui, "Class", &window.table.headers, &mut window.columns.class_name);
                        ui.end_row();
                        update_bounds |= table_column_combo(ui, "Fill", &window.table.headers, &mut window.columns.fill);
                        update_bounds |= table_column_combo(ui, "Stroke", &window.table.headers, &mut window.columns.stroke);
                        ui.end_row();
                        update_bounds |= table_column_combo(ui, "Stroke width", &window.table.headers, &mut window.columns.stroke_width);
                        update_bounds |= table_column_combo(ui, "Coordinates", &window.table.headers, &mut window.columns.coordinates);
                        ui.end_row();
                    });
                if ui
                    .checkbox(
                        &mut window.relative_after_first,
                        "For paths, first pair is the absolute origin and remaining pairs are offsets",
                    )
                    .changed()
                {
                    update_bounds = true;
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.strong("Source viewport");
                    ui.add(egui::DragValue::new(&mut window.source_x).prefix("X "));
                    ui.add(egui::DragValue::new(&mut window.source_y).prefix("Y "));
                    ui.add(egui::DragValue::new(&mut window.source_width).prefix("Width "));
                    ui.add(egui::DragValue::new(&mut window.source_height).prefix("Height "));
                    if ui.button("Fit to data").clicked() {
                        update_bounds = true;
                    }
                });
                ui.horizontal(|ui| {
                    ui.strong("Wonderdraft map size");
                    ui.add(egui::DragValue::new(&mut window.map_width).prefix("Width ").range(1..=65535));
                    ui.add(egui::DragValue::new(&mut window.map_height).prefix("Height ").range(1..=65535));
                });
                ui.separator();
                ui.heading("Preview");
                egui::ScrollArea::both().max_height(280.0).show(ui, |ui| {
                    egui::Grid::new("csv-table-preview").striped(true).show(ui, |ui| {
                        for header in &window.table.headers {
                            ui.strong(header);
                        }
                        ui.end_row();
                        for row in window.table.rows.iter().take(12) {
                            for index in 0..window.table.headers.len() {
                                let value = row.get(index).map_or("", String::as_str);
                                let shortened = value.chars().take(80).collect::<String>();
                                ui.label(if value.chars().count() > 80 {
                                    format!("{shortened}…")
                                } else {
                                    shortened
                                })
                                .on_hover_text(value);
                            }
                            ui.end_row();
                        }
                    });
                });
                ui.separator();
                if ui
                    .add_enabled(
                        window.columns.class_name.is_some()
                            && window.columns.coordinates.is_some()
                            && window.map_width > 0
                            && window.map_height > 0,
                        egui::Button::new("Continue to render settings"),
                    )
                    .clicked()
                {
                    continue_to_renderer = true;
                }
                true
            },
        );
        if !still_open {
            self.csv_importer = None;
            return;
        }
        if reload {
            match svg_render::read_table(&window.path, window.encoding, window.delimiter) {
                Ok(table) => {
                    window.columns = svg_render::auto_table_columns(&table.headers);
                    window.table = table;
                    window.error = None;
                    update_bounds = true;
                }
                Err(error) => window.error = Some(error.to_string()),
            }
        }
        if update_bounds {
            match svg_render::table_bounds(
                &window.table,
                &window.columns,
                window.relative_after_first,
            ) {
                Ok((x, y, width, height)) => {
                    window.source_x = x;
                    window.source_y = y;
                    window.source_width = width;
                    window.source_height = height;
                    window.error = None;
                }
                Err(error) => window.error = Some(error.to_string()),
            }
        }
        if continue_to_renderer {
            let options = svg_render::TableOptions {
                columns: window.columns.clone(),
                relative_after_first: window.relative_after_first,
                source_x: window.source_x,
                source_y: window.source_y,
                source_width: window.source_width,
                source_height: window.source_height,
                map_width: window.map_width,
                map_height: window.map_height,
            };
            match svg_render::analyze_table(&window.path, &window.table, &options) {
                Ok(document) => {
                    let class_count = document.classes.len();
                    self.csv_importer = None;
                    match self.open_render_document(document, ctx) {
                        Ok(()) => {
                            self.status = format!(
                                "Converted CSV table to {class_count} classes; configure rendering"
                            );
                        }
                        Err(error) => self.fail("Could not open CSV renderer", error),
                    }
                }
                Err(error) => window.error = Some(error.to_string()),
            }
        }
    }

    fn show_svg_renderer(&mut self, ctx: &egui::Context) {
        let Some(window) = self.svg_renderer.as_mut() else {
            return;
        };
        let mut load_csv = false;
        let mut save_csv = false;
        let mut render_map = false;
        let mut load_created_map = false;
        let mut dropped_load_settings = None;
        let mut dropped_created_map = None;
        let mut preview_changed = false;
        let mut propagated_name: Option<(usize, String)> = None;
        let table_source = window
            .document
            .source
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                matches!(
                    extension.to_ascii_lowercase().as_str(),
                    "csv" | "tsv" | "txt"
                )
            });
        let still_open = ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("svg-renderer-window"),
            egui::ViewportBuilder::default()
                .with_title("Render as new Wonderdraft map")
                .with_inner_size([920.0, 720.0])
                .with_min_inner_size([680.0, 500.0]),
            |ui, _| {
                if ui.input(|input| input.viewport().close_requested()) {
                    return false;
                }
                ui.label(format!("{} × {} map pixels — {}", window.document.width, window.document.height, if window.document.layer_fallback { "using Inkscape layers as classes" } else if table_source { "using table classes" } else { "using SVG classes" }));
                ui.horizontal(|ui| {
                    let load_settings = ui.button("Load settings JSON…");
                    if load_settings.clicked() { load_csv = true; }
                    dropped_load_settings = dropped_path_over(ui, &load_settings, is_json_file);
                    if ui.button("Save settings JSON…").clicked() { save_csv = true; }
                    ui.separator();
                    if ui.button("Render settings…").clicked() {
                        window.render_settings_open = true;
                    }
                    if ui.add_enabled(self.svg_render_job.is_none(), egui::Button::new("Create .wonderdraft_map…")).clicked() { render_map = true; }
                    let load_created = ui.add_enabled(
                        window.created_map.is_some() && self.map_load.is_none(),
                        egui::Button::new("Load created .wonderdraft_map"),
                    );
                    if load_created.clicked() {
                        load_created_map = true;
                    }
                    dropped_created_map = dropped_path_over(ui, &load_created, is_wonderdraft_map);
                    if self.svg_render_job.is_some() { ui.spinner(); ui.label("Rendering…"); }
                });
                if self.svg_render_job.is_none()
                    && let Some(path) = &window.created_map
                {
                    ui.colored_label(
                        egui::Color32::LIGHT_GREEN,
                        format!("Created: {}", path.display()),
                    );
                }
                ui.separator();
                ui.columns(2, |columns| {
                    columns[0].heading("Classes / layers");
                    let class_row_height = columns[0].spacing().interact_size.y;
                    egui::ScrollArea::vertical().id_salt("svg-render-classes").show_rows(
                        &mut columns[0],
                        class_row_height,
                        window.rows.len(),
                        |ui, visible_rows| {
                        for index in visible_rows {
                            let row = &window.rows[index];
                            if ui.selectable_label(window.selected == index, format!("{}  →  {}", row.class_name, row.category.label())).clicked() {
                                window.selected = index;
                                preview_changed = true;
                            }
                        }
                    });
                    columns[1].heading("Translation settings");
                    let selected = window.selected;
                    let assets = &window.assets;
                    let draw_mode_icons = &window.draw_mode_icons;
                    let path_styles = &window.path_styles;
                    let path_style_textures = &mut window.path_style_textures;
                    let preview = &window.preview;
                    let preview_background = &mut window.preview_background;
                    if let Some(row) = window.rows.get_mut(selected) {
                        egui::ScrollArea::vertical().id_salt("svg-render-settings").show(&mut columns[1], |ui| {
                            ui.strong(&row.class_name);
                            egui::ComboBox::from_label("Category").selected_text(row.category.label()).show_ui(ui, |ui| {
                                for category in svg_render::Category::ALL { ui.selectable_value(&mut row.category, category, category.label()); }
                            });
                            if row.category == svg_render::Category::Invisible { ui.small("Invisible elements are not written to the map."); return; }
                            ui.horizontal(|ui| { ui.label("Name attribute"); if ui.text_edit_singleline(&mut row.name_attribute).changed() { propagated_name=Some((selected,row.name_attribute.clone())); } });
                            ui.horizontal(|ui| {
                                ui.checkbox(&mut row.label.enabled, "Create label");
                                ui.add_enabled_ui(row.label.enabled, |ui| {
                                    ui.checkbox(&mut row.label.prepend_class, "Prepend class");
                                });
                            });
                            if row.label.enabled {
                                ui.indent("label-options", |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label("Font");
                                        ui.text_edit_singleline(&mut row.label.font);
                                        egui::ComboBox::from_id_salt(("label-font", selected))
                                            .selected_text("Presets")
                                            .show_ui(ui, |ui| {
                                                for font in svg_render::LABEL_FONT_PRESETS {
                                                    if ui.selectable_label(row.label.font == *font, *font).clicked() {
                                                        row.label.font = (*font).into();
                                                        ui.close();
                                                    }
                                                }
                                            });
                                    });
                                    ui.add(egui::Slider::new(&mut row.label.size, 4.0..=200.0).text("Size"));
                                    egui::ComboBox::from_label("Alignment")
                                        .selected_text(match row.label.align {
                                            1 => "Center align",
                                            2 => "Right align",
                                            _ => "Left align",
                                        })
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(&mut row.label.align, 0, "Left align");
                                            ui.selectable_value(&mut row.label.align, 1, "Center align");
                                            ui.selectable_value(&mut row.label.align, 2, "Right align");
                                        });
                                    color_control(ui, "Color", &mut row.label.color);
                                    color_control(ui, "Outline", &mut row.label.outline_color);
                                    ui.add(egui::Slider::new(&mut row.label.outline, 0.0..=20.0).text("Outline width"));
                                    ui.horizontal(|ui| { ui.add(egui::DragValue::new(&mut row.label.offset_x).prefix("Offset X ")); ui.add(egui::DragValue::new(&mut row.label.offset_y).prefix("Offset Y ")); });
                                });
                            }
                            ui.separator();
                            match row.category {
                                svg_render::Category::Symbol => {
                                    ui.label("Wonderdraft symbol");
                                    if ui.text_edit_singleline(&mut row.symbol).changed() { preview_changed=true; }
                                    if ui.button("Symbol gallery…").clicked() { window.gallery_open = true; }
                                    ui.menu_button("Matching symbols…", |ui| {
                                        let query=row.symbol.to_lowercase();
                                        let mut shown=0;
                                        egui::ScrollArea::vertical().max_height(230.).show(ui, |ui| {
                                            for asset in assets {
                                                let label=asset.texture.rsplit('/').next().unwrap_or(&asset.texture);
                                                if !query.is_empty() && !asset.texture.to_lowercase().contains(&query) { continue; }
                                                if ui.button(label).on_hover_text(&asset.texture).clicked() { row.symbol=asset.texture.clone(); preview_changed=true; ui.close(); }
                                                shown+=1; if shown>=250 { break; }
                                            }
                                            if shown==0 { ui.label("No matching configured assets"); }
                                        });
                                    });
                                    ui.scope(|ui| {
                                        ui.spacing_mut().slider_width *= 2.0;
                                        ui.add(
                                            egui::Slider::from_get_set(0.0..=100.0, |position| {
                                                if let Some(position) = position {
                                                    row.symbol_scale = symbol_scale_after_slider_move(
                                                        row.symbol_scale,
                                                        position,
                                                    );
                                                }
                                                slider_position_for_symbol_scale(row.symbol_scale)
                                            })
                                            .step_by(1.0)
                                            .show_value(false)
                                            .text("Scale"),
                                        );
                                    });
                                    ui.small("Slider positions: 0 (left), 1× (middle), 100 (right); each step changes scale by 10%.");
                                    ui.horizontal(|ui| {
                                        ui.label("Current scale");
                                        ui.add(
                                            egui::DragValue::new(&mut row.symbol_scale)
                                                .custom_parser(parse_symbol_scale_text)
                                                .custom_formatter(|value, _| {
                                                    format_symbol_scale(value as f32)
                                                }),
                                        );
                                    });
                                    if let Some(asset) = assets
                                        .iter()
                                        .find(|asset| asset.texture == row.symbol)
                                    {
                                        ui.label(format!(
                                            "Resulting size: {}",
                                            format_pixel_dimensions(
                                                asset.width * row.symbol_scale as f64,
                                                asset.height * row.symbol_scale as f64,
                                            )
                                        ));
                                    } else {
                                        ui.label("Resulting size: unavailable");
                                    }
                                    let draw_mode = assets
                                        .iter()
                                        .find(|asset| asset.texture == row.symbol)
                                        .map(|asset| asset.draw_mode.as_str())
                                        .unwrap_or("normal");
                                    ui.horizontal(|ui| {
                                        ui.label("Draw mode");
                                        if let Some(icon) = draw_mode_icons.get(draw_mode) {
                                            ui.add(egui::Image::new(icon).fit_to_exact_size(egui::vec2(20.0, 20.0)));
                                        }
                                        ui.label(draw_mode_label(draw_mode));
                                    });
                                    match draw_mode {
                                        "sample_color" => color_control(ui, "Tint / sample color", &mut row.tint),
                                        "custom_colors" => {
                                            color_control(ui, "Custom color 1", &mut row.custom_colors[0]);
                                            color_control(ui, "Custom color 2", &mut row.custom_colors[1]);
                                            color_control(ui, "Custom color 3", &mut row.custom_colors[2]);
                                        }
                                        _ => {
                                            ui.small("Normal symbols use their original colors.");
                                        }
                                    }
                                    ui.horizontal(|ui| { ui.label("Preview background"); ui.selectable_value(preview_background,0,"Black"); ui.selectable_value(preview_background,1,"White"); ui.selectable_value(preview_background,2,"Checkerboard"); });
                                    let (rect, _) = ui.allocate_exact_size(egui::vec2(260.,220.), egui::Sense::hover());
                                    paint_preview_background(ui.painter(), rect, *preview_background);
                                    if let Some(texture)=preview { ui.put(rect.shrink(8.), egui::Image::new(texture).fit_to_exact_size(rect.shrink(8.).size())); } else { ui.painter().text(rect.center(),egui::Align2::CENTER_CENTER,"Preview unavailable\n(select a PNG/WebP/JPEG symbol)",egui::FontId::proportional(14.),egui::Color32::GRAY); }
                                }
                                svg_render::Category::Path | svg_render::Category::Territory => {
                                    egui::ComboBox::from_label(if row.category==svg_render::Category::Path {"Path style"} else {"Border style"})
                                        .selected_text(path_style_label(&row.path_style))
                                        .show_ui(ui, |ui| {
                                            for style in path_styles {
                                                ui.horizontal(|ui| {
                                                    let (rect, _) = ui.allocate_exact_size(egui::vec2(110.0, 24.0), egui::Sense::hover());
                                                    ui.painter().rect_filled(rect, 2.0, egui::Color32::from_gray(35));
                                                    if let Some(texture) = path_style_texture(path_style_textures, style, ctx) {
                                                        ui.painter().image(texture.id(), rect.shrink2(egui::vec2(3.0, 5.0)), egui::Rect::from_min_max(egui::pos2(0.0,0.0),egui::pos2(1.0,1.0)), egui::Color32::WHITE);
                                                    }
                                                    if ui.selectable_label(row.path_style == style.texture, path_style_label(&style.texture)).clicked() { row.path_style = style.texture.clone(); }
                                                });
                                            }
                                        });
                                    color_control(ui,"Color",&mut row.path_color);
                                    ui.add(egui::Slider::new(&mut row.width,0.1..=100.).text("Width"));
                                    if row.category == svg_render::Category::Path {
                                        ui.add(egui::Slider::new(&mut row.roughness, 0.0..=10.0).text("Roughness"));
                                    }
                                }
                                svg_render::Category::Ground | svg_render::Category::WaterTint => {
                                    optional_fill_control(ui, &mut row.fill_override, &mut row.no_fill_override);
                                    optional_color_control(ui,"Border override",&mut row.border_override);
                                    optional_width_control(ui,"Border width override",&mut row.border_width_override);
                                    ui.small("‘Use SVG value’ preserves the element’s existing presentation value.");
                                }
                                svg_render::Category::Landmass => { ui.add(egui::Slider::new(&mut row.width,0.0..=100.).text("Black mask border width")); ui.small("Rendered black onto mask.png."); }
                                svg_render::Category::FillWithLand => { ui.small("Fills the complete map mask with land. The selected class or layer geometry is ignored."); }
                                svg_render::Category::Freshwater => {
                                    optional_fill_control(ui, &mut row.fill_override, &mut row.no_fill_override);
                                    optional_color_control(ui,"Border override",&mut row.border_override);
                                    optional_width_control(ui,"Border width override",&mut row.border_width_override);
                                    ui.small("Without overrides, existing fill/stroke colors become red; explicit ‘none’ and opacity are preserved. Rendered after all landmass classes.");
                                }
                                svg_render::Category::Invisible => {}
                            }
                        });
                    }
                });
                if window.render_settings_open {
                    show_render_import_settings(ui.ctx(), window);
                }
                true
            },
        );
        if let Some((selected, value)) = propagated_name {
            for next in window.rows.iter_mut().skip(selected + 1) {
                if next.name_attribute == "map:svgname" {
                    next.name_attribute = value.clone();
                }
            }
        }
        if !still_open {
            self.svg_renderer = None;
            return;
        }
        let csv_file_name = window
            .document
            .source
            .file_stem()
            .map(|v| format!("{}_render_settings.json", v.to_string_lossy()))
            .unwrap_or_else(|| "svg_render_settings.json".into());
        let map_file_name = window
            .document
            .source
            .file_stem()
            .map(|v| format!("{}.wonderdraft_map", v.to_string_lossy()))
            .unwrap_or_else(|| "rendered_svg.wonderdraft_map".into());
        let created_map_to_load = dropped_created_map.or_else(|| {
            if load_created_map {
                window.created_map.clone()
            } else {
                None
            }
        });
        if preview_changed {
            self.update_svg_symbol_preview(ctx);
        }
        if load_csv {
            self.begin_dialog(DialogAction::LoadRenderSettings, ctx);
        }
        if let Some(path) = dropped_load_settings {
            if let Err(error) = self.load_svg_render_settings(&path) {
                self.fail("Could not load render settings", error);
            }
        }
        if save_csv {
            self.begin_dialog(
                DialogAction::SaveRenderSettings {
                    file_name: csv_file_name,
                },
                ctx,
            );
        }
        if render_map {
            self.begin_dialog(
                DialogAction::SaveRenderedMap {
                    file_name: map_file_name,
                },
                ctx,
            );
        }
        if let Some(path) = created_map_to_load {
            self.begin_map_load(path, ctx);
        }
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

impl App {
    fn show_overwrite_prompt(&mut self, ctx: &egui::Context) {
        let Some(path) = self.overwrite_prompt.clone() else {
            return;
        };
        let mut open = true;
        egui::Window::new("Overwrite map file?")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(format!("Overwrite {}?", path.display()));
                ui.horizontal(|ui| {
                    if ui.button("Overwrite").clicked() {
                        self.overwrite_prompt = None;
                        if let Err(error) = self.save_to(&path) {
                            self.fail("Could not save map", error);
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        self.overwrite_prompt = None;
                    }
                });
            });
        if !open {
            self.overwrite_prompt = None;
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, root_ui: &mut egui::Ui, _: &mut eframe::Frame) {
        let ctx = root_ui.ctx().clone();
        self.poll_background_work(&ctx);
        self.poll_svg_render();
        if ctx.input_mut(|input| {
            input.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::CTRL,
                egui::Key::F,
            ))
        }) {
            self.search_open = true;
            self.focus_search = true;
        }

        let busy = self.pending_dialog.is_some()
            || self.map_load.is_some()
            || self.pck_extraction.is_some()
            || self.font_installation.is_some();
        let mut recent_selection = None;
        egui::Panel::top("toolbar").show(root_ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let open_map = ui.add_enabled(!busy, egui::Button::new("Open map"));
                if open_map.clicked() {
                    self.begin_dialog(DialogAction::OpenMap, &ctx);
                }
                if let Some(path) = dropped_path_over(ui, &open_map, is_wonderdraft_map) {
                    self.begin_map_load(path, &ctx);
                }
                ui.add_enabled_ui(!busy, |ui| {
                    ui.menu_button("Open recent", |ui| {
                        let _ = self.reload_wonderdraft_config();
                        let recent_paths = settings::merged_recent_maps(
                            &self.settings.recent_maps,
                            &self.wonderdraft_config.recently_opened,
                        );
                        if recent_paths.is_empty() {
                            ui.label("No recent maps");
                        }
                        for path in recent_paths {
                            let available = is_wonderdraft_map(&path);
                            let response = ui
                                .add_enabled(available, egui::Button::new(path.to_string_lossy()));
                            if !available {
                                response
                                    .clone()
                                    .on_disabled_hover_text("This file no longer exists");
                            }
                            if response.clicked() {
                                recent_selection = Some(path);
                                ui.close();
                            }
                        }
                    });
                });
                if ui.button("Validate text").clicked() {
                    self.validate()
                }
                let save_map = ui.button("Save map as…");
                if save_map.clicked() {
                    let file_name = self
                        .root_path
                        .as_ref()
                        .and_then(|path| path.file_stem())
                        .map(|stem| format!("{}_edited.wonderdraft_map", stem.to_string_lossy()))
                        .unwrap_or_else(|| "edited.wonderdraft_map".into());
                    self.begin_dialog(DialogAction::SaveMap { file_name }, &ctx);
                }
                if let Some(path) = dropped_path_over(ui, &save_map, |path| {
                    path.is_dir() || is_wonderdraft_map(path)
                }) {
                    if path.is_dir() {
                        let file_name = self
                            .root_path
                            .as_ref()
                            .and_then(|path| path.file_stem())
                            .map(|stem| {
                                format!("{}_edited.wonderdraft_map", stem.to_string_lossy())
                            })
                            .unwrap_or_else(|| "edited.wonderdraft_map".into());
                        self.begin_dialog_in_directory(
                            DialogAction::SaveMap { file_name },
                            path,
                            &ctx,
                        );
                    } else {
                        self.overwrite_prompt = Some(path);
                    }
                }
                let export_svg = ui.button("Export SVG…");
                if export_svg.clicked() {
                    let file_name = self
                        .root_path
                        .as_ref()
                        .and_then(|path| path.file_stem())
                        .map(|stem| format!("{}.svg", stem.to_string_lossy()))
                        .unwrap_or_else(|| "wonderdraft_map.svg".into());
                    self.begin_dialog(DialogAction::ExportSvg { file_name }, &ctx);
                }
                if let Some(path) = dropped_path_over(ui, &export_svg, |path| path.is_dir()) {
                    let file_name = self
                        .root_path
                        .as_ref()
                        .and_then(|path| path.file_stem())
                        .map(|stem| format!("{}.svg", stem.to_string_lossy()))
                        .unwrap_or_else(|| "wonderdraft_map.svg".into());
                    self.begin_dialog_in_directory(
                        DialogAction::ExportSvg { file_name },
                        path,
                        &ctx,
                    );
                }
                let import_svg = ui.button("Import SVG…");
                if import_svg.clicked() {
                    self.begin_dialog(DialogAction::ImportSvg, &ctx);
                }
                if let Some(path) = dropped_path_over(ui, &import_svg, is_svg_file) {
                    if let Err(error) = self.import_svg_from(&path) {
                        self.fail("Could not import SVG", error);
                    }
                }
                let render_svg = ui.button("Render SVG…");
                if render_svg.clicked() {
                    self.begin_dialog(DialogAction::RenderSvg, &ctx);
                }
                if let Some(path) = dropped_path_over(ui, &render_svg, is_svg_file) {
                    if let Err(error) = self.open_svg_renderer(path, &ctx) {
                        self.fail("Could not open SVG renderer", error);
                    }
                }
                let render_csv = ui.button("Render from CSV…");
                if render_csv.clicked() {
                    self.begin_dialog(DialogAction::RenderCsv, &ctx);
                }
                if let Some(path) = dropped_path_over(ui, &render_csv, is_csv_file) {
                    if let Err(error) = self.open_csv_importer(path) {
                        self.fail("Could not open CSV renderer", error);
                    }
                }
                let export_data = ui.button("Export map data…");
                if export_data.clicked() {
                    let file_name = self
                        .root_path
                        .as_ref()
                        .and_then(|path| path.file_stem())
                        .map(|stem| format!("{}_map_data.txt", stem.to_string_lossy()))
                        .unwrap_or_else(|| "wonderdraft_map_data.txt".into());
                    self.begin_dialog(DialogAction::ExportMapData { file_name }, &ctx);
                }
                if let Some(path) = dropped_path_over(ui, &export_data, |path| path.is_dir()) {
                    let file_name = self
                        .root_path
                        .as_ref()
                        .and_then(|path| path.file_stem())
                        .map(|stem| format!("{}_map_data.txt", stem.to_string_lossy()))
                        .unwrap_or_else(|| "wonderdraft_map_data.txt".into());
                    self.begin_dialog_in_directory(
                        DialogAction::ExportMapData { file_name },
                        path,
                        &ctx,
                    );
                }
                if ui.button("Export all PNGs").clicked() {
                    self.begin_dialog(DialogAction::ExportAllImages, &ctx);
                }
                if ui.button("Settings…").clicked() {
                    self.settings_backup = Some(self.settings.clone());
                    self.refresh_cache_size();
                    self.settings_open = true;
                }
            });
            let map_drop = ui.add_sized(
                [220.0, 24.0],
                egui::Label::new("Drop .wonderdraft_map here").sense(egui::Sense::hover()),
            );
            if let Some(path) = dropped_path_over(ui, &map_drop, is_wonderdraft_map) {
                self.begin_map_load(path, &ctx);
            }
            ui.horizontal_wrapped(|ui| {
                ui.label("Options:");
                ui.checkbox(&mut self.compressed, "Compress saved map");
                ui.checkbox(&mut self.verify, "Verify save");
                ui.checkbox(&mut self.embed_bg, "Embed mask in SVG");
                ui.checkbox(&mut self.embed_boxes, "Embed boxes in SVG");
                ui.checkbox(&mut self.embed_symbols, "Embed symbols in SVG");
            });
            ui.label(format!(
                "Loaded file: {}",
                self.root_path
                    .as_deref()
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "none".into())
            ));
            if busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(if self.pending_dialog.is_some() {
                        "File chooser is open — the editor remains responsive"
                    } else if self.pck_extraction.is_some() {
                        "Extracting Wonderdraft core assets in the background"
                    } else {
                        "Loading map in the background"
                    });
                });
            }
            ui.horizontal_wrapped(|ui| {
                ui.label("SVG export layers:");
                ui.checkbox(&mut self.export_background, "Background mask");
                ui.checkbox(&mut self.export_boxes, "Boxes");
                ui.checkbox(&mut self.export_paths, "Roads / paths");
                ui.checkbox(&mut self.export_symbols, "Symbols");
                ui.checkbox(&mut self.export_labels, "Labels");
                ui.checkbox(&mut self.export_territories, "Territories");
            });
            ui.label(&self.status);
        });
        if let Some(path) = recent_selection {
            self.begin_map_load(path, &ctx);
        }
        self.show_csv_importer(&ctx);
        self.show_svg_renderer(&ctx);
        self.show_full_render_preview(&ctx);
        self.show_symbol_gallery(&ctx);
        self.show_overwrite_prompt(&ctx);
        egui::Panel::right("images")
            .resizable(true)
            .default_size(280.)
            .min_size(210.)
            .max_size(380.)
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
                    let replace_png = ui.button("Replace PNG");
                    if replace_png.clicked() {
                        if self.images.get(self.selected).is_some() {
                            self.begin_dialog(
                                DialogAction::ReplaceImage {
                                    index: self.selected,
                                },
                                &ctx,
                            );
                        }
                    }
                    if let Some(path) = dropped_path_over(ui, &replace_png, |path| {
                        is_extension(path, &["png", "jpg", "jpeg", "webp"])
                    }) && let Err(error) = self.replace_image_from(self.selected, &path, &ctx)
                    {
                        self.fail("Could not replace image", error);
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
                            if matches!(
                                info.pixels,
                                miracledraft_map_helper::ByteSource::File { .. }
                            ) {
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
                ui.heading("Map tools");
                let previous_section = self.section_choice;
                egui::ComboBox::from_label("Jump to section")
                    .selected_text(if self.section_choice == 0 {
                        "Select a section"
                    } else {
                        MAP_SECTIONS[self.section_choice - 1].0
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.section_choice, 0, "Select a section");
                        for (index, (label, _)) in MAP_SECTIONS.iter().enumerate() {
                            ui.selectable_value(&mut self.section_choice, index + 1, *label);
                        }
                    });
                if self.section_choice != previous_section && self.section_choice > 0 {
                    let (label, marker) = MAP_SECTIONS[self.section_choice - 1];
                    self.jump_to_section(marker, label);
                    self.section_choice = 0;
                }
                if ui
                    .add_enabled(
                        self.root_path.is_some(),
                        egui::Button::new("Remove off-canvas symbols"),
                    )
                    .clicked()
                {
                    self.remove_off_canvas_symbols();
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
            if self.svg_renderer.is_some() || self.csv_importer.is_some() {
                ui.centered_and_justified(|ui| {
                    ui.label(
                        "Map-data editor paused while the CSV/render window is open to keep the interface responsive.",
                    );
                });
                return;
            }
            ui.label("Map data (Godot text syntax)");
            if self.search_open {
                ui.horizontal(|ui| {
                    ui.label("Find:");
                    let previous_query = self.search_query.clone();
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.search_query)
                            .id_salt("map-data-search")
                            .desired_width(260.0),
                    );
                    if self.focus_search {
                        response.request_focus();
                        self.focus_search = false;
                    }
                    if self.search_query != previous_query {
                        self.search_from = 0;
                    }
                    let enter = response.has_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter));
                    if ui.button("Find next").clicked() || enter {
                        self.find_next();
                    }
                    if ui.button("Close").clicked()
                        || ui.input(|input| input.key_pressed(egui::Key::Escape))
                    {
                        self.search_open = false;
                    }
                });
            }
            egui::ScrollArea::both()
                .id_salt("map-data-scroll")
                .auto_shrink(false)
                .show(ui, |ui| {
                    let mut output = egui::TextEdit::multiline(&mut self.text)
                        .id_salt("map-data-editor")
                        .font(egui::TextStyle::Monospace)
                        .desired_width(f32::INFINITY)
                        .desired_rows(40)
                        .show(ui);
                    if let Some((byte_start, byte_end)) = self.pending_text_selection.take() {
                        let char_start =
                            self.text[..byte_start.min(self.text.len())].chars().count();
                        let char_end = self.text[..byte_end.min(self.text.len())].chars().count();
                        let start_cursor = egui::text::CCursor::new(char_start);
                        let end_cursor = egui::text::CCursor::new(char_end);
                        output
                            .state
                            .cursor
                            .set_char_range(Some(egui::text::CCursorRange::two(
                                start_cursor,
                                end_cursor,
                            )));
                        output.state.store(ui.ctx(), output.response.id);
                        output.response.request_focus();
                        let cursor_rect = output
                            .galley
                            .pos_from_cursor(start_cursor)
                            .translate(output.galley_pos.to_vec2());
                        ui.scroll_to_rect(cursor_rect, Some(egui::Align::TOP));
                    }
                });
        });
        if self.settings_open {
            let mut open = self.settings_open;
            let mut saved = false;
            let mut cancelled = false;
            egui::Window::new("Settings")
                .open(&mut open)
                .default_width(620.0)
                .min_width(400.0)
                .resizable(true)
                .vscroll(true)
                .show(&ctx, |ui| {
                    ui.heading("Wonderdraft");
                    if ui.button("Run setup wizard…").clicked() {
                        self.setup_step = 0;
                        self.font_list_loaded = false;
                        self.setup_wizard_open = true;
                        self.settings_backup = None;
                        saved = true;
                    }
                    ui.label("User-data folder (contains config.ini)");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.settings.wonderdraft_folder)
                                .desired_width(480.0),
                        );
                        if ui.button("Browse…").clicked() {
                            self.begin_dialog(DialogAction::WonderdraftFolder, &ctx);
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.checkbox(
                            &mut self.settings.auto_locate_custom_assets,
                            "Automatically locate the custom assets folder from config.ini",
                        );
                        if ui.button("Reload config.ini").clicked() {
                            match self.reload_wonderdraft_config() {
                                Ok(()) => self.status = "Wonderdraft config.ini reloaded".into(),
                                Err(error) => self.fail("Could not reload Wonderdraft config.ini", error),
                            }
                        }
                    });
                    ui.label("Custom asset folder");
                    ui.horizontal(|ui| {
                        let response = ui.add_enabled(
                            !self.settings.auto_locate_custom_assets,
                            egui::TextEdit::singleline(&mut self.settings.custom_asset_folder)
                                .desired_width(480.0),
                        );
                        if response.changed() {
                            self.font_list_loaded = false;
                        }
                        if ui
                            .add_enabled(
                                !self.settings.auto_locate_custom_assets,
                                egui::Button::new("Browse…"),
                            )
                            .clicked()
                        {
                            self.begin_dialog(DialogAction::CustomAssets, &ctx);
                        }
                    });
                    if self.settings.auto_locate_custom_assets {
                        ui.small("Uses <Wonderdraft folder>/assets, or <custom_assets_directory>/assets when that key is present.");
                    }
                    ui.label("Default sprites folder");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.settings.default_asset_folder)
                                .desired_width(480.0),
                        );
                        if ui.button("Browse…").clicked() {
                            self.begin_dialog(DialogAction::DefaultSprites, &ctx);
                        }
                    });
                    if ui
                        .add_enabled(
                            self.pending_dialog.is_none() && self.pck_extraction.is_none(),
                            egui::Button::new("Locate and extract Wonderdraft core assets…"),
                        )
                        .clicked()
                    {
                        self.begin_core_asset_setup(&ctx);
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Rebuild symbol database").clicked() {
                            match self.rebuild_symbol_database() {
                                Ok(count) => self.status = format!("Rebuilt symbol database with {count} assets"),
                                Err(error) => self.fail("Could not rebuild symbol database", error),
                            }
                        }
                        ui.small(format!(
                            "Database: {}",
                            miracledraft_map_helper::assets::symbol_database_path().display()
                        ));
                    });

                    ui.separator();
                    ui.heading("Cache");
                    ui.label("Disk cache folder");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.settings.cache_folder)
                                .desired_width(480.0),
                        );
                        if ui.button("Browse…").clicked() {
                            self.begin_dialog(DialogAction::CacheFolder, &ctx);
                        }
                    });
                    ui.checkbox(
                        &mut self.settings.clear_cache_on_exit,
                        "Clear the cache when the program exits",
                    );
                    ui.horizontal(|ui| {
                        if ui.button("Clear cache now").clicked() {
                            self.clear_cache();
                        }
                        ui.label(format!(
                            "Current size: {}",
                            self.cache_size_bytes
                                .map(format_byte_size)
                                .unwrap_or_else(|| "unavailable".into())
                        ));
                    });
                    if self.work_dir.is_some() {
                        ui.small("The cache for the currently open map is kept until the map is replaced or the program exits.");
                    }

                    ui.separator();
                    ui.heading("About");
                    ui.label(format!("{APP_NAME} {APP_VERSION}"));
                    ui.label(format!("Build time: {BUILD_TIME}"));
                    ui.label("Native Wonderdraft map and SVG interchange editor");
                    ui.hyperlink_to(
                        "Project website and source code",
                        env!("CARGO_PKG_REPOSITORY"),
                    );
                    ui.label("License: Unlicense (public domain dedication)");

                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            let cache = PathBuf::from(self.settings.cache_folder.trim());
                            let result = fs::create_dir_all(&cache)
                                .map_err(|error| Error::format(error.to_string()))
                                .and_then(|()| self.reload_wonderdraft_config())
                                .and_then(|()| settings::save(&self.settings));
                            match result {
                                Ok(()) => {
                                    self.cache_dir = PathBuf::from(&self.settings.cache_folder);
                                    self.refresh_cache_size();
                                    self.status = "Settings saved".into();
                                    self.settings_backup = None;
                                    saved = true;
                                }
                                Err(e) => self.fail("Settings failed", e),
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            cancelled = true;
                        }
                    });
                });
            if cancelled || (!open && !saved) {
                if let Some(previous) = self.settings_backup.take() {
                    self.settings = previous;
                    let _ = self.reload_wonderdraft_config();
                }
            }
            self.settings_open = open && !saved && !cancelled;
        }

        if self.setup_wizard_open {
            self.show_setup_wizard(&ctx);
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        if self.settings.clear_cache_on_exit {
            if let Some(work_dir) = self.work_dir.take()
                && !work_dir.starts_with(&self.cache_dir)
            {
                let _ = fs::remove_dir_all(work_dir);
            }
            let _ = settings::clear_cache(&self.cache_dir, None);
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

fn configured_directory(value: &str) -> bool {
    !value.trim().is_empty() && Path::new(value.trim()).is_dir()
}

fn table_default_map_size(source_width: f64, source_height: f64) -> (u32, u32) {
    let longest = source_width.max(source_height).max(1.);
    let scale = 4096. / longest;
    (
        (source_width * scale).round().clamp(1., 65535.) as u32,
        (source_height * scale).round().clamp(1., 65535.) as u32,
    )
}

fn table_column_combo(
    ui: &mut egui::Ui,
    label: &str,
    headers: &[String],
    selected: &mut Option<usize>,
) -> bool {
    ui.label(label);
    let old = *selected;
    egui::ComboBox::from_id_salt(("csv-column", label))
        .selected_text(
            selected
                .and_then(|index| headers.get(index))
                .map_or("Not assigned", String::as_str),
        )
        .show_ui(ui, |ui| {
            ui.selectable_value(selected, None, "Not assigned");
            for (index, header) in headers.iter().enumerate() {
                ui.selectable_value(selected, Some(index), header);
            }
        });
    old != *selected
}

const MAP_SECTIONS: &[(&str, &str)] = &[
    ("Boxes", "\"boxes\": ["),
    ("Symbols", "\"symbols\": ["),
    ("Paths / roads", "\"paths\": ["),
    ("Labels", "\"labels\": ["),
    ("Territories", "\"territories\": {"),
    ("Theme", "\"theme\": {"),
];

fn map_number(value: Option<&Value>, default: f64) -> f64 {
    value.and_then(Value::as_f64).unwrap_or(default)
}

fn map_vec2(value: Option<&Value>, default: (f64, f64)) -> (f64, f64) {
    match value {
        Some(Value::Vector { values, .. }) if values.len() >= 2 => {
            (values[0] as f64, values[1] as f64)
        }
        _ => default,
    }
}

fn symbol_bounds(symbol: &Value, resolver: &Resolver) -> Option<(f64, f64, f64, f64)> {
    let (x, y) = map_vec2(symbol.get("position"), (0.0, 0.0));
    let (scale_x, scale_y) = map_vec2(symbol.get("scale"), (1.0, 1.0));
    let rotation = map_number(symbol.get("rotation"), 0.0);
    if ![x, y, scale_x, scale_y, rotation]
        .iter()
        .all(|value| value.is_finite())
    {
        return None;
    }
    let texture = symbol.get("texture").and_then(Value::as_str).unwrap_or("");
    let asset = resolver.asset_info(texture);
    let default_offset = asset
        .as_ref()
        .map(|asset| (asset.offset_x, asset.offset_y))
        .unwrap_or((0.0, 0.0));
    let (offset_x, offset_y) = map_vec2(symbol.get("offset"), default_offset);
    let (sin, cos) = rotation.sin_cos();
    let scaled_offset = (offset_x * scale_x, offset_y * scale_y);
    let center = (
        x + scaled_offset.0 * cos - scaled_offset.1 * sin,
        y + scaled_offset.0 * sin + scaled_offset.1 * cos,
    );
    let (half_width, half_height) = if let Some(asset) = asset {
        let half_width = asset.width * scale_x.abs() / 2.0;
        let half_height = asset.height * scale_y.abs() / 2.0;
        (
            cos.abs() * half_width + sin.abs() * half_height,
            sin.abs() * half_width + cos.abs() * half_height,
        )
    } else {
        let radius = map_number(symbol.get("radius"), 16.0).abs();
        let radius_x = radius * scale_x.abs();
        let radius_y = radius * scale_y.abs();
        (
            ((radius_x * cos).powi(2) + (radius_y * sin).powi(2)).sqrt(),
            ((radius_x * sin).powi(2) + (radius_y * cos).powi(2)).sqrt(),
        )
    };
    let outline = map_number(symbol.get("outline_width"), 0.0).clamp(0.0, 10_000.0);
    Some((
        center.0 - half_width - outline,
        center.1 - half_height - outline,
        center.0 + half_width + outline,
        center.1 + half_height + outline,
    ))
}

fn remove_off_canvas_symbols(root: &mut Value, resolver: &Resolver) -> usize {
    let width = map_number(root.get("map_width"), 512.0).max(0.0);
    let height = map_number(root.get("map_height"), 512.0).max(0.0);
    let Some(Value::Array(symbols)) = root.get_mut("symbols") else {
        return 0;
    };
    let before = symbols.len();
    symbols.retain(|symbol| {
        let Some((min_x, min_y, max_x, max_y)) = symbol_bounds(symbol, resolver) else {
            return true;
        };
        max_x >= 0.0 && max_y >= 0.0 && min_x <= width && min_y <= height
    });
    before - symbols.len()
}

fn color_control(ui: &mut egui::Ui, label: &str, value: &mut [u8; 4]) {
    let mut color = egui::Color32::from_rgba_unmultiplied(value[0], value[1], value[2], value[3]);
    ui.horizontal(|ui| {
        ui.label(label);
        let id_salt = (label, value.as_ptr() as usize);
        if large_color_edit_button(ui, &mut color, id_salt).changed() {
            *value = color.to_array();
        }
        for (component, name) in value.iter_mut().zip(["R", "G", "B", "A"]) {
            ui.add_sized(
                egui::vec2(68.0, ui.spacing().interact_size.y),
                egui::DragValue::new(component)
                    .range(0..=255)
                    .prefix(format!("{name}["))
                    .suffix("]"),
            );
        }
    });
}

// Increase COLOR_PICKER_VERTICAL_GAP if the hue/alpha strips should sit even
// farther away from the two-dimensional color field.
const COLOR_PICKER_SLIDER_WIDTH: f32 = 360.0;
const COLOR_PICKER_VERTICAL_GAP: f32 = 14.0;

fn large_color_edit_button(
    ui: &mut egui::Ui,
    color: &mut egui::Color32,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
) -> egui::Response {
    let popup_id = ui.make_persistent_id(("large-color-picker", id_salt));
    let is_open = egui::Popup::is_id_open(ui.ctx(), popup_id);
    let border = egui::Stroke::new(
        if is_open { 3.0 } else { 2.0 },
        egui::Color32::from_rgb(255, 0, 255),
    );
    let mut response = ui.add_sized(
        egui::vec2(32.0, ui.spacing().interact_size.y),
        egui::Button::new("").fill(*color).stroke(border),
    );
    let mut changed = false;
    egui::Popup::menu(&response)
        .id(popup_id)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .width(COLOR_PICKER_SLIDER_WIDTH + 20.0)
        .show(|ui| {
            // Both the two-dimensional color field and the rainbow hue strip
            // use this width. The hue strip is rendered below the field.
            ui.spacing_mut().slider_width = COLOR_PICKER_SLIDER_WIDTH;
            ui.spacing_mut().item_spacing.y = COLOR_PICKER_VERTICAL_GAP;
            changed = egui::color_picker::color_picker_color32(
                ui,
                color,
                egui::color_picker::Alpha::OnlyBlend,
            );
        });
    if changed {
        response.mark_changed();
    }
    response
}

fn optional_color_control(ui: &mut egui::Ui, label: &str, value: &mut Option<[u8; 4]>) {
    let mut enabled = value.is_some();
    ui.horizontal(|ui| {
        if ui.checkbox(&mut enabled, label).changed() {
            *value = enabled.then_some([0, 0, 0, 255]);
        }
        if let Some(color) = value {
            color_control(ui, "", color);
        } else {
            ui.small("Use SVG value");
        }
    });
}

fn optional_fill_control(ui: &mut egui::Ui, value: &mut Option<[u8; 4]>, no_fill: &mut bool) {
    let mut enabled = value.is_some();
    ui.horizontal(|ui| {
        if ui.checkbox(&mut enabled, "Fill override").changed() {
            *value = enabled.then_some([0, 0, 0, 255]);
            if enabled {
                *no_fill = false;
            }
        }
        if let Some(color) = value {
            color_control(ui, "", color);
        } else {
            ui.small("Use SVG value");
        }
    });
    ui.indent("no-fill-override", |ui| {
        if ui.checkbox(no_fill, "No fill").changed() && *no_fill {
            *value = None;
        }
    });
}

fn optional_width_control(ui: &mut egui::Ui, label: &str, value: &mut Option<f32>) {
    let mut enabled = value.is_some();
    ui.horizontal(|ui| {
        if ui.checkbox(&mut enabled, label).changed() {
            *value = enabled.then_some(1.0);
        }
        if let Some(width) = value {
            ui.add(egui::DragValue::new(width).range(0.0..=100.0));
        } else {
            ui.small("Use SVG value");
        }
    });
}

fn show_render_import_settings(ctx: &egui::Context, window: &mut SvgRendererWindow) {
    let presets: &[(&str, u32, u32)] = &[
        ("Custom", 0, 0),
        ("HD / 1080p (16:9)", 1920, 1080),
        ("QHD / 2K (16:9)", 2560, 1440),
        ("UHD / 4K (16:9)", 3840, 2160),
        ("A4 Paper (300 DPI)", 2480, 3508),
        ("US Letter (300 DPI)", 2550, 3300),
        ("A3 Paper (300 DPI)", 3508, 4960),
        ("Maximum (8192 × 8192)", 8192, 8192),
    ];
    let settings = &mut window.render_settings;
    let source_bounds = window.document.data_bounds().unwrap_or((
        0.0,
        0.0,
        window.document.width as f64,
        window.document.height as f64,
    ));
    let selected_preset = presets
        .iter()
        .find(|(_, width, height)| *width == settings.map_width && *height == settings.map_height)
        .map(|preset| preset.0)
        .unwrap_or("Custom");
    let mut open = true;
    let mut apply = false;
    egui::Window::new("Render Settings")
        .open(&mut open)
        .resizable(true)
        .default_width(720.0)
        .show(ctx, |ui| {
            ui.heading("Output map dimensions");
            ui.horizontal(|ui| {
                egui::ComboBox::from_label("Preset")
                    .selected_text(selected_preset)
                    .show_ui(ui, |ui| {
                        for (name, width, height) in presets {
                            if *width == 0 {
                                let _ = ui.selectable_label(selected_preset == *name, *name);
                            } else if ui.selectable_label(selected_preset == *name, *name).clicked() {
                                settings.map_width = *width;
                                settings.map_height = *height;
                                ui.close();
                            }
                        }
                    });
                let orientation = if settings.map_width == settings.map_height {
                    "Square"
                } else if settings.map_width > settings.map_height {
                    "Landscape"
                } else {
                    "Portrait"
                };
                egui::ComboBox::from_label("Orientation")
                    .selected_text(orientation)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(orientation == "Landscape", "Landscape").clicked() {
                            if settings.map_width < settings.map_height {
                                std::mem::swap(&mut settings.map_width, &mut settings.map_height);
                            }
                            ui.close();
                        }
                        if ui.selectable_label(orientation == "Portrait", "Portrait").clicked() {
                            if settings.map_width > settings.map_height {
                                std::mem::swap(&mut settings.map_width, &mut settings.map_height);
                            }
                            ui.close();
                        }
                        if ui.selectable_label(orientation == "Square", "Square").clicked() {
                            let side = settings.map_width.max(settings.map_height);
                            settings.map_width = side;
                            settings.map_height = side;
                            ui.close();
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut settings.map_width).range(512..=8192).prefix("Width "));
                ui.add(egui::DragValue::new(&mut settings.map_height).range(512..=8192).prefix("Height "));
                ui.label("pixels (512–8192)");
            });
            ui.separator();
            ui.heading("Layers / classes to import");
            ui.horizontal(|ui| {
                if ui.button("Select all").clicked() { settings.selected_layers.fill(true); }
                if ui.button("Deselect all").clicked() { settings.selected_layers.fill(false); }
                ui.label(format!("{} of {} selected", settings.selected_layers.iter().filter(|value| **value).count(), settings.selected_layers.len()));
            });
            egui::ScrollArea::vertical().max_height(145.0).show(ui, |ui| {
                for (index, row) in window.rows.iter().enumerate() {
                    ui.checkbox(&mut settings.selected_layers[index], &row.class_name);
                }
            });
            ui.separator();
            ui.heading("Source viewport");
            ui.label(format!(
                "All data: X {:.2} to {:.2}, Y {:.2} to {:.2}  —  {:.2} × {:.2}",
                source_bounds.0,
                source_bounds.0 + source_bounds.2,
                source_bounds.1,
                source_bounds.1 + source_bounds.3,
                source_bounds.2,
                source_bounds.3,
            ));
            ui.small("This fixed viewport is the total coordinate range across all layers and classes.");
            ui.heading("Selection area");
            ui.small("Only points inside this rectangle are imported. The selection is scaled to the output map dimensions.");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut settings.source_x).prefix("X "));
                ui.add(egui::DragValue::new(&mut settings.source_y).prefix("Y "));
                ui.add(egui::DragValue::new(&mut settings.source_width).range(0.001..=1_000_000.0).prefix("Width "));
                ui.add(egui::DragValue::new(&mut settings.source_height).range(0.001..=1_000_000.0).prefix("Height "));
            });
            ui.horizontal(|ui| {
                if ui.button("Reset selection to source viewport").clicked() {
                    settings.source_x = source_bounds.0;
                    settings.source_y = source_bounds.1;
                    settings.source_width = source_bounds.2;
                    settings.source_height = source_bounds.3;
                }
                if ui.button("Adjust output map aspect ratio to selection").clicked() {
                    let longest = settings.map_width.max(settings.map_height) as f64;
                    let aspect = settings.source_width / settings.source_height.max(f64::EPSILON);
                    if aspect >= 1.0 {
                        settings.map_width = longest.round() as u32;
                        settings.map_height = (longest / aspect).round().clamp(512.0, 8192.0) as u32;
                    } else {
                        settings.map_width = (longest * aspect).round().clamp(512.0, 8192.0) as u32;
                        settings.map_height = longest.round() as u32;
                    }
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Preview layer");
                egui::ComboBox::from_id_salt("render-settings-preview-layer")
                    .selected_text(settings.preview_layer.and_then(|index| window.rows.get(index)).map(|row| row.class_name.as_str()).unwrap_or("No preview"))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut settings.preview_layer, None, "No preview");
                        for (index, row) in window.rows.iter().enumerate() {
                            if settings.selected_layers[index] {
                                ui.selectable_value(&mut settings.preview_layer, Some(index), &row.class_name);
                            }
                        }
                    });
            });
            if let Some(index) = settings.preview_layer.filter(|index| settings.selected_layers.get(*index) == Some(&true)) {
                paint_source_viewport_preview(ui, &window.document, &window.rows[index].class_name, source_bounds, settings, false);
                if ui.button("View full preview").clicked() {
                    window.full_preview_open = true;
                }
            } else {
                ui.small("Choose an imported layer or class to see its source viewport preview.");
            }
            ui.separator();
            if ui.button("Apply settings").clicked() { apply = true; }
        });
    if apply {
        settings.map_width = settings.map_width.clamp(512, 8192);
        settings.map_height = settings.map_height.clamp(512, 8192);
        settings.source_width = settings.source_width.max(0.001);
        settings.source_height = settings.source_height.max(0.001);
        window.render_settings_open = false;
    } else if !open {
        window.render_settings_open = false;
    }
}

fn paint_source_viewport_preview(
    ui: &mut egui::Ui,
    document: &svg_render::Document,
    class_name: &str,
    source_bounds: (f64, f64, f64, f64),
    settings: &mut RenderImportSettings,
    full_size: bool,
) {
    let geometries = document.class_geometry(class_name);
    let points = geometries.iter().flatten().copied().collect::<Vec<_>>();
    if points.is_empty() {
        ui.small("This layer has no point geometry to preview.");
        return;
    }
    let (min_x, min_y, data_width, data_height) = source_bounds;
    let size = if full_size {
        egui::vec2(data_width as f32, data_height as f32)
    } else {
        let available_width = ui.available_width().min(680.0);
        let available_height = 300.0;
        let aspect = (data_width / data_height) as f32;
        if available_width / available_height > aspect {
            egui::vec2(available_height * aspect, available_height)
        } else {
            egui::vec2(available_width, available_width / aspect)
        }
    };
    let (rect, mut response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, egui::Color32::BLACK);
    let to_screen = |point: (f64, f64)| {
        egui::pos2(
            rect.left() + ((point.0 - min_x) / data_width) as f32 * rect.width(),
            rect.bottom() - ((point.1 - min_y) / data_height) as f32 * rect.height(),
        )
    };
    let from_screen = |point: egui::Pos2| {
        (
            min_x + (point.x - rect.left()) as f64 / rect.width() as f64 * data_width,
            min_y + (rect.bottom() - point.y) as f64 / rect.height() as f64 * data_height,
        )
    };
    let selection_screen = egui::Rect::from_two_pos(
        to_screen((
            settings.source_x,
            settings.source_y + settings.source_height,
        )),
        to_screen((settings.source_x + settings.source_width, settings.source_y)),
    );
    let handle_at = |pointer: egui::Pos2| {
        let radius = 10.0;
        let corners = [
            (selection_screen.left_top(), SelectionHandle::NorthWest),
            (selection_screen.right_top(), SelectionHandle::NorthEast),
            (selection_screen.left_bottom(), SelectionHandle::SouthWest),
            (selection_screen.right_bottom(), SelectionHandle::SouthEast),
        ];
        corners
            .into_iter()
            .find_map(|(corner, handle)| (corner.distance(pointer) <= radius).then_some(handle))
            .or_else(|| {
                selection_screen
                    .contains(pointer)
                    .then_some(SelectionHandle::Move)
            })
    };
    if let Some(pointer) = response.hover_pos()
        && let Some(handle) = handle_at(pointer)
    {
        let cursor = match handle {
            SelectionHandle::NorthWest | SelectionHandle::SouthEast => egui::CursorIcon::ResizeNwSe,
            SelectionHandle::NorthEast | SelectionHandle::SouthWest => egui::CursorIcon::ResizeNeSw,
            SelectionHandle::Move => egui::CursorIcon::Grab,
        };
        response = response.on_hover_cursor(cursor);
    }
    if response.drag_started()
        && let Some(pointer) = response.interact_pointer_pos()
        && let Some(handle) = handle_at(pointer)
    {
        settings.selection_drag = Some(SelectionDrag {
            handle,
            pointer: from_screen(pointer),
            x: settings.source_x,
            y: settings.source_y,
            width: settings.source_width,
            height: settings.source_height,
        });
    }
    if response.dragged()
        && let (Some(drag), Some(pointer)) =
            (settings.selection_drag, response.interact_pointer_pos())
    {
        let current = from_screen(pointer);
        let dx = current.0 - drag.pointer.0;
        let dy = current.1 - drag.pointer.1;
        match drag.handle {
            SelectionHandle::Move => {
                settings.source_x = drag.x + dx;
                settings.source_y = drag.y + dy;
            }
            SelectionHandle::NorthWest => {
                settings.source_x = drag.x + dx;
                settings.source_width = (drag.width - dx).max(0.001);
                settings.source_height = (drag.height + dy).max(0.001);
            }
            SelectionHandle::NorthEast => {
                settings.source_width = (drag.width + dx).max(0.001);
                settings.source_height = (drag.height + dy).max(0.001);
            }
            SelectionHandle::SouthWest => {
                settings.source_x = drag.x + dx;
                settings.source_y = drag.y + dy;
                settings.source_width = (drag.width - dx).max(0.001);
                settings.source_height = (drag.height - dy).max(0.001);
            }
            SelectionHandle::SouthEast => {
                settings.source_y = drag.y + dy;
                settings.source_width = (drag.width + dx).max(0.001);
                settings.source_height = (drag.height - dy).max(0.001);
            }
        }
    }
    if response.drag_stopped() {
        settings.selection_drag = None;
    }
    let inside = |point: (f64, f64)| {
        point.0 >= settings.source_x
            && point.0 <= settings.source_x + settings.source_width
            && point.1 >= settings.source_y
            && point.1 <= settings.source_y + settings.source_height
    };
    for geometry in geometries {
        if geometry.len() == 1 {
            let color = if inside(geometry[0]) {
                egui::Color32::RED
            } else {
                egui::Color32::from_rgb(105, 55, 55)
            };
            painter.circle_filled(to_screen(geometry[0]), 3.5, color);
        } else {
            for segment in geometry.windows(2) {
                let color = if inside(segment[0]) && inside(segment[1]) {
                    egui::Color32::from_rgb(65, 150, 255)
                } else {
                    egui::Color32::from_rgb(48, 75, 110)
                };
                painter.line_segment(
                    [to_screen(segment[0]), to_screen(segment[1])],
                    egui::Stroke::new(1.5, color),
                );
            }
        }
    }
    let selection_min = to_screen((
        settings.source_x,
        settings.source_y + settings.source_height,
    ));
    let selection_max = to_screen((settings.source_x + settings.source_width, settings.source_y));
    let selection_rect = egui::Rect::from_two_pos(selection_min, selection_max).intersect(rect);
    painter.rect_stroke(
        selection_rect,
        0.0,
        egui::Stroke::new(3.0, egui::Color32::YELLOW),
        egui::StrokeKind::Inside,
    );
    for corner in [
        selection_rect.left_top(),
        selection_rect.right_top(),
        selection_rect.left_bottom(),
        selection_rect.right_bottom(),
    ] {
        painter.circle_filled(corner, 4.0, egui::Color32::YELLOW);
    }
    ui.small("Red: single points · Blue: multi-point geometry · pale data lies outside the yellow selection area.");
}

fn paint_preview_background(painter: &egui::Painter, rect: egui::Rect, background: usize) {
    if background < 2 {
        painter.rect_filled(
            rect,
            3.0,
            if background == 0 {
                egui::Color32::BLACK
            } else {
                egui::Color32::WHITE
            },
        );
    } else {
        let tile = 14.0;
        let columns = (rect.width() / tile).ceil() as usize;
        let rows = (rect.height() / tile).ceil() as usize;
        for y in 0..rows {
            for x in 0..columns {
                let cell = egui::Rect::from_min_size(
                    rect.min + egui::vec2(x as f32 * tile, y as f32 * tile),
                    egui::vec2(tile, tile),
                )
                .intersect(rect);
                painter.rect_filled(
                    cell,
                    0.0,
                    if (x + y) % 2 == 0 {
                        egui::Color32::from_gray(205)
                    } else {
                        egui::Color32::from_gray(245)
                    },
                );
            }
        }
    }
    painter.rect_stroke(
        rect,
        3.0,
        egui::Stroke::new(1.0, egui::Color32::GRAY),
        egui::StrokeKind::Inside,
    );
}

fn gallery_texture(
    window: &mut SvgRendererWindow,
    asset: &miracledraft_map_helper::assets::AssetInfo,
    ctx: &egui::Context,
) -> Option<egui::TextureHandle> {
    if let Some(texture) = window.gallery_textures.get(&asset.texture) {
        return Some(texture.clone());
    }
    let image = image::open(&asset.path)
        .ok()?
        .thumbnail(128, 128)
        .to_rgba8();
    let texture = ctx.load_texture(
        format!("symbol-gallery:{}", asset.texture),
        egui::ColorImage::from_rgba_unmultiplied(
            [image.width() as usize, image.height() as usize],
            image.as_raw(),
        ),
        egui::TextureOptions::LINEAR,
    );
    window
        .gallery_textures
        .insert(asset.texture.clone(), texture.clone());
    Some(texture)
}

fn path_styles(resolver: &Resolver) -> Vec<PathStyleInfo> {
    let mut roots = Vec::new();
    if let Some(sprites) = &resolver.default
        && let Some(root) = sprites.parent()
    {
        roots.push(root.join("textures/paths"));
    }
    roots.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("wonderdraft_files/textures/paths"));
    let mut styles = Vec::new();
    for root in roots {
        let Ok(entries) = fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file()
                || !path
                    .extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| value.eq_ignore_ascii_case("png"))
            {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
                continue;
            };
            styles.push(PathStyleInfo {
                texture: format!("res://textures/paths/{stem}"),
                path,
            });
        }
    }
    styles.sort_by(|left, right| left.texture.cmp(&right.texture));
    styles.dedup_by(|left, right| left.texture == right.texture);
    styles
}

fn path_style_texture(
    textures: &mut HashMap<String, egui::TextureHandle>,
    style: &PathStyleInfo,
    ctx: &egui::Context,
) -> Option<egui::TextureHandle> {
    if let Some(texture) = textures.get(&style.texture) {
        return Some(texture.clone());
    }
    let image = image::open(&style.path).ok()?.thumbnail(220, 48).to_rgba8();
    let texture = ctx.load_texture(
        format!("path-style:{}", style.texture),
        egui::ColorImage::from_rgba_unmultiplied(
            [image.width() as usize, image.height() as usize],
            image.as_raw(),
        ),
        egui::TextureOptions::LINEAR,
    );
    textures.insert(style.texture.clone(), texture.clone());
    Some(texture)
}

fn path_style_label(texture: &str) -> String {
    texture
        .rsplit('/')
        .next()
        .unwrap_or(texture)
        .trim_start_matches("path_")
        .replace('_', " ")
}

fn draw_mode_label(mode: &str) -> &'static str {
    match mode {
        "sample_color" => "Sample color",
        "custom_colors" => "Three custom colors",
        _ => "Normal",
    }
}

fn load_draw_mode_icons(
    cache_dir: &Path,
    ctx: &egui::Context,
) -> HashMap<String, egui::TextureHandle> {
    let _ = fs::create_dir_all(cache_dir);
    let mut icons = HashMap::new();
    for (mode, name, bytes) in DRAW_MODE_ICONS {
        let source = cache_dir.join(format!("draw_mode_{mode}_{name}"));
        if fs::write(&source, bytes).is_err() {
            continue;
        }
        let output = cache_dir.join(format!("draw_mode_{mode}.png"));
        if !output.is_file() && svg_render::render_preview(&source, &output, 64, 64).is_err() {
            continue;
        }
        let Ok(image) = image::open(&output) else {
            continue;
        };
        let image = image.to_rgba8();
        icons.insert(
            (*mode).into(),
            ctx.load_texture(
                format!("draw-mode-{mode}"),
                egui::ColorImage::from_rgba_unmultiplied(
                    [image.width() as usize, image.height() as usize],
                    image.as_raw(),
                ),
                egui::TextureOptions::LINEAR,
            ),
        );
    }
    icons
}

fn fit_rect(bounds: egui::Rect, source_size: egui::Vec2) -> egui::Rect {
    let scale = (bounds.width() / source_size.x)
        .min(bounds.height() / source_size.y)
        .min(1.0);
    egui::Rect::from_center_size(bounds.center(), source_size * scale)
}

fn truncate_label(value: &str, max_characters: usize) -> String {
    let count = value.chars().count();
    if count <= max_characters {
        value.to_owned()
    } else {
        format!(
            "{}…",
            value
                .chars()
                .take(max_characters.saturating_sub(1))
                .collect::<String>()
        )
    }
}

const SYMBOL_SCALE_SLIDER_MIDDLE: f64 = 50.0;
const SYMBOL_SCALE_STEP_FACTOR: f64 = 1.1;

fn symbol_scale_at_slider(position: f64) -> f32 {
    if position <= 0.0 {
        0.0
    } else {
        SYMBOL_SCALE_STEP_FACTOR.powf(position - SYMBOL_SCALE_SLIDER_MIDDLE) as f32
    }
}

fn slider_position_for_symbol_scale(scale: f32) -> f64 {
    if !scale.is_finite() || scale <= 0.0 {
        0.0
    } else {
        (SYMBOL_SCALE_SLIDER_MIDDLE + f64::from(scale).ln() / SYMBOL_SCALE_STEP_FACTOR.ln())
            .clamp(0.0, 100.0)
    }
}

fn symbol_scale_after_slider_move(current_scale: f32, requested_position: f64) -> f32 {
    if requested_position <= 0.0 {
        return 0.0;
    }
    if !current_scale.is_finite() || current_scale <= 0.0 {
        return symbol_scale_at_slider(requested_position.round());
    }
    let current_position = slider_position_for_symbol_scale(current_scale).round();
    let steps = requested_position.round() - current_position;
    (f64::from(current_scale) * SYMBOL_SCALE_STEP_FACTOR.powf(steps)) as f32
}

fn format_symbol_scale(scale: f32) -> String {
    if scale == 0.0 {
        "0×".into()
    } else if scale >= 10.0 {
        format!("{scale:.1}×")
    } else {
        format!("{scale:.3}×")
    }
}

fn parse_symbol_scale_text(value: &str) -> Option<f64> {
    value
        .trim()
        .trim_end_matches('×')
        .replace(',', ".")
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value >= 0.0)
}

fn format_pixel_dimensions(width: f64, height: f64) -> String {
    format!("{width:.1} × {height:.1} px")
}

fn format_byte_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

fn dropped_path_over(
    ui: &egui::Ui,
    response: &egui::Response,
    accepts: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let pointer = ui.ctx().input(|input| input.pointer.hover_pos())?;
    if !response.rect.contains(pointer) {
        return None;
    }
    ui.ctx().input(|input| {
        input
            .raw
            .dropped_files
            .iter()
            .filter_map(|file| file.path.clone())
            .find(|path| accepts(path))
    })
}

fn is_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extensions
                .iter()
                .any(|allowed| extension.eq_ignore_ascii_case(allowed))
        })
}

fn is_svg_file(path: &Path) -> bool {
    is_extension(path, &["svg"])
}

fn is_csv_file(path: &Path) -> bool {
    is_extension(path, &["csv", "tsv", "txt"])
}

fn is_json_file(path: &Path) -> bool {
    is_extension(path, &["json"])
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1240., 820.])
            .with_min_inner_size([600., 480.]),
        ..Default::default()
    };
    let title = format!("{APP_NAME} {APP_VERSION} — Rust SVG edition");
    eframe::run_native(&title, options, Box::new(|_| Ok(Box::new(App::default()))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict(entries: Vec<(&str, Value)>) -> Value {
        Value::Dictionary(
            entries
                .into_iter()
                .map(|(key, value)| (Value::String(key.into()), value))
                .collect(),
        )
    }

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

    #[test]
    fn off_canvas_cleanup_uses_scaled_and_rotated_symbol_bounds() {
        let base = std::env::temp_dir().join(format!(
            "wonderdraft-off-canvas-symbols-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        image::RgbaImage::new(100, 20)
            .save(base.join("wide.png"))
            .unwrap();
        let position = |x| Value::Vector {
            kind: "Vector2".into(),
            values: vec![x, 50.0],
        };
        let unit_scale = || Value::Vector {
            kind: "Vector2".into(),
            values: vec![1.0, 1.0],
        };
        let symbol = |x, rotation| {
            dict(vec![
                ("position", position(x)),
                ("rotation", Value::Real(rotation)),
                ("scale", unit_scale()),
                ("texture", Value::String("user://assets/wide".into())),
            ])
        };
        let mut root = dict(vec![
            ("map_width", Value::Int(100)),
            ("map_height", Value::Int(100)),
            (
                "symbols",
                Value::Array(vec![
                    symbol(130.0, std::f64::consts::FRAC_PI_2),
                    symbol(105.0, 0.0),
                    dict(vec![
                        ("position", position(200.0)),
                        ("radius", Value::Real(20.0)),
                    ]),
                    dict(vec![
                        ("position", position(125.0)),
                        ("radius", Value::Real(20.0)),
                        ("outline_width", Value::Real(10.0)),
                    ]),
                ]),
            ),
        ]);
        let resolver = Resolver::new(&Settings {
            custom_asset_folder: base.to_string_lossy().into_owned(),
            ..Settings::default()
        });

        assert_eq!(remove_off_canvas_symbols(&mut root, &resolver), 2);
        assert_eq!(root.get("symbols").unwrap().as_array().unwrap().len(), 2);
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn symbol_scale_slider_is_centered_and_compounds_by_ten_percent() {
        assert_eq!(symbol_scale_at_slider(0.0), 0.0);
        assert!((symbol_scale_at_slider(50.0) - 1.0).abs() < f32::EPSILON);
        assert!((symbol_scale_at_slider(51.0) - 1.1).abs() < 0.000_001);
        assert!((symbol_scale_at_slider(52.0) - 1.21).abs() < 0.000_001);
        assert!((slider_position_for_symbol_scale(1.0) - 50.0).abs() < f64::EPSILON);
        assert!((symbol_scale_after_slider_move(2.0, 58.0) - 2.2).abs() < 0.000_001);
        assert_eq!(parse_symbol_scale_text("1,21×"), Some(1.21));
        assert_eq!(parse_symbol_scale_text("1.21"), Some(1.21));
    }
}

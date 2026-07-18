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
use std::{
    fs,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
};

const APP_NAME: &str = "Miracledraft Map Helper";
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

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
    svg_renderer: Option<SvgRendererWindow>,
    svg_render_job: Option<Receiver<(PathBuf, Result<svg_render::RenderSummary>)>>,
}

struct SvgRendererWindow {
    document: svg_render::Document,
    rows: Vec<svg_render::ClassSettings>,
    selected: usize,
    assets: Vec<miracledraft_map_helper::assets::AssetInfo>,
    preview: Option<egui::TextureHandle>,
    preview_texture: String,
    preview_background: usize,
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
        let open_directory = self.wonderdraft_config.last_directory.clone();
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
                DialogAction::RenderSvg => rfd::FileDialog::new()
                    .add_filter("SVG", &["svg"])
                    .pick_file(),
                DialogAction::LoadRenderSettings => rfd::FileDialog::new()
                    .add_filter("CSV table", &["csv"])
                    .pick_file(),
                DialogAction::SaveRenderSettings { file_name } => rfd::FileDialog::new()
                    .set_file_name(file_name)
                    .add_filter("CSV table", &["csv"])
                    .save_file(),
                DialogAction::SaveRenderedMap { file_name } => rfd::FileDialog::new()
                    .set_file_name(file_name)
                    .add_filter("Wonderdraft map", &["wonderdraft_map"])
                    .save_file(),
                DialogAction::ExportMapData { file_name } => rfd::FileDialog::new()
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
            DialogAction::RenderSvg => self.open_svg_renderer(path),
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
        self.settings.setup_completed = true;
        settings::save(&self.settings)?;
        self.cache_dir = cache;
        self.refresh_cache_size();
        self.status = "Setup complete — open or drop a .wonderdraft_map file".into();
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
                        if ui
                            .add_enabled(
                                self.pending_dialog.is_none()
                                    && self.pck_extraction.is_none(),
                                egui::Button::new("Choose Wonderdraft.pck…"),
                            )
                            .clicked()
                        {
                            self.begin_dialog(DialogAction::WonderdraftPck, ctx);
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
    fn open_svg_renderer(&mut self, path: PathBuf) -> Result<()> {
        let document = svg_render::analyze(&path)?;
        let rows = svg_render::default_settings(&document);
        let class_count = rows.len();
        let fallback = document.layer_fallback;
        self.svg_renderer = Some(SvgRendererWindow {
            document,
            rows,
            selected: 0,
            assets: Resolver::new(&self.settings).all_assets(),
            preview: None,
            preview_texture: String::new(),
            preview_background: 2,
        });
        self.status = if fallback {
            format!("SVG has no classes; found {class_count} Inkscape layers")
        } else {
            format!("Found {class_count} SVG classes")
        };
        Ok(())
    }
    fn load_svg_render_settings(&mut self, path: &Path) -> Result<()> {
        let window = self
            .svg_renderer
            .as_mut()
            .ok_or_else(|| Error::format("no SVG renderer is open"))?;
        let count = svg_render::load_csv(path, &mut window.rows)?;
        window.preview = None;
        window.preview_texture.clear();
        self.status = format!("Loaded settings for {count} SVG classes");
        Ok(())
    }
    fn save_svg_render_settings(&mut self, path: &Path) -> Result<()> {
        let window = self
            .svg_renderer
            .as_ref()
            .ok_or_else(|| Error::format("no SVG renderer is open"))?;
        svg_render::save_csv(path, &window.rows)?;
        self.status = format!("Saved SVG render settings to {}", path.display());
        Ok(())
    }
    fn begin_svg_render(&mut self, path: PathBuf, ctx: &egui::Context) {
        if self.svg_render_job.is_some() {
            return;
        }
        let Some(window) = self.svg_renderer.as_ref() else {
            return;
        };
        let document = window.document.clone();
        let rows = window.rows.clone();
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
        self.status = "Rendering SVG layers and creating a new Wonderdraft map…".into();
    }
    fn poll_svg_render(&mut self) {
        let Some(receiver) = self.svg_render_job.take() else {
            return;
        };
        match receiver.try_recv() {
            Ok((path, Ok(summary))) => {
                self.status = format!(
                    "Created {}: {} symbols, {} paths, {} territories, {} labels",
                    path.display(),
                    summary.symbols,
                    summary.paths,
                    summary.territories,
                    summary.labels
                );
                dialog("Wonderdraft map created", &self.status, false);
            }
            Ok((_, Err(error))) => self.fail("Could not render SVG to Wonderdraft map", error),
            Err(TryRecvError::Empty) => self.svg_render_job = Some(receiver),
            Err(TryRecvError::Disconnected) => self.fail(
                "Could not render SVG",
                "SVG render worker stopped unexpectedly",
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
    fn show_svg_renderer(&mut self, ctx: &egui::Context) {
        let Some(window) = self.svg_renderer.as_mut() else {
            return;
        };
        let mut open = true;
        let mut load_csv = false;
        let mut save_csv = false;
        let mut render_map = false;
        let mut preview_changed = false;
        let mut propagated_name: Option<(usize, String)> = None;
        egui::Window::new("Render SVG as new Wonderdraft map")
            .open(&mut open)
            .default_width(920.0)
            .default_height(720.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.label(format!("{} × {} map pixels — {}", window.document.width, window.document.height, if window.document.layer_fallback { "using Inkscape layers as classes" } else { "using SVG classes" }));
                ui.horizontal(|ui| {
                    if ui.button("Load settings CSV…").clicked() { load_csv = true; }
                    if ui.button("Save settings CSV…").clicked() { save_csv = true; }
                    ui.separator();
                    if ui.add_enabled(self.svg_render_job.is_none(), egui::Button::new("Create .wonderdraft_map…")).clicked() { render_map = true; }
                    if self.svg_render_job.is_some() { ui.spinner(); ui.label("Rendering…"); }
                });
                ui.separator();
                ui.columns(2, |columns| {
                    columns[0].heading("Classes / layers");
                    egui::ScrollArea::vertical().id_salt("svg-render-classes").show(&mut columns[0], |ui| {
                        for (index, row) in window.rows.iter().enumerate() {
                            if ui.selectable_label(window.selected == index, format!("{}  →  {}", row.class_name, row.category.label())).clicked() {
                                window.selected = index;
                                preview_changed = true;
                            }
                        }
                    });
                    columns[1].heading("Translation settings");
                    let selected = window.selected;
                    let assets = &window.assets;
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
                            ui.checkbox(&mut row.label.enabled, "Create label");
                            if row.label.enabled {
                                ui.indent("label-options", |ui| {
                                    ui.horizontal(|ui| { ui.label("Font"); ui.text_edit_singleline(&mut row.label.font); });
                                    ui.add(egui::Slider::new(&mut row.label.size, 4.0..=200.0).text("Size"));
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
                                    ui.add(egui::Slider::new(&mut row.symbol_scale, 0.01..=10.).logarithmic(true).text("Scale"));
                                    color_control(ui, "Tint / sample color", &mut row.tint);
                                    ui.horizontal(|ui| { ui.label("Preview background"); ui.selectable_value(preview_background,0,"Black"); ui.selectable_value(preview_background,1,"White"); ui.selectable_value(preview_background,2,"Checkerboard"); });
                                    let (rect, _) = ui.allocate_exact_size(egui::vec2(260.,220.), egui::Sense::hover());
                                    paint_preview_background(ui.painter(), rect, *preview_background);
                                    if let Some(texture)=preview { ui.put(rect.shrink(8.), egui::Image::new(texture).fit_to_exact_size(rect.shrink(8.).size())); } else { ui.painter().text(rect.center(),egui::Align2::CENTER_CENTER,"Preview unavailable\n(select a PNG/WebP/JPEG symbol)",egui::FontId::proportional(14.),egui::Color32::GRAY); }
                                }
                                svg_render::Category::Path | svg_render::Category::Territory => {
                                    ui.horizontal(|ui| { ui.label(if row.category==svg_render::Category::Path {"Path style"} else {"Border style"}); ui.text_edit_singleline(&mut row.path_style); });
                                    color_control(ui,"Color",&mut row.path_color);
                                    ui.add(egui::Slider::new(&mut row.width,0.1..=100.).text("Width"));
                                }
                                svg_render::Category::Ground | svg_render::Category::WaterTint => {
                                    optional_color_control(ui,"Fill override",&mut row.fill_override);
                                    optional_color_control(ui,"Border override",&mut row.border_override);
                                    optional_width_control(ui,"Border width override",&mut row.border_width_override);
                                    ui.small("‘Use SVG value’ preserves the element’s existing presentation value.");
                                }
                                svg_render::Category::Landmass => { ui.add(egui::Slider::new(&mut row.width,0.0..=100.).text("Black mask border width")); ui.small("Rendered black onto mask.png."); }
                                svg_render::Category::Freshwater => { ui.small("Existing fill/stroke colors become red; explicit ‘none’ and opacity are preserved. Rendered after all landmass classes."); }
                                svg_render::Category::Invisible => {}
                            }
                        });
                    }
                });
            });
        if let Some((selected, value)) = propagated_name {
            for next in window.rows.iter_mut().skip(selected + 1) {
                if next.name_attribute == "map:svgname" {
                    next.name_attribute = value.clone();
                }
            }
        }
        if !open {
            self.svg_renderer = None;
            return;
        }
        let csv_file_name = window
            .document
            .source
            .file_stem()
            .map(|v| format!("{}_render_settings.csv", v.to_string_lossy()))
            .unwrap_or_else(|| "svg_render_settings.csv".into());
        let map_file_name = window
            .document
            .source
            .file_stem()
            .map(|v| format!("{}.wonderdraft_map", v.to_string_lossy()))
            .unwrap_or_else(|| "rendered_svg.wonderdraft_map".into());
        if preview_changed {
            self.update_svg_symbol_preview(ctx);
        }
        if load_csv {
            self.begin_dialog(DialogAction::LoadRenderSettings, ctx);
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

        let busy = self.pending_dialog.is_some()
            || self.map_load.is_some()
            || self.pck_extraction.is_some()
            || self.font_installation.is_some();
        let mut recent_selection = None;
        egui::Panel::top("toolbar").show(root_ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(!busy, egui::Button::new("Open map"))
                    .clicked()
                {
                    self.begin_dialog(DialogAction::OpenMap, &ctx);
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
                if ui.button("Render SVG…").clicked() {
                    self.begin_dialog(DialogAction::RenderSvg, &ctx);
                }
                if ui.button("Export map data…").clicked() {
                    let file_name = self
                        .root_path
                        .as_ref()
                        .and_then(|path| path.file_stem())
                        .map(|stem| format!("{}_map_data.txt", stem.to_string_lossy()))
                        .unwrap_or_else(|| "wonderdraft_map_data.txt".into());
                    self.begin_dialog(DialogAction::ExportMapData { file_name }, &ctx);
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
        self.show_svg_renderer(&ctx);
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
        if ui.color_edit_button_srgba(&mut color).changed() {
            *value = color.to_array();
        }
    });
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
}

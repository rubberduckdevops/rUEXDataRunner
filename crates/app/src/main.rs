//! rUEXDataRunner — desktop app (egui/eframe).
//!
//! Watches the Star Citizen screenshots folder, OCRs each new capture, lets the
//! user review/correct the extracted commodity data, and submits it to UEX. Sent
//! reports are saved locally with their status so corrections can be re-submitted
//! from inside the app.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod paths;

use datarunner_core::api::{self, SubmitOptions};
use datarunner_core::config::Config;
use datarunner_core::model::{Extraction, TerminalType};
use datarunner_core::ocr::OcrConfig;
use datarunner_core::pipeline;
use datarunner_core::preprocess;
use datarunner_core::reference::CommodityRef;
use datarunner_core::store::{report_from, ReportState, ReportStore};
use datarunner_core::trade::{self, TradeEntry, TradeLog, TradeOp, TradeSync};
use datarunner_core::Reference;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

fn main() -> eframe::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1150.0, 820.0])
            .with_min_inner_size([820.0, 560.0])
            .with_title("rUEXDataRunner — SC → UEX"),
        ..Default::default()
    };

    eframe::run_native(
        "rUEXDataRunner",
        options,
        Box::new(|cc| {
            theme::apply_sc_theme(&cc.egui_ctx);
            Ok(Box::new(DatarunnerApp::new()))
        }),
    )
}

/// Star Citizen mobiGlas-style dark theme with a cyan accent.
mod theme {
    use egui::{Color32, Rounding, Stroke};

    pub const ACCENT: Color32 = Color32::from_rgb(74, 210, 219); // cyan
    pub const ACCENT_DIM: Color32 = Color32::from_rgb(46, 120, 128);
    pub const PANEL: Color32 = Color32::from_rgb(13, 19, 25);
    pub const TEXT: Color32 = Color32::from_rgb(198, 218, 224);
    pub const BORDER: Color32 = Color32::from_rgb(34, 52, 62);
    pub const WARN: Color32 = Color32::from_rgb(232, 176, 74);
    pub const BAD: Color32 = Color32::from_rgb(226, 104, 96);
    pub const GOOD: Color32 = Color32::from_rgb(120, 206, 140);

    pub fn apply_sc_theme(ctx: &egui::Context) {
        let mut style = (*ctx.style()).clone();
        let mut v = egui::Visuals::dark();

        v.panel_fill = PANEL;
        v.window_fill = PANEL;
        v.window_stroke = Stroke::new(1.0, BORDER);
        v.extreme_bg_color = Color32::from_rgb(7, 11, 14); // text edit backgrounds
        v.faint_bg_color = Color32::from_rgb(18, 27, 34); // striped rows
        v.hyperlink_color = ACCENT;
        v.selection.bg_fill = Color32::from_rgba_unmultiplied(74, 210, 219, 48);
        v.selection.stroke = Stroke::new(1.0, ACCENT);

        let rounding = Rounding::same(2.0);
        let mk = |bg: Color32, weak: Color32, border: Color32, fg: Color32| egui::style::WidgetVisuals {
            bg_fill: bg,
            weak_bg_fill: weak,
            bg_stroke: Stroke::new(1.0, border),
            fg_stroke: Stroke::new(1.0, fg),
            rounding,
            expansion: 0.0,
        };
        v.widgets.noninteractive = mk(PANEL, PANEL, BORDER, TEXT);
        v.widgets.inactive = mk(Color32::from_rgb(20, 30, 38), Color32::from_rgb(20, 30, 38), BORDER, TEXT);
        v.widgets.hovered = mk(Color32::from_rgb(26, 42, 52), Color32::from_rgb(26, 42, 52), ACCENT, Color32::WHITE);
        v.widgets.active = mk(Color32::from_rgb(30, 52, 64), Color32::from_rgb(30, 52, 64), ACCENT, Color32::WHITE);
        v.widgets.open = mk(Color32::from_rgb(22, 34, 42), Color32::from_rgb(22, 34, 42), ACCENT_DIM, TEXT);
        v.window_rounding = rounding;

        style.visuals = v;
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(9.0, 4.0);
        ctx.set_style(style);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Datarunner,
    TradeLog,
    Reports,
    Settings,
}

enum OcrResult {
    Done(Box<Extraction>),
    Failed(String),
}

/// Result of a background network submission, delivered back to the UI thread.
enum SubResult {
    Report {
        /// Screenshot path key, to match the pending item.
        key: String,
        ex: Box<Extraction>,
        supersedes: Option<String>,
        res: Result<datarunner_core::api::SubmitResponse, String>,
    },
    Trade {
        id: String,
        res: Result<datarunner_core::api::TradeResponse, String>,
    },
}

/// Input state for adding a trade log entry.
struct TradeForm {
    operation: TradeOp,
    commodity: Option<(u32, String)>,
    terminal: Option<(u32, String, String)>, // (id, name, system)
    scu: i64,
    price: f64,
    commodity_filter: String,
    terminal_filter: String,
}

impl Default for TradeForm {
    fn default() -> Self {
        TradeForm {
            operation: TradeOp::Buy,
            commodity: None,
            terminal: None,
            scu: 0,
            price: 0.0,
            commodity_filter: String::new(),
            terminal_filter: String::new(),
        }
    }
}

struct DatarunnerApp {
    config: Config,
    config_path: PathBuf,
    reference: Arc<Reference>,
    terminals_sorted: Arc<Vec<datarunner_core::reference::TerminalEntry>>,
    commodities_sorted: Arc<Vec<CommodityRef>>,
    store: ReportStore,

    // Trade log.
    trade_log: TradeLog,
    trade_form: TradeForm,

    tab: Tab,

    // Folder watching / background OCR.
    watcher: Option<datarunner_core::watcher::ScreenshotWatcher>,
    watching: bool,
    processed: HashSet<String>,
    inflight: usize,
    ocr_tx: Sender<OcrResult>,
    ocr_rx: Receiver<OcrResult>,

    // Background network submissions (reports + trades) so the UI never blocks.
    sub_tx: Sender<SubResult>,
    sub_rx: Receiver<SubResult>,
    submitting_reports: HashSet<String>, // screenshot keys currently in flight
    inflight_sub: usize,

    // Pending extractions awaiting review (Datarunner tab). Persisted to
    // `pending_path` so they survive an app restart.
    pending: Vec<Extraction>,
    pending_path: PathBuf,
    pending_cache: String,
    last_pending_save: std::time::Instant,
    last_terminal: Option<(u32, String)>,

    // Editing an existing report for re-submission (Reports tab).
    editing: Option<(String, Extraction)>, // (superseded report id, editable copy)

    logs: VecDeque<String>,
    toast: Option<(String, bool)>,

    /// Cached screenshot textures, keyed by file path, for the inline preview.
    textures: HashMap<String, Option<egui::TextureHandle>>,
}

impl DatarunnerApp {
    fn new() -> Self {
        let config_path = paths::config_path();
        let mut config = Config::load(&config_path).unwrap_or_default();
        // Fill machine defaults on first run, and self-heal if a saved path no
        // longer exists (e.g. the portable folder was moved to a new machine).
        if config.tesseract_exe.as_os_str().is_empty() || !config.tesseract_exe.is_file() {
            config.tesseract_exe = paths::default_tesseract_exe();
        }
        if config.tessdata_dir.as_os_str().is_empty() || !config.tessdata_dir.is_dir() {
            config.tessdata_dir = paths::default_tessdata_dir();
        }
        let _ = config.save(&config_path);

        let reference = Arc::new(Reference::embedded());
        let mut terminals: Vec<_> = reference.terminals().to_vec();
        terminals.sort_by(|a, b| a.name.cmp(&b.name));
        let mut commodities: Vec<_> = reference.commodities().to_vec();
        commodities.sort_by(|a, b| a.name.cmp(&b.name));

        let store_path = paths::data_dir().join("reports.json");
        let store = ReportStore::load(&store_path).unwrap_or_default();
        let mut trade_log = TradeLog::load(&paths::data_dir().join("trades.json")).unwrap_or_default();
        trade_log.reset_pending(); // clear any "Sending…" left by a previous crash

        // Restore any unsubmitted (pending) extractions from the previous session.
        let pending_path = paths::data_dir().join("pending.json");
        let pending: Vec<Extraction> = std::fs::read_to_string(&pending_path)
            .ok()
            .filter(|s| !s.trim().is_empty())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let pending_cache = serde_json::to_string(&pending).unwrap_or_default();

        let (ocr_tx, ocr_rx) = channel();
        let (sub_tx, sub_rx) = channel();

        let mut app = DatarunnerApp {
            config,
            config_path,
            reference,
            terminals_sorted: Arc::new(terminals),
            commodities_sorted: Arc::new(commodities),
            store,
            trade_log,
            trade_form: TradeForm::default(),
            tab: Tab::Datarunner,
            watcher: None,
            watching: false,
            processed: HashSet::new(),
            inflight: 0,
            ocr_tx,
            ocr_rx,
            sub_tx,
            sub_rx,
            submitting_reports: HashSet::new(),
            inflight_sub: 0,
            pending,
            pending_path,
            pending_cache,
            last_pending_save: std::time::Instant::now(),
            last_terminal: None,
            editing: None,
            logs: VecDeque::new(),
            toast: None,
            textures: HashMap::new(),
        };
        if app.config.screenshot_dir.is_dir() {
            app.start_watching();
        }
        app
    }

    /// Load (and cache) a downscaled texture for a screenshot, for inline preview.
    fn screenshot_texture(&mut self, ctx: &egui::Context, path: &str) -> Option<egui::TextureHandle> {
        if let Some(cached) = self.textures.get(path) {
            return cached.clone();
        }
        let tex = decode_thumbnail(path).map(|color| {
            ctx.load_texture(format!("shot::{path}"), color, egui::TextureOptions::LINEAR)
        });
        self.textures.insert(path.to_string(), tex.clone());
        tex
    }

    /// Persist the pending list when it changes (throttled). Captures new
    /// captures, sends/discards, and edits, so nothing is lost on restart.
    fn persist_pending_if_changed(&mut self, force: bool) {
        let serialized = serde_json::to_string(&self.pending).unwrap_or_default();
        if serialized == self.pending_cache {
            return;
        }
        if !force && self.last_pending_save.elapsed() < Duration::from_millis(400) {
            return; // wait; a repaint will retry shortly
        }
        if let Some(parent) = self.pending_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&self.pending_path, &serialized);
        self.pending_cache = serialized;
        self.last_pending_save = std::time::Instant::now();
    }

    fn log(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        log::info!("{msg}");
        self.logs.push_front(msg);
        while self.logs.len() > 200 {
            self.logs.pop_back();
        }
    }

    fn ocr_config(&self) -> OcrConfig {
        let td = &self.config.tessdata_dir;
        OcrConfig {
            tesseract_exe: self.config.tesseract_exe.clone(),
            tessdata_dir: td.clone(),
            lang: "eng_sc".to_string(),
            psm: 6,
            user_words: vec![
                td.join("commodities.user-words"),
                td.join("terminals.user-words"),
            ],
            user_patterns: Some(td.join("sc.patterns")),
        }
    }

    fn submit_options(&self) -> SubmitOptions {
        SubmitOptions {
            base_url: self.config.base_url.clone(),
            secret_key: self.config.secret_key.clone(),
            api_token: self.config.api_token.clone(),
            is_production: self.config.is_production,
            dry_run: self.config.dry_run,
            game_version: if self.config.game_version.trim().is_empty() {
                None
            } else {
                Some(self.config.game_version.clone())
            },
        }
    }

    fn start_watching(&mut self) {
        match datarunner_core::watcher::watch(&self.config.screenshot_dir) {
            Ok(w) => {
                self.watcher = Some(w);
                self.watching = true;
                let dir = self.config.screenshot_dir.display().to_string();
                self.log(format!("Watching {dir}"));
            }
            Err(e) => {
                self.watching = false;
                self.log(format!("Failed to watch folder: {e}"));
                self.toast = Some((format!("Watch failed: {e}"), true));
            }
        }
    }

    fn stop_watching(&mut self) {
        self.watcher = None;
        self.watching = false;
        self.log("Stopped watching");
    }

    fn spawn_ocr(&mut self, path: PathBuf) {
        let key = path.to_string_lossy().to_string();
        if self.processed.contains(&key) {
            return;
        }
        self.processed.insert(key.clone());
        self.inflight += 1;
        let tx = self.ocr_tx.clone();
        let reference = self.reference.clone();
        let cfg = self.ocr_config();
        self.log(format!(
            "Processing {}",
            path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or(key)
        ));
        std::thread::spawn(move || {
            let mut last_err = String::new();
            for _ in 0..6 {
                match pipeline::process_screenshot(&reference, &cfg, &path) {
                    Ok(ex) => {
                        let _ = tx.send(OcrResult::Done(Box::new(ex)));
                        return;
                    }
                    Err(e) => {
                        last_err = e.to_string();
                        std::thread::sleep(Duration::from_millis(300));
                    }
                }
            }
            let _ = tx.send(OcrResult::Failed(format!("{}: {last_err}", path.display())));
        });
    }

    /// Drain watcher + OCR channels; called every frame.
    fn pump(&mut self) {
        // New screenshots from the watcher.
        let mut new_paths = Vec::new();
        if let Some(w) = &self.watcher {
            while let Ok(p) = w.rx.try_recv() {
                new_paths.push(p);
            }
        }
        for p in new_paths {
            self.spawn_ocr(p);
        }

        // Completed OCR jobs.
        loop {
            let msg = match self.ocr_rx.try_recv() {
                Ok(m) => m,
                Err(_) => break,
            };
            self.inflight = self.inflight.saturating_sub(1);
            match msg {
                OcrResult::Done(ex) => {
                    let mut ex = *ex;
                    // Terminal carry-over: reuse the last known terminal when this
                    // scroll-continuation screenshot didn't show the header.
                    if ex.id_terminal.is_none() {
                        if let Some((id, name)) = &self.last_terminal {
                            ex.id_terminal = Some(*id);
                            ex.terminal_name = Some(name.clone());
                            ex.terminal_confidence = 0; // inherited, not detected
                        }
                    } else if let (Some(id), Some(name)) = (ex.id_terminal, &ex.terminal_name) {
                        self.last_terminal = Some((id, name.clone()));
                    }
                    let n = ex.commodities.len();
                    let term = ex.terminal_name.clone().unwrap_or_else(|| "unknown".into());
                    self.log(format!("Extracted {n} commodities @ {term}"));
                    self.pending.push(ex);
                }
                OcrResult::Failed(e) => {
                    self.log(format!("OCR failed: {e}"));
                }
            }
        }

        // Completed submissions (reports + trades).
        while let Ok(msg) = self.sub_rx.try_recv() {
            self.inflight_sub = self.inflight_sub.saturating_sub(1);
            match msg {
                SubResult::Report { key, ex, supersedes, res } => {
                    self.on_report_result(key, *ex, supersedes, res);
                }
                SubResult::Trade { id, res } => match res {
                    Ok(resp) => {
                        if let Some(e) = self.trade_log.get_mut(&id) {
                            apply_trade_response(e, &resp);
                        }
                        let _ = self.trade_log.save();
                        let ok = resp.is_ok();
                        let msg = if resp.dry_run {
                            "Trade logged (dry run — not sent to UEX)".to_string()
                        } else if ok {
                            format!("Trade synced (UEX #{:?})", resp.id_user_trade)
                        } else {
                            format!("UEX rejected trade: {}", resp.status)
                        };
                        self.toast = Some((msg, !ok));
                    }
                    Err(e) => {
                        if let Some(en) = self.trade_log.get_mut(&id) {
                            en.sync = TradeSync::Local;
                            en.status_text = e.clone();
                        }
                        let _ = self.trade_log.save();
                        self.toast = Some((format!("Trade not synced: {e}"), true));
                    }
                },
            }
        }
    }

    /// Submit an extraction on a background thread (never blocks the UI). The
    /// result is delivered via `sub_rx` and applied in [`Self::pump`].
    fn spawn_report_submit(&mut self, ex: Extraction, supersedes: Option<String>) {
        let key = ex.screenshot.clone();
        if self.submitting_reports.contains(&key) {
            return; // already in flight
        }
        self.submitting_reports.insert(key.clone());
        self.inflight_sub += 1;
        let opts = self.submit_options();
        let tx = self.sub_tx.clone();
        std::thread::spawn(move || {
            // Encode evidence (cropped panel) off the UI thread too.
            let screenshot_b64 = if !opts.dry_run {
                let p = PathBuf::from(&ex.screenshot);
                if p.is_file() {
                    datarunner_core::deskew::panel_color(&p, datarunner_core::deskew::Side::Right, 1.0)
                        .ok()
                        .and_then(|img| preprocess::encode_rgba_base64(&img).ok())
                        .or_else(|| preprocess::encode_screenshot_base64(&p).ok())
                } else {
                    None
                }
            } else {
                None
            };
            let res = api::submit(&ex, &opts, screenshot_b64.as_deref()).map_err(|e| e.to_string());
            let _ = tx.send(SubResult::Report { key, ex: Box::new(ex), supersedes, res });
        });
    }

    /// Submit a trade entry to UEX on a background thread.
    fn spawn_trade_submit(&mut self, entry: TradeEntry) {
        self.inflight_sub += 1;
        let opts = self.submit_options();
        let tx = self.sub_tx.clone();
        let id = entry.id.clone();
        std::thread::spawn(move || {
            let res = datarunner_core::api::submit_trade(&entry, &opts).map_err(|e| e.to_string());
            let _ = tx.send(SubResult::Trade { id, res });
        });
    }

    /// Apply a completed report submission (called from `pump`).
    fn on_report_result(
        &mut self,
        key: String,
        ex: Extraction,
        supersedes: Option<String>,
        res: Result<datarunner_core::api::SubmitResponse, String>,
    ) {
        self.submitting_reports.remove(&key);
        match res {
            Ok(resp) => {
                let ok = resp.is_ok();
                let msg = if resp.dry_run {
                    format!("Dry run OK — {} row(s) prepared", api::submittable_row_count(&ex))
                } else if ok {
                    format!("Submitted — reports {:?}", resp.ids_reports)
                } else if resp.message.is_empty() {
                    format!("Rejected — {} (http {})", resp.status, resp.http_code)
                } else {
                    format!("Rejected — {} — {}", resp.status, resp.message)
                };
                if let Err(e) = self.store.add(report_from(&ex, &resp, supersedes)) {
                    self.log(format!("Failed to save report: {e}"));
                }
                if ok {
                    if !resp.dry_run && self.config.delete_after_send {
                        let p = PathBuf::from(&ex.screenshot);
                        if p.is_file() {
                            let _ = std::fs::remove_file(&p);
                        }
                    }
                    // Remove the (now submitted) pending item.
                    self.pending.retain(|p| p.screenshot != key);
                }
                self.log(msg.clone());
                self.toast = Some((msg, !ok));
            }
            Err(e) => {
                let msg = format!("Submit error: {e}");
                self.log(msg.clone());
                self.toast = Some((msg, true));
            }
        }
    }
}

impl eframe::App for DatarunnerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.pump();
        // Keep polling channels even without user input.
        ctx.request_repaint_after(Duration::from_millis(400));

        self.top_bar(ctx);
        self.status_bar(ctx);

        match self.tab {
            Tab::Datarunner => self.ui_datarunner(ctx),
            Tab::TradeLog => self.ui_trade_log(ctx),
            Tab::Reports => self.ui_reports(ctx),
            Tab::Settings => self.ui_settings(ctx),
        }

        // Persist pending edits/additions/removals.
        self.persist_pending_if_changed(false);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Flush any last changes on close.
        self.persist_pending_if_changed(true);
    }
}

// ---------------------------------------------------------------------------
// UI
// ---------------------------------------------------------------------------

impl DatarunnerApp {
    fn top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("rUEXDataRunner");
                ui.separator();
                ui.selectable_value(&mut self.tab, Tab::Datarunner, "📸 Datarunner");
                ui.selectable_value(&mut self.tab, Tab::TradeLog, "💱 Trade Log");
                ui.selectable_value(&mut self.tab, Tab::Reports, "🗂 Reports");
                ui.selectable_value(&mut self.tab, Tab::Settings, "⚙ Settings");

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.config.dry_run {
                        ui.colored_label(egui::Color32::from_rgb(230, 170, 40), "DRY-RUN");
                    } else {
                        ui.colored_label(egui::Color32::from_rgb(220, 90, 90), "LIVE");
                    }
                    ui.separator();
                    if self.watching {
                        ui.colored_label(egui::Color32::from_rgb(90, 200, 120), "● watching");
                    } else {
                        ui.label("○ not watching");
                    }
                    if self.inflight > 0 {
                        ui.spinner();
                        ui.label(format!("OCR×{}", self.inflight));
                    }
                    if self.inflight_sub > 0 {
                        ui.spinner();
                        ui.label(format!("Sending×{}", self.inflight_sub));
                    }
                });
            });
        });
    }

    fn status_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            if let Some((msg, is_err)) = &self.toast {
                let color = if *is_err {
                    egui::Color32::from_rgb(220, 100, 100)
                } else {
                    egui::Color32::from_rgb(120, 200, 130)
                };
                ui.colored_label(color, msg);
            } else if let Some(latest) = self.logs.front() {
                ui.label(latest);
            } else {
                ui.label("Ready.");
            }
        });
    }

    fn ui_datarunner(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                if self.watching {
                    if ui.button("⏸ Stop watching").clicked() {
                        self.stop_watching();
                    }
                } else if ui.button("▶ Start watching").clicked() {
                    self.start_watching();
                }
                if ui.button("📂 Process screenshot…").clicked() {
                    if let Some(files) = rfd::FileDialog::new()
                        .add_filter("Screenshots", &["jpg", "jpeg", "png"])
                        .set_directory(&self.config.screenshot_dir)
                        .pick_files()
                    {
                        for f in files {
                            self.spawn_ocr(f);
                        }
                    }
                }
                ui.separator();
                let submittable = self
                    .pending
                    .iter()
                    .filter(|e| e.is_submittable())
                    .count();
                ui.label(format!("{} pending ({} ready)", self.pending.len(), submittable));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(submittable > 0, egui::Button::new("🚀 Send All"))
                        .clicked()
                    {
                        self.send_all();
                    }
                    if !self.pending.is_empty() && ui.button("🗑 Clear all").clicked() {
                        self.pending.clear();
                    }
                });
            });
            ui.separator();

            if self.pending.is_empty() {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| {
                    ui.label("No screenshots processed yet.");
                    ui.label(
                        "Press Print Screen at a trade terminal in-game, or use \
                         “Process screenshot…” to load one manually.",
                    );
                });
                return;
            }

            let terminals = self.terminals_sorted.clone();
            let reference = self.reference.clone();
            let mut send_index: Option<usize> = None;
            let mut remove_index: Option<usize> = None;

            // Preload screenshot textures + in-flight flags before the mutable loop.
            let paths: Vec<String> = self.pending.iter().map(|e| e.screenshot.clone()).collect();
            let texs: Vec<Option<egui::TextureHandle>> =
                paths.iter().map(|p| self.screenshot_texture(ctx, p)).collect();
            let submitting: Vec<bool> =
                paths.iter().map(|p| self.submitting_reports.contains(p)).collect();

            egui::ScrollArea::vertical().show(ui, |ui| {
                for (i, ex) in self.pending.iter_mut().enumerate() {
                    let title = format!(
                        "{}  —  {}  ({} rows)",
                        PathBuf::from(&ex.screenshot)
                            .file_name()
                            .map(|f| f.to_string_lossy().to_string())
                            .unwrap_or_default(),
                        ex.terminal_name.clone().unwrap_or_else(|| "· pick terminal ·".into()),
                        ex.commodities.len(),
                    );
                    egui::CollapsingHeader::new(title)
                        .id_salt(("pending", i))
                        .default_open(true)
                        .show(ui, |ui| {
                            extraction_editor(ui, ex, reference.as_ref(), terminals.as_slice(), ("p", i), texs[i].clone());
                            ui.horizontal(|ui| {
                                let busy = submitting[i];
                                if ui
                                    .add_enabled(
                                        ex.is_submittable() && !busy,
                                        egui::Button::new("🚀 Send"),
                                    )
                                    .clicked()
                                {
                                    send_index = Some(i);
                                }
                                if ui.add_enabled(!busy, egui::Button::new("Discard")).clicked() {
                                    remove_index = Some(i);
                                }
                                if busy {
                                    ui.spinner();
                                    ui.label("sending…");
                                } else if !ex.is_submittable() {
                                    ui.colored_label(
                                        egui::Color32::from_rgb(220, 170, 60),
                                        "needs terminal + at least one commodity",
                                    );
                                }
                            });
                        });
                }
            });

            if let Some(i) = send_index {
                let ex = self.pending[i].clone();
                // Submits on a background thread; the pending item is removed when
                // the result arrives (see on_report_result).
                self.spawn_report_submit(ex, None);
            }
            if let Some(i) = remove_index {
                if i < self.pending.len() {
                    self.pending.remove(i);
                }
            }
        });
    }

    fn send_all(&mut self) {
        let batch: Vec<Extraction> = self
            .pending
            .iter()
            .filter(|e| e.is_submittable() && !self.submitting_reports.contains(&e.screenshot))
            .cloned()
            .collect();
        let n = batch.len();
        for ex in batch {
            self.spawn_report_submit(ex, None);
        }
        self.log(format!("Send All: submitting {n} in background"));
    }

    fn ui_reports(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            // If we're editing a report for re-submission, show the editor.
            if self.editing.is_some() {
                self.ui_report_editor(ui);
                return;
            }

            ui.horizontal(|ui| {
                ui.heading("Submitted reports");
                ui.label(format!("({} total)", self.store.reports().len()));
            });
            ui.label(
                "Every submission is saved here with its status. Select one to correct \
                 and re-submit an update — no need to visit the UEX website.",
            );
            ui.separator();

            let active = self.store.active();
            if active.is_empty() {
                ui.add_space(30.0);
                ui.vertical_centered(|ui| ui.label("Nothing submitted yet."));
                return;
            }

            let mut edit_id: Option<String> = None;
            egui::ScrollArea::vertical().show(ui, |ui| {
                for r in active {
                    let (color, _) = state_style(r.state);
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.colored_label(color, r.state.label());
                            ui.separator();
                            ui.strong(&r.terminal_name);
                            ui.label(format!("[{}]", r.terminal_type.as_str()));
                            ui.separator();
                            ui.label(fmt_time(r.submitted_at));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.button("✏ Edit & re-submit").clicked() {
                                        edit_id = Some(r.id.clone());
                                    }
                                },
                            );
                        });
                        ui.label(format!(
                            "{} commodities · {}",
                            r.commodities.len(),
                            r.status_text
                        ));
                        if !r.ids_reports.is_empty() {
                            ui.label(format!("UEX report ids: {}", r.ids_reports.join(", ")));
                        }
                    });
                }
            });

            if let Some(id) = edit_id {
                if let Some(rep) = self.store.get(&id) {
                    self.editing = Some((id.clone(), rep.to_extraction()));
                    self.toast = None;
                }
            }
        });
    }

    fn ui_report_editor(&mut self, ui: &mut egui::Ui) {
        let terminals = self.terminals_sorted.clone();
        let reference = self.reference.clone();
        let (superseded_id, mut ex) = self.editing.take().unwrap();
        let ctx = ui.ctx().clone();
        let tex = self.screenshot_texture(&ctx, &ex.screenshot);

        ui.horizontal(|ui| {
            ui.heading("Edit & re-submit");
            ui.label("(creates a corrected report that supersedes the original)");
        });
        ui.separator();

        let mut action: Option<bool> = None; // Some(true)=submit, Some(false)=cancel
        egui::ScrollArea::vertical().show(ui, |ui| {
            extraction_editor(ui, &mut ex, reference.as_ref(), terminals.as_slice(), ("edit", 0), tex.clone());
            ui.separator();
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(ex.is_submittable(), egui::Button::new("🚀 Re-submit update"))
                    .clicked()
                {
                    action = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    action = Some(false);
                }
            });
        });

        match action {
            Some(true) => {
                self.spawn_report_submit(ex, Some(superseded_id));
                // editing stays None (taken) -> back to list
            }
            Some(false) => {
                // discard edits, back to list
            }
            None => {
                // keep editing
                self.editing = Some((superseded_id, ex));
            }
        }
    }

    fn ui_trade_log(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Trade Log");
                ui.label(egui::RichText::new("record buys & sells; push to your UEX journal").weak());
            });
            ui.separator();

            let commodities = self.commodities_sorted.clone();
            let terminals = self.terminals_sorted.clone();

            // --- Add entry form ---
            let mut do_add = false;
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.selectable_value(&mut self.trade_form.operation, TradeOp::Buy, "  Buy  ");
                    ui.selectable_value(&mut self.trade_form.operation, TradeOp::Sell, "  Sell  ");
                    ui.separator();

                    let comm_text = self
                        .trade_form
                        .commodity
                        .as_ref()
                        .map(|(_, n)| n.clone())
                        .unwrap_or_else(|| "Commodity…".into());
                    let picked = filter_combo(
                        ui,
                        "trade_comm",
                        &comm_text,
                        &mut self.trade_form.commodity_filter,
                        commodities.iter().map(|c| (c.id, c.name.clone(), String::new())),
                    );
                    if let Some((id, name, _)) = picked {
                        self.trade_form.commodity = Some((id, name));
                    }

                    let term_text = self
                        .trade_form
                        .terminal
                        .as_ref()
                        .map(|(_, n, s)| terminal_label(n, None, s))
                        .unwrap_or_else(|| "Terminal…".into());
                    let picked_t = filter_combo(
                        ui,
                        "trade_term",
                        &term_text,
                        &mut self.trade_form.terminal_filter,
                        terminals.iter().map(|t| {
                            let name = match &t.operator {
                                Some(op) => format!("{op} · {}", t.name),
                                None => t.name.clone(),
                            };
                            (t.id, name, t.system.clone())
                        }),
                    );
                    if let Some((id, name, sys)) = picked_t {
                        self.trade_form.terminal = Some((id, name, sys));
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("SCU:");
                    ui.add(egui::DragValue::new(&mut self.trade_form.scu).range(0..=10_000_000).speed(1));
                    ui.separator();
                    ui.label("Price/SCU:");
                    ui.add(egui::DragValue::new(&mut self.trade_form.price).range(0.0..=100_000_000.0).speed(1.0).suffix(" aUEC"));
                    ui.separator();
                    let total = self.trade_form.price * self.trade_form.scu as f64;
                    ui.label(egui::RichText::new(format!("Total: {}", fmt_auec(total))).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("➕  Log trade").clicked() {
                            do_add = true;
                        }
                    });
                });
            });
            if do_add {
                self.add_trade_from_form();
            }

            // --- Totals ---
            let (spent, earned, net) = self.trade_log.totals();
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(format!("Spent: {}", fmt_auec(spent)));
                ui.separator();
                ui.label(format!("Earned: {}", fmt_auec(earned)));
                ui.separator();
                let col = if net >= 0.0 { theme::GOOD } else { theme::BAD };
                ui.colored_label(col, format!("Net: {}", fmt_auec(net)));
                ui.separator();
                ui.label(format!("{} entries", self.trade_log.entries().len()));
            });
            ui.separator();

            // --- Entries table ---
            if self.trade_log.entries().is_empty() {
                ui.add_space(24.0);
                ui.vertical_centered(|ui| ui.label("No trades logged yet."));
                return;
            }
            let mut delete: Option<String> = None;
            let mut sync: Option<String> = None;
            egui::ScrollArea::vertical().show(ui, |ui| {
                egui::Grid::new("trades_grid")
                    .num_columns(9)
                    .striped(true)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        for h in ["When", "Op", "Commodity", "Terminal", "SCU", "Price", "Total", "UEX", ""] {
                            ui.strong(h);
                        }
                        ui.end_row();

                        for e in self.trade_log.newest_first() {
                            ui.label(fmt_time(e.created_at));
                            let (op_col, op_txt) = match e.operation {
                                TradeOp::Buy => (theme::ACCENT, "BUY"),
                                TradeOp::Sell => (theme::WARN, "SELL"),
                            };
                            ui.colored_label(op_col, op_txt);
                            ui.label(&e.commodity_name);
                            ui.label(terminal_label(&e.terminal_name, None, &e.terminal_system));
                            ui.label(e.scu.to_string());
                            ui.label(fmt_auec(e.price));
                            ui.label(fmt_auec(e.total()));
                            let sync_col = match e.sync {
                                datarunner_core::trade::TradeSync::Synced => theme::GOOD,
                                datarunner_core::trade::TradeSync::Failed => theme::BAD,
                                _ => theme::WARN,
                            };
                            ui.colored_label(sync_col, e.sync.label());
                            ui.horizontal(|ui| {
                                let can_sync = !matches!(
                                    e.sync,
                                    datarunner_core::trade::TradeSync::Synced
                                        | datarunner_core::trade::TradeSync::Pending
                                );
                                if can_sync
                                    && ui.small_button("⟳").on_hover_text("Push to UEX").clicked()
                                {
                                    sync = Some(e.id.clone());
                                }
                                if ui.small_button("✖").on_hover_text("Delete").clicked() {
                                    delete = Some(e.id.clone());
                                }
                            });
                            ui.end_row();
                        }
                    });
            });
            if let Some(id) = sync {
                self.sync_trade(&id);
            }
            if let Some(id) = delete {
                let _ = self.trade_log.remove(&id);
            }
        });
    }

    fn add_trade_from_form(&mut self) {
        let Some((cid, cname)) = self.trade_form.commodity.clone() else {
            self.toast = Some(("Pick a commodity first".into(), true));
            return;
        };
        let Some((tid, tname, tsys)) = self.trade_form.terminal.clone() else {
            self.toast = Some(("Pick a terminal first".into(), true));
            return;
        };
        if self.trade_form.scu <= 0 || self.trade_form.price <= 0.0 {
            self.toast = Some(("Enter SCU and price".into(), true));
            return;
        }
        let mut entry = trade::new_entry(
            self.trade_form.operation,
            cid,
            cname,
            tid,
            tname,
            tsys,
            self.trade_form.scu as u32,
            self.trade_form.price,
            chrono::Utc::now().timestamp(),
        );
        // Log locally immediately; push to UEX on a background thread.
        entry.sync = TradeSync::Pending;
        match self.trade_log.add(entry) {
            Ok(id) => {
                if let Some(e) = self.trade_log.get_mut(&id).map(|e| e.clone()) {
                    self.spawn_trade_submit(e);
                }
            }
            Err(e) => self.log(format!("Failed to save trade: {e}")),
        }
        // Keep op/commodity/terminal for rapid entry; clear amounts.
        self.trade_form.scu = 0;
        self.trade_form.price = 0.0;
    }

    fn sync_trade(&mut self, id: &str) {
        let entry = match self.trade_log.get_mut(id) {
            Some(e) => {
                e.sync = TradeSync::Pending;
                e.clone()
            }
            None => return,
        };
        let _ = self.trade_log.save();
        self.spawn_trade_submit(entry);
    }

    fn ui_settings(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Settings");
            ui.separator();
            egui::Grid::new("settings_grid")
                .num_columns(2)
                .spacing([12.0, 10.0])
                .show(ui, |ui| {
                    ui.label("UEX Secret Key");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.config.secret_key)
                            .password(true)
                            .desired_width(360.0)
                            .hint_text("from uexcorp.space account page"),
                    );
                    ui.end_row();

                    ui.label("UEX App API Token");
                    ui.vertical(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.config.api_token)
                                .password(true)
                                .desired_width(360.0)
                                .hint_text("Bearer token from uexcorp.space/api/apps"),
                        );
                        ui.small("Required to submit. Create an app at uexcorp.space/api/apps and paste its token here.");
                    });
                    ui.end_row();

                    ui.label("Screenshots folder");
                    ui.horizontal(|ui| {
                        let mut s = self.config.screenshot_dir.display().to_string();
                        if ui.add(egui::TextEdit::singleline(&mut s).desired_width(300.0)).changed() {
                            self.config.screenshot_dir = PathBuf::from(s);
                        }
                        if ui.button("Browse…").clicked() {
                            if let Some(d) = rfd::FileDialog::new().pick_folder() {
                                self.config.screenshot_dir = d;
                            }
                        }
                    });
                    ui.end_row();

                    ui.label("Tesseract executable");
                    ui.horizontal(|ui| {
                        let mut s = self.config.tesseract_exe.display().to_string();
                        if ui.add(egui::TextEdit::singleline(&mut s).desired_width(300.0)).changed() {
                            self.config.tesseract_exe = PathBuf::from(s);
                        }
                        if ui.button("Browse…").clicked() {
                            if let Some(f) = rfd::FileDialog::new()
                                .add_filter("Executable", &["exe"])
                                .pick_file()
                            {
                                self.config.tesseract_exe = f;
                            }
                        }
                    });
                    ui.end_row();

                    ui.label("Tessdata folder");
                    ui.horizontal(|ui| {
                        let mut s = self.config.tessdata_dir.display().to_string();
                        if ui.add(egui::TextEdit::singleline(&mut s).desired_width(300.0)).changed() {
                            self.config.tessdata_dir = PathBuf::from(s);
                        }
                        if ui.button("Browse…").clicked() {
                            if let Some(d) = rfd::FileDialog::new().pick_folder() {
                                self.config.tessdata_dir = d;
                            }
                        }
                    });
                    ui.end_row();

                    ui.label("Environment");
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.config.environment, "LIVE".into(), "LIVE");
                        ui.selectable_value(&mut self.config.environment, "PTU".into(), "PTU");
                    });
                    ui.end_row();

                    ui.label("Game version (optional)");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.config.game_version)
                            .desired_width(200.0)
                            .hint_text("e.g. LIVE 4.9"),
                    );
                    ui.end_row();

                    ui.label("UEX API base URL");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.config.base_url).desired_width(360.0),
                    );
                    ui.end_row();
                });

            ui.add_space(8.0);
            ui.group(|ui| {
                ui.label(egui::RichText::new("Submission mode").strong());
                ui.checkbox(
                    &mut self.config.dry_run,
                    "Dry-run (build & validate payloads, but never send to UEX)",
                );
                ui.add_enabled_ui(!self.config.dry_run, |ui| {
                    ui.checkbox(
                        &mut self.config.is_production,
                        "Publish as production data (uncheck to send test rows)",
                    );
                });
                ui.checkbox(
                    &mut self.config.delete_after_send,
                    "Delete screenshot after a successful live send",
                );
                if !self.config.dry_run {
                    ui.colored_label(
                        egui::Color32::from_rgb(220, 120, 120),
                        "⚠ LIVE mode: sends post real data to the UEX community database.",
                    );
                }
            });

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button("💾 Save settings").clicked() {
                    match self.config.save(&self.config_path) {
                        Ok(()) => {
                            self.toast = Some(("Settings saved".into(), false));
                            // Restart watcher if the folder is valid.
                            if self.config.screenshot_dir.is_dir() {
                                self.stop_watching();
                                self.start_watching();
                            }
                        }
                        Err(e) => self.toast = Some((format!("Save failed: {e}"), true)),
                    }
                }
                let ocr_ok = self.ocr_config().is_available();
                if ocr_ok {
                    ui.colored_label(egui::Color32::from_rgb(120, 200, 130), "OCR engine ready ✔");
                } else {
                    ui.colored_label(
                        egui::Color32::from_rgb(220, 120, 120),
                        "OCR engine not found — check Tesseract paths",
                    );
                }
            });

            ui.add_space(14.0);
            ui.collapsing("Activity log", |ui| {
                egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                    for line in &self.logs {
                        ui.label(line);
                    }
                });
            });
        });
    }
}

// ---------------------------------------------------------------------------
// Shared editor widget
// ---------------------------------------------------------------------------

/// Render the terminal selector, buy/sell toggle, and the editable commodity grid.
fn extraction_editor(
    ui: &mut egui::Ui,
    ex: &mut Extraction,
    reference: &Reference,
    terminals: &[datarunner_core::reference::TerminalEntry],
    salt: (&str, usize),
    screenshot_tex: Option<egui::TextureHandle>,
) {
    ui.horizontal_top(|ui| {
    ui.vertical(|ui| {
    ui.set_max_width(740.0);
    ui.horizontal(|ui| {
        ui.label("Terminal:");
        // Show the selected terminal with its star system for confirmation.
        let current = match ex.id_terminal.and_then(|id| terminals.iter().find(|t| t.id == id)) {
            Some(t) => terminal_label(&t.name, t.operator.as_deref(), &t.system),
            None => ex.terminal_name.clone().unwrap_or_else(|| "· select ·".into()),
        };
        egui::ComboBox::from_id_salt((salt.0, "term", salt.1))
            .selected_text(current)
            .width(340.0)
            .show_ui(ui, |ui| {
                // Detected candidates first for convenience.
                if !ex.terminal_candidates.is_empty() {
                    ui.label(egui::RichText::new("detected").weak());
                    let cands = ex.terminal_candidates.clone();
                    for c in &cands {
                        let sel = ex.id_terminal == Some(c.id);
                        let label = terminal_label(&c.name, c.operator.as_deref(), &c.system);
                        if ui.selectable_label(sel, label).clicked() {
                            ex.id_terminal = Some(c.id);
                            ex.terminal_name = Some(c.name.clone());
                        }
                    }
                    ui.separator();
                }
                for t in terminals {
                    let sel = ex.id_terminal == Some(t.id);
                    let label = terminal_label(&t.name, t.operator.as_deref(), &t.system);
                    if ui.selectable_label(sel, label).clicked() {
                        ex.id_terminal = Some(t.id);
                        ex.terminal_name = Some(t.name.clone());
                    }
                }
            });

        if ex.terminal_confidence > 0 {
            ui.weak(format!("conf {}", ex.terminal_confidence));
        } else if ex.id_terminal.is_some() {
            ui.weak("inherited/manual");
        }

        ui.separator();
        ui.label("Type:");
        let mut ttype = ex.terminal_type.unwrap_or(TerminalType::Sell);
        egui::ComboBox::from_id_salt((salt.0, "type", salt.1))
            .selected_text(ttype.as_str())
            .width(70.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut ttype, TerminalType::Buy, "buy");
                ui.selectable_value(&mut ttype, TerminalType::Sell, "sell");
            });
        ex.terminal_type = Some(ttype);
    });

    ui.add_space(4.0);

    let mut delete_row: Option<usize> = None;
    egui::Grid::new((salt.0, "grid", salt.1))
        .num_columns(7)
        .striped(true)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            ui.strong("Send");
            ui.strong("Commodity");
            ui.strong("Stock");
            ui.strong("SCU");
            ui.strong("Price");
            ui.strong("Conf");
            ui.strong("");
            ui.end_row();

            for (ci, c) in ex.commodities.iter_mut().enumerate() {
                ui.checkbox(&mut c.include, "");

                // Name (editable; re-match on losing focus).
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut c.name).desired_width(150.0),
                );
                if resp.lost_focus() {
                    if let Some(m) = reference.best_commodity(&c.name) {
                        c.id_commodity = Some(m.id);
                        c.name = m.name;
                        c.name_confidence = m.score;
                    }
                }

                // Stock status 1..7.
                let mut st = c.status.unwrap_or(1);
                egui::ComboBox::from_id_salt((salt.0, "st", salt.1, ci))
                    .selected_text(status_label(st))
                    .width(120.0)
                    .show_ui(ui, |ui| {
                        for (label, val) in datarunner_core::status::STOCK_LEVELS {
                            ui.selectable_value(&mut st, *val, *label);
                        }
                    });
                c.status = Some(st);

                // SCU quantity (0/blank => unknown; only set when edited).
                let mut qty = c.quantity_scu.unwrap_or(0) as i64;
                if ui.add(egui::DragValue::new(&mut qty).range(0..=10_000_000)).changed() {
                    c.quantity_scu = Some(qty as u32);
                    if c.quantity_confidence == 0 {
                        c.quantity_confidence = 100; // manual entry is trusted
                    }
                }

                // Price (0 => unknown/omit).
                let mut price = c.price.unwrap_or(0) as i64;
                let pr = ui.add(
                    egui::DragValue::new(&mut price)
                        .range(0..=100_000_000)
                        .suffix(" aUEC"),
                );
                if pr.changed() {
                    c.price = if price > 0 { Some(price as u32) } else { None };
                    c.price_confidence = 100; // manual entry is trusted
                }

                // Lowest confidence -> colour flag.
                let conf = c.min_confidence();
                let cc = conf_color(conf);
                ui.colored_label(cc, format!("{conf}"));

                if ui.button("✖").clicked() {
                    delete_row = Some(ci);
                }
                ui.end_row();
            }
        });

    if let Some(i) = delete_row {
        ex.commodities.remove(i);
    }

    ui.horizontal(|ui| {
        if ui.button("➕ Add commodity").clicked() {
            ex.commodities.push(datarunner_core::model::Commodity::new(""));
        }
    });
    }); // end left (controls) column

    // Screenshot preview on the right, so values can be verified without leaving
    // the app.
    if let Some(tex) = &screenshot_tex {
        ui.separator();
        let sized = egui::load::SizedTexture::from_handle(tex);
        ui.add(egui::Image::new(sized).max_width(460.0).max_height(820.0));
    }
    }); // end horizontal_top
}

// ---------------------------------------------------------------------------
// small helpers
// ---------------------------------------------------------------------------

/// Decode the buy/sell commodity panel from a screenshot into an egui image for
/// the inline preview (cropped/deskewed, not the whole frame).
fn decode_thumbnail(path: &str) -> Option<egui::ColorImage> {
    let p = std::path::Path::new(path);
    if !p.is_file() {
        return None;
    }
    let panel = datarunner_core::deskew::panel_color(p, datarunner_core::deskew::Side::Right, 1.0)
        .ok()
        .filter(|im| im.width() > 0 && im.height() > 0)
        // Fall back to the full frame if panel extraction somehow fails.
        .unwrap_or_else(|| image::open(p).map(|i| i.to_rgba8()).unwrap_or_default());
    let dynimg = image::DynamicImage::ImageRgba8(panel).resize(
        1000,
        1500,
        image::imageops::FilterType::Triangle,
    );
    let rgba = dynimg.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
}

/// A searchable dropdown: an always-editable filter box next to a dropdown that
/// lists the filtered items. The filter box is OUTSIDE the popup so typing in it
/// doesn't close the dropdown (an egui combo closes on any click inside it).
/// Returns the picked `(id, name, system)` when the user clicks an item.
fn filter_combo(
    ui: &mut egui::Ui,
    id: &str,
    selected_text: &str,
    filter: &mut String,
    items: impl Iterator<Item = (u32, String, String)>,
) -> Option<(u32, String, String)> {
    let mut chosen = None;
    ui.add(
        egui::TextEdit::singleline(filter)
            .hint_text("filter…")
            .desired_width(120.0),
    );
    let f = filter.to_ascii_lowercase();
    egui::ComboBox::from_id_salt(id)
        .selected_text(selected_text)
        .width(220.0)
        .show_ui(ui, |ui| {
            egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                let mut shown = 0usize;
                for (cid, name, sys) in items {
                    if !f.is_empty()
                        && !name.to_ascii_lowercase().contains(&f)
                        && !sys.to_ascii_lowercase().contains(&f)
                    {
                        continue;
                    }
                    let label = if sys.is_empty() { name.clone() } else { format!("{name} — {sys}") };
                    if ui.selectable_label(false, label).clicked() {
                        chosen = Some((cid, name, sys));
                    }
                    shown += 1;
                    if shown >= 400 {
                        ui.weak("… keep typing to narrow");
                        break;
                    }
                }
                if shown == 0 {
                    ui.weak("no matches");
                }
            });
        });
    chosen
}

/// Format an aUEC amount with thousands separators.
fn fmt_auec(v: f64) -> String {
    let n = v.round() as i64;
    let neg = n < 0;
    let digits = n.abs().to_string();
    let len = digits.len();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    format!("{}{} aUEC", if neg { "-" } else { "" }, out)
}

/// Apply a UEX trade response to a local entry (sync state, id, message).
fn apply_trade_response(entry: &mut TradeEntry, resp: &datarunner_core::api::TradeResponse) {
    entry.sync = if resp.dry_run {
        TradeSync::DryRun
    } else if resp.is_ok() {
        TradeSync::Synced
    } else {
        TradeSync::Failed
    };
    entry.uex_id = resp.id_user_trade;
    entry.status_text = if resp.message.is_empty() {
        resp.status.clone()
    } else {
        format!("{} — {}", resp.status, resp.message)
    };
}

/// Build a dropdown label that disambiguates same-named terminals by operator
/// and star system, e.g. "Admin · Nyx Gateway — Pyro".
fn terminal_label(name: &str, operator: Option<&str>, system: &str) -> String {
    let base = match operator {
        Some(op) if !op.is_empty() => format!("{op} · {name}"),
        _ => name.to_string(),
    };
    if system.is_empty() {
        base
    } else {
        format!("{base} — {system}")
    }
}

fn status_label(v: u8) -> &'static str {
    datarunner_core::status::STOCK_LEVELS
        .iter()
        .find(|(_, s)| *s == v)
        .map(|(l, _)| *l)
        .unwrap_or("?")
}

fn conf_color(conf: u8) -> egui::Color32 {
    if conf >= 85 {
        egui::Color32::from_rgb(120, 200, 130)
    } else if conf >= 65 {
        egui::Color32::from_rgb(220, 190, 80)
    } else {
        egui::Color32::from_rgb(225, 110, 100)
    }
}

fn state_style(state: ReportState) -> (egui::Color32, &'static str) {
    match state {
        ReportState::Submitted => (egui::Color32::from_rgb(120, 200, 130), "ok"),
        ReportState::DryRun => (egui::Color32::from_rgb(210, 180, 90), "dry"),
        ReportState::Failed => (egui::Color32::from_rgb(225, 110, 100), "fail"),
        ReportState::Updated => (egui::Color32::from_gray(150), "old"),
    }
}

fn fmt_time(ts: i64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_opt(ts, 0).single() {
        Some(dt) => dt.format("%Y-%m-%d %H:%M").to_string(),
        None => ts.to_string(),
    }
}

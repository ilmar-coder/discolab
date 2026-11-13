//! DiscoLab — interface gráfica (desktop e Android) sobre o `discolab-core`.

mod ui_kit;

use discolab_core::bench::{BenchConfig, BenchEvent, BenchReport, TestResult, TestSpec, default_tests, raw_tests, run_bench, run_raw_read};
use discolab_core::capacity::{CapConfig, CapEvent, CapPhase, CapResult, run_capacity};
use discolab_core::repair::{RepairEvent, RepairResult, RepairTarget, run_repair};
use discolab_core::device::{Device, DriveKind, Volume, list_devices, list_volumes};
use discolab_core::health::{Assessment, Severity, assess};
use discolab_core::report::{save_report, text_report};
use discolab_core::smart::{SelfTest, SmartReport, SmartSource, read_smart, start_self_test};
use discolab_core::surface::{BUCKET_LABELS, MAP_CELLS, ScanConfig, ScanEvent, ScanSummary, run_scan};
use discolab_core::util::human_bytes;
use eframe::egui::{self, Color32, RichText, Stroke, Vec2};
use egui_plot::{Line, Plot, PlotPoints};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, mpsc};
use ui_kit::*;

/// Pasta padrão de benchmark (definida pelo Android para o armazenamento do app).
pub static BENCH_DIR: OnceLock<PathBuf> = OnceLock::new();

enum Msg {
    Devices(Vec<Device>, Vec<Volume>),
    Smart(String, Box<SmartReport>),
    Bench(BenchEvent),
    Scan(String, ScanEvent),
    SelfTest(Result<String, String>),
    BenchTemp(f64, f64),
    Cap(CapEvent),
    Raw(BenchEvent),
    Repair(String, RepairEvent),
    Update(Result<Option<discolab_core::update::Release>, String>),
    UpdateProgress(u64, u64),
    UpdateFailed(String),
}

#[derive(PartialEq, Clone, Copy)]
enum BenchMode {
    Perf,
    Capacity,
    Raw,
}

struct CapUi {
    running: bool,
    phase: String,
    done: u64,
    total: u64,
    mbps: f64,
    writing: bool,
    result: Option<CapResult>,
    error: Option<String>,
    cancel: Arc<AtomicBool>,
    amount_gib: u32,
    keep: bool,
}

struct RawUi {
    running: bool,
    phase: String,
    progress: f32,
    results: Vec<Option<TestResult>>,
    live: Vec<[f64; 2]>,
    live_test: usize,
    error: Option<String>,
    cancel: Arc<AtomicBool>,
    dev: String,
}

struct RepairUi {
    running: bool,
    done: usize,
    total: usize,
    log: Vec<String>,
    result: Option<RepairResult>,
    error: Option<String>,
    confirm_open: bool,
    confirm_text: String,
    backup_ok: bool,
    cancel: Arc<AtomicBool>,
}

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Health,
    Smart,
    Bench,
    Surface,
    Report,
}

impl Tab {
    const ALL: [Tab; 5] = [Tab::Health, Tab::Smart, Tab::Bench, Tab::Surface, Tab::Report];
    fn label(self) -> &'static str {
        match self {
            Tab::Health => "Saúde",
            Tab::Smart => "SMART",
            Tab::Bench => "Benchmark",
            Tab::Surface => "Superfície",
            Tab::Report => "Relatório",
        }
    }
    fn icon(self) -> Icon {
        match self {
            Tab::Health => Icon::Heart,
            Tab::Smart => Icon::List,
            Tab::Bench => Icon::Speed,
            Tab::Surface => Icon::Grid,
            Tab::Report => Icon::Doc,
        }
    }
}

#[derive(Default)]
struct DevData {
    smart: Option<SmartReport>,
    health: Option<Assessment>,
    scan: Option<ScanSummary>,
    loading: bool,
}

struct BenchUi {
    vol: usize,
    custom: bool,
    dir: String,
    size_mib: u64,
    secs: f32,
    sustained_gib: f32,
    direct: bool,
    tests: Vec<TestSpec>,
    running: bool,
    phase: String,
    progress: f32,
    results: Vec<Option<TestResult>>,
    live: Vec<[f64; 2]>,
    live_test: usize,
    sustained: Vec<[f64; 2]>,
    report: Option<BenchReport>,
    error: Option<String>,
    cancel: Arc<AtomicBool>,
    temps: Vec<[f64; 2]>,
    temp_dev: Option<String>,
    temp_stop: Arc<AtomicBool>,
}

struct ScanUi {
    running: bool,
    dev: String,
    cells: Vec<u8>,
    buckets: [u64; 7],
    done: u64,
    total: u64,
    mbps: f64,
    curve: Vec<[f64; 2]>,
    log: Vec<String>,
    start_pct: f32,
    end_pct: f32,
    block_kib: usize,
    rechecking: Option<usize>,
    error: Option<String>,
    cancel: Arc<AtomicBool>,
    started: Option<std::time::Instant>,
}

pub struct App {
    ctx: egui::Context,
    tx: mpsc::Sender<Msg>,
    rx: mpsc::Receiver<Msg>,
    devices: Vec<Device>,
    volumes: Vec<Volume>,
    sel: usize,
    data: HashMap<String, DevData>,
    tab: Tab,
    bench: BenchUi,
    scan: ScanUi,
    status: String,
    selftest_msg: Option<String>,
    picker_open: bool,
    bench_mode: BenchMode,
    cap: CapUi,
    raw: RawUi,
    repair: RepairUi,
    scanning_devices: bool,
    mobile: Option<bool>,
    /// Margens das barras do sistema (pontos): esquerda, topo, direita, base.
    insets: [f32; 4],
    #[allow(dead_code)]
    insets_at: Option<std::time::Instant>,
    insets_diag: String,
    /// TBW informado pelo usuário, por nome de dispositivo.
    tbw_override: HashMap<String, u64>,
    report_client: String,
    report_tech: String,
    report_notes: String,
    update: Option<discolab_core::update::Release>,
    update_status: String,
    update_busy: bool,
    update_dismissed: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let ctx = cc.egui_ctx.clone();
        set_dark(load_dark_pref());
        apply_theme(&ctx, false);
        let (tx, rx) = mpsc::channel();
        let mut app = App {
            ctx,
            tx,
            rx,
            devices: vec![],
            volumes: vec![],
            sel: 0,
            data: HashMap::new(),
            tab: Tab::Health,
            bench: BenchUi {
                vol: 0,
                custom: BENCH_DIR.get().is_some(),
                dir: BENCH_DIR.get().map(|p| p.display().to_string()).unwrap_or_default(),
                size_mib: 1024,
                secs: 5.0,
                sustained_gib: 0.0,
                direct: true,
                tests: default_tests(),
                running: false,
                phase: String::new(),
                progress: 0.0,
                results: vec![],
                live: vec![],
                live_test: 0,
                sustained: vec![],
                report: None,
                error: None,
                cancel: Arc::new(AtomicBool::new(false)),
                temps: vec![],
                temp_dev: None,
                temp_stop: Arc::new(AtomicBool::new(true)),
            },
            scan: ScanUi {
                running: false,
                dev: String::new(),
                cells: vec![u8::MAX; MAP_CELLS],
                buckets: [0; 7],
                done: 0,
                total: 0,
                mbps: 0.0,
                curve: vec![],
                log: vec![],
                start_pct: 0.0,
                end_pct: 100.0,
                block_kib: 128,
                rechecking: None,
                error: None,
                cancel: Arc::new(AtomicBool::new(false)),
                started: None,
            },
            status: "Procurando discos…".into(),
            selftest_msg: None,
            picker_open: false,
            bench_mode: BenchMode::Perf,
            cap: CapUi {
                running: false,
                phase: String::new(),
                done: 0,
                total: 0,
                mbps: 0.0,
                writing: true,
                result: None,
                error: None,
                cancel: Arc::new(AtomicBool::new(false)),
                amount_gib: 0,
                keep: false,
            },
            raw: RawUi {
                running: false,
                phase: String::new(),
                progress: 0.0,
                results: vec![],
                live: vec![],
                live_test: 0,
                error: None,
                cancel: Arc::new(AtomicBool::new(false)),
                dev: String::new(),
            },
            repair: RepairUi {
                running: false,
                done: 0,
                total: 0,
                log: vec![],
                result: None,
                error: None,
                confirm_open: false,
                confirm_text: String::new(),
                backup_ok: false,
                cancel: Arc::new(AtomicBool::new(false)),
            },
            scanning_devices: false,
            mobile: None,
            insets: [0.0; 4],
            insets_at: None,
            insets_diag: String::new(),
            tbw_override: HashMap::new(),
            report_client: String::new(),
            report_tech: load_prefs().get("tecnico").cloned().unwrap_or_default(),
            report_notes: String::new(),
            update: None,
            update_status: String::new(),
            update_busy: false,
            update_dismissed: false,
        };
        discolab_core::update::cleanup_old();
        app.refresh_devices();
        app.check_update();
        app
    }

    fn spawn<F: FnOnce(mpsc::Sender<Msg>) + Send + 'static>(&self, f: F) {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            f(tx);
            ctx.request_repaint();
        });
    }

    fn refresh_devices(&mut self) {
        self.status = "Procurando discos…".into();
        self.scanning_devices = true;
        let ctx = self.ctx.clone();
        self.spawn(move |tx| {
            let devs = list_devices();
            let vols = list_volumes();
            let _ = tx.send(Msg::Devices(devs.clone(), vols));
            ctx.request_repaint();
            for d in devs {
                if d.smart_path.is_empty() {
                    continue;
                }
                let s = read_smart(&d);
                let _ = tx.send(Msg::Smart(d.name.clone(), Box::new(s)));
                ctx.request_repaint();
            }
        });
    }

    fn check_update(&mut self) {
        self.update_busy = true;
        self.update_status = "Verificando atualizações…".into();
        self.spawn(|tx| {
            let _ = tx.send(Msg::Update(discolab_core::update::check()));
        });
    }

    fn apply_update(&mut self) {
        let Some(rel) = self.update.clone() else { return };
        if discolab_core::update::can_self_update() {
            if let Some(asset) = discolab_core::update::platform_asset(&rel).cloned() {
                self.update_busy = true;
                let ctx = self.ctx.clone();
                self.spawn(move |tx| {
                    let dest = std::env::temp_dir().join("DiscoLab-update.exe");
                    let r = discolab_core::update::download(&asset, &dest, &mut |d, t| {
                        let _ = tx.send(Msg::UpdateProgress(d, t));
                        ctx.request_repaint();
                    })
                    .and_then(|_| discolab_core::update::replace_and_restart(&dest));
                    if let Err(e) = r {
                        let _ = tx.send(Msg::UpdateFailed(e));
                    }
                });
                return;
            }
        }
        discolab_core::report::open_url(&rel.page);
    }

    fn update_banner(&mut self, ui: &mut egui::Ui) {
        let Some(rel) = self.update.clone() else { return };
        if self.update_dismissed {
            return;
        }
        egui::Frame::new().fill(ACCENT().gamma_multiply(0.15)).stroke(Stroke::new(1.0, ACCENT())).corner_radius(14).inner_margin(12).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(format!("DiscoLab {} disponível", rel.version)).strong());
                ui.label(RichText::new(format!("(você tem a {})", discolab_core::update::current_version())).small().color(MUTED()));
            });
            if self.update_busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new(&self.update_status).small());
                });
            } else {
                ui.horizontal_wrapped(|ui| {
                    let label = if discolab_core::update::can_self_update() { "Atualizar agora" } else { "Baixar nova versão" };
                    if primary_button(ui, label, true).clicked() {
                        self.apply_update();
                    }
                    if secondary_button(ui, "Depois", true).clicked() {
                        self.update_dismissed = true;
                    }
                });
                if self.update_status.starts_with("Falha") {
                    ui.colored_label(RED(), &self.update_status);
                }
            }
        });
        ui.add_space(10.0);
    }

    fn refresh_smart(&mut self, d: &Device) {
        self.data.entry(d.name.clone()).or_default().loading = true;
        let d = d.clone();
        self.spawn(move |tx| {
            let s = read_smart(&d);
            let _ = tx.send(Msg::Smart(d.name.clone(), Box::new(s)));
        });
    }

    fn reassess(&mut self, name: &str) {
        let Some(dev) = self.devices.iter().find(|d| d.name == name).cloned() else { return };
        let e = self.data.entry(name.to_string()).or_default();
        if let Some(s) = &e.smart {
            e.health = Some(assess(&dev, s, e.scan.as_ref()));
        }
    }

    fn poll(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Devices(d, v) => {
                    self.scanning_devices = false;
                    self.status = format!("{} disco(s) · {} volume(s)", d.len(), v.len());
                    let prev = self.devices.get(self.sel).map(|x| x.name.clone());
                    for x in &d {
                        let e = self.data.entry(x.name.clone()).or_default();
                        e.loading = !x.smart_path.is_empty() && e.smart.is_none();
                    }
                    self.devices = d;
                    self.volumes = v;
                    // Mantém a seleção; um disco USB recém-conectado ganha o foco.
                    self.sel = prev.and_then(|p| self.devices.iter().position(|x| x.name == p)).unwrap_or(0);
                    if let Some(i) = self
                        .devices
                        .iter()
                        .position(|x| x.kind == DriveKind::Usb && !self.data.get(&x.name).is_some_and(|e| e.smart.is_some()))
                    {
                        self.sel = i;
                    }
                    if self.bench.vol >= self.volumes.len() {
                        self.bench.vol = 0;
                    }
                }
                Msg::Smart(name, s) => {
                    // Discos USB-OTG só revelam tamanho e modelo depois de abertos.
                    if let Some(d) = self.devices.iter_mut().find(|d| d.name == name) {
                        if d.size_bytes == 0 {
                            d.size_bytes = s.capacity.unwrap_or(0);
                        }
                        if d.kind == DriveKind::Usb {
                            if let Some(m) = s.model.clone().filter(|m| !m.is_empty()) {
                                d.model = m;
                            }
                        }
                    }
                    let e = self.data.entry(name.clone()).or_default();
                    e.smart = Some(*s);
                    e.loading = false;
                    self.reassess(&name);
                }
                Msg::SelfTest(r) => {
                    self.selftest_msg = Some(match r {
                        Ok(m) => format!("Autoteste iniciado. Atualize o SMART depois para ver o resultado.\n{m}"),
                        Err(e) => format!("Falha ao iniciar autoteste: {e}"),
                    })
                }
                Msg::Bench(e) => self.on_bench(e),
                Msg::BenchTemp(t, c) => self.bench.temps.push([t, c]),
                Msg::Scan(name, e) => self.on_scan(&name, e),
                Msg::Cap(e) => self.on_cap(e),
                Msg::Raw(e) => self.on_raw(e),
                Msg::Repair(name, e) => self.on_repair(&name, e),
                Msg::Update(r) => {
                    self.update_busy = false;
                    match r {
                        Ok(Some(rel)) => {
                            self.update_status = format!("Nova versão {} disponível.", rel.version);
                            self.update = Some(rel);
                        }
                        Ok(None) => self.update_status = format!("Você já tem a versão mais recente ({}).", discolab_core::update::current_version()),
                        Err(e) => self.update_status = format!("Não foi possível verificar: {e}."),
                    }
                }
                Msg::UpdateProgress(d, t) => {
                    self.update_status = format!("Baixando atualização… {} de {}", human_bytes(d), human_bytes(t));
                }
                Msg::UpdateFailed(e) => {
                    self.update_busy = false;
                    self.update_status = format!("Falha na atualização: {e}");
                }
            }
        }
    }

    fn on_bench(&mut self, e: BenchEvent) {
        let b = &mut self.bench;
        match e {
            BenchEvent::Phase(p) => b.phase = p,
            BenchEvent::Progress(p) => b.progress = p,
            BenchEvent::Sample { test, t, mbps } => {
                if test != b.live_test {
                    b.live.clear();
                    b.live_test = test;
                }
                b.live.push([t as f64, mbps]);
            }
            BenchEvent::TestDone(i, r) => {
                if b.results.len() <= i {
                    b.results.resize(i + 1, None);
                }
                b.results[i] = Some(r);
            }
            BenchEvent::SustainedSample { gb, mbps } => b.sustained.push([gb, mbps]),
            BenchEvent::Warning(w) => b.error = Some(w),
            BenchEvent::Failed(f) => {
                b.error = Some(f);
                b.running = false;
                b.phase = "Falhou".into();
                b.temp_stop.store(true, Ordering::Relaxed);
            }
            BenchEvent::Done(mut r) => {
                b.running = false;
                b.phase = if r.cancelled { "Interrompido".into() } else { "Concluído".into() };
                b.progress = 1.0;
                b.temp_stop.store(true, Ordering::Relaxed);
                r.temperatures = b.temps.iter().map(|p| (p[0], p[1])).collect();
                b.report = Some(r);
            }
        }
    }

    fn on_cap(&mut self, e: CapEvent) {
        let c = &mut self.cap;
        match e {
            CapEvent::Phase(p) => c.phase = p,
            CapEvent::Progress { phase, done, total, mbps } => {
                c.writing = phase == CapPhase::Write;
                c.done = done;
                c.total = total;
                c.mbps = mbps;
            }
            CapEvent::Done(r) => {
                c.running = false;
                c.phase = "Concluído".into();
                c.result = Some(r);
            }
            CapEvent::Failed(f) => {
                c.running = false;
                c.error = Some(f);
            }
        }
    }

    fn on_raw(&mut self, e: BenchEvent) {
        let r = &mut self.raw;
        match e {
            BenchEvent::Phase(p) => r.phase = p,
            BenchEvent::Progress(p) => r.progress = p,
            BenchEvent::Sample { test, t, mbps } => {
                if test != r.live_test {
                    r.live.clear();
                    r.live_test = test;
                }
                r.live.push([t as f64, mbps]);
            }
            BenchEvent::TestDone(i, res) => {
                if r.results.len() <= i {
                    r.results.resize(i + 1, None);
                }
                r.results[i] = Some(res);
            }
            BenchEvent::Warning(w) => r.error = Some(w),
            BenchEvent::Failed(f) => {
                r.error = Some(f);
                r.running = false;
            }
            BenchEvent::Done(rep) => {
                r.running = false;
                r.progress = 1.0;
                r.phase = if rep.cancelled { "Interrompido".into() } else { "Concluído".into() };
            }
            BenchEvent::SustainedSample { .. } => {}
        }
    }

    fn on_repair(&mut self, name: &str, e: RepairEvent) {
        match e {
            RepairEvent::Progress { done, total } => {
                self.repair.done = done;
                self.repair.total = total;
            }
            RepairEvent::Log(l) => self.repair.log.push(l),
            RepairEvent::Failed(f) => {
                self.repair.running = false;
                self.repair.error = Some(f);
            }
            RepairEvent::Done(r) => {
                self.repair.running = false;
                self.repair.result = Some(r);
                // relê o SMART para mostrar a mudança nos setores realocados/pendentes
                if let Some(d) = self.devices.iter().find(|d| d.name == name).cloned() {
                    self.refresh_smart(&d);
                }
            }
        }
    }

    fn on_scan(&mut self, name: &str, e: ScanEvent) {
        let s = &mut self.scan;
        match e {
            ScanEvent::Started { total_blocks, uncached, size } => {
                s.total = total_blocks;
                s.log.push(format!(
                    "Iniciado: {} · {} blocos · sem cache: {}",
                    human_bytes(size),
                    total_blocks,
                    if uncached { "sim" } else { "não" }
                ));
            }
            ScanEvent::Progress { done_blocks, mbps, position_pct, buckets } => {
                s.done = done_blocks;
                s.mbps = mbps;
                s.buckets = buckets;
                s.curve.push([position_pct as f64, mbps]);
            }
            ScanEvent::Cell { cell, bucket } => s.cells[cell] = bucket,
            ScanEvent::Rechecking { count } => {
                s.rechecking = Some(count);
                s.log.push(format!("Relendo {count} blocos suspeitos para confirmar…"));
            }
            ScanEvent::Log(l) => s.log.push(l),
            ScanEvent::Failed(f) => {
                s.error = Some(f);
                s.running = false;
            }
            ScanEvent::Done(sum) => {
                s.running = false;
                s.rechecking = None;
                s.buckets = sum.buckets;
                s.log.push(format!(
                    "Concluído em {:.0}s: {} ilegíveis, {} lentos persistentes, {} transitórios.",
                    sum.elapsed_s, sum.bad_blocks, sum.slow_persistent, sum.slow_transient
                ));
                self.data.entry(name.to_string()).or_default().scan = Some(sum);
                self.reassess(name);
            }
        }
    }

    fn current(&self) -> Option<&Device> {
        self.devices.get(self.sel)
    }

    fn is_mobile(&self) -> bool {
        self.mobile.unwrap_or(false)
    }

    fn update_insets(&mut self, ctx: &egui::Context) {
        #[cfg(target_os = "android")]
        {
            let due = self.insets_at.is_none_or(|t| t.elapsed() > std::time::Duration::from_millis(1000));
            if due {
                self.insets_at = Some(std::time::Instant::now());
                let ppp = ctx.pixels_per_point().max(0.1);
                let (px, diag) = android_insets_px();
                // Mínimos seguros em dp: o app nunca fica embaixo das barras do sistema,
                // mesmo que o aparelho não informe as margens.
                let min = [0.0, 30.0, 0.0, 34.0];
                let mut v = [0.0f32; 4];
                for i in 0..4 {
                    v[i] = (px[i] as f32 / ppp).max(min[i]);
                }
                self.insets = v;
                self.insets_diag = format!("{diag} · ppp {ppp:.2} · usado {:.0}/{:.0}/{:.0}/{:.0} dp", v[0], v[1], v[2], v[3]);
                ctx.request_repaint_after(std::time::Duration::from_millis(1100));
            }
        }
        #[cfg(not(target_os = "android"))]
        let _ = ctx;
    }

    // ------------------------------------------------------------ cabeçalho / navegação

    fn device_card(&self, ui: &mut egui::Ui, d: &Device, selected: bool, compact: bool) -> egui::Response {
        let dd = self.data.get(&d.name);
        let health = dd.and_then(|x| x.health.as_ref());
        let loading = dd.map(|x| x.loading).unwrap_or(false);
        let accent = health.map(health_color).unwrap_or(ACCENT());
        let fill = if selected { ACCENT().gamma_multiply(0.14) } else { CARD() };
        let stroke = if selected { Stroke::new(1.5, ACCENT()) } else { Stroke::new(1.0, BORDER()) };
        let r = egui::Frame::new()
            .fill(fill)
            .stroke(stroke)
            .corner_radius(14)
            .inner_margin(egui::Margin::symmetric(12, if compact { 8 } else { 10 }))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(40.0), egui::Sense::hover());
                    ui.painter().rect_filled(r, 10, accent.gamma_multiply(0.18));
                    paint_icon(ui.painter(), r.shrink(9.0), drive_icon(d.kind), accent);
                    let right_w = 58.0;
                    ui.vertical(|ui| {
                        ui.set_width((ui.available_width() - right_w).max(60.0));
                        ui.add(egui::Label::new(RichText::new(&d.model).strong().color(TEXT())).truncate());
                        let size = if d.size_bytes > 0 { human_bytes(d.size_bytes) } else { "—".into() };
                        ui.label(RichText::new(format!("{} · {}", d.kind.label(), size)).small().color(MUTED()));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(h) = health {
                            ui.label(RichText::new(format!("{}%", h.health)).size(19.0).strong().color(health_color(h)));
                        } else if loading {
                            ui.spinner();
                        }
                    });
                });
            })
            .response;
        r.interact(egui::Sense::click())
    }

    fn theme_button(&mut self, ui: &mut egui::Ui) {
        let (icon, tip) = if is_dark() { (Icon::Sun, "Tema claro") } else { (Icon::Moon, "Tema escuro") };
        if icon_button(ui, icon, tip, false).clicked() {
            set_dark(!is_dark());
            save_dark_pref(is_dark());
            apply_theme(ui.ctx(), self.is_mobile());
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            brand(ui);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| self.theme_button(ui));
        });
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("DISCOS").small().strong().color(MUTED()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icon_button(ui, Icon::Refresh, "Atualizar lista de discos", self.scanning_devices).clicked() {
                    self.refresh_devices();
                }
            });
        });
        ui.add_space(4.0);
        let mut clicked = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            let devs = self.devices.clone();
            for (i, d) in devs.iter().enumerate() {
                if self.device_card(ui, d, i == self.sel, false).clicked() {
                    clicked = Some(i);
                }
                ui.add_space(6.0);
            }
            if devs.is_empty() && !self.scanning_devices {
                ui.label(RichText::new("Nenhum disco encontrado.").color(MUTED()));
            }
        });
        if let Some(i) = clicked {
            self.sel = i;
        }
    }

    fn mobile_header(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            brand(ui);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icon_button(ui, Icon::Refresh, "Atualizar lista de discos", self.scanning_devices).clicked() {
                    self.refresh_devices();
                }
                self.theme_button(ui);
            });
        });
        ui.add_space(8.0);
        if let Some(d) = self.current().cloned() {
            let n = self.devices.len();
            let r = self.device_card(ui, &d, false, true).on_hover_text("Trocar disco");
            let hint = if n > 1 { format!("{n} discos · trocar") } else { "trocar disco".into() };
            ui.painter().text(
                egui::pos2(r.rect.right() - 12.0, r.rect.bottom() - 6.0),
                egui::Align2::RIGHT_BOTTOM,
                hint,
                egui::FontId::proportional(11.0),
                ACCENT(),
            );
            if r.clicked() {
                self.picker_open = true;
            }
        } else {
            ui.horizontal(|ui| {
                if self.scanning_devices {
                    ui.spinner();
                }
                ui.label(RichText::new("Procurando discos…").color(MUTED()));
            });
        }
        ui.add_space(8.0);
    }

    fn device_picker(&mut self, ctx: &egui::Context) {
        if !self.picker_open {
            return;
        }
        let mut chosen = None;
        let mut refresh = false;
        let mut close = false;
        let w = (ctx.content_rect().width() - 48.0).clamp(240.0, 480.0);
        let resp = egui::Modal::new(egui::Id::unique("picker")).show(ctx, |ui| {
            ui.set_width(w);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Escolher disco").size(19.0).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icon_button(ui, Icon::Refresh, "Atualizar lista de discos", self.scanning_devices).clicked() {
                        refresh = true;
                    }
                });
            });
            ui.add_space(8.0);
            let devs = self.devices.clone();
            egui::ScrollArea::vertical().max_height(ctx.content_rect().height() * 0.6).show(ui, |ui| {
                for (i, d) in devs.iter().enumerate() {
                    if self.device_card(ui, d, i == self.sel, false).clicked() {
                        chosen = Some(i);
                    }
                    ui.add_space(6.0);
                }
            });
            ui.add_space(4.0);
            ui.label(
                RichText::new("Para testar um HD/SSD externo, conecte-o por um adaptador USB-OTG e toque em atualizar.")
                    .small()
                    .color(MUTED()),
            );
            ui.add_space(8.0);
            if secondary_button(ui, "Fechar", true).clicked() {
                close = true;
            }
        });
        if resp.should_close() || close {
            self.picker_open = false;
        }
        if refresh {
            self.refresh_devices();
        }
        if let Some(i) = chosen {
            self.sel = i;
            self.picker_open = false;
        }
    }

    fn top_tabs(&mut self, ui: &mut egui::Ui) {
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            for t in Tab::ALL {
                if tab_chip(ui, t.icon(), t.label(), self.tab == t).clicked() {
                    self.tab = t;
                }
            }
        });
        ui.add_space(6.0);
    }

    fn bottom_nav(&mut self, ui: &mut egui::Ui) {
        let w = ui.available_width() / Tab::ALL.len() as f32;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for t in Tab::ALL {
                if nav_item(ui, w, t.icon(), t.label(), self.tab == t).clicked() {
                    self.tab = t;
                }
            }
        });
    }

    // ------------------------------------------------------------ Saúde

    fn tab_health(&mut self, ui: &mut egui::Ui) {
        let Some(dev) = self.current().cloned() else {
            empty_state(ui, Icon::Disk, "Nenhum disco encontrado", "Conecte um disco e toque em atualizar.");
            return;
        };
        let mobile = self.is_mobile();
        let dd = self.data.entry(dev.name.clone()).or_default();
        let (smart, health, loading) = (dd.smart.clone(), dd.health.clone(), dd.loading);

        if dev.smart_path.is_empty() {
            // Android sem root: armazenamento interno.
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(48.0), egui::Sense::hover());
                    ui.painter().rect_filled(r, 12, ACCENT().gamma_multiply(0.15));
                    paint_icon(ui.painter(), r.shrink(11.0), Icon::Chip, ACCENT());
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&dev.model).size(19.0).strong());
                        ui.label(RichText::new(format!("{} · {}", human_bytes(dev.size_bytes), dev.bus)).color(MUTED()));
                    });
                });
                ui.add_space(10.0);
                ui.label(
                    "O Android não permite que apps leiam a saúde (SMART) do chip de memória interno sem root. \
                     Você ainda pode medir o desempenho real dele no Benchmark.",
                );
                ui.add_space(10.0);
                if primary_button(ui, "Medir desempenho do armazenamento", true).clicked() {
                    self.tab = Tab::Bench;
                }
            });
            ui.add_space(10.0);
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(48.0), egui::Sense::hover());
                    ui.painter().rect_filled(r, 12, GREEN().gamma_multiply(0.15));
                    paint_icon(ui.painter(), r.shrink(11.0), Icon::Usb, GREEN());
                    ui.vertical(|ui| {
                        ui.label(RichText::new("Testar um HD ou SSD externo").strong());
                        ui.label(
                            RichText::new("Conecte o disco com um adaptador USB-OTG. O DiscoLab lê o SMART e faz a varredura de superfície, sem root.")
                                .color(MUTED()),
                        );
                    });
                });
                ui.add_space(10.0);
                if secondary_button(ui, "Procurar discos USB", !self.scanning_devices).clicked() {
                    self.refresh_devices();
                }
            });
            return;
        }

        let Some(s) = smart else {
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new(&dev.model).strong());
                });
                ui.add_space(6.0);
                if discolab_core::usb::is_usb(&dev.smart_path) {
                    ui.label("Abrindo o disco USB… Se o Android perguntar, toque em \"Permitir\" para o DiscoLab acessar o dispositivo.");
                } else {
                    ui.label(RichText::new("Lendo dados SMART…").color(MUTED()));
                }
            });
            return;
        };

        // ---- cartão principal
        let texts = |ui: &mut egui::Ui| {
            ui.label(RichText::new(&dev.model).size(20.0).strong());
            ui.label(RichText::new(format!("{} · {} · {}", dev.kind.label(), dev.bus, human_bytes(dev.size_bytes))).color(MUTED()));
            ui.add_space(8.0);
            if let Some(a) = &health {
                ui.horizontal(|ui| {
                    pill(ui, a.status_label(), health_color(a));
                    pill(ui, &format!("Confiança {}", a.confidence.split(' ').next().unwrap_or("").to_lowercase()), MUTED());
                    if loading {
                        ui.spinner();
                    }
                });
                ui.add_space(8.0);
                ui.label(RichText::new(&a.verdict).size(15.0).color(TEXT()));
                if let Some(l) = a.life_left {
                    ui.add_space(10.0);
                    life_bar(ui, l);
                }
            }
        };
        card(ui, |ui| {
            if mobile {
                ui.vertical_centered(|ui| {
                    if let Some(a) = &health {
                        gauge(ui, a, 170.0);
                        ui.add_space(10.0);
                    }
                });
                texts(ui);
            } else {
                ui.horizontal(|ui| {
                    if let Some(a) = &health {
                        gauge(ui, a, 160.0);
                        ui.add_space(18.0);
                    }
                    ui.vertical(texts);
                });
            }
        });
        ui.add_space(10.0);

        // ---- números principais
        let mut tiles: Vec<(&str, String, Color32)> = Vec::new();
        if let Some(t) = s.temperature {
            let hot = if s.rotation_rpm.unwrap_or(0) > 0 { 50 } else { 70 };
            tiles.push(("Temperatura", format!("{t} °C"), if t >= hot { ORANGE() } else { TEXT() }));
        }
        if let Some(h) = s.power_on_hours {
            tiles.push(("Tempo ligado", if h >= 8760 { format!("{:.1} anos", h as f64 / 8760.0) } else { format!("{h} h") }, TEXT()));
        }
        if let Some(w) = s.host_written_bytes {
            tiles.push(("Total escrito", human_bytes(w), TEXT()));
        }
        if let Some(r) = s.host_read_bytes {
            tiles.push(("Total lido", human_bytes(r), TEXT()));
        }
        if let Some(c) = s.power_cycles {
            tiles.push(("Ciclos de energia", c.to_string(), TEXT()));
        }
        if let Some(rpm) = s.rotation_rpm.filter(|r| *r > 0) {
            tiles.push(("Rotação", format!("{rpm} RPM"), TEXT()));
        }
        if let Some(p) = s.smart_passed {
            tiles.push(("Firmware", if p { "PASSED".into() } else { "FAILED".into() }, if p { GREEN() } else { RED() }));
        }
        if !tiles.is_empty() {
            stat_grid(ui, &tiles, if mobile { 2 } else { 4 });
            ui.add_space(10.0);
        }

        // ---- resistência (TBW) para SSDs
        let flash = s.rotation_rpm.unwrap_or(0) == 0 && (s.nvme.is_some() || dev.kind.is_flash() || s.rotation_rpm == Some(0));
        if let (true, Some(written)) = (flash, s.host_written_bytes) {
            let cap = s.capacity.unwrap_or(dev.size_bytes);
            let known = discolab_core::endurance::rated_tbw(s.model.as_deref().unwrap_or(&dev.model), cap);
            let mut tbw = self.tbw_override.get(&dev.name).copied().or(known).unwrap_or(0);
            card(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Resistência de escrita (TBW)").strong());
                    if known.is_some() && !self.tbw_override.contains_key(&dev.name) {
                        pill(ui, "garantia do fabricante", MUTED());
                    }
                });
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(format!("Escrito: {}", human_bytes(written))).color(MUTED()));
                    ui.label(RichText::new("· TBW:").color(MUTED()));
                    if ui.add(egui::DragValue::new(&mut tbw).range(0..=100_000).suffix(" TB").speed(5)).changed() {
                        self.tbw_override.insert(dev.name.clone(), tbw);
                    }
                });
                match discolab_core::endurance::life_left_by_tbw(written, tbw) {
                    Some(l) => {
                        ui.add_space(6.0);
                        life_bar(ui, l);
                        ui.label(
                            RichText::new(format!(
                                "{:.0}% do TBW garantido já foi usado. O TBW é o limite da garantia, não o fim do SSD — o desgaste informado pelo controlador (acima) é a medida principal.",
                                100 - l
                            ))
                            .small()
                            .color(MUTED()),
                        );
                    }
                    None => {
                        ui.label(RichText::new("Modelo sem TBW conhecido. Informe o valor da ficha técnica do fabricante para estimar a vida pelo volume escrito.").small().color(MUTED()));
                    }
                }
            });
            ui.add_space(10.0);
        }

        // ---- ações e detalhes
        card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if secondary_button(ui, "Reler SMART", !loading).clicked() {
                    self.refresh_smart(&dev);
                }
                if s.source == SmartSource::Smartctl {
                    for (l, k) in [("Autoteste curto", SelfTest::Short), ("Autoteste longo", SelfTest::Long), ("Abortar autoteste", SelfTest::Abort)] {
                        if secondary_button(ui, l, true).clicked() {
                            let d = dev.clone();
                            self.spawn(move |tx| {
                                let _ = tx.send(Msg::SelfTest(start_self_test(&d, k)));
                            });
                        }
                    }
                }
            });
            if let Some(p) = s.self_test_running {
                ui.label(RichText::new(format!("Autoteste em andamento: {p}% restante")).color(ACCENT()));
            }
            if let Some(m) = &self.selftest_msg {
                ui.label(RichText::new(m).small());
            }
            if s.needs_admin {
                ui.colored_label(ORANGE(), "Permissão insuficiente. Rode como Administrador (Windows) ou com sudo (Linux/macOS) para SMART completo.");
            }
            ui.add_space(6.0);
            let mut info: Vec<String> = s.messages.clone();
            info.push(format!("Fonte: {}", s.source.label()));
            info.extend(s.serial.as_ref().map(|v| format!("Serial: {v}")));
            info.extend(s.firmware.as_ref().map(|v| format!("Firmware: {v}")));
            for m in info {
                ui.label(RichText::new(format!("• {m}")).small().color(MUTED()));
            }
        });
        ui.add_space(14.0);

        // ---- diagnóstico
        section_title(ui, "Diagnóstico");
        if let Some(a) = &health {
            for f in &a.findings {
                finding_card(ui, f.severity, &f.title, &f.detail, f.penalty);
                ui.add_space(6.0);
            }
        }
        if !s.self_tests.is_empty() {
            ui.add_space(8.0);
            section_title(ui, "Histórico de autotestes");
            card(ui, |ui| {
                for t in s.self_tests.iter().take(8) {
                    let c = match t.passed {
                        Some(true) => GREEN(),
                        Some(false) => RED(),
                        None => MUTED(),
                    };
                    ui.label(RichText::new(format!("{} — {} {}", t.kind, t.status, t.hours.map(|h| format!("({h} h)")).unwrap_or_default())).color(c));
                }
            });
        }
    }

    // ------------------------------------------------------------ SMART

    fn tab_smart(&mut self, ui: &mut egui::Ui) {
        let Some(dev) = self.current().cloned() else { return };
        let Some(s) = self.data.get(&dev.name).and_then(|d| d.smart.clone()) else {
            empty_state(ui, Icon::List, "Sem dados SMART", "Este dispositivo não forneceu atributos SMART.");
            return;
        };
        if !s.ata.is_empty() {
            section_title(ui, "Atributos SMART");
            card(ui, |ui| {
                egui::ScrollArea::horizontal().id_salt("ata_h").show(ui, |ui| {
                    egui::Grid::new("ata").striped(true).num_columns(7).spacing([14.0, 6.0]).show(ui, |ui| {
                        for h in ["ID", "Atributo", "Atual", "Pior", "Limite", "Bruto", "Significado"] {
                            ui.label(RichText::new(h).small().strong().color(MUTED()));
                        }
                        ui.end_row();
                        for a in &s.ata {
                            let (desc, critical) = attr_info(a.id);
                            let failing = a.thresh > 0 && a.value <= a.thresh;
                            let bad_raw = critical && (a.raw & 0xFFFF_FFFF) > 0;
                            let col = if failing {
                                RED()
                            } else if bad_raw {
                                ORANGE()
                            } else {
                                TEXT()
                            };
                            ui.label(RichText::new(format!("{:3}", a.id)).monospace().color(MUTED()));
                            ui.label(RichText::new(&a.name).color(col));
                            ui.label(a.value.to_string());
                            ui.label(a.worst.to_string());
                            ui.label(a.thresh.to_string());
                            ui.label(RichText::new(&a.raw_str).monospace().color(col));
                            ui.label(RichText::new(desc).small().color(MUTED()));
                            ui.end_row();
                        }
                    });
                });
            });
        }
        if let Some(n) = &s.nvme {
            section_title(ui, "NVMe SMART / Health Log");
            let rows: Vec<(&str, String, Option<Color32>)> = vec![
                ("Aviso crítico", format!("0x{:02X}", n.critical_warning), (n.critical_warning != 0).then_some(RED())),
                ("Temperatura", n.temperature.map(|t| format!("{t} °C")).unwrap_or("-".into()), None),
                ("Reserva disponível", format!("{}%", n.available_spare), (n.available_spare < n.available_spare_threshold).then_some(RED())),
                ("Limite da reserva", format!("{}%", n.available_spare_threshold), None),
                ("Percentual usado (desgaste)", format!("{}%", n.percentage_used), (n.percentage_used >= 90).then_some(ORANGE())),
                ("Dados lidos", human_bytes(n.data_units_read * 512_000), None),
                ("Dados escritos", human_bytes(n.data_units_written * 512_000), None),
                ("Ciclos de energia", n.power_cycles.to_string(), None),
                ("Horas ligado", n.power_on_hours.to_string(), None),
                ("Desligamentos inseguros", n.unsafe_shutdowns.to_string(), None),
                ("Erros de mídia/integridade", n.media_errors.to_string(), (n.media_errors > 0).then_some(RED())),
                ("Entradas no log de erros", n.num_err_log_entries.to_string(), None),
                ("Tempo em temp. de aviso (min)", n.warning_temp_time.to_string(), None),
                ("Tempo em temp. crítica (min)", n.critical_comp_time.to_string(), None),
            ];
            kv_card(ui, "nvme", &rows);
        }
        if let Some(f) = &s.flash {
            section_title(ui, "Memória flash (eMMC/UFS)");
            let rows: Vec<(&str, String, Option<Color32>)> = vec![
                ("Desgaste tipo A", f.used_pct_a.map(|x| format!("{}–{x}%", x.saturating_sub(10))).unwrap_or("-".into()), None),
                ("Desgaste tipo B", f.used_pct_b.map(|x| format!("{}–{x}%", x.saturating_sub(10))).unwrap_or("-".into()), None),
                (
                    "Pre-EOL",
                    match f.pre_eol {
                        Some(1) => "Normal",
                        Some(2) => "Aviso",
                        Some(3) => "Urgente",
                        _ => "-",
                    }
                    .into(),
                    None,
                ),
            ];
            kv_card(ui, "flash", &rows);
        }
        if !s.extra.is_empty() {
            section_title(ui, "Contadores do Windows");
            let rows: Vec<(&str, String, Option<Color32>)> = s.extra.iter().map(|(k, v)| (k.as_str(), v.clone(), None)).collect();
            kv_card(ui, "extra", &rows);
        }
        if s.ata.is_empty() && s.nvme.is_none() && s.flash.is_none() && s.extra.is_empty() {
            empty_state(ui, Icon::List, "Nenhum atributo disponível", &s.messages.join("\n"));
        }
    }

    // ------------------------------------------------------------ Benchmark

    fn bench_dir(&self) -> Option<PathBuf> {
        if self.bench.custom {
            return Some(PathBuf::from(&self.bench.dir)).filter(|p| !p.as_os_str().is_empty());
        }
        let v = self.volumes.get(self.bench.vol)?;
        let tmp = std::env::temp_dir();
        let tmp = discolab_core::util::canonical(&tmp);
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from);
        let best = |root: &PathBuf| -> bool {
            self.volumes
                .iter()
                .filter(|x| root.starts_with(&x.mount))
                .max_by_key(|x| x.mount.as_os_str().len())
                .map(|x| x.mount == v.mount)
                .unwrap_or(false)
        };
        Some(if best(&tmp) {
            tmp.join("DiscoLab")
        } else if let Some(h) = home.filter(|h| best(h)) {
            h.join("DiscoLab-bench")
        } else {
            v.mount.join("DiscoLab-bench")
        })
    }

    fn dest_picker(&mut self, ui: &mut egui::Ui) {
        let internal = BENCH_DIR.get().map(|p| p.display().to_string());
        let is_internal = self.bench.custom && internal.as_deref() == Some(self.bench.dir.as_str());
            ui.label(RichText::new("Destino").small().color(MUTED()));
            let sel_text = if is_internal {
                "Armazenamento interno do app".to_string()
            } else if self.bench.custom {
                "Pasta personalizada".to_string()
            } else {
                self.volumes.get(self.bench.vol).map(|v| format!("{} ({} livres)", v.mount.display(), human_bytes(v.free))).unwrap_or("-".into())
            };
            egui::ComboBox::from_id_salt("vol").selected_text(sel_text).width(ui.available_width().min(420.0)).show_ui(ui, |ui| {
                if let Some(p) = &internal {
                    if ui.selectable_label(is_internal, "Armazenamento interno do app").clicked() {
                        self.bench.custom = true;
                        self.bench.dir = p.clone();
                    }
                }
                for (i, v) in self.volumes.iter().enumerate() {
                    let t = format!("{}  [{}]  {} livres", v.mount.display(), v.fs, human_bytes(v.free));
                    if ui.selectable_label(!self.bench.custom && self.bench.vol == i, t).clicked() {
                        self.bench.vol = i;
                        self.bench.custom = false;
                    }
                }
                if ui.selectable_label(self.bench.custom && !is_internal, "Pasta personalizada…").clicked() {
                    self.bench.custom = true;
                    self.bench.dir.clear();
                }
            });
            if self.bench.custom && !is_internal {
                ui.add(egui::TextEdit::singleline(&mut self.bench.dir).hint_text("/caminho/da/pasta").desired_width(ui.available_width().min(420.0)));
            }
    }

    fn bench_settings(&mut self, ui: &mut egui::Ui) {
        let mobile = self.is_mobile();
        ui.add_enabled_ui(!self.bench.running, |ui| {
            self.dest_picker(ui);
            ui.add_space(8.0);
            egui::Grid::new("bset").num_columns(if mobile { 2 } else { 4 }).spacing([12.0, 10.0]).show(ui, |ui| {
                ui.label(RichText::new("Arquivo de teste").color(MUTED()));
                egui::ComboBox::from_id_salt("size").selected_text(fmt_mib(self.bench.size_mib)).show_ui(ui, |ui| {
                    for m in [256u64, 512, 1024, 2048, 4096, 8192, 16384] {
                        ui.selectable_value(&mut self.bench.size_mib, m, fmt_mib(m));
                    }
                });
                if mobile {
                    ui.end_row();
                }
                ui.label(RichText::new("Duração por teste").color(MUTED()));
                ui.add(egui::Slider::new(&mut self.bench.secs, 2.0..=30.0).suffix(" s").integer());
                ui.end_row();
                ui.label(RichText::new("Escrita sustentada").color(MUTED()));
                ui.add(egui::Slider::new(&mut self.bench.sustained_gib, 0.0..=64.0).suffix(" GiB").integer()).on_hover_text(
                    "Escreve vários GB seguidos para revelar a velocidade real depois que o cache SLC/DRAM enche. 0 = desligado.",
                );
                if mobile {
                    ui.end_row();
                }
                ui.label(RichText::new("Cache do SO").color(MUTED()));
                ui.checkbox(&mut self.bench.direct, "Desligado (recomendado)");
                ui.end_row();
            });
        });
    }

    fn tab_bench(&mut self, ui: &mut egui::Ui) {
        let raw_ok = self.current().is_some_and(|d| !d.raw_path.is_empty());
        ui.horizontal_wrapped(|ui| {
            let modes: Vec<(BenchMode, Icon, &str)> = vec![
                (BenchMode::Perf, Icon::Speed, "Desempenho"),
                (BenchMode::Capacity, Icon::Chip, "Capacidade real"),
                (BenchMode::Raw, Icon::Grid, "Leitura direta"),
            ];
            for (m, icon, label) in modes {
                if m == BenchMode::Raw && !raw_ok {
                    continue;
                }
                if tab_chip(ui, icon, label, self.bench_mode == m).clicked() {
                    self.bench_mode = m;
                }
            }
        });
        ui.add_space(8.0);
        match self.bench_mode {
            BenchMode::Capacity => self.tab_capacity(ui),
            BenchMode::Raw if raw_ok => self.tab_raw(ui),
            _ => self.tab_bench_perf(ui),
        }
    }

    /// Volume que contém o destino escolhido (para espaço livre e alertas).
    fn dest_volume(&self) -> Option<Volume> {
        let dir = self.bench_dir()?;
        let dir = discolab_core::util::canonical(&dir);
        self.volumes.iter().filter(|v| dir.starts_with(&v.mount)).max_by_key(|v| v.mount.as_os_str().len()).cloned()
    }

    fn tab_capacity(&mut self, ui: &mut egui::Ui) {
        let mobile = self.is_mobile();
        let running = self.cap.running;
        let vol = self.dest_volume();
        let system = vol.as_ref().is_some_and(|v| is_system_mount(&v.mount.to_string_lossy()));
        card(ui, |ui| {
            ui.label(RichText::new("Teste de capacidade real").strong());
            ui.label(
                RichText::new(
                    "Grava dados verificáveis no espaço livre e relê tudo sem cache — como o H2testw. Pendrives, cartões SD e SSDs \
                     falsificados informam um tamanho maior que o real e devolvem dados errados além da capacidade verdadeira. \
                     Não apaga seus arquivos.",
                )
                .color(MUTED()),
            );
            ui.add_space(8.0);
            ui.add_enabled_ui(!running, |ui| {
                self.dest_picker(ui);
                ui.add_space(6.0);
                let free_gib = vol.as_ref().map(|v| (v.free >> 30) as u32).unwrap_or(0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Quantidade").color(MUTED()));
                    ui.add(egui::Slider::new(&mut self.cap.amount_gib, 0..=free_gib.max(1)).suffix(" GiB"));
                    ui.label(RichText::new(if self.cap.amount_gib == 0 { "(todo o espaço livre)" } else { "" }).small().color(MUTED()));
                });
                ui.checkbox(&mut self.cap.keep, "Manter os arquivos para verificar depois de reconectar o dispositivo");
            });
            if system {
                ui.colored_label(YELLOW(), "Este é o disco do sistema: o teste deixará 5 GB livres para não travar o sistema.");
            }
            ui.add_space(8.0);
            if running {
                if danger_button(ui, "Parar").clicked() {
                    self.cap.cancel.store(true, Ordering::Relaxed);
                }
            } else {
                ui.horizontal_wrapped(|ui| {
                    if primary_button(ui, "Iniciar teste", true).clicked() {
                        self.start_capacity(false, system);
                    }
                    if secondary_button(ui, "Só verificar arquivos existentes", true).clicked() {
                        self.start_capacity(true, system);
                    }
                });
            }
            if running || self.cap.total > 0 {
                ui.add_space(8.0);
                ui.label(RichText::new(format!("{} {:.0} MB/s", self.cap.phase, self.cap.mbps)).color(MUTED()));
                let frac = if self.cap.total > 0 { self.cap.done as f32 / self.cap.total as f32 } else { 0.0 };
                ui.add(
                    egui::ProgressBar::new(frac)
                        .fill(if self.cap.writing { ORANGE() } else { BLUE() })
                        .corner_radius(6)
                        .text(format!("{} de {}", human_bytes(self.cap.done), human_bytes(self.cap.total))),
                );
            }
            if let Some(e) = &self.cap.error {
                ui.colored_label(RED(), e);
            }
        });
        if let Some(r) = self.cap.result.clone() {
            ui.add_space(12.0);
            section_title(ui, "Resultado");
            card(ui, |ui| {
                let (label, c) = if r.cancelled {
                    ("INTERROMPIDO", MUTED())
                } else if r.is_fake() {
                    ("FALHOU", RED())
                } else {
                    ("APROVADO", GREEN())
                };
                ui.horizontal_wrapped(|ui| {
                    pill(ui, label, c);
                    ui.label(RichText::new(r.verdict()).strong());
                });
                ui.add_space(8.0);
                let mut tiles = vec![
                    ("Gravado", human_bytes(r.written), TEXT()),
                    ("Verificado", human_bytes(r.verified), TEXT()),
                    ("Corrompido", human_bytes(r.corrupted_bytes), if r.corrupted_bytes > 0 { RED() } else { TEXT() }),
                    ("Ilegível", human_bytes(r.unreadable_bytes), if r.unreadable_bytes > 0 { RED() } else { TEXT() }),
                    ("Escrita", format!("{:.0} MB/s", r.write_mbps), ORANGE()),
                    ("Leitura", format!("{:.0} MB/s", r.read_mbps), BLUE()),
                ];
                if let Some(b) = r.first_bad {
                    tiles.push(("Capacidade real ~", human_bytes(b), RED()));
                }
                stat_grid(ui, &tiles, if mobile { 2 } else { 3 });
                if let Some(e) = &r.write_error {
                    ui.label(RichText::new(format!("A gravação parou em: {e}")).small().color(MUTED()));
                }
                if !r.uncached {
                    ui.colored_label(ORANGE(), "A verificação pode ter usado o cache do sistema. Para certeza total, marque \"manter os arquivos\", reconecte o dispositivo e use \"Só verificar\".");
                }
            });
        }
    }

    fn start_capacity(&mut self, verify_only: bool, system: bool) {
        let Some(dir) = self.bench_dir() else {
            self.cap.error = Some("Escolha um destino.".into());
            return;
        };
        let mut cfg = CapConfig::new(dir.join("capacidade"));
        cfg.verify_only = verify_only;
        cfg.keep_files = self.cap.keep;
        let free = self.dest_volume().map(|v| v.free).unwrap_or(0);
        let reserve: u64 = if system { 5_000_000_000 } else { 64 << 20 };
        let max = free.saturating_sub(reserve);
        cfg.bytes = Some(if self.cap.amount_gib == 0 { max } else { ((self.cap.amount_gib as u64) << 30).min(max) });
        let c = &mut self.cap;
        c.running = true;
        c.result = None;
        c.error = None;
        c.done = 0;
        c.total = 0;
        c.cancel = Arc::new(AtomicBool::new(false));
        let cancel = c.cancel.clone();
        let ctx = self.ctx.clone();
        self.spawn(move |tx| {
            run_capacity(&cfg, &cancel, &mut |e| {
                let _ = tx.send(Msg::Cap(e));
                ctx.request_repaint();
            });
        });
    }

    fn tab_raw(&mut self, ui: &mut egui::Ui) {
        let Some(dev) = self.current().cloned() else { return };
        let mobile = self.is_mobile();
        let usb = discolab_core::usb::is_usb(&dev.raw_path);
        let running = self.raw.running;
        card(ui, |ui| {
            ui.label(RichText::new(format!("Leitura direta — {}", dev.model)).strong());
            ui.label(
                RichText::new(
                    "Mede a leitura lendo o disco diretamente, sem sistema de arquivos e sem gravar nada. \
                     Ideal para discos em adaptador USB no Android e para HDs sem partição.",
                )
                .color(MUTED()),
            );
            if !usb && !discolab_core::util::is_elevated() {
                ui.colored_label(ORANGE(), "Exige administrador/root.");
            }
            ui.add_space(6.0);
            ui.add_enabled_ui(!running, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Duração por teste").color(MUTED()));
                    ui.add(egui::Slider::new(&mut self.bench.secs, 2.0..=30.0).suffix(" s").integer());
                });
            });
            ui.add_space(8.0);
            if !running {
                if primary_button(ui, "Iniciar leitura direta", true).clicked() {
                    self.start_raw(&dev);
                }
            } else if danger_button(ui, "Parar").clicked() {
                self.raw.cancel.store(true, Ordering::Relaxed);
            }
            if running || self.raw.progress > 0.0 {
                ui.add_space(6.0);
                ui.label(RichText::new(&self.raw.phase).color(MUTED()));
                ui.add(egui::ProgressBar::new(self.raw.progress).fill(ACCENT()).corner_radius(6));
            }
            if let Some(e) = &self.raw.error {
                ui.colored_label(ORANGE(), e);
            }
        });
        ui.add_space(12.0);
        section_title(ui, &format!("Resultados{}", if self.raw.dev.is_empty() { String::new() } else { format!(" — {}", self.raw.dev) }));
        let tests = raw_tests();
        let cols = if mobile { 1 } else { 2 };
        for row in tests.iter().enumerate().collect::<Vec<_>>().chunks(cols) {
            ui.horizontal(|ui| {
                let w = (ui.available_width() - 8.0 * (cols as f32 - 1.0)) / cols as f32;
                for (i, t) in row {
                    let res = self.raw.results.get(*i).cloned().flatten();
                    ui.allocate_ui(Vec2::new(w, 90.0), |ui| {
                        ui.set_width(w);
                        bench_row(ui, &t.name.replace(" Leitura", ""), res.as_ref(), None, running && self.raw.live_test == *i, true);
                    });
                }
            });
            ui.add_space(8.0);
        }
        if !self.raw.live.is_empty() {
            let pts = self.raw.live.clone();
            card(ui, |ui| {
                Plot::new("rawlive").height(150.0).include_y(0.0).include_x(0.0).allow_scroll(false).allow_drag(false).y_axis_label("MB/s").show(ui, |p| {
                    p.line(Line::new("MB/s", PlotPoints::from(pts)).color(BLUE()).width(2.0));
                });
            });
        }
    }

    fn start_raw(&mut self, dev: &Device) {
        let cfg = ScanConfig::new(&dev.raw_path, dev.size_bytes, true);
        let secs = self.bench.secs;
        let r = &mut self.raw;
        r.running = true;
        r.results = vec![None; raw_tests().len()];
        r.live.clear();
        r.error = None;
        r.progress = 0.0;
        r.dev = dev.model.clone();
        r.cancel = Arc::new(AtomicBool::new(false));
        let cancel = r.cancel.clone();
        let ctx = self.ctx.clone();
        self.spawn(move |tx| match discolab_core::surface::open_source(&cfg) {
            Ok(mut src) => run_raw_read(src.as_mut(), secs, &cancel, &mut |e| {
                let _ = tx.send(Msg::Raw(e));
                ctx.request_repaint();
            }),
            Err(e) => {
                let _ = tx.send(Msg::Raw(BenchEvent::Failed(e)));
            }
        });
    }

    fn tab_bench_perf(&mut self, ui: &mut egui::Ui) {
        let mobile = self.is_mobile();
        let running = self.bench.running;
        card(ui, |ui| {
            if mobile {
                egui::CollapsingHeader::new(RichText::new("Configurações").strong())
                    .default_open(true)
                    .show(ui, |ui| self.bench_settings(ui));
            } else {
                self.bench_settings(ui);
            }
            ui.add_space(10.0);
            if !running {
                if primary_button(ui, "Iniciar benchmark", true).clicked() {
                    self.start_bench();
                }
            } else if danger_button(ui, "Parar").clicked() {
                self.bench.cancel.store(true, Ordering::Relaxed);
            }
            if running || self.bench.progress > 0.0 {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if running {
                        ui.spinner();
                    }
                    ui.label(RichText::new(&self.bench.phase).color(MUTED()));
                });
                ui.add(egui::ProgressBar::new(self.bench.progress).fill(ACCENT()).corner_radius(6));
            }
            if let Some(e) = &self.bench.error {
                ui.colored_label(ORANGE(), e);
            }
            if let Some(r) = &self.bench.report {
                for w in &r.warnings {
                    if Some(w) != self.bench.error.as_ref() {
                        ui.colored_label(ORANGE(), w);
                    }
                }
                if !r.uncached {
                    ui.colored_label(ORANGE(), "Atenção: resultado com cache do SO — os números de leitura não representam o disco.");
                }
            }
        });
        ui.add_space(14.0);

        ui.horizontal(|ui| {
            section_title(ui, "Resultados");
            ui.add_space(12.0);
            legend_dot(ui, BLUE(), "Leitura");
            legend_dot(ui, ORANGE(), "Escrita");
        });
        for i in 0..self.bench.tests.len() / 2 {
            let (r_i, w_i) = (2 * i, 2 * i + 1);
            let name = self.bench.tests[r_i].name.trim_end_matches(" Leitura").to_string();
            let rr = self.bench.results.get(r_i).cloned().flatten();
            let wr = self.bench.results.get(w_i).cloned().flatten();
            let active = running && (self.bench.live_test == r_i || self.bench.live_test == w_i);
            bench_row(ui, &name, rr.as_ref(), wr.as_ref(), active, mobile);
            ui.add_space(8.0);
        }

        if !self.bench.live.is_empty() {
            let name = self.bench.tests.get(self.bench.live_test).map(|t| t.name.clone()).unwrap_or_default();
            ui.add_space(6.0);
            section_title(ui, &format!("Vazão ao longo do teste — {name}"));
            let pts = self.bench.live.clone();
            card(ui, |ui| {
                Plot::new("live")
                    .height(160.0)
                    .include_y(0.0)
                    .include_x(0.0)
                    .allow_scroll(false)
                    .allow_drag(false)
                    .x_axis_label("s")
                    .y_axis_label("MB/s")
                    .show(ui, |p| {
                        p.line(Line::new("MB/s", PlotPoints::from(pts)).color(ACCENT()).width(2.0));
                    });
            });
            ui.add_space(8.0);
        }
        if !self.bench.temps.is_empty() {
            section_title(ui, &format!("Temperatura durante o teste{}", self.bench.temp_dev.as_ref().map(|m| format!(" — {m}")).unwrap_or_default()));
            let pts = self.bench.temps.clone();
            let max = pts.iter().map(|p| p[1]).fold(f64::MIN, f64::max);
            let min = pts.iter().map(|p| p[1]).fold(f64::MAX, f64::min);
            card(ui, |ui| {
                Plot::new("temps")
                    .height(140.0)
                    .include_y(min.min(30.0))
                    .include_y(max.max(60.0))
                    .include_x(0.0)
                    .allow_scroll(false)
                    .allow_drag(false)
                    .x_axis_label("s")
                    .y_axis_label("°C")
                    .show(ui, |p| {
                        p.line(Line::new("°C", PlotPoints::from(pts)).color(RED()).width(2.0));
                    });
                let hot = max >= 70.0;
                let tiles = vec![
                    ("Início", format!("{:.0} °C", self.bench.temps[0][1]), TEXT()),
                    ("Máxima", format!("{max:.0} °C"), if hot { ORANGE() } else { TEXT() }),
                    ("Aumento", format!("+{:.0} °C", max - self.bench.temps[0][1]), TEXT()),
                ];
                ui.add_space(6.0);
                stat_grid(ui, &tiles, 3);
                if hot {
                    ui.colored_label(
                        ORANGE(),
                        "Acima de ~70 °C muitos SSDs NVMe reduzem a velocidade (throttling). Se a vazão caiu junto com o pico de temperatura, melhore a refrigeração (dissipador).",
                    );
                }
            });
            ui.add_space(8.0);
        }
        if !self.bench.sustained.is_empty() {
            section_title(ui, "Escrita sustentada (cache SLC/DRAM)");
            let pts = self.bench.sustained.clone();
            card(ui, |ui| {
                Plot::new("sust")
                    .height(180.0)
                    .include_y(0.0)
                    .include_x(0.0)
                    .allow_scroll(false)
                    .allow_drag(false)
                    .x_axis_label("GB escritos")
                    .y_axis_label("MB/s")
                    .show(ui, |p| {
                        p.line(Line::new("MB/s", PlotPoints::from(pts)).color(ORANGE()).width(2.0));
                    });
                if let Some(s) = self.bench.report.as_ref().and_then(|r| r.sustained.as_ref()) {
                    let tiles = vec![
                        ("Pico", format!("{:.0} MB/s", s.burst_mbps), TEXT()),
                        ("Sustentado", format!("{:.0} MB/s", s.sustained_mbps), ORANGE()),
                        ("Mínimo", format!("{:.0} MB/s", s.min_mbps), TEXT()),
                        ("Cache estimado", s.cache_estimate_bytes.map(|c| format!("~{}", human_bytes(c))).unwrap_or("sem queda".into()), TEXT()),
                    ];
                    ui.add_space(8.0);
                    stat_grid(ui, &tiles, if mobile { 2 } else { 4 });
                }
            });
            ui.add_space(8.0);
        }
        ui.label(
            RichText::new(
                "Números honestos: E/S direta sem cache do SO, dados aleatórios incompressíveis (cada página muda a cada escrita), \
                 latência por operação (p99/máx revelam travadas) e escrita sustentada para expor o fim do cache SLC.",
            )
            .small()
            .color(MUTED()),
        );
    }

    fn start_bench(&mut self) {
        let Some(dir) = self.bench_dir() else {
            self.bench.error = Some("Escolha um destino.".into());
            return;
        };
        let mut cfg = BenchConfig::new(dir);
        cfg.file_size = self.bench.size_mib << 20;
        cfg.seconds = self.bench.secs;
        cfg.sustained_bytes = (self.bench.sustained_gib as u64) << 30;
        cfg.direct = self.bench.direct;
        cfg.tests = self.bench.tests.clone();
        let b = &mut self.bench;
        b.running = true;
        b.results = vec![None; cfg.tests.len()];
        b.live.clear();
        b.sustained.clear();
        b.report = None;
        b.error = None;
        b.progress = 0.0;
        b.cancel = Arc::new(AtomicBool::new(false));
        b.temps.clear();
        b.temp_stop = Arc::new(AtomicBool::new(false));
        let cancel = b.cancel.clone();
        // Disco que contém a pasta do teste, para acompanhar a temperatura.
        let dir = cfg.dir.clone();
        let dir = discolab_core::util::canonical(&dir);
        let dev = self
            .devices
            .iter()
            .filter_map(|d| d.mount_points.iter().filter(|m| dir.starts_with(m.as_str())).map(|m| (m.len(), d)).max_by_key(|x| x.0))
            .max_by_key(|x| x.0)
            .map(|x| x.1.clone())
            .filter(|d| !d.smart_path.is_empty());
        self.bench.temp_dev = dev.as_ref().map(|d| d.model.clone());
        if let Some(dev) = dev {
            let stop = self.bench.temp_stop.clone();
            let t0 = std::time::Instant::now();
            self.spawn(move |tx| {
                while !stop.load(Ordering::Relaxed) {
                    if let Some(c) = discolab_core::smart::read_temperature(&dev) {
                        let _ = tx.send(Msg::BenchTemp(t0.elapsed().as_secs_f64(), c as f64));
                    }
                    for _ in 0..30 {
                        if stop.load(Ordering::Relaxed) {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                }
            });
        }
        let ctx = self.ctx.clone();
        self.spawn(move |tx| {
            run_bench(&cfg, &cancel, &mut |e| {
                let _ = tx.send(Msg::Bench(e));
                ctx.request_repaint();
            });
        });
    }

    // ------------------------------------------------------------ Superfície

    fn tab_surface(&mut self, ui: &mut egui::Ui) {
        let Some(dev) = self.current().cloned() else { return };
        if dev.raw_path.is_empty() {
            empty_state(
                ui,
                Icon::Grid,
                "Varredura indisponível",
                "O Android não permite ler o chip interno sem root. Conecte um HD/SSD por adaptador USB-OTG para fazer a varredura de superfície.",
            );
            return;
        }
        let mobile = self.is_mobile();
        let running = self.scan.running;
        card(ui, |ui| {
            ui.label(
                RichText::new("Lê toda a superfície em modo SOMENTE LEITURA — seguro para os dados. Blocos lentos ou com erro são relidos para confirmar.")
                    .color(MUTED()),
            );
            if discolab_core::usb::is_usb(&dev.raw_path) {
                ui.colored_label(YELLOW(), "Disco USB acessado diretamente: o Android desmonta o volume durante o teste. Reconecte o adaptador depois.");
            } else if !discolab_core::util::is_elevated() {
                ui.colored_label(
                    ORANGE(),
                    if cfg!(windows) {
                        "A leitura bruta do disco exige executar o DiscoLab como Administrador."
                    } else {
                        "A leitura bruta do disco exige root: execute com sudo (ex.: sudo ./discolab)."
                    },
                );
            }
            ui.add_space(8.0);
            ui.add_enabled_ui(!running, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Início").color(MUTED()));
                    ui.add(egui::DragValue::new(&mut self.scan.start_pct).range(0.0..=99.0).suffix(" %"));
                    ui.label(RichText::new("Fim").color(MUTED()));
                    ui.add(egui::DragValue::new(&mut self.scan.end_pct).range(1.0..=100.0).suffix(" %"));
                    ui.label(RichText::new("Bloco").color(MUTED()));
                    egui::ComboBox::from_id_salt("blk").selected_text(format!("{} KiB", self.scan.block_kib)).show_ui(ui, |ui| {
                        for k in [64usize, 128, 256, 512, 1024, 4096] {
                            ui.selectable_value(&mut self.scan.block_kib, k, format!("{k} KiB"));
                        }
                    });
                });
            });
            ui.add_space(10.0);
            if !running {
                if primary_button(ui, "Iniciar varredura", true).clicked() {
                    self.start_scan(&dev);
                }
            } else if danger_button(ui, "Parar").clicked() {
                self.scan.cancel.store(true, Ordering::Relaxed);
            }
            if self.scan.total > 0 {
                let pct = self.scan.done as f64 / self.scan.total as f64;
                let eta = self
                    .scan
                    .started
                    .map(|t| t.elapsed().as_secs_f64())
                    .filter(|_| pct > 0.001)
                    .map(|e| e / pct - e)
                    .unwrap_or(0.0);
                ui.add_space(8.0);
                ui.add(egui::ProgressBar::new(pct as f32).fill(ACCENT()).corner_radius(6));
                ui.label(
                    RichText::new(format!(
                        "{:.1}% · {:.0} MB/s{}{}",
                        pct * 100.0,
                        self.scan.mbps,
                        if running { format!(" · faltam ~{}", fmt_dur(eta)) } else { String::new() },
                        if self.scan.dev != dev.name { format!(" · mapa de {}", self.scan.dev) } else { String::new() }
                    ))
                    .color(MUTED()),
                );
            }
            if let Some(e) = &self.scan.error {
                ui.colored_label(RED(), e);
            }
        });
        ui.add_space(14.0);

        section_title(ui, "Mapa de blocos");
        card(ui, |ui| {
            let cols_legend = if mobile { 2 } else { 4 };
            ui.spacing_mut().interact_size.y = 18.0;
            egui::Grid::new("legend").num_columns(cols_legend).spacing([16.0, 6.0]).show(ui, |ui| {
                for (i, l) in BUCKET_LABELS.iter().enumerate() {
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(12.0), egui::Sense::hover());
                        ui.painter().rect_filled(r, 3, bucket_color(i as u8));
                        ui.label(RichText::new(*l).small().color(MUTED()));
                        ui.label(RichText::new(self.scan.buckets[i].to_string()).small().strong());
                    });
                    if (i + 1) % cols_legend == 0 {
                        ui.end_row();
                    }
                }
            });
            ui.add_space(10.0);
            let w = ui.available_width().min(1060.0);
            let cols = if w > 700.0 { 128 } else { 64 };
            let rows = MAP_CELLS / cols;
            let cs = w / cols as f32;
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, cs * rows as f32), egui::Sense::hover());
            let p = ui.painter_at(rect);
            for (i, &b) in self.scan.cells.iter().enumerate() {
                let (x, y) = ((i % cols) as f32, (i / cols) as f32);
                let r = egui::Rect::from_min_size(rect.min + Vec2::new(x * cs, y * cs), Vec2::splat((cs - 1.0).max(1.0)));
                p.rect_filled(r, 1, bucket_color(b));
            }
            if let Some(pos) = resp.hover_pos() {
                let i = (((pos.y - rect.min.y) / cs) as usize) * cols + ((pos.x - rect.min.x) / cs) as usize;
                if i < MAP_CELLS {
                    let pct = self.scan.start_pct as f64 + (self.scan.end_pct - self.scan.start_pct) as f64 * i as f64 / MAP_CELLS as f64;
                    resp.on_hover_text(format!("~{:.2}% do disco · {}", pct, BUCKET_LABELS.get(self.scan.cells[i] as usize).copied().unwrap_or("não lido")));
                }
            }
        });
        self.repair_card(ui, &dev);
        if !self.scan.curve.is_empty() {
            ui.add_space(14.0);
            section_title(ui, "Velocidade ao longo do disco");
            let pts = self.scan.curve.clone();
            card(ui, |ui| {
                Plot::new("scurve")
                    .height(170.0)
                    .include_y(0.0)
                    .include_x(0.0)
                    .include_x(100.0)
                    .allow_scroll(false)
                    .allow_drag(false)
                    .x_axis_label("% do disco")
                    .y_axis_label("MB/s")
                    .show(ui, |p| {
                        p.line(Line::new("MB/s", PlotPoints::from(pts)).color(GREEN()).width(2.0));
                    });
            });
        }
        if !self.scan.log.is_empty() {
            ui.add_space(14.0);
            section_title(ui, "Registro");
            card(ui, |ui| {
                for l in self.scan.log.iter().rev().take(12) {
                    ui.label(RichText::new(l).small());
                }
            });
        }
        ui.add_space(10.0);
        ui.label(
            RichText::new(
                "Diferente do Victoria, um bloco lento só conta como problema se continuar lento nas releituras — picos causados pelo \
                 sistema são marcados como transitórios. Em SSDs, a varredura detecta blocos ilegíveis e travamentos do controlador.",
            )
            .small()
            .color(MUTED()),
        );
    }

    fn repair_card(&mut self, ui: &mut egui::Ui, dev: &Device) {
        let Some(sum) = self.data.get(&dev.name).and_then(|d| d.scan.clone()) else { return };
        let n_bad = sum.errors.len();
        let n_slow = sum.slow_offsets.len();
        if n_bad + n_slow == 0 && self.repair.result.is_none() && !self.repair.running {
            return;
        }
        ui.add_space(14.0);
        section_title(ui, "Reparar setores");
        let system = dev.mount_points.iter().any(|m| is_system_mount(m));
        card(ui, |ui| {
            ui.label(RichText::new(format!("{n_bad} bloco(s) ilegível(is) e {n_slow} lento(s) encontrados na varredura.")).strong());
            ui.label(
                RichText::new(
                    "Blocos lentos são regravados com os mesmos dados (refresh, nada se perde). Em blocos ilegíveis, o app salva os setores \
                     que ainda leem e grava zeros só nos setores mortos — o conteúdo deles já está perdido — o que força o disco a \
                     substituí-los pela área reserva e normalmente zera os \"setores pendentes\".",
                )
                .color(MUTED()),
            );
            if system {
                ui.colored_label(RED(), "Bloqueado: este é o disco do sistema em uso. Conecte-o em outro computador (ou via adaptador USB) para reparar.");
            }
            ui.add_space(8.0);
            if self.repair.running {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(format!("Reparando {}/{}…", self.repair.done, self.repair.total));
                });
                if danger_button(ui, "Parar").clicked() {
                    self.repair.cancel.store(true, Ordering::Relaxed);
                }
            } else if !system && n_bad + n_slow > 0 && danger_button(ui, &format!("Reparar {} bloco(s)…", n_bad + n_slow)).clicked() {
                self.repair.confirm_open = true;
                self.repair.confirm_text.clear();
                self.repair.backup_ok = false;
            }
            if let Some(e) = &self.repair.error {
                ui.colored_label(RED(), e);
            }
            if let Some(r) = &self.repair.result {
                ui.add_space(6.0);
                let tiles = vec![
                    ("Corrigidos", r.fixed.to_string(), GREEN()),
                    ("Ainda lentos", r.still_slow.to_string(), if r.still_slow > 0 { ORANGE() } else { TEXT() }),
                    ("Ainda ilegíveis", r.still_bad.to_string(), if r.still_bad > 0 { RED() } else { TEXT() }),
                    ("Setores zerados", r.zeroed_sectors.to_string(), TEXT()),
                ];
                stat_grid(ui, &tiles, if self.is_mobile() { 2 } else { 4 });
                ui.label(RichText::new("O SMART foi relido: compare os setores realocados/pendentes na aba Saúde. Faça uma nova varredura para confirmar.").small().color(MUTED()));
            }
            for l in self.repair.log.iter().rev().take(6) {
                ui.label(RichText::new(l).small().color(MUTED()));
            }
        });
        if self.repair.confirm_open {
            let mut start = false;
            let mut close = false;
            egui::Modal::new(egui::Id::unique("repair_confirm")).show(ui.ctx(), |ui| {
                ui.set_width(380.0_f32.min(ui.ctx().content_rect().width() - 48.0));
                ui.label(RichText::new("Confirmar reparo").size(18.0).strong());
                ui.add_space(6.0);
                ui.label(format!("Disco: {} ({})", dev.model, human_bytes(dev.size_bytes)));
                ui.label(RichText::new("Esta operação GRAVA no disco. Setores ilegíveis terão o conteúdo substituído por zeros.").color(ORANGE()));
                ui.add_space(6.0);
                ui.checkbox(&mut self.repair.backup_ok, "Os dados importantes deste disco já têm cópia");
                ui.label(RichText::new("Digite REPARAR para confirmar:").small().color(MUTED()));
                ui.add(egui::TextEdit::singleline(&mut self.repair.confirm_text).desired_width(f32::INFINITY));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let ok = self.repair.backup_ok && self.repair.confirm_text.trim() == "REPARAR";
                    if ui.add_enabled(ok, egui::Button::new(RichText::new("Reparar agora").strong().color(egui::Color32::WHITE)).fill(RED())).clicked() {
                        start = true;
                    }
                    if secondary_button(ui, "Cancelar", true).clicked() {
                        close = true;
                    }
                });
            });
            if close || start {
                self.repair.confirm_open = false;
            }
            if start {
                self.start_repair(dev, &sum);
            }
        }
    }

    fn start_repair(&mut self, dev: &Device, sum: &ScanSummary) {
        let bs = sum.block_size.max(4096);
        let mut targets: Vec<RepairTarget> = sum.errors.iter().map(|(o, _)| RepairTarget { offset: *o, len: bs, unreadable: true }).collect();
        targets.extend(sum.slow_offsets.iter().map(|(o, _)| RepairTarget { offset: *o, len: bs, unreadable: false }));
        let path = PathBuf::from(&dev.raw_path);
        let size = dev.size_bytes;
        let name = dev.name.clone();
        let slow = if dev.kind.is_flash() { 50 } else { 200 };
        let r = &mut self.repair;
        r.running = true;
        r.result = None;
        r.error = None;
        r.log.clear();
        r.done = 0;
        r.total = targets.len();
        r.cancel = Arc::new(AtomicBool::new(false));
        let cancel = r.cancel.clone();
        let ctx = self.ctx.clone();
        self.spawn(move |tx| match discolab_core::surface::open_source_rw(&path, size) {
            Ok(mut dev) => run_repair(dev.as_mut(), &targets, slow, &cancel, &mut |e| {
                let _ = tx.send(Msg::Repair(name.clone(), e));
                ctx.request_repaint();
            }),
            Err(e) => {
                let _ = tx.send(Msg::Repair(name.clone(), RepairEvent::Failed(e)));
            }
        });
    }

    fn start_scan(&mut self, dev: &Device) {
        let smart = self.data.get(&dev.name).and_then(|d| d.smart.as_ref());
        let flash = dev.kind.is_flash() || smart.map(|s| s.nvme.is_some() || s.rotation_rpm == Some(0)).unwrap_or(false);
        let mut cfg = ScanConfig::new(&dev.raw_path, dev.size_bytes, flash);
        cfg.start_pct = self.scan.start_pct;
        cfg.end_pct = self.scan.end_pct.max(self.scan.start_pct + 0.01);
        cfg.block_size = self.scan.block_kib * 1024;
        let s = &mut self.scan;
        s.running = true;
        s.dev = dev.name.clone();
        s.cells = vec![u8::MAX; MAP_CELLS];
        s.buckets = [0; 7];
        s.done = 0;
        s.total = 0;
        s.curve.clear();
        s.log.clear();
        s.error = None;
        s.started = Some(std::time::Instant::now());
        s.cancel = Arc::new(AtomicBool::new(false));
        let cancel = s.cancel.clone();
        let ctx = self.ctx.clone();
        let name = dev.name.clone();
        self.spawn(move |tx| {
            run_scan(&cfg, &cancel, &mut |e| {
                let _ = tx.send(Msg::Scan(name.clone(), e));
                ctx.request_repaint();
            });
        });
    }

    // ------------------------------------------------------------ Relatório

    fn tab_report(&mut self, ui: &mut egui::Ui) {
        let Some(dev) = self.current().cloned() else { return };
        let dd = self.data.get(&dev.name);
        let smart = dd.and_then(|d| d.smart.as_ref());
        let health = dd.and_then(|d| d.health.as_ref());
        let scan = dd.and_then(|d| d.scan.as_ref());
        let bench = self.bench.report.as_ref();
        let mut text = text_report(&dev, smart, health, bench, scan);
        let mut status = None;
        // ---- relatório para o cliente (HTML / PDF)
        let cap = self.cap.result.clone();
        let tbw = {
            let cap_b = smart.and_then(|s| s.capacity).unwrap_or(dev.size_bytes);
            self.tbw_override
                .get(&dev.name)
                .copied()
                .or_else(|| discolab_core::endurance::rated_tbw(smart.and_then(|s| s.model.as_deref()).unwrap_or(&dev.model), cap_b))
        };
        card(ui, |ui| {
            ui.label(RichText::new("Relatório para o cliente").strong());
            ui.label(RichText::new("Página com gráficos e diagnóstico, que abre no navegador e pode ser salva em PDF (Imprimir > Salvar como PDF).").color(MUTED()));
            ui.add_space(6.0);
            egui::Grid::new("repinfo").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                ui.label(RichText::new("Cliente").color(MUTED()));
                ui.add(egui::TextEdit::singleline(&mut self.report_client).hint_text("nome do cliente / equipamento").desired_width(ui.available_width().min(380.0)));
                ui.end_row();
                ui.label(RichText::new("Técnico").color(MUTED()));
                if ui.add(egui::TextEdit::singleline(&mut self.report_tech).hint_text("seu nome / empresa").desired_width(ui.available_width().min(380.0))).lost_focus() {
                    save_pref("tecnico", &self.report_tech);
                }
                ui.end_row();
            });
            ui.label(RichText::new("Observações").color(MUTED()));
            ui.add(egui::TextEdit::multiline(&mut self.report_notes).desired_rows(3).desired_width(f32::INFINITY).hint_text("ex.: recomendado trocar o disco e migrar os dados"));
            ui.add_space(8.0);
            if primary_button(ui, "Gerar relatório (HTML / PDF)", true).clicked() {
                save_pref("tecnico", &self.report_tech);
                let info = discolab_core::html_report::ReportInfo {
                    client: self.report_client.clone(),
                    technician: self.report_tech.clone(),
                    notes: self.report_notes.clone(),
                    tbw,
                    date: discolab_core::report::now_string(),
                };
                let html = discolab_core::html_report::html_report(&dev, smart, health, bench, scan, cap.as_ref(), &info);
                status = Some(match discolab_core::report::save_and_open_html(&dev, &html) {
                    Ok(p) => format!("Relatório salvo em {p}"),
                    Err(e) => format!("Erro ao salvar: {e}"),
                });
            }
        });
        ui.add_space(10.0);
        card(ui, |ui| {
            ui.label(RichText::new("Texto e dados").strong());
            ui.label(RichText::new("Versão em texto (para colar em mensagens) e JSON com todos os dados brutos.").color(MUTED()));
            ui.add_space(10.0);
            ui.horizontal_wrapped(|ui| {
                if primary_button(ui, "Salvar (.txt + .json)", true).clicked() {
                    let json = serde_json::json!({"device": dev, "smart": smart, "assessment": health, "benchmark": bench, "surface": scan});
                    status = Some(match save_report(&dev, &text, &json) {
                        Ok(p) => format!("Relatório salvo em {}", p.display()),
                        Err(e) => format!("Erro ao salvar: {e}"),
                    });
                }
                if secondary_button(ui, "Copiar", true).clicked() {
                    ui.ctx().copy_text(text.clone());
                    status = Some("Relatório copiado.".into());
                }
            });
            if self.status.starts_with("Relatório") || self.status.starts_with("Erro") {
                ui.label(RichText::new(&self.status).small().color(ACCENT()));
            }
            if !self.insets_diag.is_empty() {
                ui.add_space(6.0);
                ui.label(RichText::new(format!("Tela: {}", self.insets_diag)).small().color(MUTED()));
            }
        });
        if let Some(s) = status {
            self.status = s;
        }
        ui.add_space(10.0);
        card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(format!("DiscoLab {}", discolab_core::update::current_version())).strong());
                if self.update_busy {
                    ui.spinner();
                } else if secondary_button(ui, "Verificar atualizações", true).clicked() {
                    self.update_dismissed = false;
                    self.check_update();
                }
            });
            if !self.update_status.is_empty() {
                ui.label(RichText::new(&self.update_status).small().color(MUTED()));
            }
        });
        ui.add_space(10.0);
        card(ui, |ui| {
            egui::ScrollArea::horizontal().id_salt("rep_h").show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut text)
                        .font(egui::TextStyle::Monospace)
                        .desired_width(f32::INFINITY)
                        .frame(egui::Frame::NONE)
                        .interactive(true),
                );
            });
        });
    }
}

/// Ponto de montagem do sistema em uso (não reparar / preservar espaço).
fn is_system_mount(m: &str) -> bool {
    let m = m.trim_end_matches(['\\', '/']);
    m.is_empty() || m == "/System/Volumes/Data" || m == "/boot" || m == "/data" || m.eq_ignore_ascii_case("C:")
}

fn prefs_path() -> PathBuf {
    discolab_core::report::reports_dir().join("preferencias.txt")
}

fn load_prefs() -> HashMap<String, String> {
    std::fs::read_to_string(prefs_path())
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once('=').map(|(k, v)| (k.trim().to_string(), v.trim().to_string())))
        .collect()
}

fn save_pref(key: &str, value: &str) {
    let mut m = load_prefs();
    m.insert(key.to_string(), value.replace('\n', " "));
    let p = prefs_path();
    let _ = std::fs::create_dir_all(p.parent().unwrap_or(&p));
    let mut keys: Vec<_> = m.keys().cloned().collect();
    keys.sort();
    let body: String = keys.iter().map(|k| format!("{k}={}\n", m[k])).collect();
    let _ = std::fs::write(p, body);
}

fn load_dark_pref() -> bool {
    load_prefs().get("tema").map(|t| t != "claro").unwrap_or(true)
}

fn save_dark_pref(dark: bool) {
    save_pref("tema", if dark { "escuro" } else { "claro" });
}

fn fmt_mib(m: u64) -> String {
    if m >= 1024 { format!("{} GiB", m / 1024) } else { format!("{m} MiB") }
}

fn fmt_dur(s: f64) -> String {
    let s = s.max(0.0) as u64;
    if s >= 3600 { format!("{}h{:02}m", s / 3600, (s % 3600) / 60) } else { format!("{}m{:02}s", s / 60, s % 60) }
}

fn fmt_us(us: u32) -> String {
    if us >= 1_000_000 {
        format!("{:.2} s", us as f64 / 1e6)
    } else if us >= 10_000 {
        format!("{:.0} ms", us as f64 / 1e3)
    } else if us >= 1000 {
        format!("{:.2} ms", us as f64 / 1e3)
    } else {
        format!("{us} µs")
    }
}

fn m(l: f32, r: f32, t: f32, b: f32) -> egui::Margin {
    egui::Margin { left: l.clamp(0.0, 120.0) as i8, right: r.clamp(0.0, 120.0) as i8, top: t.clamp(0.0, 120.0) as i8, bottom: b.clamp(0.0, 120.0) as i8 }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll();
        let ctx = ui.ctx().clone();
        self.update_insets(&ctx);
        let mobile = ui.available_width() < 760.0;
        if self.mobile != Some(mobile) {
            self.mobile = Some(mobile);
            apply_theme(&ctx, mobile);
        }
        let [il, it, ir, ib] = self.insets;

        if mobile {
            egui::Panel::top("m_top")
                .frame(egui::Frame::new().fill(PANEL()).inner_margin(m(14.0 + il, 14.0 + ir, it, 0.0)))
                .show(ui, |ui| self.mobile_header(ui));
            egui::Panel::bottom("m_nav")
                .frame(egui::Frame::new().fill(PANEL()).stroke(Stroke::new(1.0, BORDER())).inner_margin(m(il, ir, 6.0, ib + 6.0)))
                .show(ui, |ui| self.bottom_nav(ui));
        } else {
            egui::Panel::bottom("status").frame(egui::Frame::new().fill(PANEL()).inner_margin(egui::Margin::symmetric(14, 5))).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&self.status).small().color(MUTED()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(concat!("DiscoLab v", env!("CARGO_PKG_VERSION"))).small().color(MUTED()));
                    });
                });
            });
            egui::Panel::left("devs")
                .resizable(true)
                .default_size(300.0)
                .frame(egui::Frame::new().fill(PANEL()).inner_margin(egui::Margin::symmetric(14, 10)))
                .show(ui, |ui| self.sidebar(ui));
            egui::Panel::top("tabs").frame(egui::Frame::new().fill(BG()).inner_margin(egui::Margin::symmetric(18, 0))).show(ui, |ui| self.top_tabs(ui));
        }
        let margin = if mobile { m(12.0 + il, 12.0 + ir, 12.0, 8.0) } else { egui::Margin::symmetric(18, 12) };
        egui::CentralPanel::default().frame(egui::Frame::new().fill(BG()).inner_margin(margin)).show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt(("main", self.tab as u8)).auto_shrink([false, false]).show(ui, |ui| {
                let w = ui.available_width().min(1100.0);
                ui.set_max_width(w);
                self.update_banner(ui);
                match self.tab {
                    Tab::Health => self.tab_health(ui),
                    Tab::Smart => self.tab_smart(ui),
                    Tab::Bench => self.tab_bench(ui),
                    Tab::Surface => self.tab_surface(ui),
                    Tab::Report => self.tab_report(ui),
                }
                ui.add_space(16.0);
            });
        });
        self.device_picker(&ctx);
        if self.bench.running || self.scan.running {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
    }
}

pub fn options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1200.0, 800.0]).with_min_inner_size([380.0, 480.0]).with_title("DiscoLab"),
        ..Default::default()
    }
}

pub fn run(options: eframe::NativeOptions) -> eframe::Result {
    eframe::run_native("DiscoLab", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

#[cfg(target_os = "android")]
static ANDROID_APP: OnceLock<winit::platform::android::activity::AndroidApp> = OnceLock::new();

/// Margens das barras do sistema (px), combinando três fontes, e o texto de diagnóstico.
#[cfg(target_os = "android")]
fn android_insets_px() -> ([i32; 4], String) {
    let mut v = [0i32; 4];
    let mut diag = Vec::new();
    // 1) área de conteúdo informada pela NativeActivity
    if let Some(app) = ANDROID_APP.get() {
        let r = app.content_rect();
        if let Some(w) = app.native_window() {
            let (ww, wh) = (w.width(), w.height());
            if r.right > r.left && r.bottom > r.top {
                let c = [r.left, r.top, (ww - r.right).max(0), (wh - r.bottom).max(0)];
                for i in 0..4 {
                    v[i] = v[i].max(c[i]);
                }
            }
            diag.push(format!("rect {},{},{},{} janela {ww}x{wh}", r.left, r.top, r.right, r.bottom));
        } else {
            diag.push("sem janela".into());
        }
    }
    // 2) WindowInsets
    match discolab_core::usb::android::system_insets_px() {
        Ok(c) => {
            for i in 0..4 {
                v[i] = v[i].max(c[i]);
            }
            diag.push(format!("insets {:?}", c));
        }
        Err(e) => diag.push(format!("insets erro: {e}")),
    }
    // 3) alturas das barras pelos recursos do sistema
    match discolab_core::usb::android::bar_heights_px() {
        Ok((top, bottom)) => {
            v[1] = v[1].max(top);
            v[3] = v[3].max(bottom);
            diag.push(format!("barras {top}/{bottom}"));
        }
        Err(e) => diag.push(format!("barras erro: {e}")),
    }
    (v, diag.join(" · "))
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub fn android_main(app: winit::platform::android::activity::AndroidApp) {
    let _ = ANDROID_APP.set(app.clone());
    if let Some(p) = app.internal_data_path() {
        let _ = BENCH_DIR.set(p.join("bench"));
        let _ = discolab_core::report::REPORT_DIR.set(p.join("relatorios"));
    }
    let mut opts = options();
    opts.android_app = Some(app);
    let _ = run(opts);
}

#[doc(hidden)]
impl App {
    /// Usado pelos testes de captura de tela.
    pub fn test_hook(&mut self, cmd: &str) {
        match cmd {
            "health" => self.tab = Tab::Health,
            "smart" => self.tab = Tab::Smart,
            "bench" => self.tab = Tab::Bench,
            "surface" => self.tab = Tab::Surface,
            "report" => self.tab = Tab::Report,
            "capacity" => {
                self.tab = Tab::Bench;
                self.bench_mode = BenchMode::Capacity;
            }
            "perf" => self.bench_mode = BenchMode::Perf,
            "picker" => self.picker_open = true,
            "nopicker" => self.picker_open = false,
            "insets" => self.insets = [0.0, 32.0, 0.0, 24.0],
            "light" => {
                set_dark(false);
                apply_theme(&self.ctx, self.is_mobile());
            }
            "quickbench" => {
                self.bench.size_mib = 256;
                self.bench.secs = 1.0;
                self.bench.sustained_gib = 1.0;
                self.start_bench();
            }
            _ => {}
        }
    }
    pub fn test_temps(&self) -> (usize, Option<String>) {
        (self.bench.temps.len(), self.bench.temp_dev.clone())
    }
    pub fn test_busy(&self) -> bool {
        self.bench.running || self.data.values().any(|d| d.loading) || self.devices.is_empty()
    }
}

fn attr_info(id: u32) -> (&'static str, bool) {
    match id {
        1 => ("Taxa de erros de leitura (Seagate: valor bruto empacotado, ignore)", false),
        3 => ("Tempo para o motor atingir rotação", false),
        4 => ("Ciclos de partida/parada", false),
        5 => ("Setores realocados — degradação real da mídia", true),
        7 => ("Erros de posicionamento (Seagate: valor bruto empacotado)", false),
        9 => ("Horas ligado", false),
        10 => ("Repetições de partida do motor — problema mecânico", true),
        12 => ("Ciclos de energia", false),
        170 | 180 => ("Blocos reserva disponíveis", false),
        171 | 172 | 181 | 182 => ("Falhas de programação/apagamento da flash", true),
        173 | 177 => ("Contador de nivelamento de desgaste", false),
        174 | 192 => ("Desligamentos inesperados", false),
        183 => ("Downshift de interface SATA", false),
        184 => ("Erros fim-a-fim (cache/DRAM do disco)", true),
        187 => ("Erros incorrigíveis reportados", true),
        188 => ("Timeouts de comando (cabo/fonte)", false),
        190 | 194 => ("Temperatura", false),
        191 => ("Choques/vibração detectados", false),
        193 => ("Ciclos de estacionamento das cabeças", false),
        195 => ("Correções ECC por hardware", false),
        196 => ("Eventos de realocação", false),
        197 => ("Setores pendentes — instáveis agora", true),
        198 => ("Incorrigíveis offline", true),
        199 => ("Erros CRC de interface — cabo/porta, não o disco", false),
        200 => ("Erros de escrita multi-zona", false),
        202 | 231 | 233 | 169 => ("Vida útil / desgaste (normalizado = % restante)", false),
        232 => ("Espaço reserva disponível", false),
        241 => ("Total escrito pelo host", false),
        242 => ("Total lido pelo host", false),
        _ => ("", false),
    }
}

fn health_color(a: &Assessment) -> Color32 {
    match a.status {
        Severity::Critical => RED(),
        Severity::Warning => ORANGE(),
        _ if a.health >= 90 => GREEN(),
        _ => YELLOW(),
    }
}

fn drive_icon(k: DriveKind) -> Icon {
    match k {
        DriveKind::Hdd => Icon::Disk,
        DriveKind::Usb => Icon::Usb,
        DriveKind::Nvme | DriveKind::SataSsd => Icon::Ssd,
        _ => Icon::Chip,
    }
}

fn gauge(ui: &mut egui::Ui, a: &Assessment, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    let p = ui.painter();
    let c = rect.center();
    let r = size * 0.40;
    let col = health_color(a);
    p.circle_filled(c, r + 12.0, col.gamma_multiply(0.07));
    arc(p, c, r, 0.0, 1.0, Stroke::new(13.0, TRACK()));
    let frac = (a.health as f32 / 100.0).clamp(0.0, 1.0);
    if frac > 0.0 {
        arc(p, c, r, 0.0, frac, Stroke::new(13.0, col));
        for t in [0.0, frac] {
            let ang = -std::f32::consts::FRAC_PI_2 + t * std::f32::consts::TAU;
            p.circle_filled(c + Vec2::new(ang.cos(), ang.sin()) * r, 6.5, col);
        }
    }
    p.text(c - Vec2::new(0.0, 6.0), egui::Align2::CENTER_CENTER, format!("{}%", a.health), egui::FontId::proportional(size * 0.21), TEXT());
    p.text(c + Vec2::new(0.0, size * 0.15), egui::Align2::CENTER_CENTER, "SAÚDE", egui::FontId::proportional(11.0), MUTED());
}

fn life_bar(ui: &mut egui::Ui, l: u32) {
    let w = ui.available_width().min(460.0);
    ui.allocate_ui(Vec2::new(w, 34.0), |ui| {
        ui.set_width(w);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Vida útil restante").color(MUTED()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(format!("{l}%")).strong());
            });
        });
        let (r, _) = ui.allocate_exact_size(Vec2::new(w, 10.0), egui::Sense::hover());
        ui.painter().rect_filled(r, 5, TRACK());
        let col = match l {
            0..=10 => RED(),
            11..=30 => ORANGE(),
            _ => GREEN(),
        };
        let mut f = r;
        f.set_width((r.width() * l as f32 / 100.0).max(10.0));
        ui.painter().rect_filled(f, 5, col);
    });
}

fn bench_row(ui: &mut egui::Ui, name: &str, rd: Option<&TestResult>, wr: Option<&TestResult>, active: bool, mobile: bool) {
    let stroke = if active { Stroke::new(1.5, ACCENT()) } else { Stroke::new(1.0, BORDER()) };
    egui::Frame::new().fill(CARD()).stroke(stroke).corner_radius(14).inner_margin(12).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let tile = |ui: &mut egui::Ui, r: Option<&TestResult>, color: Color32, w: f32| {
            ui.vertical(|ui| {
                ui.set_width(w);
                match r {
                    Some(r) => {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("{:.1}", r.mbps)).size(if mobile { 23.0 } else { 26.0 }).strong().color(color));
                            ui.label(RichText::new("MB/s").small().color(MUTED()));
                        });
                        ui.label(RichText::new(format!("{:.0} IOPS · p99 {}", r.iops, fmt_us(r.lat_p99_us))).small().color(MUTED()));
                    }
                    None => {
                        ui.label(RichText::new("—").size(23.0).color(TRACK()));
                        ui.label(RichText::new(" ").small());
                    }
                }
            });
        };
        if mobile {
            ui.label(RichText::new(name).strong());
            ui.add_space(2.0);
            let w = (ui.available_width() - 12.0) / 2.0;
            ui.horizontal(|ui| {
                tile(ui, rd, BLUE(), w);
                tile(ui, wr, ORANGE(), w);
            });
        } else {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(170.0);
                    ui.add_space(6.0);
                    ui.label(RichText::new(name).size(16.0).strong());
                    if let Some(r) = rd {
                        ui.label(RichText::new(format!("latência {}", fmt_us(r.lat_p50_us))).small().color(MUTED()));
                    }
                });
                let w = (ui.available_width() - 24.0) / 2.0;
                tile(ui, rd, BLUE(), w);
                tile(ui, wr, ORANGE(), w);
            });
        }
    });
}

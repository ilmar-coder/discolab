//! DiscoLab — interface gráfica (desktop e Android) sobre o `discolab-core`.

use discolab_core::bench::{BenchConfig, BenchEvent, BenchReport, TestResult, TestSpec, default_tests, run_bench};
use discolab_core::device::{Device, DriveKind, Volume, list_devices, list_volumes};
use discolab_core::health::{Assessment, Severity, assess};
use discolab_core::report::{save_report, text_report};
use discolab_core::smart::{SelfTest, SmartReport, read_smart, start_self_test};
use discolab_core::surface::{BUCKET_LABELS, MAP_CELLS, ScanConfig, ScanEvent, ScanSummary, run_scan};
use discolab_core::util::human_bytes;
use eframe::egui::{self, Color32, RichText, Stroke, Vec2};
use egui_plot::{Line, Plot, PlotPoints};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, mpsc};

/// Pasta padrão de benchmark (definida pelo Android para o armazenamento do app).
pub static BENCH_DIR: OnceLock<PathBuf> = OnceLock::new();

enum Msg {
    Devices(Vec<Device>, Vec<Volume>),
    Smart(String, Box<SmartReport>),
    Bench(BenchEvent),
    Scan(String, ScanEvent),
    SelfTest(Result<String, String>),
}

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Health,
    Smart,
    Bench,
    Surface,
    Report,
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
}

// ------------------------------------------------------------------ cores

const GREEN: Color32 = Color32::from_rgb(46, 204, 113);
const YELLOW: Color32 = Color32::from_rgb(241, 196, 15);
const ORANGE: Color32 = Color32::from_rgb(243, 156, 18);
const RED: Color32 = Color32::from_rgb(231, 76, 60);
const BLUE: Color32 = Color32::from_rgb(52, 152, 219);
const DIM: Color32 = Color32::from_rgb(140, 140, 150);

fn sev_color(s: Severity) -> Color32 {
    match s {
        Severity::Critical => RED,
        Severity::Warning => ORANGE,
        Severity::Notice => YELLOW,
        Severity::Ok => GREEN,
        Severity::Info => DIM,
    }
}

fn health_color(a: &Assessment) -> Color32 {
    match a.status {
        Severity::Critical => RED,
        Severity::Warning => ORANGE,
        _ if a.health >= 90 => GREEN,
        _ => YELLOW,
    }
}

fn bucket_color(b: u8) -> Color32 {
    match b {
        0 => Color32::from_rgb(85, 90, 100),
        1 => Color32::from_rgb(125, 130, 140),
        2 => Color32::from_rgb(175, 180, 190),
        3 => Color32::from_rgb(46, 204, 113),
        4 => ORANGE,
        5 => RED,
        6 => BLUE,
        _ => Color32::from_rgb(38, 40, 46),
    }
}

/// Explicação curta de cada atributo SMART ATA (e se é crítico para a saúde).
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

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let ctx = cc.egui_ctx.clone();
        ctx.set_visuals(egui::Visuals::dark());
        ctx.global_style_mut(|s| {
            s.spacing.item_spacing = Vec2::new(8.0, 6.0);
            s.spacing.button_padding = Vec2::new(10.0, 5.0);
        });
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
        };
        app.refresh_devices();
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
                    self.status = format!("{} disco(s), {} volume(s)", d.len(), v.len());
                    for x in &d {
                        self.data.entry(x.name.clone()).or_default().loading = !x.smart_path.is_empty();
                    }
                    self.devices = d;
                    self.volumes = v;
                    self.sel = self.sel.min(self.devices.len().saturating_sub(1));
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
                Msg::Scan(name, e) => self.on_scan(&name, e),
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
            }
            BenchEvent::Done(r) => {
                b.running = false;
                b.phase = if r.cancelled { "Interrompido".into() } else { "Concluído".into() };
                b.progress = 1.0;
                b.report = Some(r);
            }
        }
    }

    fn on_scan(&mut self, name: &str, e: ScanEvent) {
        let s = &mut self.scan;
        match e {
            ScanEvent::Started { total_blocks, uncached, size } => {
                s.total = total_blocks;
                s.log.push(format!("Iniciado: {} · {} blocos · sem cache: {}", human_bytes(size), total_blocks, if uncached { "sim" } else { "não" }));
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

    // ------------------------------------------------------------ UI

    fn device_list(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Discos");
            if ui.small_button("Atualizar").on_hover_text("Atualizar lista").clicked() {
                self.refresh_devices();
            }
        });
        ui.separator();
        let mut clicked = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (i, d) in self.devices.iter().enumerate() {
                let dd = self.data.get(&d.name);
                let frame = egui::Frame::group(ui.style()).fill(if i == self.sel {
                    ui.visuals().selection.bg_fill.gamma_multiply(0.35)
                } else {
                    ui.visuals().faint_bg_color
                });
                let r = frame
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(RichText::new(&d.model).strong());
                        ui.label(RichText::new(format!("{} · {} · {}", d.kind.label(), human_bytes(d.size_bytes), d.name)).small().color(DIM));
                        match dd.and_then(|x| x.health.as_ref()) {
                            Some(h) => {
                                ui.label(RichText::new(format!("Saúde {}% · {}", h.health, h.status_label())).color(health_color(h)).strong());
                            }
                            None if dd.map(|x| x.loading).unwrap_or(false) => {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.label(RichText::new("lendo SMART…").small());
                                });
                            }
                            None => {
                                ui.label(RichText::new("sem dados de saúde").small().color(DIM));
                            }
                        }
                    })
                    .response
                    .interact(egui::Sense::click());
                if r.clicked() {
                    clicked = Some(i);
                }
            }
        });
        if let Some(i) = clicked {
            self.sel = i;
        }
    }

    fn tabs(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            for (t, l) in [
                (Tab::Health, "Saúde"),
                (Tab::Smart, "SMART"),
                (Tab::Bench, "Benchmark"),
                (Tab::Surface, "Superfície"),
                (Tab::Report, "Relatório"),
            ] {
                ui.selectable_value(&mut self.tab, t, RichText::new(l).size(15.0));
            }
        });
    }

    fn gauge(ui: &mut egui::Ui, a: &Assessment) {
        let size = 150.0;
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
        let p = ui.painter();
        let c = rect.center();
        let r = size * 0.42;
        p.circle_stroke(c, r, Stroke::new(12.0, Color32::from_gray(50)));
        let frac = a.health as f32 / 100.0;
        let n = 96;
        let pts: Vec<egui::Pos2> = (0..=((n as f32 * frac) as usize))
            .map(|i| {
                let ang = -std::f32::consts::FRAC_PI_2 + i as f32 / n as f32 * std::f32::consts::TAU;
                c + Vec2::new(ang.cos(), ang.sin()) * r
            })
            .collect();
        if pts.len() > 1 {
            p.add(egui::Shape::line(pts, Stroke::new(12.0, health_color(a))));
        }
        p.text(c - Vec2::new(0.0, 8.0), egui::Align2::CENTER_CENTER, format!("{}%", a.health), egui::FontId::proportional(34.0), Color32::WHITE);
        p.text(c + Vec2::new(0.0, 22.0), egui::Align2::CENTER_CENTER, a.status_label(), egui::FontId::proportional(14.0), health_color(a));
    }

    fn tab_health(&mut self, ui: &mut egui::Ui) {
        let Some(dev) = self.current().cloned() else {
            ui.label("Nenhum disco encontrado.");
            return;
        };
        let dd = self.data.entry(dev.name.clone()).or_default();
        let (smart, health, loading) = (dd.smart.clone(), dd.health.clone(), dd.loading);
        ui.horizontal(|ui| {
            ui.heading(&dev.model);
            if loading {
                ui.spinner();
            }
        });
        ui.label(RichText::new(format!("{} · {} · {} · {}", dev.kind.label(), dev.bus, human_bytes(dev.size_bytes), dev.name)).color(DIM));
        ui.add_space(6.0);

        let Some(s) = smart else {
            if dev.smart_path.is_empty() {
                ui.label("Este dispositivo não expõe SMART (Android sem root). Use a aba Benchmark.");
            } else if discolab_core::usb::is_usb(&dev.smart_path) {
                ui.label("Abrindo o disco USB… Se o Android perguntar, toque em \"Permitir\" para o DiscoLab acessar o dispositivo.");
            } else {
                ui.label("Lendo dados SMART…");
            }
            return;
        };
        let layout = if ui.available_width() < 560.0 {
            egui::Layout::top_down(egui::Align::Min)
        } else {
            egui::Layout::left_to_right(egui::Align::Min)
        };
        ui.with_layout(layout, |ui| {
            if let Some(a) = &health {
                Self::gauge(ui, a);
            }
            ui.vertical(|ui| {
                egui::Grid::new("facts").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
                    let mut row = |k: &str, v: String| {
                        ui.label(RichText::new(k).color(DIM));
                        ui.label(RichText::new(v).strong());
                        ui.end_row();
                    };
                    if let Some(a) = &health {
                        if let Some(l) = a.life_left {
                            row("Vida útil restante", format!("{l}%"));
                        }
                        row("Confiança", a.confidence.to_string());
                    }
                    if let Some(t) = s.temperature {
                        row("Temperatura", format!("{t} °C"));
                    }
                    if let Some(h) = s.power_on_hours {
                        row("Tempo ligado", format!("{h} h ({:.1} anos)", h as f64 / 8760.0));
                    }
                    if let Some(c) = s.power_cycles {
                        row("Ciclos de energia", c.to_string());
                    }
                    if let Some(w) = s.host_written_bytes {
                        row("Total escrito", human_bytes(w));
                    }
                    if let Some(r) = s.host_read_bytes {
                        row("Total lido", human_bytes(r));
                    }
                    if let Some(rpm) = s.rotation_rpm.filter(|r| *r > 0) {
                        row("Rotação", format!("{rpm} RPM"));
                    }
                    if let Some(x) = &s.serial {
                        row("Serial", x.clone());
                    }
                    if let Some(x) = &s.firmware {
                        row("Firmware", x.clone());
                    }
                    if let Some(p) = s.smart_passed {
                        row("Status do firmware", if p { "PASSED".into() } else { "FAILED".into() });
                    }
                    row("Fonte", s.source.label().to_string());
                });
            });
        });
        if let Some(a) = &health {
            ui.add_space(4.0);
            ui.label(RichText::new(&a.verdict).size(16.0).color(health_color(a)));
            if let Some(l) = a.life_left {
                ui.add(egui::ProgressBar::new(l as f32 / 100.0).text(format!("Vida útil {l}%")).desired_width(320.0));
            }
        }
        for m in &s.messages {
            ui.label(RichText::new(format!("• {m}")).small().color(DIM));
        }
        if s.needs_admin {
            ui.colored_label(ORANGE, "Permissão insuficiente. Rode como Administrador (Windows) ou com sudo (Linux/macOS) para SMART completo e varredura de superfície.");
        }
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            if ui.button("Reler SMART").clicked() {
                self.refresh_smart(&dev);
            }
            if s.source == discolab_core::smart::SmartSource::Smartctl {
                for (l, k) in [("Autoteste curto (~2 min)", SelfTest::Short), ("Autoteste longo", SelfTest::Long), ("Abortar autoteste", SelfTest::Abort)] {
                    if ui.button(l).clicked() {
                        let d = dev.clone();
                        self.spawn(move |tx| {
                            let _ = tx.send(Msg::SelfTest(start_self_test(&d, k)));
                        });
                    }
                }
            }
            if let Some(p) = s.self_test_running {
                ui.label(format!("Autoteste em andamento: {p}% restante"));
            }
        });
        if let Some(m) = &self.selftest_msg {
            ui.label(RichText::new(m).small());
        }
        ui.separator();
        ui.heading("Diagnóstico");
        if let Some(a) = &health {
            ui.scope(|ui| {
                for f in &a.findings {
                    egui::Frame::group(ui.style()).stroke(Stroke::new(1.0, sev_color(f.severity).gamma_multiply(0.6))).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(f.severity.label()).color(sev_color(f.severity)).strong());
                            ui.label(RichText::new(&f.title).strong());
                            if f.penalty > 0.0 {
                                ui.label(RichText::new(format!("-{:.0} pts", f.penalty)).small().color(DIM));
                            }
                        });
                        ui.label(&f.detail);
                    });
                }
                if !s.self_tests.is_empty() {
                    ui.add_space(6.0);
                    ui.label(RichText::new("Histórico de autotestes").strong());
                    for t in s.self_tests.iter().take(8) {
                        let c = match t.passed {
                            Some(true) => GREEN,
                            Some(false) => RED,
                            None => DIM,
                        };
                        ui.label(RichText::new(format!("{} — {} {}", t.kind, t.status, t.hours.map(|h| format!("({h} h)")).unwrap_or_default())).color(c));
                    }
                }
            });
        }
    }

    fn tab_smart(&mut self, ui: &mut egui::Ui) {
        let Some(dev) = self.current().cloned() else { return };
        let Some(s) = self.data.get(&dev.name).and_then(|d| d.smart.clone()) else {
            ui.label("Sem dados SMART.");
            return;
        };
        egui::ScrollArea::both().show(ui, |ui| {
            if !s.ata.is_empty() {
                egui::Grid::new("ata").striped(true).num_columns(7).spacing([14.0, 4.0]).show(ui, |ui| {
                    for h in ["ID", "Atributo", "Atual", "Pior", "Limite", "Bruto", "Significado"] {
                        ui.label(RichText::new(h).strong());
                    }
                    ui.end_row();
                    for a in &s.ata {
                        let (desc, critical) = attr_info(a.id);
                        let failing = a.thresh > 0 && a.value <= a.thresh;
                        let bad_raw = critical && (a.raw & 0xFFFF_FFFF) > 0;
                        let col = if failing {
                            RED
                        } else if bad_raw {
                            ORANGE
                        } else {
                            ui.visuals().text_color()
                        };
                        ui.label(RichText::new(format!("{:3}", a.id)).monospace());
                        ui.label(RichText::new(&a.name).color(col));
                        ui.label(a.value.to_string());
                        ui.label(a.worst.to_string());
                        ui.label(a.thresh.to_string());
                        ui.label(RichText::new(&a.raw_str).monospace().color(col));
                        ui.label(RichText::new(desc).small().color(DIM));
                        ui.end_row();
                    }
                });
            }
            if let Some(n) = &s.nvme {
                ui.heading("NVMe SMART / Health Log");
                egui::Grid::new("nvme").striped(true).num_columns(2).spacing([24.0, 4.0]).show(ui, |ui| {
                    let mut row = |k: &str, v: String, c: Option<Color32>| {
                        ui.label(k);
                        let t = RichText::new(v).monospace();
                        ui.label(if let Some(c) = c { t.color(c) } else { t });
                        ui.end_row();
                    };
                    row("Aviso crítico", format!("0x{:02X}", n.critical_warning), (n.critical_warning != 0).then_some(RED));
                    row("Temperatura", n.temperature.map(|t| format!("{t} °C")).unwrap_or("-".into()), None);
                    row("Reserva disponível", format!("{}%", n.available_spare), (n.available_spare < n.available_spare_threshold).then_some(RED));
                    row("Limite da reserva", format!("{}%", n.available_spare_threshold), None);
                    row("Percentual usado (desgaste)", format!("{}%", n.percentage_used), (n.percentage_used >= 90).then_some(ORANGE));
                    row("Dados lidos", human_bytes(n.data_units_read * 512_000), None);
                    row("Dados escritos", human_bytes(n.data_units_written * 512_000), None);
                    row("Ciclos de energia", n.power_cycles.to_string(), None);
                    row("Horas ligado", n.power_on_hours.to_string(), None);
                    row("Desligamentos inseguros", n.unsafe_shutdowns.to_string(), None);
                    row("Erros de mídia/integridade", n.media_errors.to_string(), (n.media_errors > 0).then_some(RED));
                    row("Entradas no log de erros", n.num_err_log_entries.to_string(), None);
                    row("Tempo em temp. de aviso (min)", n.warning_temp_time.to_string(), None);
                    row("Tempo em temp. crítica (min)", n.critical_comp_time.to_string(), None);
                });
            }
            if let Some(f) = &s.flash {
                ui.heading("Memória flash (eMMC/UFS)");
                ui.label(format!("Desgaste tipo A: {}", f.used_pct_a.map(|x| format!("{}–{x}%", x.saturating_sub(10))).unwrap_or("-".into())));
                ui.label(format!("Desgaste tipo B: {}", f.used_pct_b.map(|x| format!("{}–{x}%", x.saturating_sub(10))).unwrap_or("-".into())));
                ui.label(format!("Pre-EOL: {}", match f.pre_eol { Some(1) => "Normal", Some(2) => "Aviso", Some(3) => "Urgente", _ => "-" }));
            }
            for (k, v) in &s.extra {
                ui.label(format!("{k}: {v}"));
            }
            if s.ata.is_empty() && s.nvme.is_none() && s.flash.is_none() && s.extra.is_empty() {
                ui.label("Nenhum atributo disponível.");
            }
        });
    }

    fn bench_dir(&self) -> Option<PathBuf> {
        if self.bench.custom {
            return Some(PathBuf::from(&self.bench.dir)).filter(|p| !p.as_os_str().is_empty());
        }
        let v = self.volumes.get(self.bench.vol)?;
        let tmp = std::env::temp_dir();
        let tmp = std::fs::canonicalize(&tmp).unwrap_or(tmp);
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from);
        let best = |root: &PathBuf| -> bool {
            // o volume mais específico que contém o caminho
            self.volumes.iter().filter(|x| root.starts_with(&x.mount)).max_by_key(|x| x.mount.as_os_str().len()).map(|x| x.mount == v.mount).unwrap_or(false)
        };
        Some(if best(&tmp) {
            tmp.join("DiscoLab")
        } else if let Some(h) = home.filter(|h| best(h)) {
            h.join("DiscoLab-bench")
        } else {
            v.mount.join("DiscoLab-bench")
        })
    }

    fn tab_bench(&mut self, ui: &mut egui::Ui) {
        let running = self.bench.running;
        ui.add_enabled_ui(!running, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Destino:");
                let sel_text = if self.bench.custom {
                    "Pasta personalizada".to_string()
                } else {
                    self.volumes.get(self.bench.vol).map(|v| format!("{} ({} livres)", v.mount.display(), human_bytes(v.free))).unwrap_or("-".into())
                };
                egui::ComboBox::from_id_salt("vol").selected_text(sel_text).width(280.0).show_ui(ui, |ui| {
                    for (i, v) in self.volumes.iter().enumerate() {
                        if ui.selectable_label(!self.bench.custom && self.bench.vol == i, format!("{}  [{}]  {} livres", v.mount.display(), v.fs, human_bytes(v.free))).clicked() {
                            self.bench.vol = i;
                            self.bench.custom = false;
                        }
                    }
                    if ui.selectable_label(self.bench.custom, "Pasta personalizada…").clicked() {
                        self.bench.custom = true;
                    }
                });
                if self.bench.custom {
                    ui.add(egui::TextEdit::singleline(&mut self.bench.dir).hint_text("/caminho/da/pasta").desired_width(220.0));
                }
            });
            ui.horizontal_wrapped(|ui| {
                ui.label("Arquivo:");
                egui::ComboBox::from_id_salt("size").selected_text(fmt_mib(self.bench.size_mib)).show_ui(ui, |ui| {
                    for m in [256u64, 512, 1024, 2048, 4096, 8192, 16384] {
                        ui.selectable_value(&mut self.bench.size_mib, m, fmt_mib(m));
                    }
                });
                ui.label("Duração/teste:");
                ui.add(egui::Slider::new(&mut self.bench.secs, 2.0..=30.0).suffix(" s").integer());
                ui.label("Escrita sustentada:");
                ui.add(egui::Slider::new(&mut self.bench.sustained_gib, 0.0..=64.0).suffix(" GiB").integer())
                    .on_hover_text("Escreve vários GB seguidos para revelar a velocidade real depois que o cache SLC/DRAM enche. 0 = desligado.");
                ui.checkbox(&mut self.bench.direct, "Sem cache do SO").on_hover_text("Recomendado. Desligar mede a RAM, como alguns programas fazem.");
            });
        });
        ui.horizontal(|ui| {
            if !running {
                if ui.add(egui::Button::new(RichText::new("Iniciar benchmark").strong()).fill(Color32::from_rgb(30, 110, 60))).clicked() {
                    self.start_bench();
                }
            } else if ui.add(egui::Button::new("Parar").fill(Color32::from_rgb(130, 40, 40))).clicked() {
                self.bench.cancel.store(true, Ordering::Relaxed);
            }
            if running {
                ui.spinner();
            }
            ui.label(&self.bench.phase);
        });
        if running || self.bench.progress > 0.0 {
            ui.add(egui::ProgressBar::new(self.bench.progress).show_percentage());
        }
        if let Some(e) = &self.bench.error {
            ui.colored_label(ORANGE, e);
        }
        if let Some(r) = &self.bench.report {
            for w in &r.warnings {
                if Some(w) != self.bench.error.as_ref() {
                    ui.colored_label(ORANGE, w);
                }
            }
            if !r.uncached {
                ui.colored_label(ORANGE, "Atenção: resultado com cache do SO: os números de leitura não representam o disco.");
            }
        }
        ui.separator();
        egui::ScrollArea::both().id_salt("bench_scroll").show(ui, |ui| {
            egui::Grid::new("bres").striped(true).num_columns(7).spacing([18.0, 6.0]).show(ui, |ui| {
                for h in ["Teste", "MB/s", "IOPS", "Lat. média", "p50", "p99", "p99.9 / máx"] {
                    ui.label(RichText::new(h).strong());
                }
                ui.end_row();
                for (i, t) in self.bench.tests.iter().enumerate() {
                    ui.label(&t.name);
                    match self.bench.results.get(i).and_then(|x| x.as_ref()) {
                        Some(r) => {
                            ui.label(RichText::new(format!("{:.1}", r.mbps)).size(18.0).strong().color(if t.write { ORANGE } else { BLUE }));
                            ui.label(format!("{:.0}", r.iops));
                            ui.label(fmt_us(r.lat_avg_us as u32));
                            ui.label(fmt_us(r.lat_p50_us));
                            ui.label(fmt_us(r.lat_p99_us));
                            ui.label(format!("{} / {}", fmt_us(r.lat_p999_us), fmt_us(r.lat_max_us)));
                        }
                        None => {
                            for _ in 0..6 {
                                ui.label(RichText::new("—").color(DIM));
                            }
                        }
                    }
                    ui.end_row();
                }
            });
            ui.add_space(8.0);
            if !self.bench.live.is_empty() {
                let name = self.bench.tests.get(self.bench.live_test).map(|t| t.name.clone()).unwrap_or_default();
                ui.label(RichText::new(format!("Vazão ao longo do teste: {name}")).strong());
                let pts = self.bench.live.clone();
                Plot::new("live").height(160.0).include_y(0.0).include_x(0.0).allow_scroll(false).x_axis_label("s").y_axis_label("MB/s").show(ui, |p| {
                    p.line(Line::new("MB/s", PlotPoints::from(pts)).color(BLUE));
                });
            }
            if !self.bench.sustained.is_empty() {
                ui.label(RichText::new("Escrita sustentada (cache SLC/DRAM)").strong());
                let pts = self.bench.sustained.clone();
                Plot::new("sust").height(180.0).include_y(0.0).include_x(0.0).allow_scroll(false).x_axis_label("GB escritos").y_axis_label("MB/s").show(ui, |p| {
                    p.line(Line::new("MB/s", PlotPoints::from(pts)).color(ORANGE));
                });
                if let Some(s) = self.bench.report.as_ref().and_then(|r| r.sustained.as_ref()) {
                    ui.label(format!(
                        "Pico {:.0} MB/s · Sustentado {:.0} MB/s · Mínimo {:.0} MB/s · {}",
                        s.burst_mbps,
                        s.sustained_mbps,
                        s.min_mbps,
                        s.cache_estimate_bytes.map(|c| format!("cache estimado ~{}", human_bytes(c))).unwrap_or("sem queda de cache detectada".into())
                    ));
                }
            }
            ui.add_space(6.0);
            ui.label(RichText::new(
                "Como garantimos números honestos: E/S direta sem cache do SO, dados aleatórios (incompressíveis) com \
                 cada página alterada a cada escrita, latência por operação (p99/máx revelam travadas) e teste de escrita \
                 sustentada para expor o fim do cache SLC.",
            ).small().color(DIM));
        });
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
        let cancel = b.cancel.clone();
        let ctx = self.ctx.clone();
        self.spawn(move |tx| {
            run_bench(&cfg, &cancel, &mut |e| {
                let _ = tx.send(Msg::Bench(e));
                ctx.request_repaint();
            });
        });
    }

    fn tab_surface(&mut self, ui: &mut egui::Ui) {
        let Some(dev) = self.current().cloned() else { return };
        if dev.raw_path.is_empty() {
            ui.label("Acesso bruto ao dispositivo indisponível (no Android é necessário root).");
            return;
        }
        let running = self.scan.running;
        let usb = discolab_core::usb::is_usb(&dev.raw_path);
        if usb {
            ui.colored_label(YELLOW, "Disco USB acessado diretamente: o Android desmonta o volume durante o teste. Reconecte o adaptador para usá-lo normalmente depois.");
        } else if !discolab_core::util::is_elevated() {
            ui.colored_label(ORANGE, if cfg!(windows) {
                "A leitura bruta do disco exige executar o DiscoLab como Administrador."
            } else {
                "A leitura bruta do disco exige root: execute com sudo (ex.: sudo ./discolab)."
            });
        }
        ui.label(RichText::new("Leitura de toda a superfície em modo SOMENTE LEITURA — seguro para os dados. Blocos lentos ou com erro são relidos para confirmar.").color(DIM));
        ui.add_enabled_ui(!running, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Início:");
                ui.add(egui::DragValue::new(&mut self.scan.start_pct).range(0.0..=99.0).suffix(" %"));
                ui.label("Fim:");
                ui.add(egui::DragValue::new(&mut self.scan.end_pct).range(1.0..=100.0).suffix(" %"));
                ui.label("Bloco:");
                egui::ComboBox::from_id_salt("blk").selected_text(format!("{} KiB", self.scan.block_kib)).show_ui(ui, |ui| {
                    for k in [64usize, 128, 256, 512, 1024, 4096] {
                        ui.selectable_value(&mut self.scan.block_kib, k, format!("{k} KiB"));
                    }
                });
            });
        });
        ui.horizontal(|ui| {
            if !running {
                if ui.add(egui::Button::new(RichText::new("Iniciar varredura").strong()).fill(Color32::from_rgb(30, 110, 60))).clicked() {
                    self.start_scan(&dev);
                }
            } else {
                if ui.add(egui::Button::new("Parar").fill(Color32::from_rgb(130, 40, 40))).clicked() {
                    self.scan.cancel.store(true, Ordering::Relaxed);
                }
                ui.spinner();
            }
            if self.scan.total > 0 {
                let pct = self.scan.done as f64 / self.scan.total as f64;
                let eta = self.scan.started.map(|t| t.elapsed().as_secs_f64()).filter(|_| pct > 0.001).map(|e| e / pct - e).unwrap_or(0.0);
                ui.label(format!("{:.1}% · {:.0} MB/s{}", pct * 100.0, self.scan.mbps, if running { format!(" · faltam ~{}", fmt_dur(eta)) } else { String::new() }));
            }
            if self.scan.dev != dev.name && !self.scan.dev.is_empty() {
                ui.label(RichText::new(format!("(mapa de {})", self.scan.dev)).color(DIM));
            }
        });
        if let Some(e) = &self.scan.error {
            ui.colored_label(RED, e);
        }
        if self.scan.total > 0 {
            ui.add(egui::ProgressBar::new(self.scan.done as f32 / self.scan.total as f32));
        }
        egui::ScrollArea::vertical().id_salt("scan_scroll").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (i, l) in BUCKET_LABELS.iter().enumerate() {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(12.0), egui::Sense::hover());
                    ui.painter().rect_filled(r, 2.0, bucket_color(i as u8));
                    ui.label(format!("{l}: {}", self.scan.buckets[i]));
                    ui.add_space(6.0);
                }
            });
            // mapa de blocos
            let w = ui.available_width().min(1100.0);
            let cols = if w > 700.0 { 128 } else { 64 };
            let rows = MAP_CELLS / cols;
            let cs = w / cols as f32;
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, cs * rows as f32), egui::Sense::hover());
            let p = ui.painter_at(rect);
            for (i, &b) in self.scan.cells.iter().enumerate() {
                let (x, y) = ((i % cols) as f32, (i / cols) as f32);
                let r = egui::Rect::from_min_size(rect.min + Vec2::new(x * cs, y * cs), Vec2::splat(cs - 1.0));
                p.rect_filled(r, 1.0, bucket_color(b));
            }
            if let Some(pos) = resp.hover_pos() {
                let i = (((pos.y - rect.min.y) / cs) as usize) * cols + ((pos.x - rect.min.x) / cs) as usize;
                if i < MAP_CELLS {
                    let pct = self.scan.start_pct as f64 + (self.scan.end_pct - self.scan.start_pct) as f64 * i as f64 / MAP_CELLS as f64;
                    resp.on_hover_text(format!("~{:.2}% do disco · {}", pct, BUCKET_LABELS.get(self.scan.cells[i] as usize).copied().unwrap_or("não lido")));
                }
            }
            if !self.scan.curve.is_empty() {
                ui.add_space(6.0);
                ui.label(RichText::new("Velocidade de leitura ao longo do disco").strong());
                let pts = self.scan.curve.clone();
                Plot::new("scurve").height(170.0).include_y(0.0).include_x(0.0).include_x(100.0).allow_scroll(false).x_axis_label("% do disco").y_axis_label("MB/s").show(ui, |p| {
                    p.line(Line::new("MB/s", PlotPoints::from(pts)).color(GREEN));
                });
            }
            for l in self.scan.log.iter().rev().take(12) {
                ui.label(RichText::new(l).small());
            }
            ui.label(RichText::new(
                "Diferente do Victoria, um bloco lento só conta como problema se continuar lento nas releituras — \
                 picos causados pelo sistema operacional são marcados como transitórios. Em SSDs, a varredura detecta \
                 blocos ilegíveis e travamentos do controlador; a lentidão por posição só é relevante em HDDs.",
            ).small().color(DIM));
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

    fn tab_report(&mut self, ui: &mut egui::Ui) {
        let Some(dev) = self.current().cloned() else { return };
        let dd = self.data.get(&dev.name);
        let smart = dd.and_then(|d| d.smart.as_ref());
        let health = dd.and_then(|d| d.health.as_ref());
        let scan = dd.and_then(|d| d.scan.as_ref());
        let bench = self.bench.report.as_ref();
        let mut text = text_report(&dev, smart, health, bench, scan);
        ui.horizontal(|ui| {
            if ui.button("Salvar (.txt + .json)").clicked() {
                let json = serde_json::json!({"device": dev, "smart": smart, "assessment": health, "benchmark": bench, "surface": scan});
                self.status = match save_report(&dev, &text, &json) {
                    Ok(p) => format!("Relatório salvo em {}", p.display()),
                    Err(e) => format!("Erro ao salvar: {e}"),
                };
            }
            if ui.button("Copiar").clicked() {
                ui.ctx().copy_text(text.clone());
                self.status = "Relatório copiado.".into();
            }
        });
        egui::ScrollArea::both().show(ui, |ui| {
            ui.add(egui::TextEdit::multiline(&mut text).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY).interactive(true));
        });
    }
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

fn fmt_mib(m: u64) -> String {
    if m >= 1024 { format!("{} GiB", m / 1024) } else { format!("{m} MiB") }
}

fn fmt_dur(s: f64) -> String {
    let s = s.max(0.0) as u64;
    if s >= 3600 { format!("{}h{:02}m", s / 3600, (s % 3600) / 60) } else { format!("{}m{:02}s", s / 60, s % 60) }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll();
        let narrow = ui.available_width() < 760.0;
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&self.status).small().color(DIM));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(concat!("DiscoLab v", env!("CARGO_PKG_VERSION"))).small().color(DIM));
                });
            });
        });
        if narrow {
            egui::Panel::top("top").show(ui, |ui| {
                ui.horizontal(|ui| {
                    let names: Vec<String> = self.devices.iter().map(|d| format!("{} ({})", d.model, human_bytes(d.size_bytes))).collect();
                    egui::ComboBox::new("devsel", "Disco").selected_text(names.get(self.sel).cloned().unwrap_or_default()).width(ui.available_width() - 140.0).show_ui(ui, |ui| {
                        for (i, n) in names.iter().enumerate() {
                            ui.selectable_value(&mut self.sel, i, n);
                        }
                    });
                    if ui.button("Atualizar").clicked() {
                        self.refresh_devices();
                    }
                });
                self.tabs(ui);
            });
        } else {
            egui::Panel::left("devs").resizable(true).default_size(270.0).show(ui, |ui| self.device_list(ui));
            egui::Panel::top("tabs").show(ui, |ui| self.tabs(ui));
        }
        egui::CentralPanel::default().show(ui, |ui| match self.tab {
            Tab::Health => {
                egui::ScrollArea::vertical().id_salt("health_scroll").show(ui, |ui| self.tab_health(ui));
            }
            Tab::Smart => self.tab_smart(ui),
            Tab::Bench => self.tab_bench(ui),
            Tab::Surface => self.tab_surface(ui),
            Tab::Report => self.tab_report(ui),
        });
        if self.bench.running || self.scan.running {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
        }
    }
}

pub fn options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1180.0, 780.0]).with_min_inner_size([420.0, 400.0]).with_title("DiscoLab"),
        ..Default::default()
    }
}

pub fn run(options: eframe::NativeOptions) -> eframe::Result {
    eframe::run_native("DiscoLab", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub fn android_main(app: winit::platform::android::activity::AndroidApp) {
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
            "quickbench" => {
                self.bench.size_mib = 256;
                self.bench.secs = 1.0;
                self.bench.sustained_gib = 1.0;
                self.start_bench();
            }
            _ => {}
        }
    }
    pub fn test_busy(&self) -> bool {
        self.bench.running || self.data.values().any(|d| d.loading) || self.devices.is_empty()
    }
}

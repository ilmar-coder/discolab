//! Benchmark honesto.
//!
//! Por que outros programas "enganam" e o que fazemos diferente:
//! 1. Cache do SO: abrimos com O_DIRECT / F_NOCACHE / FILE_FLAG_NO_BUFFERING e
//!    avisamos claramente quando não foi possível.
//! 2. Dados compressíveis/deduplicáveis: escrevemos dados aleatórios e mudamos cada
//!    página de 4 KiB a cada escrita (controladores SandForce/Phison não "trapaceiam").
//! 3. Cache SLC/DRAM: além do pico, o teste de escrita sustentada mostra a curva de
//!    velocidade ao longo de vários GB e estima onde o cache acaba.
//! 4. Latência: além de MB/s, reportamos p50/p99/p99.9/máx — médias escondem travadas.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::directio::{self, AlignedBuf};
use crate::util::Rng;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestSpec {
    pub name: String,
    pub write: bool,
    pub random: bool,
    pub block: usize,
    pub threads: usize,
}

impl TestSpec {
    fn new(name: &str, write: bool, random: bool, block: usize, threads: usize) -> Self {
        TestSpec { name: name.into(), write, random, block, threads }
    }
}

pub fn default_tests() -> Vec<TestSpec> {
    const M: usize = 1 << 20;
    const K4: usize = 4096;
    vec![
        TestSpec::new("SEQ 1M Q8 Leitura", false, false, M, 8),
        TestSpec::new("SEQ 1M Q8 Escrita", true, false, M, 8),
        TestSpec::new("SEQ 1M Q1 Leitura", false, false, M, 1),
        TestSpec::new("SEQ 1M Q1 Escrita", true, false, M, 1),
        TestSpec::new("RND 4K Q32 Leitura", false, true, K4, 32),
        TestSpec::new("RND 4K Q32 Escrita", true, true, K4, 32),
        TestSpec::new("RND 4K Q1 Leitura", false, true, K4, 1),
        TestSpec::new("RND 4K Q1 Escrita", true, true, K4, 1),
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchConfig {
    pub dir: PathBuf,
    pub file_size: u64,
    pub seconds: f32,
    pub tests: Vec<TestSpec>,
    /// Bytes para o teste de escrita sustentada (0 = desligado).
    pub sustained_bytes: u64,
    pub direct: bool,
}

impl BenchConfig {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        BenchConfig {
            dir: dir.into(),
            file_size: 1 << 30,
            seconds: 5.0,
            tests: default_tests(),
            sustained_bytes: 0,
            direct: true,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TestResult {
    pub name: String,
    pub mbps: f64,
    pub iops: f64,
    pub lat_avg_us: f64,
    pub lat_p50_us: u32,
    pub lat_p99_us: u32,
    pub lat_p999_us: u32,
    pub lat_max_us: u32,
    pub uncached: bool,
    pub ops: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SustainedResult {
    pub written_bytes: u64,
    pub burst_mbps: f64,
    pub sustained_mbps: f64,
    pub min_mbps: f64,
    pub cache_estimate_bytes: Option<u64>,
    /// (GB escritos, MB/s)
    pub curve: Vec<(f64, f64)>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BenchReport {
    pub dir: String,
    pub uncached: bool,
    pub results: Vec<TestResult>,
    pub sustained: Option<SustainedResult>,
    pub warnings: Vec<String>,
    pub cancelled: bool,
    /// (segundos desde o início, °C) — preenchido pela interface durante o teste.
    #[serde(default)]
    pub temperatures: Vec<(f64, f64)>,
}

#[derive(Debug, Clone)]
pub enum BenchEvent {
    Phase(String),
    Progress(f32),
    Sample { test: usize, t: f32, mbps: f64 },
    TestDone(usize, TestResult),
    SustainedSample { gb: f64, mbps: f64 },
    Warning(String),
    Done(BenchReport),
    Failed(String),
}

/// Espaço livre no volume que contém `dir`.
pub fn free_space(dir: &Path) -> Option<u64> {
    let dir = std::fs::canonicalize(dir).ok()?;
    let disks = sysinfo::Disks::new_with_refreshed_list().ok()?;
    disks
        .iter()
        .filter(|d| dir.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| d.available_space())
}

struct Cleanup(Vec<PathBuf>);
impl Drop for Cleanup {
    fn drop(&mut self) {
        for p in &self.0 {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Marca cada página de 4 KiB com um contador para impedir deduplicação/compressão.
#[inline]
fn stamp(buf: &mut [u8], v: u64) {
    for page in buf.chunks_exact_mut(4096) {
        page[..8].copy_from_slice(&v.to_le_bytes());
    }
}

pub fn run_bench(cfg: &BenchConfig, cancel: &AtomicBool, emit: &mut dyn FnMut(BenchEvent)) {
    let mut report = BenchReport { dir: cfg.dir.display().to_string(), uncached: true, ..Default::default() };
    if let Err(e) = std::fs::create_dir_all(&cfg.dir) {
        emit(BenchEvent::Failed(format!("Pasta inválida: {e}")));
        return;
    }
    let file_size = (cfg.file_size / (1 << 20)).max(16) << 20;
    let need = file_size + cfg.sustained_bytes + (256 << 20);
    if let Some(free) = free_space(&cfg.dir) {
        if free < need {
            emit(BenchEvent::Failed(format!(
                "Espaço livre insuficiente: {} disponíveis, {} necessários.",
                crate::util::human_bytes(free),
                crate::util::human_bytes(need)
            )));
            return;
        }
    }
    let path = cfg.dir.join(format!("discolab_bench_{}.tmp", std::process::id()));
    let spath = cfg.dir.join(format!("discolab_sustained_{}.tmp", std::process::id()));
    let _cleanup = Cleanup(vec![path.clone(), spath.clone()]);

    // ---- preparação: arquivo de teste com dados aleatórios
    emit(BenchEvent::Phase(format!("Preparando arquivo de teste ({})…", crate::util::human_bytes(file_size))));
    match prepare_file(&path, file_size, cfg.direct, cancel, emit) {
        Ok(unc) => {
            if !unc && cfg.direct {
                report.uncached = false;
                let w = "O sistema de arquivos não permite E/S sem cache aqui. Os resultados de leitura podem refletir a RAM. \
                         Prefira um volume nativo (ext4/APFS/NTFS) ou use a varredura de superfície."
                    .to_string();
                report.warnings.push(w.clone());
                emit(BenchEvent::Warning(w));
            }
        }
        Err(e) => {
            emit(BenchEvent::Failed(format!("Falha ao preparar arquivo: {e}")));
            return;
        }
    }

    let n = cfg.tests.len() + usize::from(cfg.sustained_bytes > 0);
    for (i, t) in cfg.tests.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
            break;
        }
        emit(BenchEvent::Phase(format!("{} ({}/{})", t.name, i + 1, cfg.tests.len())));
        // Pausa curta entre testes para o controlador esvaziar filas internas.
        std::thread::sleep(Duration::from_millis(500));
        match run_test(&path, file_size, t, cfg, i, cancel, emit) {
            Ok(r) => {
                emit(BenchEvent::TestDone(i, r.clone()));
                report.results.push(r);
            }
            Err(e) => {
                let w = format!("{}: {e}", t.name);
                emit(BenchEvent::Warning(w.clone()));
                report.warnings.push(w);
            }
        }
        emit(BenchEvent::Progress((i + 1) as f32 / n as f32));
    }
    let _ = std::fs::remove_file(&path);

    if cfg.sustained_bytes > 0 && !cancel.load(Ordering::Relaxed) {
        emit(BenchEvent::Phase(format!(
            "Escrita sustentada ({}) — detectando cache SLC…",
            crate::util::human_bytes(cfg.sustained_bytes)
        )));
        match sustained(&spath, cfg.sustained_bytes, cfg.direct, cancel, emit) {
            Ok(s) => {
                if s.curve.len() < 8 {
                    report.warnings.push(
                        "Escrita sustentada terminou rápido demais para medir o cache SLC. Use 8 GiB ou mais (≈ 2–3× o cache esperado).".into(),
                    );
                }
                report.sustained = Some(s)
            }
            Err(e) => report.warnings.push(format!("Escrita sustentada: {e}")),
        }
    }
    report.cancelled |= cancel.load(Ordering::Relaxed);
    emit(BenchEvent::Done(report));
}

fn prepare_file(path: &Path, size: u64, direct: bool, cancel: &AtomicBool, emit: &mut dyn FnMut(BenchEvent)) -> std::io::Result<bool> {
    let o = directio::open(path, true, true, direct)?;
    let chunk = 4usize << 20;
    let mut buf = AlignedBuf::new(chunk);
    let mut rng = Rng::seeded();
    rng.fill(&mut buf);
    let mut off = 0u64;
    let mut c = 0u64;
    while off < size {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        stamp(&mut buf, rng.next() ^ c);
        let n = (size - off).min(chunk as u64) as usize;
        directio::write_full(&o.file, &buf[..n], off)?;
        off += n as u64;
        c += 1;
        if c % 16 == 0 {
            emit(BenchEvent::Progress(0.0));
        }
    }
    o.file.sync_all()?;
    Ok(o.uncached)
}

fn percentile(sorted: &[u32], p: f64) -> u32 {
    if sorted.is_empty() {
        return 0;
    }
    let i = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[i.min(sorted.len() - 1)]
}

fn run_test(
    path: &Path,
    file_size: u64,
    t: &TestSpec,
    cfg: &BenchConfig,
    idx: usize,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(BenchEvent),
) -> std::io::Result<TestResult> {
    let stop = Arc::new(AtomicBool::new(false));
    let bytes = Arc::new(AtomicU64::new(0));
    let cursor = Arc::new(AtomicU64::new(0));
    let blocks = (file_size / t.block as u64).max(1);
    let mut handles = Vec::new();
    let mut uncached = true;
    for th in 0..t.threads.max(1) {
        let o = directio::open(path, t.write, false, cfg.direct)?;
        uncached &= o.uncached;
        let (stop, bytes, cursor) = (stop.clone(), bytes.clone(), cursor.clone());
        let (block, random, write) = (t.block, t.random, t.write);
        handles.push(std::thread::spawn(move || -> std::io::Result<Vec<u32>> {
            let f = o.file;
            let mut buf = AlignedBuf::new(block);
            let mut rng = Rng::new(0xD15C_0000 + th as u64 * 7919 + Rng::seeded().next());
            rng.fill(&mut buf);
            let mut lats = Vec::with_capacity(1 << 16);
            let mut n = 0u64;
            while !stop.load(Ordering::Relaxed) {
                let b = if random { rng.next() % blocks } else { cursor.fetch_add(1, Ordering::Relaxed) % blocks };
                let off = b * block as u64;
                let t0 = Instant::now();
                if write {
                    stamp(&mut buf, rng.next() ^ n);
                    directio::write_full(&f, &buf, off)?;
                } else {
                    directio::read_full(&f, &mut buf, off)?;
                }
                let us = t0.elapsed().as_micros().min(u32::MAX as u128) as u32;
                if lats.len() < 4_000_000 {
                    lats.push(us);
                }
                bytes.fetch_add(block as u64, Ordering::Relaxed);
                n += 1;
            }
            if write {
                f.sync_data()?;
            }
            Ok(lats)
        }));
    }
    let start = Instant::now();
    let dur = Duration::from_secs_f32(cfg.seconds.max(0.5));
    let mut last_b = 0u64;
    let mut last_t = Instant::now();
    while start.elapsed() < dur && !cancel.load(Ordering::Relaxed) && !handles.iter().all(|h| h.is_finished()) {
        std::thread::sleep(Duration::from_millis(250));
        let b = bytes.load(Ordering::Relaxed);
        let dt = last_t.elapsed().as_secs_f64();
        emit(BenchEvent::Sample { test: idx, t: start.elapsed().as_secs_f32(), mbps: (b - last_b) as f64 / dt / 1e6 });
        last_b = b;
        last_t = Instant::now();
    }
    stop.store(true, Ordering::Relaxed);
    let mut lats = Vec::new();
    let mut err = None;
    for h in handles {
        match h.join() {
            Ok(Ok(l)) => lats.extend(l),
            Ok(Err(e)) => err = Some(e),
            Err(_) => err = Some(std::io::Error::other("thread falhou")),
        }
    }
    let elapsed = start.elapsed().as_secs_f64();
    if let Some(e) = err {
        if lats.is_empty() {
            return Err(e);
        }
    }
    let total = bytes.load(Ordering::Relaxed);
    let ops = total / t.block as u64;
    lats.sort_unstable();
    let avg = if lats.is_empty() { 0.0 } else { lats.iter().map(|&x| x as f64).sum::<f64>() / lats.len() as f64 };
    Ok(TestResult {
        name: t.name.clone(),
        mbps: total as f64 / elapsed / 1e6,
        iops: ops as f64 / elapsed,
        lat_avg_us: avg,
        lat_p50_us: percentile(&lats, 0.50),
        lat_p99_us: percentile(&lats, 0.99),
        lat_p999_us: percentile(&lats, 0.999),
        lat_max_us: lats.last().copied().unwrap_or(0),
        uncached,
        ops,
    })
}

fn median(v: &[f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.total_cmp(b));
    s[s.len() / 2]
}

fn sustained(path: &Path, total: u64, direct: bool, cancel: &AtomicBool, emit: &mut dyn FnMut(BenchEvent)) -> std::io::Result<SustainedResult> {
    let block = 4usize << 20;
    let blocks = total / block as u64;
    let bytes = Arc::new(AtomicU64::new(0));
    let cursor = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let first = directio::open(path, true, true, direct)?;
    drop(first);
    let mut hs = Vec::new();
    for th in 0..4u64 {
        let o = directio::open(path, true, false, direct)?;
        let (bytes, cursor, stop) = (bytes.clone(), cursor.clone(), stop.clone());
        hs.push(std::thread::spawn(move || -> std::io::Result<()> {
            let mut buf = AlignedBuf::new(block);
            let mut rng = Rng::new(Rng::seeded().next() ^ th);
            rng.fill(&mut buf);
            loop {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let b = cursor.fetch_add(1, Ordering::Relaxed);
                if b >= blocks {
                    break;
                }
                stamp(&mut buf, rng.next());
                directio::write_full(&o.file, &buf, b * block as u64)?;
                bytes.fetch_add(block as u64, Ordering::Relaxed);
            }
            o.file.sync_data()
        }));
    }
    let mut curve = Vec::new();
    let mut last_b = 0;
    let mut last_t = Instant::now();
    while !hs.iter().all(|h| h.is_finished()) {
        if cancel.load(Ordering::Relaxed) {
            stop.store(true, Ordering::Relaxed);
        }
        std::thread::sleep(Duration::from_millis(250));
        let b = bytes.load(Ordering::Relaxed);
        let dt = last_t.elapsed().as_secs_f64();
        if b == last_b {
            continue;
        }
        let mbps = (b - last_b) as f64 / dt / 1e6;
        let gb = b as f64 / 1e9;
        curve.push((gb, mbps));
        emit(BenchEvent::SustainedSample { gb, mbps });
        emit(BenchEvent::Progress((b as f64 / total as f64) as f32));
        last_b = b;
        last_t = Instant::now();
    }
    for h in hs {
        h.join().map_err(|_| std::io::Error::other("thread falhou"))??;
    }
    let _ = std::fs::remove_file(path);
    Ok(analyze_sustained(curve, bytes.load(Ordering::Relaxed)))
}

pub fn analyze_sustained(curve: Vec<(f64, f64)>, written: u64) -> SustainedResult {
    let curve: Vec<(f64, f64)> = curve.into_iter().filter(|c| c.1 > 0.0 && c.1.is_finite()).collect();
    let speeds: Vec<f64> = curve.iter().map(|c| c.1).collect();
    if speeds.len() < 8 {
        // Poucas amostras: volume pequeno demais para enxergar o fim do cache.
        let avg = if speeds.is_empty() { 0.0 } else { speeds.iter().sum::<f64>() / speeds.len() as f64 };
        let min = speeds.iter().copied().fold(f64::MAX, f64::min);
        return SustainedResult {
            written_bytes: written,
            burst_mbps: speeds.iter().copied().fold(0.0, f64::max),
            sustained_mbps: avg,
            min_mbps: if min == f64::MAX { 0.0 } else { min },
            cache_estimate_bytes: None,
            curve,
        };
    }
    let burst = median(&speeds[..speeds.len().min(8)]);
    let tail = &speeds[speeds.len() / 2..];
    let sustained = median(tail);
    let min = speeds.iter().copied().filter(|x| *x > 0.0).fold(f64::MAX, f64::min);
    let mut cache = None;
    if burst > 0.0 && sustained < burst * 0.6 {
        for i in 2..speeds.len() {
            let w = &speeds[i.saturating_sub(2)..=i];
            if w.iter().sum::<f64>() / (w.len() as f64) < burst * 0.6 {
                cache = Some((curve[i.saturating_sub(2)].0 * 1e9) as u64);
                break;
            }
        }
    }
    SustainedResult {
        written_bytes: written,
        burst_mbps: burst,
        sustained_mbps: sustained,
        min_mbps: if min == f64::MAX { 0.0 } else { min },
        cache_estimate_bytes: cache,
        curve,
    }
}

// ---------------------------------------------------------------- leitura direta

/// Testes do benchmark de leitura direta (disco bruto, somente leitura, fila 1).
pub fn raw_tests() -> Vec<TestSpec> {
    vec![
        TestSpec::new("SEQ 1M Q1 Leitura", false, false, 1 << 20, 1),
        TestSpec::new("SEQ 128K Q1 Leitura", false, false, 128 << 10, 1),
        TestSpec::new("RND 64K Q1 Leitura", false, true, 64 << 10, 1),
        TestSpec::new("RND 4K Q1 Leitura", false, true, 4096, 1),
    ]
}

/// Benchmark somente leitura direto no dispositivo (sem sistema de arquivos).
/// Serve para discos USB acessados diretamente (Android sem root) e discos brutos.
pub fn run_raw_read(src: &mut dyn crate::surface::BlockRead, seconds: f32, cancel: &AtomicBool, emit: &mut dyn FnMut(BenchEvent)) {
    let size = src.size();
    let align = src.align().max(512);
    let mut report = BenchReport { dir: "leitura direta do dispositivo".into(), uncached: src.uncached(), ..Default::default() };
    if size < (64 << 20) {
        emit(BenchEvent::Failed("Dispositivo pequeno demais para o teste.".into()));
        return;
    }
    let tests = raw_tests();
    let mut rng = Rng::seeded();
    for (i, t) in tests.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
            break;
        }
        emit(BenchEvent::Phase(format!("{} ({}/{})", t.name, i + 1, tests.len())));
        let block = t.block.div_ceil(align) * align;
        let blocks = size / block as u64;
        let mut buf = AlignedBuf::new(block);
        let mut lats: Vec<u32> = Vec::new();
        let start = Instant::now();
        let dur = Duration::from_secs_f32(seconds.max(1.0));
        let mut bytes = 0u64;
        let mut last_b = 0u64;
        let mut last_t = Instant::now();
        // sequencial começa no meio do disco (região típica de dados)
        let mut cursor = blocks / 4;
        let mut err = None;
        while start.elapsed() < dur && !cancel.load(Ordering::Relaxed) {
            let b = if t.random { rng.next() % blocks } else { cursor };
            cursor = (cursor + 1) % blocks;
            let t0 = Instant::now();
            if let Err(e) = src.read_at(&mut buf, b * block as u64) {
                err = Some(e);
                break;
            }
            lats.push(t0.elapsed().as_micros().min(u32::MAX as u128) as u32);
            bytes += block as u64;
            if last_t.elapsed() >= Duration::from_millis(250) {
                let dt = last_t.elapsed().as_secs_f64();
                emit(BenchEvent::Sample { test: i, t: start.elapsed().as_secs_f32(), mbps: (bytes - last_b) as f64 / dt / 1e6 });
                last_b = bytes;
                last_t = Instant::now();
            }
        }
        if let Some(e) = err {
            let w = format!("{}: erro de leitura ({e})", t.name);
            emit(BenchEvent::Warning(w.clone()));
            report.warnings.push(w);
            continue;
        }
        let elapsed = start.elapsed().as_secs_f64();
        lats.sort_unstable();
        let ops = lats.len() as u64;
        let avg = if lats.is_empty() { 0.0 } else { lats.iter().map(|&x| x as f64).sum::<f64>() / lats.len() as f64 };
        let r = TestResult {
            name: t.name.clone(),
            mbps: bytes as f64 / elapsed / 1e6,
            iops: ops as f64 / elapsed,
            lat_avg_us: avg,
            lat_p50_us: percentile(&lats, 0.5),
            lat_p99_us: percentile(&lats, 0.99),
            lat_p999_us: percentile(&lats, 0.999),
            lat_max_us: lats.last().copied().unwrap_or(0),
            uncached: report.uncached,
            ops,
        };
        emit(BenchEvent::TestDone(i, r.clone()));
        report.results.push(r);
        emit(BenchEvent::Progress((i + 1) as f32 / tests.len() as f32));
    }
    report.cancelled |= cancel.load(Ordering::Relaxed);
    emit(BenchEvent::Done(report));
}

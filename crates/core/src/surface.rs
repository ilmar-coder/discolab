//! Varredura de superfície SOMENTE LEITURA (estilo Victoria/HD Tune "Error Scan"),
//! com releitura de blocos suspeitos para separar defeitos reais de lentidão transitória.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::directio::{self, AlignedBuf};

/// Limites das faixas de latência (ms), por bloco.
pub const BUCKET_MS: [u32; 5] = [5, 20, 50, 200, 600];
pub const BUCKET_LABELS: [&str; 7] = ["<5 ms", "<20 ms", "<50 ms", "<200 ms", "<600 ms", "≥600 ms", "Erro"];
pub const MAP_CELLS: usize = 4096;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanConfig {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub block_size: usize,
    pub start_pct: f32,
    pub end_pct: f32,
    /// Releituras para blocos lentos/erro.
    pub retries: u32,
    /// Latência (ms) a partir da qual o bloco é relido.
    pub recheck_ms: u32,
    /// Flash (SSD): limites mais apertados no veredito.
    pub flash: bool,
}

impl ScanConfig {
    pub fn new(path: impl Into<PathBuf>, size_bytes: u64, flash: bool) -> Self {
        ScanConfig {
            path: path.into(),
            size_bytes,
            block_size: 128 * 1024,
            start_pct: 0.0,
            end_pct: 100.0,
            retries: 3,
            recheck_ms: if flash { 50 } else { 200 },
            flash,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ScanEvent {
    Started { total_blocks: u64, uncached: bool, size: u64 },
    Progress { done_blocks: u64, mbps: f64, position_pct: f32, buckets: [u64; 7] },
    /// Célula do mapa atualizada (índice, faixa 0..6).
    Cell { cell: usize, bucket: u8 },
    Rechecking { count: usize },
    Log(String),
    Done(ScanSummary),
    Failed(String),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScanSummary {
    pub buckets: [u64; 7],
    pub bad_blocks: u64,
    pub slow_persistent: u64,
    pub slow_transient: u64,
    pub errors: Vec<(u64, String)>,
    pub slow_offsets: Vec<(u64, u32)>,
    pub avg_mbps: f64,
    pub min_mbps: f64,
    pub max_mbps: f64,
    /// (posição %, MB/s) — gráfico de transferência ao longo do disco.
    pub speed_curve: Vec<(f32, f64)>,
    pub elapsed_s: f64,
    pub scanned_bytes: u64,
    pub cancelled: bool,
    pub uncached: bool,
    #[serde(default)]
    pub block_size: u64,
}

pub fn bucket_of(ms: f64) -> u8 {
    for (i, &b) in BUCKET_MS.iter().enumerate() {
        if ms < b as f64 {
            return i as u8;
        }
    }
    5
}

/// Fonte de leitura em blocos: dispositivo/arquivo do SO ou disco USB direto.
pub trait BlockRead: Send {
    fn size(&self) -> u64;
    fn uncached(&self) -> bool;
    fn read_at(&mut self, buf: &mut [u8], off: u64) -> std::io::Result<()>;
    fn align(&self) -> usize {
        4096
    }
}

/// Leitura + escrita (usado só pelo reparo de setores).
pub trait BlockRw: BlockRead {
    fn write_at(&mut self, buf: &[u8], off: u64) -> std::io::Result<()>;
    fn flush(&mut self) -> std::io::Result<()>;
}

struct FileDev {
    f: std::fs::File,
    size: u64,
    uncached: bool,
}

impl BlockRw for FileDev {
    fn write_at(&mut self, buf: &[u8], off: u64) -> std::io::Result<()> {
        directio::write_full(&self.f, buf, off)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.f.sync_all()
    }
}

/// Abre o dispositivo para leitura e escrita (reparo). Exige privilégios e, em geral,
/// que o disco não esteja montado.
pub fn open_source_rw(path: &std::path::Path, size_hint: u64) -> Result<Box<dyn BlockRw>, String> {
    let p = path.to_string_lossy();
    if crate::usb::is_usb(&p) {
        return crate::usb::open_block_rw(&p).map_err(|e| format!("Não foi possível abrir o dispositivo USB: {e}"));
    }
    let o = directio::open(path, true, false, true).map_err(|e| format!("Não foi possível abrir {} para escrita: {e}", path.display()))?;
    let size = directio::device_size(&o.file).filter(|s| *s > 0).unwrap_or(size_hint);
    Ok(Box::new(FileDev { f: o.file, size, uncached: o.uncached }))
}

impl BlockRead for FileDev {
    fn size(&self) -> u64 {
        self.size
    }
    fn uncached(&self) -> bool {
        self.uncached
    }
    fn read_at(&mut self, buf: &mut [u8], off: u64) -> std::io::Result<()> {
        directio::read_full(&self.f, buf, off)
    }
}

pub fn open_source(cfg: &ScanConfig) -> Result<Box<dyn BlockRead>, String> {
    let p = cfg.path.to_string_lossy();
    if crate::usb::is_usb(&p) {
        return crate::usb::open_block(&p).map_err(|e| format!("Não foi possível abrir o dispositivo USB: {e}"));
    }
    let opened = directio::open(&cfg.path, false, false, true).map_err(|e| {
        let hint = if e.kind() == std::io::ErrorKind::PermissionDenied { " — execute como Administrador/root (sudo)." } else { "" };
        format!("Não foi possível abrir {}: {e}{hint}", cfg.path.display())
    })?;
    let size = directio::device_size(&opened.file).filter(|s| *s > 0).unwrap_or(cfg.size_bytes);
    Ok(Box::new(FileDev { f: opened.file, size, uncached: opened.uncached }))
}

pub fn run_scan(cfg: &ScanConfig, cancel: &AtomicBool, emit: &mut dyn FnMut(ScanEvent)) {
    match open_source(cfg) {
        Ok(mut src) => scan_source(src.as_mut(), cfg, cancel, emit),
        Err(e) => emit(ScanEvent::Failed(e)),
    }
}

pub fn scan_source(src: &mut dyn BlockRead, cfg: &ScanConfig, cancel: &AtomicBool, emit: &mut dyn FnMut(ScanEvent)) {
    let size = src.size();
    let uncached = src.uncached();
    if size == 0 {
        emit(ScanEvent::Failed("Tamanho do dispositivo desconhecido.".into()));
        return;
    }
    let align = src.align().max(512) as u64;
    let bs = (cfg.block_size.max(4096) as u64).div_ceil(align) * align;
    let total_blocks = size / bs;
    let first = ((cfg.start_pct.clamp(0.0, 100.0) as f64 / 100.0) * total_blocks as f64) as u64;
    let last = ((cfg.end_pct.clamp(0.0, 100.0) as f64 / 100.0) * total_blocks as f64) as u64;
    let last = last.max(first + 1).min(total_blocks);
    emit(ScanEvent::Started { total_blocks: last - first, uncached, size });
    if !uncached {
        emit(ScanEvent::Log("Aviso: cache do SO não pôde ser desligado; latências podem ficar otimistas.".into()));
    }

    let mut buf = AlignedBuf::new(bs as usize);
    let mut sum = ScanSummary { uncached, block_size: bs, min_mbps: f64::MAX, ..Default::default() };
    let mut cell_worst = vec![u8::MAX; MAP_CELLS];
    let blocks_per_cell = ((last - first) as f64 / MAP_CELLS as f64).max(1.0);
    let mut suspects: Vec<(u64, u8, String)> = Vec::new();
    let t0 = Instant::now();
    let mut last_emit = Instant::now();
    let mut win_bytes = 0u64;
    let mut win_t = Instant::now();
    let mut done = 0u64;

    for blk in first..last {
        if cancel.load(Ordering::Relaxed) {
            sum.cancelled = true;
            break;
        }
        let off = blk * bs;
        let t = Instant::now();
        let r = src.read_at(&mut buf, off);
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let b = match &r {
            Ok(_) => bucket_of(ms),
            Err(_) => 6,
        };
        sum.buckets[b as usize] += 1;
        if b == 6 || ms >= cfg.recheck_ms as f64 {
            let why = match r {
                Err(e) => e.to_string(),
                Ok(_) => format!("{ms:.0} ms"),
            };
            if suspects.len() < 100_000 {
                suspects.push((blk, b, why));
            }
        }
        sum.scanned_bytes += bs;
        win_bytes += bs;
        done += 1;
        let cell = (((blk - first) as f64 / blocks_per_cell) as usize).min(MAP_CELLS - 1);
        if cell_worst[cell] == u8::MAX || b > cell_worst[cell] {
            cell_worst[cell] = b;
            emit(ScanEvent::Cell { cell, bucket: b });
        }
        if win_t.elapsed() >= Duration::from_millis(400) || blk + 1 == last {
            let dt = win_t.elapsed().as_secs_f64().max(1e-6);
            let mbps = win_bytes as f64 / dt / 1e6;
            let pos = blk as f32 / total_blocks as f32 * 100.0;
            sum.speed_curve.push((pos, mbps));
            if dt > 0.2 {
                sum.min_mbps = sum.min_mbps.min(mbps);
            }
            sum.max_mbps = sum.max_mbps.max(mbps);
            win_bytes = 0;
            win_t = Instant::now();
            if last_emit.elapsed() >= Duration::from_millis(100) {
                emit(ScanEvent::Progress { done_blocks: done, mbps, position_pct: pos, buckets: sum.buckets });
                last_emit = Instant::now();
            }
        }
    }

    // Releitura dos suspeitos: separa defeito real de lentidão passageira.
    if !suspects.is_empty() && !sum.cancelled {
        emit(ScanEvent::Rechecking { count: suspects.len() });
        for (blk, b, why) in &suspects {
            if cancel.load(Ordering::Relaxed) {
                sum.cancelled = true;
                break;
            }
            let off = blk * bs;
            let mut ok_fast = 0;
            let mut worst_ms = 0.0f64;
            let mut last_err = None;
            for _ in 0..cfg.retries.max(1) {
                let t = Instant::now();
                match src.read_at(&mut buf, off) {
                    Ok(_) => {
                        let ms = t.elapsed().as_secs_f64() * 1000.0;
                        worst_ms = worst_ms.max(ms);
                        if ms < cfg.recheck_ms as f64 {
                            ok_fast += 1;
                        }
                    }
                    Err(e) => last_err = Some(e.to_string()),
                }
            }
            if last_err.is_some() && ok_fast == 0 && worst_ms == 0.0 {
                sum.bad_blocks += 1;
                sum.errors.push((off, last_err.unwrap_or_else(|| why.clone())));
                emit(ScanEvent::Log(format!("Bloco ilegível em {} (LBA {})", off, off / 512)));
            } else if ok_fast as u32 >= cfg.retries.max(1).div_ceil(2) {
                sum.slow_transient += 1;
            } else if *b == 6 {
                // Erro inicial que depois leu, mas lento: setor fraco.
                sum.slow_persistent += 1;
                sum.slow_offsets.push((off, worst_ms as u32));
            } else {
                sum.slow_persistent += 1;
                sum.slow_offsets.push((off, worst_ms as u32));
            }
        }
    }
    sum.elapsed_s = t0.elapsed().as_secs_f64();
    sum.avg_mbps = sum.scanned_bytes as f64 / sum.elapsed_s.max(1e-6) / 1e6;
    if sum.min_mbps == f64::MAX {
        sum.min_mbps = 0.0;
    }
    emit(ScanEvent::Done(sum));
}

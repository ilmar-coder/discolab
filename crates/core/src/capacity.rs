//! Teste de capacidade real (estilo H2testw/F3): preenche o espaço livre com dados
//! verificáveis e relê tudo sem cache. Pendrives, cartões e SSDs falsificados informam
//! um tamanho maior que o real e "embaralham" os dados além da capacidade verdadeira.
//! Não apaga nada: só usa o espaço livre e remove os arquivos ao final (opcional).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::directio::{self, AlignedBuf};

pub const PAGE: usize = 4096;
const CHUNK: usize = 1 << 20;
const PREFIX: &str = "DiscoLab_cap_";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapConfig {
    pub dir: PathBuf,
    /// Quantidade a testar; None = todo o espaço livre (menos uma margem).
    pub bytes: Option<u64>,
    pub file_size: u64,
    /// Manter os arquivos para verificar depois de reconectar o dispositivo.
    pub keep_files: bool,
    /// Só verificar arquivos gravados antes.
    pub verify_only: bool,
}

impl CapConfig {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        CapConfig { dir: dir.into(), bytes: None, file_size: 1 << 30, keep_files: false, verify_only: false }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapPhase {
    Write,
    Verify,
}

#[derive(Debug, Clone)]
pub enum CapEvent {
    Phase(String),
    Progress { phase: CapPhase, done: u64, total: u64, mbps: f64 },
    Done(CapResult),
    Failed(String),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CapResult {
    pub dir: String,
    pub written: u64,
    pub verified: u64,
    pub ok_bytes: u64,
    pub corrupted_bytes: u64,
    pub unreadable_bytes: u64,
    pub first_bad: Option<u64>,
    pub write_mbps: f64,
    pub read_mbps: f64,
    pub uncached: bool,
    pub cancelled: bool,
    pub write_error: Option<String>,
}

impl CapResult {
    pub fn is_fake(&self) -> bool {
        self.corrupted_bytes + self.unreadable_bytes > 0
    }
    pub fn verdict(&self) -> String {
        if self.cancelled {
            return "Teste interrompido.".into();
        }
        if self.verified == 0 {
            return "Nada foi verificado.".into();
        }
        if !self.is_fake() {
            return format!("Capacidade confirmada: {} gravados e lidos sem nenhum erro.", crate::util::human_bytes(self.verified));
        }
        format!(
            "FALHA: {} corrompidos e {} ilegíveis. A capacidade utilizável real é de aproximadamente {}. \
             Dispositivo falsificado ou com defeito.",
            crate::util::human_bytes(self.corrupted_bytes),
            crate::util::human_bytes(self.unreadable_bytes),
            crate::util::human_bytes(self.first_bad.unwrap_or(0))
        )
    }
}

#[inline]
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Preenche `buf` com o padrão da posição absoluta `pos` (cada palavra depende do endereço).
pub fn fill_pattern(buf: &mut [u8], pos: u64) {
    for (i, w) in buf.chunks_exact_mut(8).enumerate() {
        let p = pos + (i * 8) as u64;
        w.copy_from_slice(&mix(p ^ 0xD15C_0CA9).to_le_bytes());
    }
}

/// Número de páginas de 4 KiB que não batem com o padrão esperado.
pub fn bad_pages(buf: &[u8], pos: u64, scratch: &mut [u8]) -> Vec<usize> {
    fill_pattern(&mut scratch[..buf.len()], pos);
    buf.chunks(PAGE).zip(scratch[..buf.len()].chunks(PAGE)).enumerate().filter(|(_, (a, b))| a != b).map(|(i, _)| i).collect()
}

fn file_path(dir: &Path, i: usize) -> PathBuf {
    dir.join(format!("{PREFIX}{i:05}.bin"))
}

fn existing_files(dir: &Path) -> Vec<(usize, PathBuf, u64)> {
    let mut v: Vec<(usize, PathBuf, u64)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let idx = name.strip_prefix(PREFIX)?.strip_suffix(".bin")?.parse().ok()?;
            Some((idx, e.path(), e.metadata().ok()?.len()))
        })
        .collect();
    v.sort_by_key(|x| x.0);
    v
}

pub fn run_capacity(cfg: &CapConfig, cancel: &AtomicBool, emit: &mut dyn FnMut(CapEvent)) {
    let mut r = CapResult { dir: cfg.dir.display().to_string(), uncached: true, ..Default::default() };
    if let Err(e) = std::fs::create_dir_all(&cfg.dir) {
        emit(CapEvent::Failed(format!("Pasta inválida: {e}")));
        return;
    }
    let file_size = (cfg.file_size / CHUNK as u64).max(1) * CHUNK as u64;
    let mut buf = AlignedBuf::new(CHUNK);
    let mut scratch = vec![0u8; CHUNK];

    // ------------------------------------------------ gravação
    let files: Vec<(usize, PathBuf, u64)> = if cfg.verify_only {
        existing_files(&cfg.dir)
    } else {
        for (_, p, _) in existing_files(&cfg.dir) {
            let _ = std::fs::remove_file(p);
        }
        let free = crate::bench::free_space(&cfg.dir).unwrap_or(0);
        let margin = 64u64 << 20;
        let total = cfg.bytes.unwrap_or(free.saturating_sub(margin)).min(free.saturating_sub(margin)) / CHUNK as u64 * CHUNK as u64;
        if total == 0 {
            emit(CapEvent::Failed("Sem espaço livre para testar.".into()));
            return;
        }
        emit(CapEvent::Phase(format!("Gravando {} de dados verificáveis…", crate::util::human_bytes(total))));
        let t0 = Instant::now();
        let mut done = 0u64;
        let mut last = Instant::now();
        let mut out = Vec::new();
        let mut idx = 0usize;
        'files: while done < total {
            let size = (total - done).min(file_size);
            let path = file_path(&cfg.dir, idx);
            let o = match directio::open(&path, true, true, true) {
                Ok(o) => o,
                Err(e) => {
                    r.write_error = Some(e.to_string());
                    break;
                }
            };
            r.uncached &= o.uncached;
            let mut off = 0u64;
            while off < size {
                if cancel.load(Ordering::Relaxed) {
                    r.cancelled = true;
                    out.push((idx, path.clone(), off));
                    break 'files;
                }
                fill_pattern(&mut buf, done);
                if let Err(e) = directio::write_full(&o.file, &buf, off) {
                    // Dispositivo cheio ou com erro de escrita: verifica o que foi gravado.
                    r.write_error = Some(e.to_string());
                    out.push((idx, path.clone(), off));
                    break 'files;
                }
                off += CHUNK as u64;
                done += CHUNK as u64;
                if last.elapsed() > Duration::from_millis(250) {
                    let mbps = done as f64 / t0.elapsed().as_secs_f64() / 1e6;
                    emit(CapEvent::Progress { phase: CapPhase::Write, done, total, mbps });
                    last = Instant::now();
                }
            }
            if let Err(e) = o.file.sync_all() {
                r.write_error = Some(e.to_string());
            }
            out.push((idx, path, size));
            idx += 1;
        }
        r.written = done;
        r.write_mbps = done as f64 / t0.elapsed().as_secs_f64().max(1e-6) / 1e6;
        out
    };

    // ------------------------------------------------ verificação
    let total: u64 = files.iter().map(|f| f.2 / CHUNK as u64 * CHUNK as u64).sum();
    if total == 0 {
        emit(CapEvent::Failed("Nenhum arquivo de teste para verificar.".into()));
        return;
    }
    emit(CapEvent::Phase(format!("Verificando {} sem cache…", crate::util::human_bytes(total))));
    let t0 = Instant::now();
    let mut last = Instant::now();
    let mut pos = 0u64;
    'verify: for (i, path, len) in &files {
        let start = *i as u64 * file_size;
        let o = match directio::open(path, false, false, true) {
            Ok(o) => o,
            Err(_) => {
                r.unreadable_bytes += len;
                r.first_bad.get_or_insert(start);
                pos += len;
                continue;
            }
        };
        r.uncached &= o.uncached;
        let mut off = 0u64;
        while off + CHUNK as u64 <= *len {
            if cancel.load(Ordering::Relaxed) {
                r.cancelled = true;
                break 'verify;
            }
            let abs = start + off;
            match directio::read_full(&o.file, &mut buf, off) {
                Ok(()) => {
                    let bad = bad_pages(&buf, abs, &mut scratch);
                    let n = bad.len() as u64 * PAGE as u64;
                    r.corrupted_bytes += n;
                    r.ok_bytes += CHUNK as u64 - n;
                    if let Some(p) = bad.first() {
                        r.first_bad.get_or_insert(abs + (*p * PAGE) as u64);
                    }
                }
                Err(_) => {
                    r.unreadable_bytes += CHUNK as u64;
                    r.first_bad.get_or_insert(abs);
                }
            }
            off += CHUNK as u64;
            pos += CHUNK as u64;
            r.verified += CHUNK as u64;
            if last.elapsed() > Duration::from_millis(250) {
                let mbps = pos as f64 / t0.elapsed().as_secs_f64() / 1e6;
                emit(CapEvent::Progress { phase: CapPhase::Verify, done: pos, total, mbps });
                last = Instant::now();
            }
        }
    }
    r.read_mbps = r.verified as f64 / t0.elapsed().as_secs_f64().max(1e-6) / 1e6;
    if !cfg.keep_files {
        for (_, p, _) in &files {
            let _ = std::fs::remove_file(p);
        }
    }
    emit(CapEvent::Done(r));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_detects_wraparound() {
        let mut a = vec![0u8; PAGE * 4];
        fill_pattern(&mut a, 0);
        let mut s = vec![0u8; a.len()];
        assert!(bad_pages(&a, 0, &mut s).is_empty());
        // um dispositivo falso que devolve os dados do início para um endereço alto
        assert_eq!(bad_pages(&a, 8 << 30, &mut s).len(), 4);
    }

    #[test]
    fn fake_drive_is_detected() {
        let dir = std::env::temp_dir().join(format!("discolab_fake_{}", std::process::id()));
        let mut cfg = CapConfig::new(&dir);
        cfg.bytes = Some(6 << 20);
        cfg.file_size = 2 << 20;
        cfg.keep_files = true;
        run_capacity(&cfg, &AtomicBool::new(false), &mut |_| {});
        // "dispositivo falso": a partir de 3 MiB os dados voltam do início (wraparound)
        let f = file_path(&dir, 1);
        let mut d = std::fs::read(&f).unwrap();
        let head = std::fs::read(file_path(&dir, 0)).unwrap();
        d[1 << 20..].copy_from_slice(&head[..1 << 20]);
        std::fs::write(&f, d).unwrap();
        cfg.verify_only = true;
        cfg.keep_files = false;
        let mut res = None;
        run_capacity(&cfg, &AtomicBool::new(false), &mut |e| {
            if let CapEvent::Done(r) = e {
                res = Some(r)
            }
        });
        let r = res.unwrap();
        assert!(r.is_fake());
        assert_eq!(r.first_bad, Some(3 << 20));
        assert_eq!(r.corrupted_bytes, 1 << 20);
        assert!(r.verdict().contains("FALHA"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn full_run_on_temp_dir() {
        let dir = std::env::temp_dir().join(format!("discolab_cap_{}", std::process::id()));
        let mut cfg = CapConfig::new(&dir);
        cfg.bytes = Some(8 << 20);
        cfg.file_size = 3 << 20;
        let mut res = None;
        run_capacity(&cfg, &AtomicBool::new(false), &mut |e| {
            if let CapEvent::Done(r) = e {
                res = Some(r)
            }
        });
        let r = res.expect("terminou");
        assert_eq!(r.written, 8 << 20);
        assert_eq!(r.verified, 8 << 20);
        assert!(!r.is_fake(), "{r:?}");
        assert!(existing_files(&dir).is_empty(), "arquivos removidos");
        let _ = std::fs::remove_dir_all(dir);
    }
}

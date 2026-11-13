//! Relatório em texto (para compartilhar) e JSON (para análise).

use std::fmt::Write;
use std::path::PathBuf;

use crate::bench::BenchReport;
use crate::device::Device;
use crate::health::Assessment;
use crate::smart::SmartReport;
use crate::surface::{BUCKET_LABELS, ScanSummary};
use crate::util::human_bytes;

pub fn text_report(
    dev: &Device,
    smart: Option<&SmartReport>,
    health: Option<&Assessment>,
    bench: Option<&BenchReport>,
    scan: Option<&ScanSummary>,
) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "DiscoLab {} — relatório de disco", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(s, "==============================================");
    let _ = writeln!(s, "Dispositivo : {} ({})", dev.model, dev.name);
    let _ = writeln!(s, "Tipo        : {} · {}", dev.kind.label(), dev.bus);
    let _ = writeln!(s, "Capacidade  : {}", human_bytes(dev.size_bytes));
    if let Some(sm) = smart {
        if let Some(x) = &sm.serial {
            let _ = writeln!(s, "Serial      : {x}");
        }
        if let Some(x) = &sm.firmware {
            let _ = writeln!(s, "Firmware    : {x}");
        }
        if let Some(x) = sm.power_on_hours {
            let _ = writeln!(s, "Horas ligado: {x} h ({:.1} anos)", x as f64 / 8760.0);
        }
        if let Some(x) = sm.temperature {
            let _ = writeln!(s, "Temperatura : {x} °C");
        }
        if let Some(x) = sm.host_written_bytes {
            let _ = writeln!(s, "Total escrito: {}", human_bytes(x));
            let cap = sm.capacity.unwrap_or(dev.size_bytes);
            if let Some(tbw) = crate::endurance::rated_tbw(sm.model.as_deref().unwrap_or(&dev.model), cap) {
                let left = crate::endurance::life_left_by_tbw(x, tbw).unwrap_or(0);
                let _ = writeln!(s, "Resistência  : {tbw} TBW (garantia) — {left}% restante pelo volume escrito");
            }
        }
        let _ = writeln!(s, "Fonte SMART : {}", sm.source.label());
    }
    if let Some(h) = health {
        let _ = writeln!(s, "\nSAÚDE: {}% — {}", h.health, h.status_label());
        if let Some(l) = h.life_left {
            let _ = writeln!(s, "Vida útil restante: {l}%");
        }
        let _ = writeln!(s, "Confiança: {}", h.confidence);
        let _ = writeln!(s, "{}", h.verdict);
        for f in &h.findings {
            let _ = writeln!(s, "  [{}] {} — {}", f.severity.label(), f.title, f.detail);
        }
    }
    if let Some(sm) = smart {
        if !sm.ata.is_empty() {
            let _ = writeln!(s, "\nATRIBUTOS SMART");
            let _ = writeln!(s, " ID  Nome                         Atual Pior Limite  Bruto");
            for a in &sm.ata {
                let _ = writeln!(s, "{:>3}  {:<28} {:>5} {:>4} {:>6}  {}", a.id, a.name, a.value, a.worst, a.thresh, a.raw_str);
            }
        }
        if let Some(n) = &sm.nvme {
            let _ = writeln!(s, "\nNVMe SMART/Health");
            let _ = writeln!(s, "  Aviso crítico: 0x{:02X}  Reserva: {}% (limite {}%)  Uso: {}%", n.critical_warning, n.available_spare, n.available_spare_threshold, n.percentage_used);
            let _ = writeln!(s, "  Erros de mídia: {}  Log de erros: {}  Desligamentos inseguros: {}", n.media_errors, n.num_err_log_entries, n.unsafe_shutdowns);
        }
    }
    if let Some(b) = bench {
        let _ = writeln!(s, "\nBENCHMARK ({}){}", b.dir, if b.uncached { " — sem cache do SO" } else { " — COM cache (resultado não confiável)" });
        let _ = writeln!(s, "  {:<22} {:>10} {:>10} {:>9} {:>9} {:>9}", "Teste", "MB/s", "IOPS", "p50 µs", "p99 µs", "máx µs");
        for r in &b.results {
            let _ = writeln!(s, "  {:<22} {:>10.1} {:>10.0} {:>9} {:>9} {:>9}", r.name, r.mbps, r.iops, r.lat_p50_us, r.lat_p99_us, r.lat_max_us);
        }
        if let Some(st) = &b.sustained {
            let _ = writeln!(s, "  Escrita sustentada: pico {:.0} MB/s · sustentado {:.0} MB/s · mínimo {:.0} MB/s", st.burst_mbps, st.sustained_mbps, st.min_mbps);
            match st.cache_estimate_bytes {
                Some(c) => {
                    let _ = writeln!(s, "  Cache de escrita estimado: ~{}", human_bytes(c));
                }
                None => {
                    let _ = writeln!(s, "  Nenhuma queda de cache detectada em {}", human_bytes(st.written_bytes));
                }
            }
        }
        if !b.temperatures.is_empty() {
            let max = b.temperatures.iter().map(|t| t.1).fold(f64::MIN, f64::max);
            let min = b.temperatures.iter().map(|t| t.1).fold(f64::MAX, f64::min);
            let _ = writeln!(s, "  Temperatura durante o teste: {min:.0} a {max:.0} °C");
        }
        for w in &b.warnings {
            let _ = writeln!(s, "  Aviso: {w}");
        }
    }
    if let Some(sc) = scan {
        let _ = writeln!(s, "\nVARREDURA DE SUPERFÍCIE (somente leitura){}", if sc.cancelled { " — interrompida" } else { "" });
        for (i, l) in BUCKET_LABELS.iter().enumerate() {
            let _ = writeln!(s, "  {:<9} {}", l, sc.buckets[i]);
        }
        let _ = writeln!(s, "  Ilegíveis: {}  Lentos persistentes: {}  Lentos transitórios: {}", sc.bad_blocks, sc.slow_persistent, sc.slow_transient);
        let _ = writeln!(s, "  Velocidade: média {:.0} MB/s · mín {:.0} · máx {:.0} · {} em {:.0} s", sc.avg_mbps, sc.min_mbps, sc.max_mbps, human_bytes(sc.scanned_bytes), sc.elapsed_s);
    }
    s
}

/// Pasta padrão de relatórios.
pub fn reports_dir() -> PathBuf {
    if let Some(d) = REPORT_DIR.get() {
        return d.clone();
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from);
    let base = home.map(|h| if h.join("Documents").is_dir() { h.join("Documents") } else { h }).unwrap_or_else(std::env::temp_dir);
    base.join("DiscoLab")
}

/// Permite ao app Android definir a pasta interna.
pub static REPORT_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

pub fn save_report(dev: &Device, text: &str, json: &serde_json::Value) -> std::io::Result<PathBuf> {
    let dir = reports_dir();
    std::fs::create_dir_all(&dir)?;
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let base: String = dev.model.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    let p = dir.join(format!("{base}_{ts}.txt"));
    std::fs::write(&p, text)?;
    std::fs::write(p.with_extension("json"), serde_json::to_string_pretty(json).unwrap_or_default())?;
    Ok(p)
}

/// Data/hora local formatada (para relatórios).
pub fn now_string() -> String {
    chrono::Local::now().format("%d/%m/%Y %H:%M").to_string()
}

fn safe_name(dev: &Device) -> String {
    let base: String = dev.model.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    format!("{base}_{}", chrono::Local::now().format("%Y%m%d_%H%M"))
}

/// Salva o relatório HTML e abre no navegador / visualizador do sistema.
/// Retorna a descrição de onde ficou salvo.
pub fn save_and_open_html(dev: &Device, html: &str) -> Result<String, String> {
    let name = format!("{}.html", safe_name(dev));
    #[cfg(target_os = "android")]
    {
        return crate::usb::android::save_to_downloads_and_open(&name, "text/html", html.as_bytes());
    }
    #[cfg(not(target_os = "android"))]
    {
        let dir = reports_dir();
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let p = dir.join(name);
        std::fs::write(&p, html).map_err(|e| e.to_string())?;
        open_with_system(&p);
        Ok(p.display().to_string())
    }
}

/// Abre um endereço (https://…) no navegador do sistema.
pub fn open_url(url: &str) {
    #[cfg(target_os = "android")]
    crate::usb::android::open_url(url);
    #[cfg(not(target_os = "android"))]
    open_with_system(std::path::Path::new(url));
}

#[allow(dead_code)]
fn open_with_system(p: &std::path::Path) {
    let mut c = if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg(p);
        c
    } else if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]).arg(p);
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(p);
        c
    };
    crate::util::hide_window(&mut c);
    let _ = c.spawn();
}

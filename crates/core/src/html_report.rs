//! Relatório em HTML autocontido (CSS e gráficos SVG embutidos), pronto para
//! entregar ao cliente ou "Imprimir → Salvar como PDF" no navegador.

use std::fmt::Write;

use crate::bench::BenchReport;
use crate::capacity::CapResult;
use crate::device::Device;
use crate::health::{Assessment, Severity};
use crate::smart::SmartReport;
use crate::surface::{BUCKET_LABELS, ScanSummary};
use crate::util::human_bytes;

#[derive(Debug, Clone, Default)]
pub struct ReportInfo {
    pub client: String,
    pub technician: String,
    pub notes: String,
    pub tbw: Option<u64>,
    pub date: String,
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn sev_color(s: Severity) -> &'static str {
    match s {
        Severity::Critical => "#d03030",
        Severity::Warning => "#d46810",
        Severity::Notice => "#b08000",
        Severity::Ok => "#0e9662",
        Severity::Info => "#5f687a",
    }
}

fn health_color(a: &Assessment) -> &'static str {
    match a.status {
        Severity::Critical => "#d03030",
        Severity::Warning => "#d46810",
        _ if a.health >= 90 => "#0e9662",
        _ => "#b08000",
    }
}

fn gauge_svg(a: &Assessment) -> String {
    let r = 52.0f64;
    let c = 2.0 * std::f64::consts::PI * r;
    let frac = a.health as f64 / 100.0;
    format!(
        r##"<svg viewBox="0 0 140 140" width="140" height="140" role="img" aria-label="Saúde {h}%">
<circle cx="70" cy="70" r="{r}" fill="none" stroke="#e3e8f0" stroke-width="14"/>
<circle cx="70" cy="70" r="{r}" fill="none" stroke="{col}" stroke-width="14" stroke-linecap="round"
 stroke-dasharray="{d:.1} {c:.1}" transform="rotate(-90 70 70)"/>
<text x="70" y="72" text-anchor="middle" font-size="25" font-weight="700" fill="#161b26">{h}%</text>
<text x="70" y="94" text-anchor="middle" font-size="11" fill="#5f687a">SAÚDE</text></svg>"##,
        h = a.health,
        col = health_color(a),
        d = c * frac,
    )
}

/// Gráfico de linha simples (x, y) em SVG.
fn line_svg(points: &[(f64, f64)], color: &str, x_label: &str, y_label: &str) -> String {
    if points.len() < 2 {
        return String::new();
    }
    let (w, h, pl, pb) = (640.0, 180.0, 46.0, 26.0);
    let xmax = points.iter().map(|p| p.0).fold(f64::MIN, f64::max).max(1e-9);
    let xmin = points.iter().map(|p| p.0).fold(f64::MAX, f64::min).min(0.0);
    let ymax = points.iter().map(|p| p.1).fold(0.0, f64::max).max(1e-9) * 1.1;
    let sx = |x: f64| pl + (x - xmin) / (xmax - xmin).max(1e-9) * (w - pl - 8.0);
    let sy = |y: f64| 8.0 + (1.0 - y / ymax) * (h - pb - 8.0);
    let mut pts = String::new();
    for p in points {
        let _ = write!(pts, "{:.1},{:.1} ", sx(p.0), sy(p.1));
    }
    let mut grid = String::new();
    for i in 0..=4 {
        let y = ymax * i as f64 / 4.0;
        let _ = write!(
            grid,
            r##"<line x1="{pl}" x2="{x2}" y1="{yy:.1}" y2="{yy:.1}" stroke="#e3e8f0"/><text x="{tx}" y="{ty:.1}" font-size="10" text-anchor="end" fill="#5f687a">{y:.0}</text>"##,
            x2 = w - 8.0,
            yy = sy(y),
            tx = pl - 6.0,
            ty = sy(y) + 3.0
        );
    }
    format!(
        r##"<svg viewBox="0 0 {w} {h}" width="100%" preserveAspectRatio="none" class="chart">{grid}
<polyline points="{pts}" fill="none" stroke="{color}" stroke-width="2.2" stroke-linejoin="round"/>
<text x="{cx}" y="{h2}" font-size="10" text-anchor="middle" fill="#5f687a">{xl}</text>
<text x="10" y="12" font-size="10" fill="#5f687a">{yl}</text></svg>"##,
        cx = (w + pl) / 2.0,
        h2 = h - 6.0,
        xl = esc(x_label),
        yl = esc(y_label)
    )
}

const CSS: &str = r#"
*{box-sizing:border-box}body{font-family:-apple-system,"Segoe UI",Roboto,Ubuntu,sans-serif;background:#f1f4f9;color:#161b26;margin:0;padding:24px}
.page{max-width:900px;margin:0 auto}
header{display:flex;align-items:center;gap:14px;margin-bottom:18px}
.logo{width:44px;height:44px;border-radius:12px;background:#0078d4;display:flex;align-items:center;justify-content:center;color:#fff;font-weight:800;font-size:22px}
h1{font-size:22px;margin:0}h2{font-size:13px;letter-spacing:.06em;text-transform:uppercase;color:#5f687a;margin:26px 0 8px}
.sub{color:#5f687a;font-size:13px}
.card{background:#fff;border:1px solid #dbe0e9;border-radius:16px;padding:18px;margin-bottom:12px}
.hero{display:flex;gap:22px;align-items:center;flex-wrap:wrap}
.pill{display:inline-block;padding:3px 10px;border-radius:20px;font-size:12px;font-weight:700}
.grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(160px,1fr));gap:10px}
.tile{background:#fff;border:1px solid #dbe0e9;border-radius:14px;padding:10px 12px}
.tile .k{font-size:11px;color:#5f687a}.tile .v{font-size:18px;font-weight:700;overflow-wrap:anywhere}
.finding{border-left:4px solid;padding:8px 12px;margin:8px 0;background:#fafbfd;border-radius:8px}
.finding b{display:block}.finding span{color:#5f687a;font-size:13px}
table{width:100%;border-collapse:collapse;font-size:13px}th,td{text-align:left;padding:6px 8px;border-bottom:1px solid #edf0f5}
th{color:#5f687a;font-weight:600}td.num{text-align:right;font-variant-numeric:tabular-nums}
.bar{height:10px;background:#e3e8f0;border-radius:5px;overflow:hidden}.bar i{display:block;height:100%}
.chart{height:180px}.meta td{border:none;padding:3px 8px 3px 0}
.notes{white-space:pre-wrap}
.print{position:fixed;right:20px;bottom:20px;background:#0078d4;color:#fff;border:0;border-radius:12px;padding:12px 18px;font-size:14px;font-weight:700;cursor:pointer;box-shadow:0 4px 14px rgba(0,0,0,.15)}
footer{color:#5f687a;font-size:11px;margin-top:24px;text-align:center}
@media print{body{background:#fff;padding:0}.print{display:none}.card,.tile{break-inside:avoid}}
"#;

#[allow(clippy::too_many_arguments)]
pub fn html_report(
    dev: &Device,
    smart: Option<&SmartReport>,
    health: Option<&Assessment>,
    bench: Option<&BenchReport>,
    scan: Option<&ScanSummary>,
    cap: Option<&CapResult>,
    info: &ReportInfo,
) -> String {
    let mut h = String::new();
    let model = smart.and_then(|s| s.model.clone()).unwrap_or_else(|| dev.model.clone());
    let _ = write!(
        h,
        r#"<!doctype html><html lang="pt-BR"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Relatório DiscoLab — {m}</title><style>{CSS}</style></head><body><div class="page">
<header><div class="logo">D</div><div><h1>Relatório de disco</h1><div class="sub">DiscoLab {v} · {date}</div></div></header>"#,
        m = esc(&model),
        v = env!("CARGO_PKG_VERSION"),
        date = esc(&info.date)
    );
    if !info.client.is_empty() || !info.technician.is_empty() {
        let _ = write!(h, r#"<div class="card"><table class="meta">"#);
        if !info.client.is_empty() {
            let _ = write!(h, "<tr><td class=sub>Cliente</td><td><b>{}</b></td></tr>", esc(&info.client));
        }
        if !info.technician.is_empty() {
            let _ = write!(h, "<tr><td class=sub>Técnico</td><td>{}</td></tr>", esc(&info.technician));
        }
        let _ = write!(h, "</table></div>");
    }

    // ---- resumo
    let _ = write!(h, r#"<div class="card hero">"#);
    if let Some(a) = health {
        let _ = write!(h, "{}", gauge_svg(a));
    }
    let _ = write!(h, "<div style=\"flex:1;min-width:240px\"><div style=\"font-size:20px;font-weight:700\">{}</div>", esc(&model));
    let _ = write!(h, "<div class=sub>{} · {} · {}</div>", dev.kind.label(), esc(&dev.bus), human_bytes(smart.and_then(|s| s.capacity).unwrap_or(dev.size_bytes)));
    if let Some(a) = health {
        let c = health_color(a);
        let _ = write!(
            h,
            r#"<p><span class="pill" style="background:{c}22;color:{c}">{}</span> <span class=sub>Confiança: {}</span></p><p>{}</p>"#,
            a.status_label(),
            esc(a.confidence),
            esc(&a.verdict)
        );
        if let Some(l) = a.life_left {
            let col = if l <= 10 { "#d03030" } else if l <= 30 { "#d46810" } else { "#0e9662" };
            let _ = write!(h, r#"<div class=sub>Vida útil restante: <b>{l}%</b></div><div class="bar"><i style="width:{l}%;background:{col}"></i></div>"#);
        }
    }
    let _ = write!(h, "</div></div>");

    // ---- números
    if let Some(s) = smart {
        let mut tiles: Vec<(String, String)> = Vec::new();
        if let Some(x) = &s.serial {
            tiles.push(("Serial".into(), esc(x)));
        }
        if let Some(x) = &s.firmware {
            tiles.push(("Firmware".into(), esc(x)));
        }
        if let Some(t) = s.temperature {
            tiles.push(("Temperatura".into(), format!("{t} °C")));
        }
        if let Some(x) = s.power_on_hours {
            tiles.push(("Tempo ligado".into(), format!("{x} h ({:.1} anos)", x as f64 / 8760.0)));
        }
        if let Some(x) = s.power_cycles {
            tiles.push(("Ciclos de energia".into(), x.to_string()));
        }
        if let Some(x) = s.host_written_bytes {
            tiles.push(("Total escrito".into(), human_bytes(x)));
            if let Some(tbw) = info.tbw {
                let left = crate::endurance::life_left_by_tbw(x, tbw).unwrap_or(0);
                tiles.push(("TBW (garantia)".into(), format!("{tbw} TB · {left}% restante")));
            }
        }
        if let Some(x) = s.host_read_bytes {
            tiles.push(("Total lido".into(), human_bytes(x)));
        }
        if let Some(p) = s.smart_passed {
            tiles.push(("Status do firmware".into(), if p { "PASSED".into() } else { "FAILED".into() }));
        }
        if !tiles.is_empty() {
            let _ = write!(h, r#"<div class="grid">"#);
            for (k, v) in tiles {
                let _ = write!(h, r#"<div class="tile"><div class="k">{k}</div><div class="v">{v}</div></div>"#);
            }
            let _ = write!(h, "</div>");
        }
    }

    // ---- diagnóstico
    if let Some(a) = health {
        let _ = write!(h, r#"<h2>Diagnóstico</h2><div class="card">"#);
        for f in &a.findings {
            let _ = write!(
                h,
                r#"<div class="finding" style="border-color:{c}"><b>{t} <span class=pill style="background:{c}22;color:{c}">{s}</span></b><span>{d}</span></div>"#,
                c = sev_color(f.severity),
                t = esc(&f.title),
                s = f.severity.label(),
                d = esc(&f.detail)
            );
        }
        let _ = write!(h, "</div>");
    }

    // ---- SMART
    if let Some(s) = smart {
        if !s.ata.is_empty() {
            let _ = write!(h, r#"<h2>Atributos SMART</h2><div class="card"><table><tr><th>ID</th><th>Atributo</th><th>Atual</th><th>Pior</th><th>Limite</th><th>Bruto</th></tr>"#);
            for a in &s.ata {
                let _ = write!(
                    h,
                    "<tr><td>{}</td><td>{}</td><td class=num>{}</td><td class=num>{}</td><td class=num>{}</td><td class=num>{}</td></tr>",
                    a.id,
                    esc(&a.name),
                    a.value,
                    a.worst,
                    a.thresh,
                    esc(&a.raw_str)
                );
            }
            let _ = write!(h, "</table></div>");
        }
        if let Some(n) = &s.nvme {
            let _ = write!(h, r#"<h2>NVMe SMART / Health</h2><div class="card"><table>"#);
            for (k, v) in [
                ("Aviso crítico", format!("0x{:02X}", n.critical_warning)),
                ("Reserva disponível", format!("{}% (limite {}%)", n.available_spare, n.available_spare_threshold)),
                ("Percentual usado", format!("{}%", n.percentage_used)),
                ("Erros de mídia", n.media_errors.to_string()),
                ("Entradas no log de erros", n.num_err_log_entries.to_string()),
                ("Desligamentos inseguros", n.unsafe_shutdowns.to_string()),
            ] {
                let _ = write!(h, "<tr><td class=sub>{k}</td><td class=num>{v}</td></tr>");
            }
            let _ = write!(h, "</table></div>");
        }
    }

    // ---- benchmark
    if let Some(b) = bench.filter(|b| !b.results.is_empty()) {
        let max = b.results.iter().map(|r| r.mbps).fold(1.0, f64::max);
        let _ = write!(h, r#"<h2>Benchmark</h2><div class="card"><table><tr><th>Teste</th><th style="width:40%"></th><th>MB/s</th><th>IOPS</th><th>p99</th></tr>"#);
        for r in &b.results {
            let col = if r.name.contains("Escrita") { "#d46810" } else { "#2563eb" };
            let _ = write!(
                h,
                r#"<tr><td>{}</td><td><div class="bar"><i style="width:{:.1}%;background:{col}"></i></div></td><td class=num><b>{:.1}</b></td><td class=num>{:.0}</td><td class=num>{} µs</td></tr>"#,
                esc(&r.name),
                r.mbps / max * 100.0,
                r.mbps,
                r.iops,
                r.lat_p99_us
            );
        }
        let _ = write!(h, "</table>");
        if !b.uncached {
            let _ = write!(h, "<p class=sub>Atenção: executado com cache do sistema.</p>");
        }
        if let Some(st) = &b.sustained {
            let pts: Vec<(f64, f64)> = st.curve.clone();
            let _ = write!(
                h,
                "<p><b>Escrita sustentada:</b> pico {:.0} MB/s · sustentado {:.0} MB/s · mínimo {:.0} MB/s{}</p>{}",
                st.burst_mbps,
                st.sustained_mbps,
                st.min_mbps,
                st.cache_estimate_bytes.map(|c| format!(" · cache ~{}", human_bytes(c))).unwrap_or_default(),
                line_svg(&pts, "#d46810", "GB escritos", "MB/s")
            );
        }
        if !b.temperatures.is_empty() {
            let max = b.temperatures.iter().map(|t| t.1).fold(f64::MIN, f64::max);
            let _ = write!(h, "<p><b>Temperatura durante o teste:</b> máxima {max:.0} °C</p>{}", line_svg(&b.temperatures, "#d03030", "segundos", "°C"));
        }
        let _ = write!(h, "</div>");
    }

    // ---- capacidade
    if let Some(c) = cap {
        let col = if c.is_fake() { "#d03030" } else { "#0e9662" };
        let _ = write!(
            h,
            r#"<h2>Capacidade real</h2><div class="card"><p><span class="pill" style="background:{col}22;color:{col}">{}</span> {}</p><div class=sub>Gravado {} · verificado {} · escrita {:.0} MB/s · leitura {:.0} MB/s</div></div>"#,
            if c.is_fake() { "FALHOU" } else { "APROVADO" },
            esc(&c.verdict()),
            human_bytes(c.written),
            human_bytes(c.verified),
            c.write_mbps,
            c.read_mbps
        );
    }

    // ---- superfície
    if let Some(sc) = scan {
        let total: u64 = sc.buckets.iter().sum::<u64>().max(1);
        let colors = ["#a0aabc", "#78829a", "#545e72", "#0e9662", "#d46810", "#d03030", "#8b45e0"];
        let _ = write!(h, r#"<h2>Varredura de superfície</h2><div class="card"><div class="bar" style="height:16px;display:flex">"#);
        for (i, n) in sc.buckets.iter().enumerate() {
            if *n > 0 {
                let _ = write!(h, r#"<i style="width:{:.3}%;background:{}"></i>"#, *n as f64 / total as f64 * 100.0, colors[i]);
            }
        }
        let _ = write!(h, r#"</div><table style="margin-top:8px">"#);
        for (i, l) in BUCKET_LABELS.iter().enumerate() {
            let _ = write!(h, r#"<tr><td><span style="display:inline-block;width:10px;height:10px;border-radius:3px;background:{}"></span> {}</td><td class=num>{}</td></tr>"#, colors[i], esc(l), sc.buckets[i]);
        }
        let _ = write!(
            h,
            "</table><p>Ilegíveis: <b>{}</b> · lentos persistentes: <b>{}</b> · lentos transitórios: {} · {} lidos a {:.0} MB/s em média{}</p>",
            sc.bad_blocks,
            sc.slow_persistent,
            sc.slow_transient,
            human_bytes(sc.scanned_bytes),
            sc.avg_mbps,
            if sc.cancelled { " (interrompida)" } else { "" }
        );
        let pts: Vec<(f64, f64)> = sc.speed_curve.iter().map(|(p, m)| (*p as f64, *m)).collect();
        let _ = write!(h, "{}</div>", line_svg(&pts, "#0e9662", "% do disco", "MB/s"));
    }

    if !info.notes.trim().is_empty() {
        let _ = write!(h, r#"<h2>Observações</h2><div class="card notes">{}</div>"#, esc(&info.notes));
    }
    let _ = write!(
        h,
        r#"<footer>Gerado pelo DiscoLab {} — SMART lido via {}.</footer></div>
<button class="print" onclick="window.print()">Imprimir / salvar PDF</button></body></html>"#,
        env!("CARGO_PKG_VERSION"),
        smart.map(|s| s.source.label()).unwrap_or("—")
    );
    h
}

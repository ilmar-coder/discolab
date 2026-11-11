use discolab_core::bench::{BenchConfig, BenchEvent, run_bench};
use discolab_core::device::{Device, list_devices, list_volumes};
use discolab_core::smart::{SelfTest, start_self_test};
use discolab_core::surface::{BUCKET_LABELS, ScanConfig, ScanEvent, run_scan};
use discolab_core::util::human_bytes;
use discolab_core::{assess, read_smart, report};
use std::io::Write;
use std::sync::atomic::AtomicBool;

const HELP: &str = "DiscoLab CLI — teste de HDD/SSD/NVMe

USO:
  discolab-cli list                         Lista discos e volumes
  discolab-cli health <disco> [--json]      SMART + diagnóstico
  discolab-cli bench <pasta> [--size MiB] [--secs N] [--sustained GiB] [--cached]
  discolab-cli scan <disco> [--start %] [--end %] [--block KiB]   (somente leitura)
  discolab-cli selftest <disco> short|long|abort

<disco> = nome mostrado em `list` (ex.: sda, nvme0n1, disk0, PhysicalDrive0)";

fn arg<T: std::str::FromStr>(args: &[String], flag: &str) -> Option<T> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok())
}

fn find(name: &str) -> Device {
    let devs = list_devices();
    devs.iter()
        .find(|d| d.name == name || d.raw_path == name || d.smart_path == name)
        .cloned()
        .unwrap_or_else(|| {
            eprintln!("Disco '{name}' não encontrado. Use `list`.");
            std::process::exit(2)
        })
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cancel = AtomicBool::new(false);
    match args.first().map(String::as_str) {
        Some("list") => {
            println!("DISCOS");
            for d in list_devices() {
                println!(
                    "  {:<14} {:<10} {:>10}  {}  [{}]{}",
                    d.name,
                    d.kind.label(),
                    human_bytes(d.size_bytes),
                    d.model,
                    d.bus,
                    if d.mount_points.is_empty() { String::new() } else { format!("  → {}", d.mount_points.join(", ")) }
                );
            }
            println!("\nVOLUMES (alvos de benchmark)");
            for v in list_volumes() {
                println!("  {:<30} {:<8} {:>10} livres de {}", v.mount.display(), v.fs, human_bytes(v.free), human_bytes(v.total));
            }
        }
        Some("health") if args.len() > 1 => {
            let d = find(&args[1]);
            let s = read_smart(&d);
            let a = assess(&d, &s, None);
            if args.iter().any(|a| a == "--json") {
                println!("{}", serde_json::to_string_pretty(&serde_json::json!({"device": d, "smart": s, "assessment": a})).unwrap());
                return;
            }
            for m in &s.messages {
                println!("! {m}");
            }
            print!("{}", report::text_report(&d, Some(&s), Some(&a), None, None));
        }
        Some("bench") if args.len() > 1 => {
            let mut cfg = BenchConfig::new(&args[1]);
            if let Some(m) = arg::<u64>(&args, "--size") {
                cfg.file_size = m << 20;
            }
            if let Some(s) = arg::<f32>(&args, "--secs") {
                cfg.seconds = s;
            }
            if let Some(g) = arg::<f64>(&args, "--sustained") {
                cfg.sustained_bytes = (g * (1u64 << 30) as f64) as u64;
            }
            cfg.direct = !args.iter().any(|a| a == "--cached");
            let mut rep = None;
            run_bench(&cfg, &cancel, &mut |e| match e {
                BenchEvent::Phase(p) => println!("» {p}"),
                BenchEvent::Warning(w) => println!("! {w}"),
                BenchEvent::TestDone(_, r) => println!(
                    "  {:<22} {:>9.1} MB/s {:>9.0} IOPS   lat p50 {} µs · p99 {} µs · máx {} µs",
                    r.name, r.mbps, r.iops, r.lat_p50_us, r.lat_p99_us, r.lat_max_us
                ),
                BenchEvent::SustainedSample { gb, mbps } => {
                    print!("\r  {gb:6.2} GB  {mbps:8.0} MB/s   ");
                    let _ = std::io::stdout().flush();
                }
                BenchEvent::Failed(f) => eprintln!("ERRO: {f}"),
                BenchEvent::Done(r) => rep = Some(r),
                _ => {}
            });
            if let Some(r) = rep {
                if let Some(s) = &r.sustained {
                    println!(
                        "\n  Sustentada: pico {:.0} MB/s · sustentado {:.0} · mín {:.0}{}",
                        s.burst_mbps,
                        s.sustained_mbps,
                        s.min_mbps,
                        s.cache_estimate_bytes.map(|c| format!(" · cache ~{}", human_bytes(c))).unwrap_or_default()
                    );
                }
                println!("Modo: {}", if r.uncached { "sem cache do SO (honesto)" } else { "COM cache do SO" });
            }
        }
        Some("scan") if args.len() > 1 => {
            let d = find(&args[1]);
            let mut cfg = ScanConfig::new(&d.raw_path, d.size_bytes, d.kind.is_flash());
            if let Some(v) = arg(&args, "--start") {
                cfg.start_pct = v;
            }
            if let Some(v) = arg(&args, "--end") {
                cfg.end_pct = v;
            }
            if let Some(v) = arg::<usize>(&args, "--block") {
                cfg.block_size = v * 1024;
            }
            let mut total = 1;
            run_scan(&cfg, &cancel, &mut |e| match e {
                ScanEvent::Started { total_blocks, uncached, size } => {
                    total = total_blocks.max(1);
                    println!("Varrendo {} ({}) — {} blocos, sem cache: {uncached}", d.raw_path, human_bytes(size), total_blocks);
                }
                ScanEvent::Progress { done_blocks, mbps, position_pct, .. } => {
                    print!("\r  {:5.1}%  pos {:5.1}%  {:7.1} MB/s   ", done_blocks as f64 * 100.0 / total as f64, position_pct, mbps);
                    let _ = std::io::stdout().flush();
                }
                ScanEvent::Rechecking { count } => println!("\n  Relendo {count} blocos suspeitos…"),
                ScanEvent::Log(l) => println!("\n  {l}"),
                ScanEvent::Failed(f) => eprintln!("ERRO: {f}"),
                ScanEvent::Done(s) => {
                    println!();
                    for (i, l) in BUCKET_LABELS.iter().enumerate() {
                        println!("  {:<9} {}", l, s.buckets[i]);
                    }
                    println!(
                        "  Ilegíveis {} · lentos persistentes {} · transitórios {} · média {:.0} MB/s em {:.0}s",
                        s.bad_blocks, s.slow_persistent, s.slow_transient, s.avg_mbps, s.elapsed_s
                    );
                    let sm = read_smart(&d);
                    let a = assess(&d, &sm, Some(&s));
                    println!("  Saúde combinada: {}% — {}", a.health, a.status_label());
                }
                _ => {}
            });
        }
        Some("selftest") if args.len() > 2 => {
            let d = find(&args[1]);
            let k = match args[2].as_str() {
                "long" => SelfTest::Long,
                "abort" => SelfTest::Abort,
                _ => SelfTest::Short,
            };
            match start_self_test(&d, k) {
                Ok(m) => println!("{m}"),
                Err(e) => eprintln!("ERRO: {e}"),
            }
        }
        _ => println!("{HELP}"),
    }
}

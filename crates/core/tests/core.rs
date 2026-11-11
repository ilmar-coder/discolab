use discolab_core::bench::analyze_sustained;
use discolab_core::device::{Device, DriveKind, parse_windows_disks};
use discolab_core::health::{Severity, assess};
use discolab_core::smart::parse_smartctl;
use discolab_core::surface::{ScanConfig, ScanEvent, run_scan};
use serde_json::json;
use std::sync::atomic::AtomicBool;

fn dev(kind: DriveKind) -> Device {
    Device {
        name: "sda".into(),
        smart_path: "/dev/sda".into(),
        raw_path: "/dev/sda".into(),
        model: "ST2000DM008".into(),
        serial: None,
        firmware: None,
        size_bytes: 2_000_000_000_000,
        kind,
        bus: "SATA".into(),
        removable: false,
        mount_points: vec![],
        sysfs: None,
        os_health: None,
    }
}

fn attr(id: u32, name: &str, value: u32, thresh: u32, raw: u64) -> serde_json::Value {
    json!({"id": id, "name": name, "value": value, "worst": value, "thresh": thresh,
           "flags": {"prefailure": true}, "raw": {"value": raw, "string": raw.to_string()}})
}

fn ata(attrs: Vec<serde_json::Value>) -> serde_json::Value {
    json!({
        "smartctl": {"exit_status": 0},
        "model_name": "ST2000DM008-2FR102",
        "rotation_rate": 7200,
        "smart_status": {"passed": true},
        "ata_smart_attributes": {"table": attrs},
        "temperature": {"current": 38}
    })
}

#[test]
fn seagate_packed_raw_values_are_not_alarms() {
    let s = parse_smartctl(ata(vec![
        attr(1, "Raw_Read_Error_Rate", 82, 6, 176_843_920),
        attr(7, "Seek_Error_Rate", 88, 45, 667_403_022),
        attr(5, "Reallocated_Sector_Ct", 100, 10, 0),
        attr(199, "UDMA_CRC_Error_Count", 200, 0, 12),
    ]));
    let a = assess(&dev(DriveKind::Hdd), &s, None);
    assert!(a.health >= 95, "saúde {}", a.health);
    assert!(a.status <= Severity::Notice);
}

#[test]
fn few_reallocated_is_notice_not_failure() {
    let s = parse_smartctl(ata(vec![attr(5, "Reallocated_Sector_Ct", 100, 10, 3)]));
    let a = assess(&dev(DriveKind::Hdd), &s, None);
    assert_eq!(a.status, Severity::Notice);
    assert!(a.health >= 90);
}

#[test]
fn pending_sectors_are_flagged_even_when_smart_passed() {
    let s = parse_smartctl(ata(vec![
        attr(197, "Current_Pending_Sector", 100, 0, 24),
        attr(198, "Offline_Uncorrectable", 100, 0, 24),
    ]));
    let a = assess(&dev(DriveKind::Hdd), &s, None);
    assert_eq!(a.status, Severity::Critical);
    assert!(a.health < 40);
}

#[test]
fn nvme_wear_and_read_only() {
    let v = json!({
        "smartctl": {"exit_status": 0}, "model_name": "X", "smart_status": {"passed": true},
        "nvme_smart_health_information_log": {"critical_warning": 8, "available_spare": 100,
            "available_spare_threshold": 10, "percentage_used": 37, "media_errors": 0,
            "num_err_log_entries": 900, "data_units_written": 1000}
    });
    let s = parse_smartctl(v);
    let a = assess(&dev(DriveKind::Nvme), &s, None);
    assert_eq!(a.life_left, Some(63));
    assert_eq!(a.status, Severity::Critical);
    assert_eq!(s.host_written_bytes, Some(512_000_000));
}

#[test]
fn windows_disk_json() {
    let j = r#"[{"Number":0,"Model":"Samsung SSD 970","Serial":"S4","Fw":"2B2Q","Media":"4","Bus":"17","Size":1000204886016,"Health":"0","Vols":["C:\\"]},
                {"Number":1,"Model":"WDC","Serial":"","Fw":"","Media":"HDD","Bus":"SATA","Size":2000,"Health":"Warning","Vols":null}]"#;
    let d = parse_windows_disks(j);
    assert_eq!(d.len(), 2);
    assert_eq!(d[0].kind, DriveKind::Nvme);
    assert_eq!(d[0].raw_path, r"\\.\PhysicalDrive0");
    assert_eq!(d[1].kind, DriveKind::Hdd);
    assert_eq!(d[1].os_health.as_deref(), Some("Aviso"));
}

#[test]
fn slc_cache_cliff_detected() {
    let mut curve = vec![];
    for i in 0..40 {
        let gb = i as f64 * 0.5;
        curve.push((gb, if gb < 6.0 { 3000.0 } else { 800.0 }));
    }
    let r = analyze_sustained(curve, 20_000_000_000);
    assert!(r.burst_mbps > 2900.0 && r.sustained_mbps < 900.0);
    let c = r.cache_estimate_bytes.unwrap();
    assert!((5_000_000_000..7_000_000_000).contains(&c), "{c}");
}

#[test]
fn surface_scan_on_file() {
    let dir = std::env::temp_dir().join(format!("discolab_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("img.bin");
    std::fs::write(&p, vec![7u8; 32 << 20]).unwrap();
    let cfg = ScanConfig::new(&p, 32 << 20, true);
    let mut done = None;
    run_scan(&cfg, &AtomicBool::new(false), &mut |e| {
        if let ScanEvent::Done(s) = e {
            done = Some(s)
        }
    });
    let s = done.expect("scan terminou");
    assert_eq!(s.buckets.iter().sum::<u64>(), 256);
    assert_eq!(s.bad_blocks, 0);
    std::fs::remove_dir_all(dir).unwrap();
}

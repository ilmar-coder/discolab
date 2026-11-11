//! Enumeração de discos físicos em Linux, Android, macOS e Windows.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::util::{read_trim, run};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DriveKind {
    Hdd,
    SataSsd,
    Nvme,
    Usb,
    Emmc,
    Ufs,
    SdCard,
    Unknown,
}

impl DriveKind {
    pub fn label(self) -> &'static str {
        match self {
            DriveKind::Hdd => "HDD",
            DriveKind::SataSsd => "SSD SATA",
            DriveKind::Nvme => "NVMe",
            DriveKind::Usb => "USB",
            DriveKind::Emmc => "eMMC",
            DriveKind::Ufs => "UFS",
            DriveKind::SdCard => "Cartão SD",
            DriveKind::Unknown => "Desconhecido",
        }
    }
    pub fn is_rotational(self) -> bool {
        matches!(self, DriveKind::Hdd)
    }
    pub fn is_flash(self) -> bool {
        matches!(
            self,
            DriveKind::SataSsd | DriveKind::Nvme | DriveKind::Emmc | DriveKind::Ufs | DriveKind::SdCard
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    /// Nome curto (sda, nvme0n1, disk0, PhysicalDrive0).
    pub name: String,
    /// Argumento de dispositivo para o smartctl.
    pub smart_path: String,
    /// Caminho bruto para leitura de superfície.
    pub raw_path: String,
    pub model: String,
    pub serial: Option<String>,
    pub firmware: Option<String>,
    pub size_bytes: u64,
    pub kind: DriveKind,
    pub bus: String,
    pub removable: bool,
    pub mount_points: Vec<String>,
    /// Pasta sysfs (Linux/Android) para ler saúde de eMMC/UFS.
    pub sysfs: Option<PathBuf>,
    /// Status de saúde informado pelo SO (Windows), se houver.
    pub os_health: Option<String>,
}

/// Volume montado — alvo para benchmark baseado em arquivo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Volume {
    pub mount: PathBuf,
    pub name: String,
    pub fs: String,
    pub total: u64,
    pub free: u64,
    pub removable: bool,
}

pub fn list_volumes() -> Vec<Volume> {
    let Ok(disks) = sysinfo::Disks::new_with_refreshed_list() else { return Vec::new() };
    let mut v: Vec<Volume> = disks
        .iter()
        .filter(|d| d.total_space() > 0)
        .map(|d| Volume {
            mount: d.mount_point().to_path_buf(),
            name: d.name().to_string_lossy().into_owned(),
            fs: d.file_system().to_string_lossy().into_owned(),
            total: d.total_space(),
            free: d.available_space(),
            removable: d.is_removable(),
        })
        .filter(|v| {
            let m = v.mount.to_string_lossy();
            // Ignora volumes de sistema somente-leitura / virtuais do macOS
            !m.starts_with("/System/Volumes/")
                && !m.starts_with("/dev")
                && !m.starts_with("/proc")
                && !m.starts_with("/sys")
                && !m.starts_with("/run")
                && !m.starts_with("/snap")
                && !m.starts_with("/boot/efi")
        })
        .collect();
    #[cfg(target_os = "android")]
    v.retain(|x| {
        let m = x.mount.to_string_lossy();
        (m == "/data" || m.starts_with("/storage/") || m.starts_with("/mnt/media_rw/")) && !m.starts_with("/storage/self")
    });
    v.sort_by(|a, b| a.mount.cmp(&b.mount));
    v.dedup_by(|a, b| a.mount == b.mount);
    v
}

pub fn list_devices() -> Vec<Device> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        #[allow(unused_mut)]
        let mut v = list_sysfs(Path::new("/sys/block"));
        // Android sem root: o SELinux esconde /sys/block. Expomos o armazenamento
        // interno como pseudo-dispositivo para permitir benchmark.
        #[cfg(target_os = "android")]
        if v.is_empty() {
            let vols = list_volumes();
            let data = vols.iter().find(|x| x.mount == Path::new("/data")).or(vols.first());
            v.push(Device {
                name: "interno".into(),
                smart_path: String::new(),
                raw_path: String::new(),
                model: "Armazenamento interno".into(),
                serial: None,
                firmware: None,
                size_bytes: data.map(|d| d.total).unwrap_or(0),
                kind: DriveKind::Unknown,
                bus: "Android".into(),
                removable: false,
                mount_points: data.map(|d| vec![d.mount.display().to_string()]).unwrap_or_default(),
                sysfs: None,
                os_health: None,
            });
        }
        // Discos em adaptadores USB-OTG: acesso direto sem root (pede permissão ao usuário).
        #[cfg(target_os = "android")]
        for u in crate::usb::android::list_usb_storage() {
            let path = format!("{}{}", crate::usb::PREFIX, u.name);
            let model = format!("{} {}", u.manufacturer, u.product).trim().to_string();
            v.push(Device {
                name: format!("USB {:04x}:{:04x}", u.vid, u.pid),
                smart_path: path.clone(),
                raw_path: path,
                model: if model.is_empty() { "Disco USB".into() } else { model },
                serial: None,
                firmware: None,
                size_bytes: 0,
                kind: DriveKind::Usb,
                bus: "USB-OTG".into(),
                removable: true,
                mount_points: vec![],
                sysfs: None,
                os_health: None,
            });
        }
        v
    }
    #[cfg(target_os = "macos")]
    {
        list_macos()
    }
    #[cfg(windows)]
    {
        list_windows()
    }
    #[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos", windows)))]
    {
        Vec::new()
    }
}

// ---------------------------------------------------------------- Linux / Android

#[allow(dead_code)]
pub fn list_sysfs(root: &Path) -> Vec<Device> {
    let Ok(rd) = std::fs::read_dir(root) else { return Vec::new() };
    let mounts = std::fs::read_to_string("/proc/mounts").unwrap_or_default();
    let mut out = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let skip = ["loop", "ram", "zram", "dm-", "md", "sr", "fd", "nbd", "mtdblock"];
        if skip.iter().any(|p| name.starts_with(p)) || name.contains("boot") || name.contains("rpmb") {
            continue;
        }
        let sys = root.join(&name);
        let sectors: u64 = read_trim(sys.join("size")).and_then(|s| s.parse().ok()).unwrap_or(0);
        if sectors == 0 {
            continue;
        }
        let rotational = read_trim(sys.join("queue/rotational")).as_deref() == Some("1");
        let removable = read_trim(sys.join("removable")).as_deref() == Some("1");
        let real = std::fs::canonicalize(&sys).unwrap_or_default().to_string_lossy().into_owned();
        let dev = sys.join("device");
        let model = read_trim(dev.join("model"))
            .or_else(|| read_trim(dev.join("name")))
            .unwrap_or_else(|| name.clone());
        let vendor = read_trim(dev.join("vendor")).filter(|v| v != "ATA");
        let model = match vendor {
            Some(v) if !model.starts_with(&v) => format!("{v} {model}"),
            _ => model,
        };
        let serial = read_trim(dev.join("serial")).or_else(|| read_trim(dev.join("wwid")));
        let firmware = read_trim(dev.join("firmware_rev")).or_else(|| read_trim(dev.join("rev")));

        let (kind, bus) = if name.starts_with("nvme") {
            (DriveKind::Nvme, "NVMe")
        } else if name.starts_with("mmcblk") {
            match read_trim(dev.join("type")).as_deref() {
                Some("SD") => (DriveKind::SdCard, "SD"),
                _ => (DriveKind::Emmc, "eMMC"),
            }
        } else if real.contains("/usb") {
            (DriveKind::Usb, "USB")
        } else if real.contains("ufs") {
            (DriveKind::Ufs, "UFS")
        } else if rotational {
            (DriveKind::Hdd, "SATA/SAS")
        } else if name.starts_with("vd") || name.starts_with("xvd") {
            (DriveKind::Unknown, "Virtual")
        } else {
            (DriveKind::SataSsd, "SATA/SAS")
        };

        let devnode = if Path::new(&format!("/dev/block/{name}")).exists() {
            format!("/dev/block/{name}")
        } else {
            format!("/dev/{name}")
        };
        let mount_points = mounts
            .lines()
            .filter_map(|l| {
                let mut it = l.split_whitespace();
                let src = it.next()?;
                let mp = it.next()?;
                (src.starts_with(&devnode) || src.starts_with(&format!("/dev/{name}")))
                    .then(|| mp.replace("\\040", " "))
            })
            .collect();
        out.push(Device {
            smart_path: devnode.clone(),
            raw_path: devnode,
            name: name.clone(),
            model,
            serial,
            firmware,
            size_bytes: sectors * 512,
            kind,
            bus: bus.into(),
            removable,
            mount_points,
            sysfs: Some(sys),
            os_health: None,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

// ---------------------------------------------------------------- macOS

#[allow(dead_code)]
fn list_macos() -> Vec<Device> {
    let Some(xml) = run("diskutil", &["list", "-plist", "physical"]) else { return Vec::new() };
    let Ok(root) = plist::Value::from_reader_xml(xml.as_bytes()) else { return Vec::new() };
    let whole: Vec<String> = root
        .as_dictionary()
        .and_then(|d| d.get("WholeDisks"))
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_string().map(String::from)).collect())
        .unwrap_or_default();
    let mounts = mac_mounts();
    let mut out = Vec::new();
    for id in whole {
        let Some(info) = run("diskutil", &["info", "-plist", &id]) else { continue };
        let Ok(v) = plist::Value::from_reader_xml(info.as_bytes()) else { continue };
        let Some(d) = v.as_dictionary() else { continue };
        let s = |k: &str| d.get(k).and_then(|x| x.as_string()).unwrap_or("").to_string();
        let b = |k: &str| d.get(k).and_then(|x| x.as_boolean()).unwrap_or(false);
        let size = d
            .get("TotalSize")
            .or_else(|| d.get("Size"))
            .and_then(|x| x.as_unsigned_integer())
            .unwrap_or(0);
        let bus = s("BusProtocol");
        let ssd = b("SolidState");
        let kind = match bus.as_str() {
            "USB" | "Thunderbolt" if !ssd => DriveKind::Usb,
            "USB" => DriveKind::Usb,
            "PCI-Express" | "Apple Fabric" | "PCI" => DriveKind::Nvme,
            "Secure Digital" => DriveKind::SdCard,
            _ if ssd => DriveKind::SataSsd,
            "SATA" => DriveKind::Hdd,
            _ => DriveKind::Unknown,
        };
        let pref = format!("/dev/{id}s");
        let mount_points = mounts
            .iter()
            .filter(|(dev, _)| dev == &format!("/dev/{id}") || dev.starts_with(&pref))
            .map(|(_, m)| m.clone())
            .collect();
        out.push(Device {
            name: id.clone(),
            smart_path: id.clone(),
            raw_path: format!("/dev/r{id}"),
            model: {
                let m = s("MediaName");
                if m.is_empty() { id.clone() } else { m }
            },
            serial: None,
            firmware: None,
            size_bytes: size,
            kind,
            bus,
            removable: b("Removable") || b("RemovableMedia") || b("Ejectable"),
            mount_points,
            sysfs: None,
            os_health: d.get("SMARTStatus").and_then(|x| x.as_string()).map(String::from),
        });
    }
    out
}

#[allow(dead_code)]
fn mac_mounts() -> Vec<(String, String)> {
    // APFS: volumes ficam em discos sintéticos; mapeamos via "APFSPhysicalStores".
    let text = run("mount", &[]).unwrap_or_default();
    let mut v = Vec::new();
    for l in text.lines() {
        let Some((dev, rest)) = l.split_once(" on ") else { continue };
        let mp = rest.split(" (").next().unwrap_or("").to_string();
        v.push((dev.to_string(), mp));
    }
    // Resolve containers APFS para o disco físico.
    if let Some(xml) = run("diskutil", &["apfs", "list", "-plist"]) {
        if let Ok(root) = plist::Value::from_reader_xml(xml.as_bytes()) {
            let containers = root
                .as_dictionary()
                .and_then(|d| d.get("Containers"))
                .and_then(|c| c.as_array())
                .cloned()
                .unwrap_or_default();
            for c in containers {
                let Some(c) = c.as_dictionary() else { continue };
                let cref = c.get("ContainerReference").and_then(|x| x.as_string()).unwrap_or("");
                let phys: Vec<String> = c
                    .get("PhysicalStores")
                    .and_then(|x| x.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|p| {
                                p.as_dictionary()?.get("DeviceIdentifier")?.as_string().map(String::from)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let extra: Vec<(String, String)> = v
                    .iter()
                    .filter(|(dev, _)| dev.starts_with(&format!("/dev/{cref}s")))
                    .flat_map(|(_, m)| phys.iter().map(move |p| (format!("/dev/{p}"), m.clone())))
                    .collect();
                v.extend(extra);
            }
        }
    }
    v
}

// ---------------------------------------------------------------- Windows

#[allow(dead_code)]
fn list_windows() -> Vec<Device> {
    let script = r#"
$ErrorActionPreference='SilentlyContinue'
Get-PhysicalDisk | ForEach-Object {
  $d=$_; $n=[int]$d.DeviceId
  $vols=@(Get-Partition -DiskNumber $n | Where-Object DriveLetter | ForEach-Object { "$($_.DriveLetter):\" })
  [pscustomobject]@{Number=$n;Model=$d.FriendlyName;Serial=$d.SerialNumber;Fw=$d.FirmwareVersion;
    Media="$($d.MediaType)";Bus="$($d.BusType)";Size=[uint64]$d.Size;Health="$($d.HealthStatus)";Vols=$vols}
} | ConvertTo-Json -Depth 3
"#;
    let Some(out) = run("powershell", &["-NoProfile", "-NonInteractive", "-Command", script]) else {
        return Vec::new();
    };
    parse_windows_disks(&out)
}

/// Converte a saída JSON do PowerShell (objeto único ou lista).
pub fn parse_windows_disks(json: &str) -> Vec<Device> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json.trim()) else { return Vec::new() };
    let items = match v {
        serde_json::Value::Array(a) => a,
        o @ serde_json::Value::Object(_) => vec![o],
        _ => return Vec::new(),
    };
    let mut out = Vec::new();
    for it in items {
        let n = it["Number"].as_u64().unwrap_or(0);
        let st = |k: &str| it[k].as_str().map(|s| s.trim().to_string()).unwrap_or_default();
        let media = st("Media");
        let bus = st("Bus");
        let bus_name = match bus.as_str() {
            "17" | "NVMe" => "NVMe",
            "11" | "SATA" => "SATA",
            "7" | "USB" => "USB",
            "3" | "ATA" => "ATA",
            "10" | "SAS" => "SAS",
            "8" | "RAID" => "RAID",
            "12" | "SD" => "SD",
            "13" | "MMC" => "MMC",
            "15" | "File Backed Virtual" => "Virtual",
            "18" | "SCM" => "SCM",
            other => other,
        }
        .to_string();
        let kind = match (bus_name.as_str(), media.as_str()) {
            ("NVMe", _) => DriveKind::Nvme,
            ("USB", _) => DriveKind::Usb,
            ("SD", _) => DriveKind::SdCard,
            ("MMC", _) => DriveKind::Emmc,
            (_, "3" | "HDD") => DriveKind::Hdd,
            (_, "4" | "SSD") => DriveKind::SataSsd,
            _ => DriveKind::Unknown,
        };
        let vols = match &it["Vols"] {
            serde_json::Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(String::from)).collect(),
            serde_json::Value::String(s) => vec![s.clone()],
            _ => vec![],
        };
        let health = match st("Health").as_str() {
            "0" | "Healthy" => "Saudável",
            "1" | "Warning" => "Aviso",
            "2" | "Unhealthy" => "Com falha",
            _ => "",
        };
        out.push(Device {
            name: format!("PhysicalDrive{n}"),
            smart_path: format!("/dev/pd{n}"),
            raw_path: format!(r"\\.\PhysicalDrive{n}"),
            model: st("Model"),
            serial: Some(st("Serial")).filter(|s| !s.is_empty()),
            firmware: Some(st("Fw")).filter(|s| !s.is_empty()),
            size_bytes: it["Size"].as_u64().unwrap_or(0),
            kind,
            bus: bus_name,
            removable: kind == DriveKind::Usb,
            mount_points: vols,
            sysfs: None,
            os_health: Some(health.to_string()).filter(|s| !s.is_empty()),
        });
    }
    out.sort_by_key(|d| d.name.trim_start_matches("PhysicalDrive").parse::<u32>().unwrap_or(0));
    out
}

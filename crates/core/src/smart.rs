//! Leitura de SMART. Fonte principal: smartmontools (`smartctl --json`), que é a
//! implementação mais confiável e multiplataforma de ATA/SCSI/NVMe passthrough.
//! Alternativas: contadores de confiabilidade do Windows e sysfs de eMMC/UFS (Android).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::device::{Device, DriveKind};
use crate::util::{hide_window, read_trim};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SmartSource {
    Smartctl,
    WindowsReliability,
    EmmcSysfs,
    UfsSysfs,
    UsbDirect,
    None,
}

impl SmartSource {
    pub fn label(self) -> &'static str {
        match self {
            SmartSource::Smartctl => "smartctl (SMART completo)",
            SmartSource::WindowsReliability => "Windows Storage Reliability (parcial)",
            SmartSource::EmmcSysfs => "eMMC EXT_CSD via sysfs (parcial)",
            SmartSource::UfsSysfs => "UFS health descriptor (parcial)",
            SmartSource::UsbDirect => "Acesso USB direto (SAT / ponte NVMe)",
            SmartSource::None => "indisponível",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtaAttr {
    pub id: u32,
    pub name: String,
    pub value: u32,
    pub worst: u32,
    pub thresh: u32,
    pub raw: u64,
    pub raw_str: String,
    pub prefailure: bool,
    pub when_failed: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NvmeHealth {
    pub critical_warning: u32,
    pub temperature: Option<i64>,
    pub available_spare: u32,
    pub available_spare_threshold: u32,
    pub percentage_used: u32,
    pub data_units_read: u64,
    pub data_units_written: u64,
    pub power_cycles: u64,
    pub power_on_hours: u64,
    pub unsafe_shutdowns: u64,
    pub media_errors: u64,
    pub num_err_log_entries: u64,
    pub warning_temp_time: u64,
    pub critical_comp_time: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfTestEntry {
    pub kind: String,
    pub status: String,
    pub passed: Option<bool>,
    pub hours: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FlashLife {
    /// Estimativa de desgaste (0..100%+) — eMMC/UFS life_time.
    pub used_pct_a: Option<u32>,
    pub used_pct_b: Option<u32>,
    /// 1 = normal, 2 = aviso (80% dos blocos reserva), 3 = urgente.
    pub pre_eol: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartReport {
    pub source: SmartSource,
    pub ok: bool,
    pub messages: Vec<String>,
    pub needs_admin: bool,
    pub model: Option<String>,
    pub serial: Option<String>,
    pub firmware: Option<String>,
    pub capacity: Option<u64>,
    pub rotation_rpm: Option<u64>,
    pub protocol: Option<String>,
    pub power_on_hours: Option<u64>,
    pub power_cycles: Option<u64>,
    pub temperature: Option<i64>,
    pub smart_passed: Option<bool>,
    pub ata: Vec<AtaAttr>,
    pub nvme: Option<NvmeHealth>,
    /// "Percentage Used Endurance Indicator" das estatísticas ATA.
    pub ata_endurance_used: Option<u32>,
    pub ata_error_count: Option<u64>,
    pub self_tests: Vec<SelfTestEntry>,
    pub self_test_running: Option<u32>,
    pub flash: Option<FlashLife>,
    /// Campos extra (Windows reliability).
    pub extra: Vec<(String, String)>,
    pub host_written_bytes: Option<u64>,
    pub host_read_bytes: Option<u64>,
    #[serde(skip)]
    pub raw_json: Option<Value>,
}

impl SmartReport {
    pub(crate) fn empty(source: SmartSource) -> Self {
        SmartReport {
            source,
            ok: false,
            messages: vec![],
            needs_admin: false,
            model: None,
            serial: None,
            firmware: None,
            capacity: None,
            rotation_rpm: None,
            protocol: None,
            power_on_hours: None,
            power_cycles: None,
            temperature: None,
            smart_passed: None,
            ata: vec![],
            nvme: None,
            ata_endurance_used: None,
            ata_error_count: None,
            self_tests: vec![],
            self_test_running: None,
            flash: None,
            extra: vec![],
            host_written_bytes: None,
            host_read_bytes: None,
            raw_json: None,
        }
    }
    pub fn attr(&self, id: u32) -> Option<&AtaAttr> {
        self.ata.iter().find(|a| a.id == id)
    }
}

/// Localiza o executável do smartctl (PATH, locais comuns ou ao lado do app).
pub fn find_smartctl() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "smartctl.exe" } else { "smartctl" };
    let mut cands: Vec<PathBuf> = Vec::new();
    if let Ok(me) = std::env::current_exe() {
        if let Some(dir) = me.parent() {
            cands.push(dir.join(exe));
            cands.push(dir.join("smartmontools").join(exe));
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        cands.extend(std::env::split_paths(&path).map(|p| p.join(exe)));
    }
    for p in [
        "/opt/homebrew/sbin",
        "/opt/homebrew/bin",
        "/usr/local/sbin",
        "/usr/local/bin",
        "/usr/sbin",
        "/sbin",
        "/data/local/tmp",
        "/system/xbin",
        r"C:\Program Files\smartmontools\bin",
        r"C:\Program Files (x86)\smartmontools\bin",
    ] {
        cands.push(Path::new(p).join(exe));
    }
    cands.into_iter().find(|p| p.is_file()).or_else(embedded_smartctl)
}

/// Versão do smartctl embutida no executável (Windows), extraída para a pasta temporária.
#[cfg(embedded_smartctl)]
pub fn embedded_smartctl() -> Option<PathBuf> {
    static BYTES: &[u8] = include_bytes!(env!("DISCOLAB_SMARTCTL_PATH"));
    let dir = std::env::temp_dir().join("DiscoLab");
    let path = dir.join(format!("smartctl-{}.exe", BYTES.len()));
    let ok = std::fs::metadata(&path).map(|m| m.len() == BYTES.len() as u64).unwrap_or(false);
    if !ok {
        std::fs::create_dir_all(&dir).ok()?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, BYTES).ok()?;
        std::fs::rename(&tmp, &path).ok()?;
    }
    Some(path)
}

#[cfg(not(embedded_smartctl))]
pub fn embedded_smartctl() -> Option<PathBuf> {
    None
}

/// Indica se este executável traz o smartctl embutido.
pub const SMARTCTL_EMBEDDED: bool = cfg!(embedded_smartctl);

pub fn read_smart(dev: &Device) -> SmartReport {
    if crate::usb::is_usb(&dev.smart_path) {
        return crate::usb::read_smart_usb(dev);
    }
    let mut msgs = Vec::new();
    if let Some(sc) = find_smartctl() {
        let r = smartctl_report(&sc, &dev.smart_path);
        if r.ok {
            return r;
        }
        msgs.extend(r.messages.clone());
        if r.needs_admin {
            let mut alt = fallback(dev);
            alt.needs_admin = true;
            alt.messages.splice(0..0, msgs);
            return alt;
        }
    } else {
        msgs.push(
            "smartctl não encontrado. Instale o smartmontools para SMART completo \
             (macOS: brew install smartmontools · Linux: apt/dnf install smartmontools · \
             Windows: smartmontools.org) ou coloque smartctl ao lado do DiscoLab."
                .into(),
        );
    }
    let mut r = fallback(dev);
    r.messages.splice(0..0, msgs);
    r
}

fn fallback(dev: &Device) -> SmartReport {
    if let Some(sys) = &dev.sysfs {
        if let Some(r) = read_flash_sysfs(sys, dev.kind) {
            return r;
        }
    }
    #[cfg(windows)]
    {
        if let Some(r) = windows_reliability(dev) {
            return r;
        }
    }
    let mut r = SmartReport::empty(SmartSource::None);
    r.messages.push("Não foi possível ler dados de saúde deste dispositivo.".into());
    r
}

pub fn smartctl_report(smartctl: &Path, dev: &str) -> SmartReport {
    let mut cmd = Command::new(smartctl);
    cmd.args(["-x", "-j", dev]);
    hide_window(&mut cmd);
    let out = match cmd.output() {
        Ok(o) => o,
        Err(e) => {
            let mut r = SmartReport::empty(SmartSource::Smartctl);
            r.messages.push(format!("Falha ao executar smartctl: {e}"));
            return r;
        }
    };
    let text = String::from_utf8_lossy(&out.stdout);
    match serde_json::from_str::<Value>(&text) {
        Ok(v) => parse_smartctl(v),
        Err(e) => {
            let mut r = SmartReport::empty(SmartSource::Smartctl);
            r.messages.push(format!("Saída inválida do smartctl: {e}"));
            r
        }
    }
}

/// Converte o JSON do smartctl para o nosso modelo.
pub fn parse_smartctl(v: Value) -> SmartReport {
    let mut r = SmartReport::empty(SmartSource::Smartctl);
    let exit = v["smartctl"]["exit_status"].as_u64().unwrap_or(0);
    for m in v["smartctl"]["messages"].as_array().into_iter().flatten() {
        if let Some(s) = m["string"].as_str() {
            r.messages.push(s.to_string());
            let l = s.to_lowercase();
            if l.contains("permission denied") || l.contains("operation not permitted") || l.contains("administrator") || l.contains("root") {
                r.needs_admin = true;
            }
        }
    }
    // bit 1 = falha ao abrir o dispositivo
    r.ok = exit & 0b11 == 0 && (v.get("model_name").is_some() || v.get("smart_status").is_some());
    let s = |k: &str| v[k].as_str().map(|x| x.trim().to_string());
    r.model = s("model_name").or_else(|| {
        let a = v["scsi_vendor"].as_str().unwrap_or("");
        let b = v["scsi_product"].as_str().unwrap_or("");
        let m = format!("{a} {b}").trim().to_string();
        (!m.is_empty()).then_some(m)
    });
    r.serial = s("serial_number");
    r.firmware = s("firmware_version");
    r.capacity = v["user_capacity"]["bytes"].as_u64().or_else(|| v["nvme_total_capacity"].as_u64());
    r.rotation_rpm = v["rotation_rate"].as_u64();
    r.protocol = v["device"]["protocol"].as_str().map(String::from);
    r.power_on_hours = v["power_on_time"]["hours"].as_u64();
    r.power_cycles = v["power_cycle_count"].as_u64();
    r.temperature = v["temperature"]["current"].as_i64();
    r.smart_passed = v["smart_status"]["passed"].as_bool();

    for a in v["ata_smart_attributes"]["table"].as_array().into_iter().flatten() {
        r.ata.push(AtaAttr {
            id: a["id"].as_u64().unwrap_or(0) as u32,
            name: a["name"].as_str().unwrap_or("?").to_string(),
            value: a["value"].as_u64().unwrap_or(0) as u32,
            worst: a["worst"].as_u64().unwrap_or(0) as u32,
            thresh: a["thresh"].as_u64().unwrap_or(0) as u32,
            raw: a["raw"]["value"].as_u64().unwrap_or(0),
            raw_str: a["raw"]["string"].as_str().unwrap_or("").to_string(),
            prefailure: a["flags"]["prefailure"].as_bool().unwrap_or(false),
            when_failed: a["when_failed"].as_str().unwrap_or("").to_string(),
        });
    }

    let n = &v["nvme_smart_health_information_log"];
    if n.is_object() {
        let g = |k: &str| n[k].as_u64().unwrap_or(0);
        let h = NvmeHealth {
            critical_warning: g("critical_warning") as u32,
            temperature: n["temperature"].as_i64(),
            available_spare: g("available_spare") as u32,
            available_spare_threshold: g("available_spare_threshold") as u32,
            percentage_used: g("percentage_used") as u32,
            data_units_read: g("data_units_read"),
            data_units_written: g("data_units_written"),
            power_cycles: g("power_cycles"),
            power_on_hours: g("power_on_hours"),
            unsafe_shutdowns: g("unsafe_shutdowns"),
            media_errors: g("media_errors"),
            num_err_log_entries: g("num_err_log_entries"),
            warning_temp_time: g("warning_temp_time"),
            critical_comp_time: g("critical_comp_time"),
        };
        // 1 unidade de dados NVMe = 1000 setores de 512 bytes
        r.host_written_bytes = Some(h.data_units_written * 512_000);
        r.host_read_bytes = Some(h.data_units_read * 512_000);
        r.power_on_hours = r.power_on_hours.or(Some(h.power_on_hours));
        r.power_cycles = r.power_cycles.or(Some(h.power_cycles));
        r.temperature = r.temperature.or(h.temperature);
        r.nvme = Some(h);
    }

    // Estatísticas de dispositivo ATA (GP log 0x04)
    for page in v["ata_device_statistics"]["pages"].as_array().into_iter().flatten() {
        for e in page["table"].as_array().into_iter().flatten() {
            let name = e["name"].as_str().unwrap_or("");
            if name.contains("Percentage Used Endurance") {
                r.ata_endurance_used = e["value"].as_u64().map(|x| x as u32);
            }
            if name == "Logical Sectors Written" && r.host_written_bytes.is_none() {
                r.host_written_bytes = e["value"].as_u64().map(|x| x * 512);
            }
            if name == "Logical Sectors Read" && r.host_read_bytes.is_none() {
                r.host_read_bytes = e["value"].as_u64().map(|x| x * 512);
            }
        }
    }
    if r.host_written_bytes.is_none() {
        // Atributo 241 (Total_LBAs_Written) – unidade varia entre fabricantes.
        if let Some(a) = r.attr(241) {
            let lname = a.name.to_lowercase();
            let unit: u64 = if lname.contains("gib") || lname.contains("_32mib") {
                if lname.contains("gib") { 1 << 30 } else { 32 << 20 }
            } else if lname.contains("lbas") {
                512
            } else {
                0
            };
            if unit > 0 {
                r.host_written_bytes = Some(a.raw.saturating_mul(unit));
            }
        }
    }

    r.ata_error_count = v["ata_smart_error_log"]["extended"]["count"]
        .as_u64()
        .or_else(|| v["ata_smart_error_log"]["summary"]["count"].as_u64());

    let tests = v["ata_smart_self_test_log"]["extended"]["table"]
        .as_array()
        .or_else(|| v["ata_smart_self_test_log"]["standard"]["table"].as_array());
    for t in tests.into_iter().flatten() {
        r.self_tests.push(SelfTestEntry {
            kind: t["type"]["string"].as_str().unwrap_or("?").to_string(),
            status: t["status"]["string"].as_str().unwrap_or("?").to_string(),
            passed: t["status"]["passed"].as_bool(),
            hours: t["lifetime_hours"].as_u64(),
        });
    }
    for t in v["nvme_self_test_log"]["table"].as_array().into_iter().flatten() {
        r.self_tests.push(SelfTestEntry {
            kind: t["self_test_code"]["string"].as_str().unwrap_or("?").to_string(),
            status: t["self_test_result"]["string"].as_str().unwrap_or("?").to_string(),
            passed: t["self_test_result"]["value"].as_u64().map(|x| x == 0),
            hours: t["power_on_hours"].as_u64(),
        });
    }
    let st = &v["ata_smart_data"]["self_test"]["status"];
    if st["remaining_percent"].is_u64() {
        r.self_test_running = st["remaining_percent"].as_u64().map(|x| x as u32);
    }
    if let Some(p) = v["nvme_self_test_log"]["current_self_test_completion_percent"].as_u64() {
        r.self_test_running = Some(100u32.saturating_sub(p as u32));
    }
    r.raw_json = Some(v);
    r
}

#[derive(Debug, Clone, Copy)]
pub enum SelfTest {
    Short,
    Long,
    Abort,
}

/// Dispara um autoteste SMART (executado pelo próprio firmware do disco).
pub fn start_self_test(dev: &Device, kind: SelfTest) -> Result<String, String> {
    let sc = find_smartctl().ok_or("smartctl não encontrado")?;
    let arg = match kind {
        SelfTest::Short => "-t short",
        SelfTest::Long => "-t long",
        SelfTest::Abort => "-X",
    };
    let mut cmd = Command::new(sc);
    cmd.args(arg.split(' ')).arg(&dev.smart_path);
    hide_window(&mut cmd);
    let out = cmd.output().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let tail: String = text.lines().skip(3).collect::<Vec<_>>().join("\n");
    if out.status.code().unwrap_or(0) & 0b11 != 0 {
        Err(tail)
    } else {
        Ok(tail)
    }
}

// ------------------------------------------------------------ eMMC / UFS (Android)

fn life_to_pct(s: &str) -> Option<u32> {
    // 0x01 = 0-10% usado ... 0x0A = 90-100%, 0x0B = excedido
    let v = u32::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok()?;
    (1..=11).contains(&v).then_some(v * 10)
}

pub fn read_flash_sysfs(sys: &Path, kind: DriveKind) -> Option<SmartReport> {
    let dev = sys.join("device");
    if kind == DriveKind::Emmc || kind == DriveKind::SdCard {
        let lt = read_trim(dev.join("life_time"));
        let eol = read_trim(dev.join("pre_eol_info"));
        if lt.is_none() && eol.is_none() {
            return None;
        }
        let mut f = FlashLife::default();
        if let Some(lt) = lt {
            let mut it = lt.split_whitespace();
            f.used_pct_a = it.next().and_then(life_to_pct);
            f.used_pct_b = it.next().and_then(life_to_pct);
        }
        f.pre_eol = eol.and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok());
        let mut r = SmartReport::empty(SmartSource::EmmcSysfs);
        r.ok = true;
        r.model = read_trim(dev.join("name"));
        r.serial = read_trim(dev.join("serial"));
        r.firmware = read_trim(dev.join("fwrev"));
        r.flash = Some(f);
        return Some(r);
    }
    // UFS: procura health_descriptor nos controladores ufshcd
    let roots = ["/sys/bus/platform/drivers/ufshcd", "/sys/class/ufs"];
    for root in roots {
        let Ok(rd) = std::fs::read_dir(root) else { continue };
        for e in rd.flatten() {
            let hd = e.path().join("health_descriptor");
            if !hd.is_dir() {
                continue;
            }
            let mut f = FlashLife::default();
            f.used_pct_a = read_trim(hd.join("life_time_estimation_a")).and_then(|s| life_to_pct(&s));
            f.used_pct_b = read_trim(hd.join("life_time_estimation_b")).and_then(|s| life_to_pct(&s));
            f.pre_eol = read_trim(hd.join("eol_info"))
                .and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok());
            let mut r = SmartReport::empty(SmartSource::UfsSysfs);
            r.ok = true;
            r.flash = Some(f);
            return Some(r);
        }
    }
    None
}

// ------------------------------------------------------------ Windows fallback

#[allow(dead_code)]
fn windows_reliability(dev: &Device) -> Option<SmartReport> {
    let n = dev.name.trim_start_matches("PhysicalDrive");
    let script = format!(
        "$ErrorActionPreference='SilentlyContinue'; Get-PhysicalDisk | Where-Object DeviceId -eq '{n}' | \
         Get-StorageReliabilityCounter | Select-Object Temperature,TemperatureMax,Wear,PowerOnHours,StartStopCycleCount,\
         ReadErrorsUncorrected,ReadErrorsCorrected,WriteErrorsUncorrected,ReadLatencyMax,WriteLatencyMax,LoadUnloadCycleCount | ConvertTo-Json"
    );
    let out = crate::util::run("powershell", &["-NoProfile", "-NonInteractive", "-Command", &script])?;
    Some(parse_windows_reliability(&out, dev))
}

pub fn parse_windows_reliability(json: &str, dev: &Device) -> SmartReport {
    let mut r = SmartReport::empty(SmartSource::WindowsReliability);
    let Ok(v) = serde_json::from_str::<Value>(json.trim()) else {
        r.messages.push("Contadores de confiabilidade indisponíveis (execute como Administrador).".into());
        return r;
    };
    r.ok = true;
    r.model = Some(dev.model.clone());
    r.serial = dev.serial.clone();
    r.firmware = dev.firmware.clone();
    r.temperature = v["Temperature"].as_i64().filter(|t| *t > 0);
    r.power_on_hours = v["PowerOnHours"].as_u64();
    r.power_cycles = v["StartStopCycleCount"].as_u64();
    r.smart_passed = match dev.os_health.as_deref() {
        Some("Saudável") => Some(true),
        Some("Com falha") => Some(false),
        _ => None,
    };
    if let Some(w) = v["Wear"].as_u64() {
        if dev.kind.is_flash() {
            r.ata_endurance_used = Some(w as u32);
        }
    }
    for k in [
        "ReadErrorsUncorrected",
        "WriteErrorsUncorrected",
        "ReadErrorsCorrected",
        "ReadLatencyMax",
        "WriteLatencyMax",
        "LoadUnloadCycleCount",
        "TemperatureMax",
    ] {
        if let Some(x) = v[k].as_u64() {
            r.extra.push((k.to_string(), x.to_string()));
        }
    }
    r
}

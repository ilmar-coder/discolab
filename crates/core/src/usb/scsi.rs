//! Comandos SCSI sobre BOT, SMART via SAT (ATA PASS-THROUGH) e passthrough NVMe
//! das pontes USB mais comuns (Realtek RTL9210/9220, JMicron JMS583, ASMedia ASM2362).
//! Os formatos de comando seguem o que o smartmontools usa (-d sat, sntrealtek,
//! sntjmicron, sntasmedia). Toda resposta é validada (checksum/plausibilidade) antes
//! de ser usada, porque pontes que não suportam um comando às vezes devolvem lixo.

use serde_json::{Value, json};
use std::io;

use super::bot::{Bot, CmdResult, Dir, Transport};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtaPt {
    Sat16,
    Sat12,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NvmeBridge {
    Realtek,
    JMicron,
    ASMedia,
}

impl NvmeBridge {
    pub fn label(self) -> &'static str {
        match self {
            NvmeBridge::Realtek => "Realtek RTL92xx",
            NvmeBridge::JMicron => "JMicron JMS58x",
            NvmeBridge::ASMedia => "ASMedia ASM236x",
        }
    }
}

pub struct Scsi<T: Transport> {
    pub bot: Bot<T>,
    pub vid: u16,
    pub pid: u16,
    pub block_size: u32,
    pub blocks: u64,
    pub vendor: String,
    pub product: String,
    pub revision: String,
    ata: Option<AtaPt>,
    nvme: Option<NvmeBridge>,
}

fn be_u32(b: &[u8]) -> u32 {
    u32::from_be_bytes(b[..4].try_into().unwrap())
}
fn be_u64(b: &[u8]) -> u64 {
    u64::from_be_bytes(b[..8].try_into().unwrap())
}
fn le_u64(b: &[u8]) -> u64 {
    u64::from_le_bytes(b[..8].try_into().unwrap())
}
fn le_u32(b: &[u8]) -> u32 {
    u32::from_le_bytes(b[..4].try_into().unwrap())
}
fn ascii(b: &[u8]) -> String {
    String::from_utf8_lossy(b).chars().filter(|c| !c.is_control()).collect::<String>().trim().to_string()
}
/// Strings ATA vêm com bytes trocados em cada palavra de 16 bits.
pub fn ata_string(id: &[u8], w0: usize, w1: usize) -> String {
    let mut v = Vec::new();
    for w in w0..=w1 {
        v.push(id[2 * w + 1]);
        v.push(id[2 * w]);
    }
    ascii(&v)
}
fn word(id: &[u8], w: usize) -> u16 {
    u16::from_le_bytes([id[2 * w], id[2 * w + 1]])
}
pub fn checksum_ok(b: &[u8]) -> bool {
    b.iter().fold(0u8, |a, x| a.wrapping_add(*x)) == 0
}

const T_SHORT: u32 = 10_000;

impl<T: Transport> Scsi<T> {
    pub fn new(t: T, vid: u16, pid: u16) -> Self {
        Scsi {
            bot: Bot::new(t),
            vid,
            pid,
            block_size: 512,
            blocks: 0,
            vendor: String::new(),
            product: String::new(),
            revision: String::new(),
            ata: None,
            nvme: None,
        }
    }

    pub fn size_bytes(&self) -> u64 {
        self.blocks * self.block_size as u64
    }

    /// INQUIRY + TEST UNIT READY + READ CAPACITY.
    pub fn init(&mut self) -> io::Result<()> {
        let mut inq = [0u8; 36];
        let r = self.bot.cmd(&[0x12, 0, 0, 0, 36, 0], Dir::In(&mut inq), T_SHORT)?;
        if r.ok() && r.transferred >= 36 {
            self.vendor = ascii(&inq[8..16]);
            self.product = ascii(&inq[16..32]);
            self.revision = ascii(&inq[32..36]);
        }
        // Alguns adaptadores precisam de algumas tentativas logo após conectar.
        for _ in 0..5 {
            let r = self.bot.cmd(&[0x00, 0, 0, 0, 0, 0], Dir::None, T_SHORT)?;
            if r.ok() {
                break;
            }
            let mut sense = [0u8; 18];
            let _ = self.bot.cmd(&[0x03, 0, 0, 0, 18, 0], Dir::In(&mut sense), T_SHORT);
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
        let mut cap = [0u8; 8];
        let r = self.bot.cmd(&[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0], Dir::In(&mut cap), T_SHORT)?;
        if !r.ok() {
            return Err(io::Error::other("READ CAPACITY falhou (sem mídia?)"));
        }
        let last = be_u32(&cap[0..4]);
        self.block_size = be_u32(&cap[4..8]).max(512);
        self.blocks = last as u64 + 1;
        if last == 0xFFFF_FFFF {
            let mut c16 = [0u8; 32];
            let cdb = [0x9E, 0x10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 32, 0, 0];
            let r = self.bot.cmd(&cdb, Dir::In(&mut c16), T_SHORT)?;
            if r.ok() {
                self.blocks = be_u64(&c16[0..8]) + 1;
                self.block_size = be_u32(&c16[8..12]).max(512);
            }
        }
        Ok(())
    }

    /// Lê `buf.len()` bytes a partir de `off` (ambos múltiplos do tamanho de bloco).
    pub fn read_bytes(&mut self, buf: &mut [u8], off: u64, timeout_ms: u32) -> io::Result<()> {
        let bs = self.block_size as u64;
        if off % bs != 0 || buf.len() as u64 % bs != 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "leitura desalinhada"));
        }
        let lba = off / bs;
        let count = buf.len() as u64 / bs;
        let cdb: Vec<u8> = if lba + count <= u32::MAX as u64 && count <= 0xFFFF {
            let mut c = vec![0x28, 0];
            c.extend_from_slice(&(lba as u32).to_be_bytes());
            c.push(0);
            c.extend_from_slice(&(count as u16).to_be_bytes());
            c.push(0);
            c
        } else {
            let mut c = vec![0x88, 0];
            c.extend_from_slice(&lba.to_be_bytes());
            c.extend_from_slice(&(count as u32).to_be_bytes());
            c.extend_from_slice(&[0, 0]);
            c
        };
        let len = buf.len();
        let r = self.bot.cmd(&cdb, Dir::In(buf), timeout_ms)?;
        if !r.ok() {
            return Err(io::Error::other(format!("erro de leitura no LBA {lba}")));
        }
        if r.transferred < len {
            return Err(io::Error::other(format!("leitura incompleta no LBA {lba}")));
        }
        Ok(())
    }

    // ------------------------------------------------------------- ATA (SAT)

    fn ata_cdb(kind: AtaPt, cmd: u8, feature: u8, count: u8, lba_low: u8, lba_mid: u8, lba_high: u8) -> Vec<u8> {
        // protocolo 4 = PIO data-in; byte 2: T_DIR=1, BYTE_BLOCK=1, T_LENGTH=2 (no campo count)
        match kind {
            AtaPt::Sat16 => vec![0x85, 4 << 1, 0x0E, 0, feature, 0, count, 0, lba_low, 0, lba_mid, 0, lba_high, 0xA0, cmd, 0],
            AtaPt::Sat12 => vec![0xA1, 4 << 1, 0x0E, feature, count, lba_low, lba_mid, lba_high, 0xA0, cmd, 0, 0],
        }
    }

    fn ata_in(&mut self, cmd: u8, feature: u8, lba_low: u8, lba_mid: u8, lba_high: u8) -> Option<[u8; 512]> {
        let kinds: Vec<AtaPt> = match self.ata {
            Some(k) => vec![k],
            None => vec![AtaPt::Sat16, AtaPt::Sat12],
        };
        for k in kinds {
            let mut buf = [0u8; 512];
            let cdb = Self::ata_cdb(k, cmd, feature, 1, lba_low, lba_mid, lba_high);
            if let Ok(CmdResult { status: 0, transferred: 512, .. }) = self.bot.cmd(&cdb, Dir::In(&mut buf), T_SHORT) {
                if buf.iter().any(|b| *b != 0) {
                    self.ata = Some(k);
                    return Some(buf);
                }
            }
        }
        None
    }

    pub fn ata_identify(&mut self) -> Option<[u8; 512]> {
        let id = self.ata_in(0xEC, 0, 0, 0, 0)?;
        // Palavra 255: assinatura 0xA5 + checksum (quando presente).
        if id[510] == 0xA5 && !checksum_ok(&id) {
            return None;
        }
        let model = ata_string(&id, 27, 46);
        (!model.is_empty()).then_some(id)
    }

    pub fn ata_smart(&mut self) -> Option<([u8; 512], Option<[u8; 512]>)> {
        let data = self.ata_in(0xB0, 0xD0, 0, 0x4F, 0xC2).filter(|d| checksum_ok(d))?;
        let th = self.ata_in(0xB0, 0xD1, 0, 0x4F, 0xC2).filter(|d| checksum_ok(d));
        Some((data, th))
    }

    // ------------------------------------------------------------- NVMe

    fn nvme_in(&mut self, br: NvmeBridge, opcode: u8, nsid: u32, cdw10: u32, buf: &mut [u8]) -> io::Result<bool> {
        let size = buf.len();
        match br {
            NvmeBridge::Realtek => {
                let mut c = [0u8; 16];
                c[0] = 0xE4;
                c[1..3].copy_from_slice(&(size as u16).to_le_bytes());
                c[3] = opcode;
                c[4] = cdw10 as u8;
                let r = self.bot.cmd(&c, Dir::In(buf), T_SHORT)?;
                Ok(r.ok() && r.transferred == size)
            }
            NvmeBridge::ASMedia => {
                let mut c = [0u8; 16];
                c[0] = 0xE6;
                c[1] = opcode;
                c[3] = cdw10 as u8;
                c[7] = (cdw10 >> 16) as u8;
                let r = self.bot.cmd(&c, Dir::In(buf), T_SHORT)?;
                Ok(r.ok() && r.transferred == size)
            }
            NvmeBridge::JMicron => {
                let mut cmd = [0u8; 512];
                cmd[0..4].copy_from_slice(&0x454D_564Eu32.to_le_bytes()); // "NVME"
                cmd[8..12].copy_from_slice(&(opcode as u32).to_le_bytes());
                cmd[12..16].copy_from_slice(&nsid.to_le_bytes());
                cmd[48..52].copy_from_slice(&cdw10.to_le_bytes());
                let mut c = [0u8; 12];
                c[0] = 0xA1;
                c[1] = 0x80; // envio do comando NVMe
                c[3..5].copy_from_slice(&512u16.to_be_bytes());
                let r = self.bot.cmd(&c, Dir::Out(&cmd), T_SHORT)?;
                if !r.ok() {
                    return Ok(false);
                }
                c[1] = 0x80 | 0x02; // DMA in
                c[3..5].copy_from_slice(&(size as u16).to_be_bytes());
                let r = self.bot.cmd(&c, Dir::In(buf), T_SHORT)?;
                Ok(r.ok() && r.transferred == size)
            }
        }
    }

    fn bridge_order(&self) -> Vec<NvmeBridge> {
        use NvmeBridge::*;
        if let Some(b) = self.nvme {
            return vec![b];
        }
        match self.vid {
            0x0BDA => vec![Realtek],
            0x152D => vec![JMicron],
            0x174C => vec![ASMedia],
            _ => vec![Realtek, ASMedia, JMicron],
        }
    }

    /// (identify controller, SMART/Health log) via ponte NVMe.
    pub fn nvme_smart(&mut self) -> Option<(NvmeBridge, Option<Vec<u8>>, [u8; 512])> {
        for br in self.bridge_order() {
            let mut log = [0u8; 512];
            // Get Log Page (0x02), LID 0x02, NUMDL = 127 (512 bytes)
            match self.nvme_in(br, 0x02, 0xFFFF_FFFF, 0x007F_0002, &mut log) {
                Ok(true) if nvme_log_plausible(&log) => {}
                _ => continue,
            }
            self.nvme = Some(br);
            let mut id = vec![0u8; 4096];
            let id = match self.nvme_in(br, 0x06, 0, 1, &mut id) {
                Ok(true) if !ascii(&id[24..64]).is_empty() => Some(id),
                _ => None,
            };
            return Some((br, id, log));
        }
        None
    }
}

pub fn nvme_log_plausible(log: &[u8]) -> bool {
    let temp_k = u16::from_le_bytes([log[1], log[2]]);
    let spare = log[3];
    let thresh = log[4];
    log.iter().any(|b| *b != 0) && (250..=400).contains(&temp_k) && spare <= 100 && thresh <= 100 && le_u64(&log[128..136]) < 2_000_000
}

pub fn ata_attr_name(id: u8, ssd: bool) -> &'static str {
    match id {
        1 => "Raw_Read_Error_Rate",
        2 => "Throughput_Performance",
        3 => "Spin_Up_Time",
        4 => "Start_Stop_Count",
        5 => "Reallocated_Sector_Ct",
        7 => "Seek_Error_Rate",
        8 => "Seek_Time_Performance",
        9 => "Power_On_Hours",
        10 => "Spin_Retry_Count",
        11 => "Calibration_Retry_Count",
        12 => "Power_Cycle_Count",
        160 => "Uncorrectable_Error_Cnt",
        161 => "Valid_Spare_Block_Cnt",
        163 => "Initial_Bad_Block_Count",
        164 => "Total_Erase_Count",
        165 => "Max_Erase_Count",
        166 => "Min_Erase_Count",
        167 => "Average_Erase_Count",
        168 => "Max_Erase_Count_of_Spec",
        169 => "Remaining_Lifetime_Perc",
        170 => "Available_Reservd_Space",
        171 => "Program_Fail_Count",
        172 => "Erase_Fail_Count",
        173 => "Ave_Block-Erase_Count",
        174 => "Unexpect_Power_Loss_Ct",
        175 => "Program_Fail_Count_Chip",
        176 => "Erase_Fail_Count_Chip",
        177 => "Wear_Leveling_Count",
        178 => "Used_Rsvd_Blk_Cnt_Chip",
        179 => "Used_Rsvd_Blk_Cnt_Tot",
        180 => "Unused_Rsvd_Blk_Cnt_Tot",
        181 => "Program_Fail_Cnt_Total",
        182 => "Erase_Fail_Count_Total",
        183 => "Runtime_Bad_Block",
        184 => "End-to-End_Error",
        187 => "Reported_Uncorrect",
        188 => "Command_Timeout",
        189 => "High_Fly_Writes",
        190 => "Airflow_Temperature_Cel",
        191 => "G-Sense_Error_Rate",
        192 => "Power-Off_Retract_Count",
        193 => "Load_Cycle_Count",
        194 => "Temperature_Celsius",
        195 => "Hardware_ECC_Recovered",
        196 => "Reallocated_Event_Count",
        197 => "Current_Pending_Sector",
        198 => "Offline_Uncorrectable",
        199 => "UDMA_CRC_Error_Count",
        200 => "Multi_Zone_Error_Rate",
        202 => "Percent_Lifetime_Remain",
        206 => "Flying_Height",
        220 => "Disk_Shift",
        222 => "Loaded_Hours",
        223 => "Load_Retry_Count",
        224 => "Load_Friction",
        225 => "Load_Cycle_Count",
        226 => "Load-in_Time",
        230 => if ssd { "Media_Wearout_Indicator" } else { "Head_Amplitude" },
        231 => if ssd { "SSD_Life_Left" } else { "Temperature_Celsius" },
        232 => "Available_Reservd_Space",
        233 => "Media_Wearout_Indicator",
        234 => "Thermal_Throttle",
        235 => "Good_Block_Count",
        240 => "Head_Flying_Hours",
        241 => "Total_LBAs_Written",
        242 => "Total_LBAs_Read",
        _ => "Unknown_Attribute",
    }
}

/// Monta um JSON no mesmo formato do `smartctl --json` a partir dos dados ATA crus,
/// para reutilizar todo o parser e o motor de diagnóstico.
pub fn ata_to_json(id: &[u8; 512], smart: &[u8; 512], thresh: Option<&[u8; 512]>) -> Value {
    let model = ata_string(id, 27, 46);
    let serial = ata_string(id, 10, 19);
    let fw = ata_string(id, 23, 26);
    let lba48 = word(id, 83) & (1 << 10) != 0;
    let sectors = if lba48 {
        le_u64(&id[200..208])
    } else {
        le_u32(&id[120..124]) as u64
    };
    let rot = word(id, 217);
    let ssd = rot == 1;
    let mut table = Vec::new();
    let mut temp = None;
    let mut poh = None;
    let mut cycles = None;
    for i in 0..30 {
        let e = &smart[2 + i * 12..2 + i * 12 + 12];
        let aid = e[0];
        if aid == 0 {
            continue;
        }
        let flags = u16::from_le_bytes([e[1], e[2]]);
        let value = e[3];
        let worst = e[4];
        let mut rb = [0u8; 8];
        rb[..6].copy_from_slice(&e[5..11]);
        let raw = u64::from_le_bytes(rb);
        let th = thresh
            .and_then(|t| (0..30).map(|j| &t[2 + j * 12..2 + j * 12 + 12]).find(|x| x[0] == aid).map(|x| x[1]))
            .unwrap_or(0);
        let raw_str = match aid {
            194 | 190 => {
                let t = raw & 0xFF;
                if aid == 194 || temp.is_none() {
                    temp = Some(t);
                }
                t.to_string()
            }
            9 => {
                poh = Some(raw & 0xFFFF_FFFF);
                (raw & 0xFFFF_FFFF).to_string()
            }
            12 => {
                cycles = Some(raw);
                raw.to_string()
            }
            _ => raw.to_string(),
        };
        let when_failed = if th > 0 && value <= th {
            "now"
        } else if th > 0 && worst <= th {
            "past"
        } else {
            ""
        };
        table.push(json!({
            "id": aid, "name": ata_attr_name(aid, ssd), "value": value, "worst": worst, "thresh": th,
            "flags": {"prefailure": flags & 1 != 0},
            "raw": {"value": raw, "string": raw_str}, "when_failed": when_failed
        }));
    }
    let mut v = json!({
        "smartctl": {"exit_status": 0},
        "device": {"protocol": "ATA"},
        "model_name": model, "serial_number": serial, "firmware_version": fw,
        "user_capacity": {"bytes": sectors * 512},
        "rotation_rate": if ssd { 0 } else { rot as u64 },
        "ata_smart_attributes": {"table": table},
    });
    if let Some(t) = temp {
        v["temperature"] = json!({"current": t});
    }
    if let Some(h) = poh {
        v["power_on_time"] = json!({"hours": h});
    }
    if let Some(c) = cycles {
        v["power_cycle_count"] = json!(c);
    }
    v
}

pub fn nvme_to_json(id: Option<&[u8]>, log: &[u8; 512], fallback_model: &str, capacity: u64) -> Value {
    let g = |o: usize| le_u64(&log[o..o + 8]);
    let mut v = json!({
        "smartctl": {"exit_status": 0},
        "device": {"protocol": "NVMe"},
        "model_name": fallback_model,
        "user_capacity": {"bytes": capacity},
        "nvme_smart_health_information_log": {
            "critical_warning": log[0],
            "temperature": u16::from_le_bytes([log[1], log[2]]) as i64 - 273,
            "available_spare": log[3],
            "available_spare_threshold": log[4],
            "percentage_used": log[5],
            "data_units_read": g(32),
            "data_units_written": g(48),
            "host_reads": g(64),
            "host_writes": g(80),
            "controller_busy_time": g(96),
            "power_cycles": g(112),
            "power_on_hours": g(128),
            "unsafe_shutdowns": g(144),
            "media_errors": g(160),
            "num_err_log_entries": g(176),
            "warning_temp_time": le_u32(&log[192..196]),
            "critical_comp_time": le_u32(&log[196..200]),
        }
    });
    if let Some(id) = id {
        v["serial_number"] = json!(ascii(&id[4..24]));
        v["model_name"] = json!(ascii(&id[24..64]));
        v["firmware_version"] = json!(ascii(&id[64..72]));
    }
    v
}

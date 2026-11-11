//! Simula um adaptador USB Mass Storage (protocolo BOT) para testar o caminho USB
//! inteiro sem hardware: CBW/CSW, SCSI, SAT, pontes NVMe e varredura de superfície.

use discolab_core::device::{Device, DriveKind};
use discolab_core::health::{Severity, assess};
use discolab_core::smart::SmartSource;
use discolab_core::surface::{BlockRead, ScanConfig, ScanEvent, scan_source};
use discolab_core::usb::bot::{Transport, build_cbw};
use discolab_core::usb::scsi::Scsi;
use discolab_core::usb::smart_from_scsi;
use discolab_core::usb::usbfs::parse_descriptors;
use std::io;
use std::sync::atomic::AtomicBool;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Sat16,
    Sat12Only,
    NvmeRealtek,
    NvmeAsmedia,
    NvmeJmicron,
    NoPassthrough,
}

struct Mock {
    kind: Kind,
    blocks: u64,
    bad_lba: Option<(u64, u64)>,
    /// Comandos sem suporte: travar o endpoint (true) ou só devolver CSW de falha.
    stall_unsupported: bool,
    reallocated: u64,
    pending: u64,
    // estado
    tag: u32,
    data: Vec<u8>,
    pos: usize,
    status: u8,
    expect_out: usize,
    pending_out: usize,
    stall_next: bool,
    jm_cmd: Option<Vec<u8>>,
    pub cmds: Vec<u8>,
}

fn swap_str(id: &mut [u8], w0: usize, w1: usize, s: &str) {
    let mut b: Vec<u8> = s.bytes().collect();
    b.resize((w1 - w0 + 1) * 2, b' ');
    for (i, w) in (w0..=w1).enumerate() {
        id[2 * w] = b[2 * i + 1];
        id[2 * w + 1] = b[2 * i];
    }
}

fn finish_checksum(b: &mut [u8; 512]) {
    let s = b[..511].iter().fold(0u8, |a, x| a.wrapping_add(*x));
    b[511] = 0u8.wrapping_sub(s);
}

impl Mock {
    fn new(kind: Kind) -> Self {
        Mock {
            kind,
            blocks: 2048,
            bad_lba: None,
            stall_unsupported: true,
            reallocated: 0,
            pending: 0,
            tag: 0,
            data: vec![],
            pos: 0,
            status: 0,
            expect_out: 0,
            pending_out: 0,
            stall_next: false,
            jm_cmd: None,
            cmds: vec![],
        }
    }

    fn identify(&self) -> Vec<u8> {
        let mut id = [0u8; 512];
        swap_str(&mut id, 10, 19, "SN123456");
        swap_str(&mut id, 23, 26, "FW01");
        swap_str(&mut id, 27, 46, "Samsung SSD 870 EVO 500GB");
        id[83 * 2 + 1] = 0x04; // bit 10 = LBA48
        id[200..208].copy_from_slice(&976_773_168u64.to_le_bytes());
        id[217 * 2] = 1; // SSD
        id[510] = 0xA5;
        finish_checksum(&mut id);
        id.to_vec()
    }

    fn smart(&self, thresholds: bool) -> Vec<u8> {
        let mut d = [0u8; 512];
        let attrs: [(u8, u16, u8, u64, u8); 6] = [
            (5, 0x33, 100, self.reallocated, 10),
            (9, 0x32, 99, 1234, 0),
            (12, 0x32, 99, 56, 0),
            (177, 0x13, 95, 20, 0),
            (194, 0x22, 64, 36, 0),
            (197, 0x32, 100, self.pending, 0),
        ];
        for (i, (aid, flags, val, raw, th)) in attrs.iter().enumerate() {
            let o = 2 + i * 12;
            if thresholds {
                d[o] = *aid;
                d[o + 1] = *th;
            } else {
                d[o] = *aid;
                d[o + 1..o + 3].copy_from_slice(&flags.to_le_bytes());
                d[o + 3] = *val;
                d[o + 4] = *val;
                d[o + 5..o + 11].copy_from_slice(&raw.to_le_bytes()[..6]);
            }
        }
        finish_checksum(&mut d);
        d.to_vec()
    }

    fn nvme_log() -> Vec<u8> {
        let mut l = vec![0u8; 512];
        l[1..3].copy_from_slice(&310u16.to_le_bytes()); // 37 °C
        l[3] = 100;
        l[4] = 10;
        l[5] = 7;
        l[48..56].copy_from_slice(&20_000_000u64.to_le_bytes());
        l[112..120].copy_from_slice(&300u64.to_le_bytes());
        l[128..136].copy_from_slice(&4321u64.to_le_bytes());
        l
    }

    fn nvme_id() -> Vec<u8> {
        let mut id = vec![b' '; 4096];
        id[4..12].copy_from_slice(b"NVSERIAL");
        let m = b"WD_BLACK SN770 1TB";
        id[24..24 + m.len()].copy_from_slice(m);
        id[64..72].copy_from_slice(b"731100WD");
        id
    }

    fn unsupported(&mut self) {
        self.data.clear();
        self.status = 1;
        self.stall_next = self.stall_unsupported;
    }

    fn handle(&mut self, cdb: &[u8], len: usize) {
        self.cmds.push(cdb[0]);
        self.data.clear();
        self.pos = 0;
        self.status = 0;
        self.expect_out = 0;
        let nvme_kind = matches!(self.kind, Kind::NvmeRealtek | Kind::NvmeAsmedia | Kind::NvmeJmicron);
        match cdb[0] {
            0x12 => {
                let mut d = vec![0u8; 36];
                d[8..16].copy_from_slice(b"JMicron ");
                d[16..32].copy_from_slice(b"Generic         ");
                d[32..36].copy_from_slice(b"0508");
                self.data = d;
            }
            0x00 | 0x03 => self.data = vec![0; if cdb[0] == 3 { 18 } else { 0 }],
            0x25 => {
                let mut d = ((self.blocks - 1) as u32).to_be_bytes().to_vec();
                d.extend_from_slice(&512u32.to_be_bytes());
                self.data = d;
            }
            0x28 => {
                let lba = u32::from_be_bytes(cdb[2..6].try_into().unwrap()) as u64;
                let n = u16::from_be_bytes(cdb[7..9].try_into().unwrap()) as u64;
                if let Some((a, b)) = self.bad_lba {
                    if lba < b && lba + n > a {
                        // setor ilegível: sem dados, CSW com falha
                        self.unsupported();
                        return;
                    }
                }
                self.data = vec![0xAB; (n * 512) as usize];
            }
            0x85 | 0xA1 if !nvme_kind || (cdb[0] == 0xA1 && self.kind == Kind::NvmeJmicron) => {
                if self.kind == Kind::NvmeJmicron {
                    // passthrough JMicron NVMe
                    match cdb[1] {
                        0x80 => self.expect_out = len,
                        0x82 => {
                            let cmd = self.jm_cmd.take().unwrap_or_default();
                            let op = cmd.get(8).copied().unwrap_or(0);
                            self.data = if op == 0x02 { Self::nvme_log() } else { Self::nvme_id() };
                        }
                        _ => self.unsupported(),
                    }
                    return;
                }
                if self.kind == Kind::NoPassthrough || (cdb[0] == 0x85 && self.kind == Kind::Sat12Only) {
                    self.unsupported();
                    return;
                }
                let (feature, cmd) = if cdb[0] == 0x85 { (cdb[4], cdb[14]) } else { (cdb[3], cdb[9]) };
                self.data = match (cmd, feature) {
                    (0xEC, _) => self.identify(),
                    (0xB0, 0xD0) => self.smart(false),
                    (0xB0, 0xD1) => self.smart(true),
                    _ => {
                        self.unsupported();
                        return;
                    }
                };
            }
            0xE4 if self.kind == Kind::NvmeRealtek => {
                self.data = if cdb[3] == 0x02 { Self::nvme_log() } else { Self::nvme_id() };
            }
            0xE6 if self.kind == Kind::NvmeAsmedia => {
                assert_eq!(cdb[7], 0x7F * u8::from(cdb[1] == 0x02), "NUMDL");
                self.data = if cdb[1] == 0x02 { Self::nvme_log() } else { Self::nvme_id() };
            }
            _ => self.unsupported(),
        }
        self.data.truncate(len);
    }
}

impl Transport for Mock {
    fn bulk_out(&mut self, data: &[u8], _: u32) -> io::Result<usize> {
        if self.pending_out > 0 {
            self.pending_out = 0;
            if self.status != 0 {
                // comando recusado: o adaptador trava o endpoint OUT
                return Err(io::Error::from_raw_os_error(32));
            }
            if self.expect_out > 0 {
                self.jm_cmd = Some(data.to_vec());
                self.expect_out = 0;
            }
            return Ok(data.len());
        }
        assert_eq!(data.len(), 31, "CBW deve ter 31 bytes");
        assert_eq!(&data[0..4], b"USBC");
        self.tag = u32::from_le_bytes(data[4..8].try_into().unwrap());
        let len = u32::from_le_bytes(data[8..12].try_into().unwrap()) as usize;
        let cdb_len = data[14] as usize;
        let cdb = data[15..15 + cdb_len].to_vec();
        self.handle(&cdb, len);
        if data[12] & 0x80 == 0 && len > 0 {
            self.pending_out = len;
        }
        Ok(31)
    }
    fn bulk_in(&mut self, buf: &mut [u8], _: u32) -> io::Result<usize> {
        if self.stall_next {
            self.stall_next = false;
            return Err(io::Error::from_raw_os_error(32));
        }
        if self.pos < self.data.len() {
            let n = (self.data.len() - self.pos).min(buf.len());
            buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
            self.pos += n;
            return Ok(n);
        }
        let mut csw = [0u8; 13];
        csw[0..4].copy_from_slice(b"USBS");
        csw[4..8].copy_from_slice(&self.tag.to_le_bytes());
        csw[12] = self.status;
        let n = 13.min(buf.len());
        buf[..n].copy_from_slice(&csw[..n]);
        self.data.clear();
        self.pos = 0;
        Ok(n)
    }
    fn clear_halt_in(&mut self) -> io::Result<()> {
        Ok(())
    }
    fn clear_halt_out(&mut self) -> io::Result<()> {
        Ok(())
    }
    fn reset_recovery(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn dev() -> Device {
    Device {
        name: "USB".into(),
        smart_path: "usb:/dev/bus/usb/001/002".into(),
        raw_path: "usb:/dev/bus/usb/001/002".into(),
        model: "Disco USB".into(),
        serial: None,
        firmware: None,
        size_bytes: 0,
        kind: DriveKind::Usb,
        bus: "USB-OTG".into(),
        removable: true,
        mount_points: vec![],
        sysfs: None,
        os_health: None,
    }
}

fn scsi(m: Mock, vid: u16) -> Scsi<Mock> {
    let mut s = Scsi::new(m, vid, 0x0001);
    s.init().unwrap();
    s
}

#[test]
fn cbw_layout() {
    let c = build_cbw(7, 512, true, 0, &[0x12, 0, 0, 0, 36, 0]);
    assert_eq!(&c[0..4], b"USBC");
    assert_eq!(u32::from_le_bytes(c[4..8].try_into().unwrap()), 7);
    assert_eq!(c[12], 0x80);
    assert_eq!(c[14], 6);
    assert_eq!(c[15], 0x12);
}

#[test]
fn descriptors_pick_bot_interface_not_uas() {
    // device(18) + config(9) + iface alt0 BOT + 2 eps + iface alt1 UAS + 2 eps
    let mut d = vec![18, 1, 0x10, 0x03, 0, 0, 0, 9, 0xDA, 0x0B, 0x10, 0x92, 0, 0, 1, 2, 3, 1];
    d.extend([9, 2, 0, 0, 1, 1, 0, 0x80, 0x32]);
    d.extend([9, 4, 0, 0, 2, 8, 6, 0x50, 0]);
    d.extend([7, 5, 0x81, 2, 0, 2, 0]);
    d.extend([7, 5, 0x02, 2, 0, 2, 0]);
    d.extend([9, 4, 0, 1, 2, 8, 6, 0x62, 0]);
    d.extend([7, 5, 0x83, 2, 0, 2, 0]);
    d.extend([7, 5, 0x04, 2, 0, 2, 0]);
    let (vid, pid, i) = parse_descriptors(&d).unwrap();
    assert_eq!((vid, pid), (0x0BDA, 0x9210));
    assert_eq!((i.number, i.alt, i.ep_in, i.ep_out), (0, 0, 0x81, 0x02));
}

#[test]
fn sata_ssd_via_sat16() {
    let mut s = scsi(Mock::new(Kind::Sat16), 0x152D);
    assert_eq!(s.size_bytes(), 2048 * 512);
    assert_eq!(s.product, "Generic");
    let r = smart_from_scsi(&mut s);
    assert!(r.ok, "{:?}", r.messages);
    assert_eq!(r.source, SmartSource::UsbDirect);
    assert_eq!(r.model.as_deref(), Some("Samsung SSD 870 EVO 500GB"));
    assert_eq!(r.serial.as_deref(), Some("SN123456"));
    assert_eq!(r.capacity, Some(976_773_168 * 512));
    assert_eq!(r.temperature, Some(36));
    assert_eq!(r.power_on_hours, Some(1234));
    assert_eq!(r.attr(5).unwrap().thresh, 10);
    let a = assess(&dev(), &r, None);
    assert_eq!(a.life_left, Some(95));
    assert!(a.health >= 95);
}

#[test]
fn sat12_fallback_with_stalls() {
    let mut m = Mock::new(Kind::Sat12Only);
    m.pending = 8;
    let mut s = scsi(m, 0x174C);
    let r = smart_from_scsi(&mut s);
    assert!(r.ok, "{:?}", r.messages);
    assert!(s.bot.t.cmds.contains(&0x85) && s.bot.t.cmds.contains(&0xA1));
    let a = assess(&dev(), &r, None);
    assert_eq!(a.status, Severity::Warning);
}

#[test]
fn nvme_bridges() {
    for (k, vid) in [(Kind::NvmeRealtek, 0x0BDA), (Kind::NvmeAsmedia, 0x174C), (Kind::NvmeJmicron, 0x152D), (Kind::NvmeRealtek, 0x1234)] {
        let mut m = Mock::new(k);
        m.stall_unsupported = vid == 0x1234; // fornecedor desconhecido + stalls
        let mut s = scsi(m, vid);
        let r = smart_from_scsi(&mut s);
        assert!(r.ok, "{vid:04x}: {:?}", r.messages);
        let n = r.nvme.as_ref().unwrap();
        assert_eq!(n.percentage_used, 7);
        assert_eq!(r.temperature, Some(37));
        assert_eq!(r.model.as_deref(), Some("WD_BLACK SN770 1TB"));
        assert_eq!(r.power_on_hours, Some(4321));
        let a = assess(&dev(), &r, None);
        assert_eq!(a.life_left, Some(93));
    }
}

#[test]
fn adapter_without_passthrough_still_reports_capacity() {
    let mut s = scsi(Mock::new(Kind::NoPassthrough), 0x05E3);
    let r = smart_from_scsi(&mut s);
    assert!(!r.ok);
    assert_eq!(r.capacity, Some(2048 * 512));
    assert!(r.messages[0].contains("não repassa"));
}

struct MockBlock(Scsi<Mock>);
impl BlockRead for MockBlock {
    fn size(&self) -> u64 {
        self.0.size_bytes()
    }
    fn uncached(&self) -> bool {
        true
    }
    fn align(&self) -> usize {
        512
    }
    fn read_at(&mut self, buf: &mut [u8], off: u64) -> io::Result<()> {
        self.0.read_bytes(buf, off, 1000)
    }
}

#[test]
fn surface_scan_over_usb_finds_bad_sectors() {
    let mut m = Mock::new(Kind::Sat16);
    m.blocks = 8192; // 4 MiB
    m.bad_lba = Some((3000, 3001));
    m.stall_unsupported = false;
    let mut blk = MockBlock(scsi(m, 0x152D));
    let mut cfg = ScanConfig::new("usb:mock", 0, true);
    cfg.block_size = 64 * 1024;
    let mut done = None;
    scan_source(&mut blk, &cfg, &AtomicBool::new(false), &mut |e| {
        if let ScanEvent::Done(s) = e {
            done = Some(s);
        }
    });
    let s = done.unwrap();
    assert_eq!(s.buckets.iter().sum::<u64>(), 64);
    assert_eq!(s.bad_blocks, 1);
    assert_eq!(s.errors[0].0, (3000 * 512 / 65536) * 65536);
}

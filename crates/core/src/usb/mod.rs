//! Acesso direto a discos em adaptadores USB (SATA e NVMe), sem root no Android.
//! Caminhos de dispositivo deste módulo começam com `usb:` (ex.: `usb:/dev/bus/usb/001/002`).

pub mod bot;
pub mod scsi;
pub mod usbfs;

#[cfg(target_os = "android")]
pub mod android;

use crate::device::Device;
use crate::smart::{SmartReport, SmartSource, parse_smartctl};
use crate::surface::{BlockRead, BlockRw};
use std::io;

pub const PREFIX: &str = "usb:";

pub fn is_usb(path: &str) -> bool {
    path.starts_with(PREFIX)
}

/// Gera o relatório SMART a partir de uma sessão SCSI já aberta (testável com simulador).
pub fn smart_from_scsi<T: bot::Transport>(s: &mut scsi::Scsi<T>) -> SmartReport {
    let bridge = format!("{} {} (USB {:04x}:{:04x})", s.vendor, s.product, s.vid, s.pid).trim().to_string();
    let cap = s.size_bytes();
    if let Some(id) = s.ata_identify() {
        if let Some((d, th)) = s.ata_smart() {
            let mut r = parse_smartctl(scsi::ata_to_json(&id, &d, th.as_ref()));
            r.source = SmartSource::UsbDirect;
            r.messages.push(format!("Lido via adaptador SATA/USB (SAT): {bridge}"));
            if th.is_none() {
                r.messages.push("O adaptador não devolveu os limites dos atributos; a análise usa os valores brutos.".into());
            }
            return r;
        }
        let mut r = parse_smartctl(scsi::ata_to_json(&id, &[0u8; 512], None));
        r.source = SmartSource::UsbDirect;
        r.ata.clear();
        r.messages.push("Disco identificado, mas o SMART está desativado ou bloqueado pelo adaptador.".into());
        return r;
    }
    if let Some((br, id, log)) = s.nvme_smart() {
        let fallback = format!("{} {}", s.vendor, s.product);
        let mut r = parse_smartctl(scsi::nvme_to_json(id.as_deref(), &log, fallback.trim(), cap));
        r.source = SmartSource::UsbDirect;
        r.messages.push(format!("Lido via ponte NVMe/USB {}: {bridge}", br.label()));
        return r;
    }
    let mut r = SmartReport::empty(SmartSource::UsbDirect);
    r.model = Some(format!("{} {}", s.vendor, s.product).trim().to_string()).filter(|m| !m.is_empty());
    r.capacity = Some(cap);
    r.messages.push(format!(
        "O adaptador {bridge} não repassa comandos SMART (nem SAT, nem passthrough NVMe conhecido). \
         A varredura de superfície funciona normalmente. Adaptadores com chip JMicron, ASMedia ou Realtek costumam funcionar."
    ));
    r
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod session {
    use super::*;
    use crate::usb::scsi::Scsi;
    use crate::usb::usbfs::UsbFs;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, OnceLock};

    pub type Sess = Arc<Mutex<Scsi<UsbFs>>>;
    static SESSIONS: OnceLock<Mutex<HashMap<String, Sess>>> = OnceLock::new();

    pub fn open(path: &str) -> io::Result<Sess> {
        let map = SESSIONS.get_or_init(Default::default);
        if let Some(s) = map.lock().unwrap().get(path) {
            return Ok(s.clone());
        }
        let name = &path[PREFIX.len()..];
        #[cfg(target_os = "android")]
        let (fd, own) = (crate::usb::android::open_device(name)?, false);
        #[cfg(target_os = "linux")]
        let (fd, own) = {
            use std::os::unix::io::IntoRawFd;
            (std::fs::OpenOptions::new().read(true).write(true).open(name)?.into_raw_fd(), true)
        };
        let (t, vid, pid) = UsbFs::open(fd, own)?;
        let mut s = Scsi::new(t, vid, pid);
        if let Err(e) = s.init() {
            forget(path);
            return Err(e);
        }
        let s = Arc::new(Mutex::new(s));
        map.lock().unwrap().insert(path.to_string(), s.clone());
        Ok(s)
    }

    pub fn forget(path: &str) {
        if let Some(m) = SESSIONS.get() {
            m.lock().unwrap().remove(path);
        }
        #[cfg(target_os = "android")]
        crate::usb::android::forget(&path[PREFIX.len()..]);
    }

    pub struct UsbBlock {
        pub s: Sess,
        pub path: String,
        pub size: u64,
        pub bs: usize,
    }

    impl BlockRead for UsbBlock {
        fn size(&self) -> u64 {
            self.size
        }
        fn uncached(&self) -> bool {
            true
        }
        fn align(&self) -> usize {
            self.bs
        }
        fn read_at(&mut self, buf: &mut [u8], off: u64) -> io::Result<()> {
            let r = self.s.lock().unwrap().read_bytes(buf, off, 30_000);
            if let Err(e) = &r {
                if e.raw_os_error() == Some(libc::ENODEV) {
                    forget(&self.path);
                }
            }
            r
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
impl BlockRw for session::UsbBlock {
    fn write_at(&mut self, buf: &[u8], off: u64) -> io::Result<()> {
        self.s.lock().unwrap().write_bytes(buf, off, 30_000)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.s.lock().unwrap().sync_cache()
    }
}

pub fn open_block_rw(path: &str) -> io::Result<Box<dyn BlockRw>> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let s = session::open(path)?;
        let (size, bs) = {
            let g = s.lock().unwrap();
            (g.size_bytes(), g.block_size as usize)
        };
        Ok(Box::new(session::UsbBlock { s, path: path.to_string(), size, bs }))
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        let _ = path;
        Err(io::Error::new(io::ErrorKind::Unsupported, "acesso USB direto disponível apenas no Linux/Android"))
    }
}

pub fn read_smart_usb(dev: &Device) -> SmartReport {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        match session::open(&dev.smart_path) {
            Ok(s) => {
                let mut g = s.lock().unwrap();
                smart_from_scsi(&mut g)
            }
            Err(e) => {
                let mut r = SmartReport::empty(SmartSource::UsbDirect);
                r.messages.push(format!("Não foi possível acessar o dispositivo USB: {e}"));
                if e.kind() == io::ErrorKind::PermissionDenied {
                    r.messages.push("Toque em \"Permitir\" quando o Android pedir acesso ao dispositivo USB.".into());
                }
                session::forget(&dev.smart_path);
                r
            }
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        let _ = dev;
        let mut r = SmartReport::empty(SmartSource::UsbDirect);
        r.messages.push("Acesso USB direto disponível apenas no Linux/Android.".into());
        r
    }
}

pub fn open_block(path: &str) -> io::Result<Box<dyn BlockRead>> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let s = session::open(path)?;
        let (size, bs) = {
            let g = s.lock().unwrap();
            (g.size_bytes(), g.block_size as usize)
        };
        Ok(Box::new(session::UsbBlock { s, path: path.to_string(), size, bs }))
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        let _ = path;
        Err(io::Error::new(io::ErrorKind::Unsupported, "acesso USB direto disponível apenas no Linux/Android"))
    }
}

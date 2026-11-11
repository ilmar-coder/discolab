//! Transporte via usbfs (Linux/Android). No Android o descritor de arquivo vem de
//! `UsbDeviceConnection.getFileDescriptor()`, obtido após o usuário permitir o acesso.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StorageIface {
    pub number: u8,
    pub alt: u8,
    pub ep_in: u8,
    pub ep_out: u8,
    pub protocol: u8,
}

/// Descritores crus (device + configuração): encontra a interface Mass Storage BOT.
pub fn parse_descriptors(raw: &[u8]) -> Option<(u16, u16, StorageIface)> {
    if raw.len() < 18 || raw[1] != 1 {
        return None;
    }
    let vid = u16::from_le_bytes([raw[8], raw[9]]);
    let pid = u16::from_le_bytes([raw[10], raw[11]]);
    let mut i = raw[0] as usize;
    let mut cur: Option<StorageIface> = None;
    let mut found: Option<StorageIface> = None;
    while i + 2 <= raw.len() {
        let len = raw[i] as usize;
        if len < 2 || i + len > raw.len() {
            break;
        }
        let ty = raw[i + 1];
        if ty == 4 && len >= 9 {
            if let Some(c) = cur.take() {
                if c.ep_in != 0 && c.ep_out != 0 && found.is_none() {
                    found = Some(c);
                }
            }
            let (class, sub, proto) = (raw[i + 5], raw[i + 6], raw[i + 7]);
            // Classe 8 (Mass Storage), subclasse 6 (SCSI), protocolo 0x50 (Bulk-Only).
            if class == 8 && sub == 6 && proto == 0x50 {
                cur = Some(StorageIface { number: raw[i + 2], alt: raw[i + 3], ep_in: 0, ep_out: 0, protocol: proto });
            }
        } else if ty == 5 && len >= 7 {
            if let Some(c) = cur.as_mut() {
                let addr = raw[i + 2];
                if raw[i + 3] & 3 == 2 {
                    if addr & 0x80 != 0 {
                        c.ep_in = addr;
                    } else {
                        c.ep_out = addr;
                    }
                }
            }
        }
        i += len;
    }
    if let Some(c) = cur {
        if c.ep_in != 0 && c.ep_out != 0 && found.is_none() {
            found = Some(c);
        }
    }
    found.map(|f| (vid, pid, f))
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub use imp::UsbFs;

#[cfg(any(target_os = "linux", target_os = "android"))]
mod imp {
    use super::*;
    use crate::usb::bot::Transport;
    use std::io;
    use std::os::raw::{c_int, c_void};
    use std::os::unix::io::RawFd;

    #[repr(C)]
    struct BulkTransfer {
        ep: u32,
        len: u32,
        timeout: u32,
        data: *mut c_void,
    }
    #[repr(C)]
    struct CtrlTransfer {
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        length: u16,
        timeout: u32,
        data: *mut c_void,
    }
    #[repr(C)]
    struct IoctlS {
        ifno: c_int,
        ioctl_code: c_int,
        data: *mut c_void,
    }
    #[repr(C)]
    struct SetIntf {
        interface: u32,
        altsetting: u32,
    }

    const fn ioc(dir: u32, nr: u32, size: usize) -> u32 {
        (dir << 30) | ((size as u32) << 16) | ((b'U' as u32) << 8) | nr
    }
    const USBDEVFS_CONTROL: u32 = ioc(3, 0, std::mem::size_of::<CtrlTransfer>());
    const USBDEVFS_BULK: u32 = ioc(3, 2, std::mem::size_of::<BulkTransfer>());
    const USBDEVFS_SETINTERFACE: u32 = ioc(2, 4, std::mem::size_of::<SetIntf>());
    const USBDEVFS_CLAIMINTERFACE: u32 = ioc(2, 15, 4);
    const USBDEVFS_IOCTL: u32 = ioc(3, 18, std::mem::size_of::<IoctlS>());
    const USBDEVFS_CLEAR_HALT: u32 = ioc(2, 21, 4);
    const USBDEVFS_DISCONNECT: u32 = ioc(0, 22, 0);

    fn check(r: c_int) -> io::Result<c_int> {
        if r < 0 { Err(io::Error::last_os_error()) } else { Ok(r) }
    }

    pub struct UsbFs {
        fd: RawFd,
        iface: StorageIface,
        own_fd: bool,
    }
    // O fd é usado por uma thread por vez (protegido por Mutex na sessão).
    unsafe impl Send for UsbFs {}

    impl UsbFs {
        /// Lê os descritores, desconecta o driver do kernel e reivindica a interface.
        pub fn open(fd: RawFd, own_fd: bool) -> io::Result<(Self, u16, u16)> {
            let mut raw = vec![0u8; 4096];
            let n = unsafe { libc::pread(fd, raw.as_mut_ptr().cast(), raw.len(), 0) };
            if n <= 0 {
                return Err(io::Error::last_os_error());
            }
            raw.truncate(n as usize);
            let (vid, pid, iface) = parse_descriptors(&raw).ok_or_else(|| {
                io::Error::new(io::ErrorKind::Unsupported, "dispositivo USB sem interface Mass Storage Bulk-Only (UAS-only?)")
            })?;
            let mut s = UsbFs { fd, iface, own_fd };
            // Desliga o driver usb-storage do kernel (o Android desmonta o volume).
            let mut req = IoctlS { ifno: iface.number as c_int, ioctl_code: USBDEVFS_DISCONNECT as c_int, data: std::ptr::null_mut() };
            unsafe { libc::ioctl(fd, USBDEVFS_IOCTL as _, &mut req) };
            let mut ifno = iface.number as u32;
            if let Err(e) = check(unsafe { libc::ioctl(fd, USBDEVFS_CLAIMINTERFACE as _, &mut ifno) }) {
                if e.raw_os_error() != Some(libc::EBUSY) {
                    return Err(e);
                }
            }
            if iface.alt != 0 {
                let mut si = SetIntf { interface: iface.number as u32, altsetting: iface.alt as u32 };
                check(unsafe { libc::ioctl(fd, USBDEVFS_SETINTERFACE as _, &mut si) })?;
            }
            s.iface = iface;
            Ok((s, vid, pid))
        }

        fn bulk(&mut self, ep: u8, ptr: *mut c_void, len: usize, timeout_ms: u32) -> io::Result<usize> {
            let mut bt = BulkTransfer { ep: ep as u32, len: len as u32, timeout: timeout_ms, data: ptr };
            check(unsafe { libc::ioctl(self.fd, USBDEVFS_BULK as _, &mut bt) }).map(|n| n as usize)
        }

        fn clear_halt(&mut self, ep: u8) -> io::Result<()> {
            let mut e = ep as u32;
            check(unsafe { libc::ioctl(self.fd, USBDEVFS_CLEAR_HALT as _, &mut e) }).map(|_| ())
        }
    }

    impl Drop for UsbFs {
        fn drop(&mut self) {
            if self.own_fd {
                unsafe { libc::close(self.fd) };
            }
        }
    }

    impl Transport for UsbFs {
        fn bulk_out(&mut self, data: &[u8], timeout_ms: u32) -> io::Result<usize> {
            let ep = self.iface.ep_out;
            self.bulk(ep, data.as_ptr() as *mut c_void, data.len(), timeout_ms)
        }
        fn bulk_in(&mut self, buf: &mut [u8], timeout_ms: u32) -> io::Result<usize> {
            let ep = self.iface.ep_in;
            self.bulk(ep, buf.as_mut_ptr().cast(), buf.len(), timeout_ms)
        }
        fn clear_halt_in(&mut self) -> io::Result<()> {
            let ep = self.iface.ep_in;
            self.clear_halt(ep)
        }
        fn clear_halt_out(&mut self) -> io::Result<()> {
            let ep = self.iface.ep_out;
            self.clear_halt(ep)
        }
        fn reset_recovery(&mut self) -> io::Result<()> {
            // Bulk-Only Mass Storage Reset (classe, interface)
            let mut ct = CtrlTransfer {
                request_type: 0x21,
                request: 0xFF,
                value: 0,
                index: self.iface.number as u16,
                length: 0,
                timeout: 5000,
                data: std::ptr::null_mut(),
            };
            let _ = check(unsafe { libc::ioctl(self.fd, USBDEVFS_CONTROL as _, &mut ct) });
            let _ = self.clear_halt_in();
            let _ = self.clear_halt_out();
            Ok(())
        }
    }
}

//! USB Mass Storage Bulk-Only Transport (BOT): envelopa comandos SCSI em CBW/CSW.

use std::io;

/// Canal bruto de endpoints bulk (usbfs no Linux/Android, ou um simulador nos testes).
pub trait Transport: Send {
    fn bulk_out(&mut self, data: &[u8], timeout_ms: u32) -> io::Result<usize>;
    fn bulk_in(&mut self, buf: &mut [u8], timeout_ms: u32) -> io::Result<usize>;
    fn clear_halt_in(&mut self) -> io::Result<()>;
    fn clear_halt_out(&mut self) -> io::Result<()>;
    /// Reset de classe BOT + limpeza de halt nos dois endpoints.
    fn reset_recovery(&mut self) -> io::Result<()>;
}

pub enum Dir<'a> {
    None,
    In(&'a mut [u8]),
    Out(&'a [u8]),
}

#[derive(Debug, Clone, Copy)]
pub struct CmdResult {
    /// 0 = sucesso, 1 = comando falhou (CHECK CONDITION).
    pub status: u8,
    pub transferred: usize,
    pub residue: u32,
}

impl CmdResult {
    pub fn ok(&self) -> bool {
        self.status == 0
    }
}

const CBW_SIG: u32 = 0x4342_5355; // "USBC"
const CSW_SIG: u32 = 0x5342_5355; // "USBS"
const CHUNK: usize = 64 * 1024;

pub fn is_stall(e: &io::Error) -> bool {
    e.raw_os_error() == Some(32) // EPIPE
}

pub struct Bot<T: Transport> {
    pub t: T,
    tag: u32,
    pub lun: u8,
}

pub fn build_cbw(tag: u32, len: u32, dir_in: bool, lun: u8, cdb: &[u8]) -> [u8; 31] {
    let mut c = [0u8; 31];
    c[0..4].copy_from_slice(&CBW_SIG.to_le_bytes());
    c[4..8].copy_from_slice(&tag.to_le_bytes());
    c[8..12].copy_from_slice(&len.to_le_bytes());
    c[12] = if dir_in { 0x80 } else { 0 };
    c[13] = lun & 0x0F;
    c[14] = cdb.len().min(16) as u8;
    c[15..15 + cdb.len().min(16)].copy_from_slice(&cdb[..cdb.len().min(16)]);
    c
}

fn parse_csw(b: &[u8], tag: u32) -> Option<(u32, u8)> {
    if b.len() != 13 || u32::from_le_bytes(b[0..4].try_into().ok()?) != CSW_SIG {
        return None;
    }
    if u32::from_le_bytes(b[4..8].try_into().ok()?) != tag {
        return None;
    }
    Some((u32::from_le_bytes(b[8..12].try_into().ok()?), b[12]))
}

impl<T: Transport> Bot<T> {
    pub fn new(t: T) -> Self {
        Bot { t, tag: 1, lun: 0 }
    }

    fn fail<R>(&mut self, e: io::Error) -> io::Result<R> {
        let _ = self.t.reset_recovery();
        Err(e)
    }

    pub fn cmd(&mut self, cdb: &[u8], dir: Dir<'_>, timeout_ms: u32) -> io::Result<CmdResult> {
        self.tag = self.tag.wrapping_add(1);
        let tag = self.tag;
        let (len, dir_in) = match &dir {
            Dir::None => (0, false),
            Dir::In(b) => (b.len(), true),
            Dir::Out(b) => (b.len(), false),
        };
        let cbw = build_cbw(tag, len as u32, dir_in, self.lun, cdb);
        if let Err(e) = self.t.bulk_out(&cbw, 5000) {
            return self.fail(e);
        }
        let mut transferred = 0usize;
        let mut early_csw: Option<(u32, u8)> = None;
        match dir {
            Dir::None => {}
            Dir::In(buf) => {
                while transferred < buf.len() {
                    let start = transferred;
                    let end = (start + CHUNK).min(buf.len());
                    match self.t.bulk_in(&mut buf[start..end], timeout_ms) {
                        Ok(n) => {
                            // Dispositivo encerrou cedo e mandou o CSW no lugar dos dados.
                            if n == 13 && start == 0 {
                                if let Some(c) = parse_csw(&buf[..13], tag) {
                                    early_csw = Some(c);
                                    break;
                                }
                            }
                            transferred += n;
                            if n < end - start {
                                break; // pacote curto = fim dos dados
                            }
                        }
                        Err(e) if is_stall(&e) => {
                            self.t.clear_halt_in()?;
                            break;
                        }
                        Err(e) => return self.fail(e),
                    }
                }
            }
            Dir::Out(buf) => {
                while transferred < buf.len() {
                    let end = (transferred + CHUNK).min(buf.len());
                    match self.t.bulk_out(&buf[transferred..end], timeout_ms) {
                        Ok(n) => transferred += n,
                        Err(e) if is_stall(&e) => {
                            self.t.clear_halt_out()?;
                            break;
                        }
                        Err(e) => return self.fail(e),
                    }
                }
            }
        }
        let (residue, status) = match early_csw {
            Some(c) => c,
            None => {
                let mut csw = [0u8; 13];
                let mut got = None;
                for _ in 0..2 {
                    match self.t.bulk_in(&mut csw, timeout_ms.max(5000)) {
                        Ok(13) => {
                            got = parse_csw(&csw, tag);
                            break;
                        }
                        Ok(_) => break,
                        Err(e) if is_stall(&e) => {
                            self.t.clear_halt_in()?;
                        }
                        Err(e) => return self.fail(e),
                    }
                }
                match got {
                    Some(c) => c,
                    None => return self.fail(io::Error::other("CSW inválido (erro de fase)")),
                }
            }
        };
        if status >= 2 {
            return self.fail(io::Error::other("erro de fase BOT"));
        }
        Ok(CmdResult { status, transferred, residue })
    }
}

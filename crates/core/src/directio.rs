//! E/S sem cache do sistema operacional. É aqui que muitos benchmarks "enganam":
//! se o cache de página do SO estiver ativo, você mede a RAM, não o disco.

use std::alloc::{Layout, alloc_zeroed, dealloc};
use std::fs::{File, OpenOptions};
use std::io;
use std::ops::{Deref, DerefMut};
use std::path::Path;

pub const ALIGN: usize = 4096;

/// Buffer alinhado a 4 KiB (exigido por O_DIRECT / FILE_FLAG_NO_BUFFERING).
pub struct AlignedBuf {
    ptr: *mut u8,
    len: usize,
    layout: Layout,
}
unsafe impl Send for AlignedBuf {}

impl AlignedBuf {
    pub fn new(len: usize) -> Self {
        let len = len.max(ALIGN).div_ceil(ALIGN) * ALIGN;
        let layout = Layout::from_size_align(len, ALIGN).expect("layout");
        let ptr = unsafe { alloc_zeroed(layout) };
        assert!(!ptr.is_null(), "falha ao alocar buffer");
        AlignedBuf { ptr, len, layout }
    }
}
impl Drop for AlignedBuf {
    fn drop(&mut self) {
        unsafe { dealloc(self.ptr, self.layout) }
    }
}
impl Deref for AlignedBuf {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}
impl DerefMut for AlignedBuf {
    fn deref_mut(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}

/// Resultado de abrir um arquivo: se conseguiu desligar o cache ou não.
pub struct Opened {
    pub file: File,
    pub uncached: bool,
}

/// Abre arquivo/dispositivo tentando desligar o cache. Se o sistema de arquivos
/// não suportar (ex.: FUSE no Android), cai para modo normal e avisa.
pub fn open(path: &Path, write: bool, create: bool, want_direct: bool) -> io::Result<Opened> {
    if want_direct {
        if let Ok(f) = open_direct(path, write, create) {
            return Ok(Opened { file: f, uncached: true });
        }
    }
    let f = OpenOptions::new().read(true).write(write).create(create).open(path)?;
    Ok(Opened { file: f, uncached: false })
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn open_direct(path: &Path, write: bool, create: bool) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .read(true)
        .write(write)
        .create(create)
        .custom_flags(libc::O_DIRECT)
        .open(path)
}

#[cfg(target_os = "macos")]
fn open_direct(path: &Path, write: bool, create: bool) -> io::Result<File> {
    use std::os::unix::io::AsRawFd;
    let f = OpenOptions::new().read(true).write(write).create(create).open(path)?;
    let r = unsafe { libc::fcntl(f.as_raw_fd(), libc::F_NOCACHE, 1) };
    if r == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(f)
}

#[cfg(windows)]
fn open_direct(path: &Path, write: bool, create: bool) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_FLAG_NO_BUFFERING: u32 = 0x2000_0000;
    const FILE_FLAG_WRITE_THROUGH: u32 = 0x8000_0000;
    const FILE_SHARE_READ: u32 = 1;
    const FILE_SHARE_WRITE: u32 = 2;
    OpenOptions::new()
        .read(true)
        .write(write)
        .create(create)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_NO_BUFFERING | FILE_FLAG_WRITE_THROUGH)
        .open(path)
}

#[cfg(not(any(unix, windows)))]
fn open_direct(_: &Path, _: bool, _: bool) -> io::Result<File> {
    Err(io::Error::other("sem suporte"))
}

#[cfg(unix)]
pub fn pread(f: &File, buf: &mut [u8], off: u64) -> io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(f, buf, off)
}
#[cfg(unix)]
pub fn pwrite(f: &File, buf: &[u8], off: u64) -> io::Result<usize> {
    std::os::unix::fs::FileExt::write_at(f, buf, off)
}
#[cfg(windows)]
pub fn pread(f: &File, buf: &mut [u8], off: u64) -> io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(f, buf, off)
}
#[cfg(windows)]
pub fn pwrite(f: &File, buf: &[u8], off: u64) -> io::Result<usize> {
    std::os::windows::fs::FileExt::seek_write(f, buf, off)
}

pub fn read_full(f: &File, buf: &mut [u8], off: u64) -> io::Result<()> {
    let mut done = 0;
    while done < buf.len() {
        let n = pread(f, &mut buf[done..], off + done as u64)?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "fim do dispositivo"));
        }
        done += n;
    }
    Ok(())
}

pub fn write_full(f: &File, buf: &[u8], off: u64) -> io::Result<()> {
    let mut done = 0;
    while done < buf.len() {
        let n = pwrite(f, &buf[done..], off + done as u64)?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "escrita retornou 0"));
        }
        done += n;
    }
    Ok(())
}

/// Tamanho de um dispositivo de bloco aberto (quando o seek ao final funciona).
pub fn device_size(f: &File) -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::io::AsRawFd;
        // DKIOCGETBLOCKSIZE = _IOR('d', 24, u32), DKIOCGETBLOCKCOUNT = _IOR('d', 25, u64)
        const DKIOCGETBLOCKSIZE: libc::c_ulong = 0x4004_6418;
        const DKIOCGETBLOCKCOUNT: libc::c_ulong = 0x4008_6419;
        let mut bs: u32 = 0;
        let mut bc: u64 = 0;
        unsafe {
            if libc::ioctl(f.as_raw_fd(), DKIOCGETBLOCKSIZE, &mut bs) == 0
                && libc::ioctl(f.as_raw_fd(), DKIOCGETBLOCKCOUNT, &mut bc) == 0
                && bs > 0
                && bc > 0
            {
                return Some(bs as u64 * bc);
            }
        }
    }
    use std::io::{Seek, SeekFrom};
    let mut f = f;
    let n = f.seek(SeekFrom::End(0)).ok()?;
    let _ = f.seek(SeekFrom::Start(0));
    (n > 0).then_some(n)
}

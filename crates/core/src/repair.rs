//! Reparo de setores (estilo "Refresh/Remap" do Victoria), aplicado só aos blocos que
//! a varredura de superfície confirmou como lentos ou ilegíveis.
//!
//! * Bloco lento mas legível → regrava os mesmos dados (refresh). Nada é perdido.
//! * Bloco ilegível → lê setor a setor, aproveita o que ainda lê e grava zeros só nos
//!   setores mortos (cujo conteúdo já está perdido). A escrita força o disco a substituir
//!   o setor por um da área reserva (realocação), o que zera "setores pendentes".

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::directio::AlignedBuf;
use crate::surface::BlockRw;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RepairTarget {
    pub offset: u64,
    pub len: u64,
    /// true = era ilegível; false = lento.
    pub unreadable: bool,
}

#[derive(Debug, Clone)]
pub enum RepairEvent {
    Progress { done: usize, total: usize },
    Log(String),
    Done(RepairResult),
    Failed(String),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RepairResult {
    pub targets: usize,
    pub refreshed: usize,
    pub rewritten_blocks: usize,
    pub zeroed_sectors: u64,
    pub lost_bytes: u64,
    pub fixed: usize,
    pub still_slow: usize,
    pub still_bad: usize,
    pub write_errors: usize,
    pub cancelled: bool,
}

fn read_retry(dev: &mut dyn BlockRw, buf: &mut [u8], off: u64, tries: usize) -> bool {
    (0..tries).any(|_| dev.read_at(buf, off).is_ok())
}

pub fn run_repair(dev: &mut dyn BlockRw, targets: &[RepairTarget], slow_ms: u32, cancel: &AtomicBool, emit: &mut dyn FnMut(RepairEvent)) {
    let sector = dev.align().max(512);
    let mut r = RepairResult { targets: targets.len(), ..Default::default() };
    for (i, t) in targets.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            r.cancelled = true;
            break;
        }
        emit(RepairEvent::Progress { done: i, total: targets.len() });
        let len = (t.len as usize).div_ceil(sector) * sector;
        if t.offset + len as u64 > dev.size() {
            continue;
        }
        let mut buf = AlignedBuf::new(len);
        let mut lost = 0u64;
        let mut zeroed = 0u64;
        let whole_ok = read_retry(dev, &mut buf[..len], t.offset, 2);
        if !whole_ok {
            // setor a setor: salva o que ainda lê
            let mut sb = AlignedBuf::new(sector);
            for s in 0..len / sector {
                let off = t.offset + (s * sector) as u64;
                if read_retry(dev, &mut sb[..sector], off, 3) {
                    buf[s * sector..(s + 1) * sector].copy_from_slice(&sb[..sector]);
                } else {
                    buf[s * sector..(s + 1) * sector].fill(0);
                    lost += sector as u64;
                    zeroed += 1;
                }
            }
            emit(RepairEvent::Log(format!(
                "LBA {}: {} setor(es) ilegível(is) gravado(s) com zeros, demais preservados",
                t.offset / 512,
                zeroed
            )));
        }
        if let Err(e) = dev.write_at(&buf[..len], t.offset).and_then(|_| dev.flush()) {
            r.write_errors += 1;
            emit(RepairEvent::Log(format!("LBA {}: falha ao gravar ({e})", t.offset / 512)));
            if r.write_errors >= 3 && r.write_errors == i + 1 {
                emit(RepairEvent::Failed(format!(
                    "O sistema recusou a escrita no disco ({e}). Desmonte/coloque o disco offline e execute como administrador/root."
                )));
                return;
            }
            continue;
        }
        if whole_ok {
            r.refreshed += 1;
        } else {
            r.rewritten_blocks += 1;
        }
        r.zeroed_sectors += zeroed;
        r.lost_bytes += lost;
        // confere
        let t0 = Instant::now();
        let ok = dev.read_at(&mut buf[..len], t.offset).is_ok();
        let ms = t0.elapsed().as_millis() as u32;
        if !ok {
            r.still_bad += 1;
        } else if ms >= slow_ms {
            r.still_slow += 1;
        } else {
            r.fixed += 1;
        }
    }
    emit(RepairEvent::Progress { done: targets.len(), total: targets.len() });
    emit(RepairEvent::Done(r));
}

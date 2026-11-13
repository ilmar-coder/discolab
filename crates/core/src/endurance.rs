//! Resistência garantida (TBW) publicada pelos fabricantes para SSDs comuns.
//! Valores da garantia limitada de cada linha; servem de referência e podem ser
//! corrigidos pelo usuário na interface quando o modelo não estiver aqui.

/// (padrão no nome do modelo, [(capacidade em GB, TBW)])
const TABLE: &[(&[&str], &[(u64, u64)])] = &[
    (&["990 PRO"], &[(1000, 600), (2000, 1200), (4000, 2400)]),
    (&["990 EVO"], &[(1000, 600), (2000, 1200)]),
    (&["980 PRO"], &[(250, 150), (500, 300), (1000, 600), (2000, 1200)]),
    (&["980"], &[(250, 150), (500, 300), (1000, 600)]),
    (&["970 EVO PLUS", "970 EVO"], &[(250, 150), (500, 300), (1000, 600), (2000, 1200)]),
    (&["870 EVO"], &[(250, 150), (500, 300), (1000, 600), (2000, 1200), (4000, 2400)]),
    (&["860 EVO"], &[(250, 150), (500, 300), (1000, 600), (2000, 1200), (4000, 2400)]),
    (&["870 QVO", "860 QVO"], &[(1000, 360), (2000, 720), (4000, 1440), (8000, 2880)]),
    (&["MX500"], &[(250, 100), (500, 180), (1000, 360), (2000, 700), (4000, 1000)]),
    (&["BX500"], &[(240, 80), (480, 120), (1000, 360), (2000, 720)]),
    (&["CT500P3", "CT1000P3", "CT2000P3", "CT4000P3", "P3 PLUS", "CRUCIAL P3"], &[(500, 110), (1000, 220), (2000, 440), (4000, 800)]),
    (&["P5 PLUS"], &[(500, 300), (1000, 600), (2000, 1200)]),
    (&["T500"], &[(500, 300), (1000, 600), (2000, 1200), (4000, 2400)]),
    (&["SA400", "A400"], &[(120, 40), (240, 80), (480, 160), (960, 300), (1920, 600)]),
    (&["SNV2S", "NV2"], &[(250, 80), (500, 160), (1000, 320), (2000, 640), (4000, 1280)]),
    (&["SNV3S", "NV3"], &[(500, 160), (1000, 320), (2000, 640), (4000, 1280)]),
    (&["SKC3000", "KC3000"], &[(512, 400), (1024, 800), (2048, 1600), (4096, 3200)]),
    (&["SFYR", "FURY RENEGADE"], &[(500, 500), (1000, 1000), (2000, 2000), (4000, 4000)]),
    (&["SN850X"], &[(1000, 600), (2000, 1200), (4000, 2400)]),
    (&["SN850"], &[(500, 300), (1000, 600), (2000, 1200)]),
    (&["SN770"], &[(250, 200), (500, 300), (1000, 600), (2000, 1200)]),
    (&["SN580"], &[(250, 150), (500, 300), (1000, 600), (2000, 900)]),
    (&["SN570"], &[(250, 150), (500, 300), (1000, 600), (2000, 900)]),
    (&["WDS250G2B0A", "WDS500G2B0A", "WDS100T2B0A", "WD BLUE SA510", "SA510"], &[(250, 100), (500, 200), (1000, 400), (2000, 500)]),
    (&["WDS240G2G0A", "WDS480G2G0A", "WD GREEN"], &[(240, 80), (480, 80), (1000, 80)]),
    (&["SDSSDH3", "ULTRA 3D"], &[(250, 100), (500, 200), (1000, 400), (2000, 500), (4000, 600)]),
    (&["MP600"], &[(500, 850), (1000, 1800), (2000, 3600)]),
    (&["MP510"], &[(240, 400), (480, 800), (960, 1700), (1920, 3120)]),
];

/// TBW (em TB) do modelo e capacidade, se conhecido.
pub fn rated_tbw(model: &str, capacity_bytes: u64) -> Option<u64> {
    let m = model.to_uppercase();
    let gb = capacity_bytes / 1_000_000_000;
    for (pats, caps) in TABLE {
        if pats.iter().any(|p| m.contains(p)) {
            // capacidade mais próxima (tolerância de 25%)
            let best = caps.iter().min_by_key(|(c, _)| c.abs_diff(gb))?;
            if gb > 0 && best.0.abs_diff(gb) * 4 <= best.0 {
                return Some(best.1);
            }
            return None;
        }
    }
    None
}

/// Percentual de vida restante pelo TBW (pode ser negativo → limitado a 0).
pub fn life_left_by_tbw(written_bytes: u64, tbw_tb: u64) -> Option<u32> {
    if tbw_tb == 0 {
        return None;
    }
    let used = written_bytes as f64 / (tbw_tb as f64 * 1e12) * 100.0;
    Some((100.0 - used).clamp(0.0, 100.0).round() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lookup() {
        assert_eq!(rated_tbw("Samsung SSD 870 EVO 500GB", 500_107_862_016), Some(300));
        assert_eq!(rated_tbw("Samsung SSD 970 EVO Plus 1TB", 1_000_204_886_016), Some(600));
        assert_eq!(rated_tbw("CT1000MX500SSD1", 1_000_204_886_016), Some(360));
        assert_eq!(rated_tbw("KINGSTON SA400S37240G", 240_057_409_536), Some(80));
        assert_eq!(rated_tbw("WD_BLACK SN770 1TB", 1_000_204_886_016), Some(600));
        assert_eq!(rated_tbw("APPLE SSD AP0256Q", 251_000_193_024), None);
        assert_eq!(life_left_by_tbw(150_000_000_000_000, 600), Some(75));
    }
}

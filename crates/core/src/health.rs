//! Motor de diagnóstico.
//!
//! Filosofia ("nem rigoroso demais, nem otimista demais"):
//! * Só atributos que realmente indicam degradação da mídia pesam na nota
//!   (realocados, pendentes, incorrigíveis, erros de mídia NVMe, spare, desgaste).
//! * Atributos com valor bruto "empacotado" pelo fabricante (ex.: Raw_Read_Error_Rate
//!   e Seek_Error_Rate da Seagate) NÃO são lidos pelo valor bruto — é aí que outros
//!   programas mostram alarmes falsos.
//! * Erros de CRC de interface apontam para cabo/porta, não para o disco.
//! * Na varredura de superfície, blocos lentos são relidos: lentidão que some na
//!   releitura é transitória (atividade do SO, recalibração) e não é defeito.
//! * Status "SMART PASSED" do firmware não é suficiente: muitos discos com setores
//!   pendentes continuam "PASSED". Por isso a nota considera os contadores brutos.

use serde::{Deserialize, Serialize};

use crate::device::{Device, DriveKind};
use crate::smart::{SmartReport, SmartSource};
use crate::surface::ScanSummary;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Severity {
    Info,
    Ok,
    Notice,
    Warning,
    Critical,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Info => "Info",
            Severity::Ok => "OK",
            Severity::Notice => "Observação",
            Severity::Warning => "Atenção",
            Severity::Critical => "Crítico",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub severity: Severity,
    pub title: String,
    pub detail: String,
    /// Pontos descontados da saúde.
    pub penalty: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Assessment {
    /// Integridade da mídia (0..100).
    pub health: u32,
    /// Vida útil restante estimada pelo desgaste (flash), se conhecida.
    pub life_left: Option<u32>,
    pub status: Severity,
    pub verdict: String,
    pub confidence: &'static str,
    pub findings: Vec<Finding>,
}

impl Assessment {
    pub fn status_label(&self) -> &'static str {
        match self.status {
            Severity::Critical => "CRÍTICO",
            Severity::Warning => "ATENÇÃO",
            Severity::Notice => "BOM",
            _ => {
                if self.health >= 95 { "EXCELENTE" } else { "BOM" }
            }
        }
    }
}

struct Ctx {
    f: Vec<Finding>,
}
impl Ctx {
    fn add(&mut self, severity: Severity, penalty: f32, title: impl Into<String>, detail: impl Into<String>) {
        self.f.push(Finding { severity, title: title.into(), detail: detail.into(), penalty });
    }
}

/// Penalidade com retorno decrescente: os primeiros eventos pesam mais por unidade,
/// mas a nota não despenca por números grandes isolados.
fn curve(count: u64, per_unit: f32, cap: f32) -> f32 {
    if count == 0 {
        return 0.0;
    }
    let c = count as f32;
    (per_unit * c.sqrt() * 3.0).min(per_unit * c).max(per_unit).min(cap)
}

pub fn assess(dev: &Device, s: &SmartReport, scan: Option<&ScanSummary>) -> Assessment {
    let mut c = Ctx { f: Vec::new() };
    let mut life_left: Option<u32> = None;
    let kind = if s.rotation_rpm.map(|r| r > 0).unwrap_or(false) {
        DriveKind::Hdd
    } else if s.nvme.is_some() {
        DriveKind::Nvme
    } else {
        dev.kind
    };
    let hdd = kind == DriveKind::Hdd;
    let model = s.model.clone().unwrap_or_else(|| dev.model.clone()).to_uppercase();
    let seagate = model.starts_with("ST") || model.contains("SEAGATE");

    if !s.ok {
        c.add(
            Severity::Notice,
            0.0,
            "Sem dados SMART",
            if s.needs_admin {
                "Execute como Administrador/root para ler SMART."
            } else {
                "O dispositivo (ou adaptador USB) não expôs dados SMART. A avaliação usa só a varredura de superfície."
            },
        );
    }

    if s.smart_passed == Some(false) {
        c.add(
            Severity::Critical,
            60.0,
            "Firmware reporta SMART FAILED",
            "O próprio disco declara falha iminente. Faça backup imediatamente.",
        );
    }

    // ---------------- ATA
    for a in &s.ata {
        if a.thresh > 0 && a.value > 0 && a.value <= a.thresh {
            c.add(
                Severity::Critical,
                if a.prefailure { 45.0 } else { 15.0 },
                format!("Atributo {} ({}) abaixo do limite", a.id, a.name),
                format!("Valor {} abaixo ou igual ao limite {}. O fabricante considera este atributo em falha.", a.value, a.thresh),
            );
        } else if a.when_failed == "past" && a.prefailure {
            c.add(
                Severity::Notice,
                3.0,
                format!("Atributo {} ({}) já esteve abaixo do limite", a.id, a.name),
                "Falhou no passado, mas está normal agora.",
            );
        }
    }
    let raw = |id: u32| s.attr(id).map(|a| a.raw & 0xFFFF_FFFF);

    if let Some(n) = raw(5).filter(|n| *n > 0) {
        let (sev, d) = match n {
            1..=9 => (Severity::Notice, "Poucos setores realocados. Comum em discos usados; acompanhe se o número cresce."),
            10..=99 => (Severity::Warning, "Quantidade relevante de setores substituídos. Mantenha backup em dia e monitore."),
            _ => (Severity::Critical, "Muitos setores realocados: a superfície/flash está degradando."),
        };
        c.add(sev, curve(n, 1.0, 45.0), format!("{n} setores realocados (ID 5)"), d);
    }
    if let Some(n) = raw(197).filter(|n| *n > 0) {
        c.add(
            if n >= 10 { Severity::Critical } else { Severity::Warning },
            curve(n, 3.0, 45.0),
            format!("{n} setores pendentes (ID 197)"),
            "Setores instáveis aguardando realocação. Podem sumir após serem reescritos, \
             mas indicam leitura falhando agora. Faça backup.",
        );
    }
    if let Some(n) = raw(198).filter(|n| *n > 0) {
        c.add(
            Severity::Warning,
            curve(n, 2.0, 35.0),
            format!("{n} setores incorrigíveis offline (ID 198)"),
            "Detectados pela varredura interna do disco.",
        );
    }
    if let Some(n) = raw(187).filter(|n| *n > 0) {
        c.add(
            if n > 50 { Severity::Warning } else { Severity::Notice },
            curve(n, 1.0, 25.0),
            format!("{n} erros incorrigíveis reportados (ID 187)"),
            "Leituras que o ECC não conseguiu corrigir.",
        );
    }
    if hdd {
        if let Some(n) = raw(10).filter(|n| *n > 0) {
            c.add(Severity::Warning, curve(n, 3.0, 25.0), format!("{n} repetições de partida do motor (ID 10)"), "Problema mecânico ou de alimentação.");
        }
        if let Some(n) = raw(196).filter(|n| *n > 0) {
            c.add(Severity::Info, 0.0, format!("{n} eventos de realocação (ID 196)"), "Já refletido no ID 5.");
        }
    }
    if let Some(n) = raw(199).filter(|n| *n > 0) {
        c.add(
            Severity::Notice,
            0.0,
            format!("{n} erros de CRC na interface (ID 199)"),
            "Não é defeito do disco: indica cabo SATA, porta ou adaptador com mau contato. \
             Se o número aumentar, troque o cabo.",
        );
    }
    if seagate && (s.attr(1).is_some() || s.attr(7).is_some()) {
        c.add(
            Severity::Info,
            0.0,
            "Seagate: ID 1 e 7 usam valor bruto empacotado",
            "Valores brutos enormes nesses atributos são normais na Seagate; apenas o valor normalizado é considerado.",
        );
    }

    // Desgaste em SSD SATA (normalizado = % restante na maioria dos fabricantes)
    if !hdd {
        let wear_ids = [231u32, 233, 177, 202, 169, 173];
        let mut best: Option<(u32, String)> = None;
        for id in wear_ids {
            if let Some(a) = s.attr(id) {
                let n = a.name.to_lowercase();
                let plausible = n.contains("life") || n.contains("wear") || n.contains("percent") || n.contains("remain") || n.contains("endurance");
                if plausible && a.value <= 100 {
                    best = Some((a.value, a.name.clone()));
                    break;
                }
            }
        }
        if let Some(used) = s.ata_endurance_used {
            life_left = Some(100u32.saturating_sub(used));
        } else if let Some((v, _)) = best {
            life_left = Some(v);
        }
    }

    // ---------------- NVMe
    if let Some(n) = &s.nvme {
        let cw = n.critical_warning;
        if cw & 0x01 != 0 {
            c.add(Severity::Critical, 40.0, "Área reserva abaixo do limite", "O SSD está sem blocos de reserva suficientes.");
        }
        if cw & 0x02 != 0 {
            c.add(Severity::Warning, 5.0, "Alerta de temperatura ativo", "Melhore a refrigeração do SSD.");
        }
        if cw & 0x04 != 0 {
            c.add(Severity::Critical, 50.0, "Confiabilidade degradada", "O controlador reportou erros internos graves de mídia.");
        }
        if cw & 0x08 != 0 {
            c.add(Severity::Critical, 60.0, "SSD em modo somente leitura", "O SSD bloqueou a escrita para proteger os dados. Copie tudo agora.");
        }
        if cw & 0x10 != 0 {
            c.add(Severity::Warning, 10.0, "Falha no backup de memória volátil", "Capacitores/PLP com falha.");
        }
        if n.available_spare_threshold > 0 && n.available_spare < n.available_spare_threshold && cw & 0x01 == 0 {
            c.add(Severity::Critical, 35.0, "Reserva abaixo do limite", format!("{}% < limite {}%", n.available_spare, n.available_spare_threshold));
        } else if n.available_spare < 100 && n.available_spare > 0 {
            let used = 100 - n.available_spare;
            c.add(
                if n.available_spare < 50 { Severity::Warning } else { Severity::Notice },
                (used as f32 * 0.5).min(30.0),
                format!("Área reserva em {}%", n.available_spare),
                "Parte dos blocos reserva já foi consumida para substituir blocos ruins.",
            );
        }
        if n.media_errors > 0 {
            c.add(
                if n.media_errors > 20 { Severity::Critical } else { Severity::Warning },
                curve(n.media_errors, 3.0, 45.0),
                format!("{} erros de integridade de mídia", n.media_errors),
                "Erros de dados não corrigidos registrados pelo controlador.",
            );
        }
        if n.num_err_log_entries > 0 && n.media_errors == 0 {
            c.add(
                Severity::Info,
                0.0,
                format!("{} entradas no log de erros", n.num_err_log_entries),
                "Normalmente são comandos não suportados enviados pelo SO, não defeitos. Sem erros de mídia = sem impacto.",
            );
        }
        if n.critical_comp_time > 0 {
            c.add(Severity::Notice, 2.0, format!("{} min acima da temperatura crítica", n.critical_comp_time), "Histórico de superaquecimento.");
        }
        life_left = Some(100u32.saturating_sub(n.percentage_used));
        if n.unsafe_shutdowns > 0 {
            c.add(Severity::Info, 0.0, format!("{} desligamentos inesperados", n.unsafe_shutdowns), "Informativo; quedas de energia aumentam este número.");
        }
    }

    // ---------------- eMMC/UFS
    if let Some(fl) = &s.flash {
        let used = fl.used_pct_a.max(fl.used_pct_b);
        if let Some(u) = used {
            life_left = Some(100u32.saturating_sub(u));
        }
        match fl.pre_eol {
            Some(2) => c.add(Severity::Warning, 20.0, "Pre-EOL: aviso", "80% dos blocos reserva consumidos."),
            Some(3) => c.add(Severity::Critical, 45.0, "Pre-EOL: urgente", "Memória flash perto do fim da vida útil."),
            _ => {}
        }
    }

    // ---------------- Windows
    for (k, v) in &s.extra {
        let n: u64 = v.parse().unwrap_or(0);
        if (k == "ReadErrorsUncorrected" || k == "WriteErrorsUncorrected") && n > 0 {
            c.add(Severity::Warning, curve(n, 2.0, 35.0), format!("{n} erros não corrigidos ({k})"), "Contador de confiabilidade do Windows.");
        }
    }

    // ---------------- Desgaste
    if let Some(l) = life_left {
        let (sev, pen) = match l {
            0..=4 => (Severity::Critical, 25.0),
            5..=14 => (Severity::Warning, 10.0),
            15..=40 => (Severity::Notice, 0.0),
            _ => (Severity::Ok, 0.0),
        };
        c.add(
            sev,
            pen,
            format!("Vida útil restante estimada: {l}%"),
            "Baseado no desgaste informado pelo controlador. SSDs costumam continuar funcionando \
             além de 100% de uso nominal, mas a retenção de dados diminui.",
        );
    }

    // ---------------- Temperatura
    if let Some(t) = s.temperature {
        let (warn, crit) = if hdd { (50, 60) } else { (70, 80) };
        if t >= crit {
            c.add(Severity::Warning, 8.0, format!("Temperatura muito alta: {t} °C"), "Risco de throttling e redução de vida útil.");
        } else if t >= warn {
            c.add(Severity::Notice, 2.0, format!("Temperatura elevada: {t} °C"), "Verifique ventilação.");
        }
    }

    // ---------------- Autotestes
    if let Some(last) = s.self_tests.first() {
        if last.passed == Some(false) && !last.status.to_lowercase().contains("abort") && !last.status.to_lowercase().contains("interrupt") {
            c.add(Severity::Critical, 30.0, "Último autoteste SMART falhou", last.status.clone());
        }
    }
    if let Some(n) = s.ata_error_count.filter(|n| *n > 0) {
        c.add(
            Severity::Info,
            curve(n, 0.3, 5.0),
            format!("{n} erros no log ATA"),
            "Histórico de erros de comando. Analise junto com realocados/pendentes; sozinho tem pouco peso.",
        );
    }

    // ---------------- Superfície
    if let Some(sc) = scan {
        if sc.bad_blocks > 0 {
            c.add(
                if sc.bad_blocks > 5 { Severity::Critical } else { Severity::Warning },
                curve(sc.bad_blocks, 5.0, 50.0),
                format!("{} blocos ilegíveis (confirmados por releitura)", sc.bad_blocks),
                "Erros de leitura persistentes. Dados nessa região podem estar perdidos.",
            );
        }
        if sc.slow_persistent > 0 {
            c.add(
                if sc.slow_persistent > 50 { Severity::Warning } else { Severity::Notice },
                curve(sc.slow_persistent, 0.5, 20.0),
                format!("{} blocos persistentemente lentos", sc.slow_persistent),
                "Setores fracos que o disco lê só depois de várias tentativas internas — candidatos a virar setores defeituosos.",
            );
        }
        if sc.slow_transient > 0 {
            c.add(
                Severity::Info,
                0.0,
                format!("{} blocos lentos transitórios", sc.slow_transient),
                "Ficaram rápidos na releitura: causados por atividade do sistema, não por defeito. \
                 (Outros programas contariam estes como problema.)",
            );
        }
    }

    let penalty: f32 = c.f.iter().map(|f| f.penalty).sum();
    let mut health = (100.0 - penalty).clamp(0.0, 100.0).round() as u32;
    if c.f.iter().any(|f| f.severity == Severity::Critical) {
        health = health.min(39);
    }
    let status = c.f.iter().map(|f| f.severity).filter(|s| *s >= Severity::Notice).max().unwrap_or(Severity::Ok);
    let verdict = match status {
        Severity::Critical => "Risco alto de perda de dados. Faça backup imediatamente e planeje a troca.",
        Severity::Warning => "Há sinais reais de degradação. Mantenha backup atualizado e monitore com frequência.",
        Severity::Notice => "Saudável, com pontos de observação sem urgência.",
        _ => "Nenhum problema encontrado.",
    }
    .to_string();
    let confidence = match (s.source, scan.is_some()) {
        (SmartSource::Smartctl | SmartSource::UsbDirect, true) if s.ok => "Alta (SMART completo + superfície)",
        (SmartSource::Smartctl | SmartSource::UsbDirect, false) if s.ok => "Boa (SMART completo)",
        (SmartSource::UsbDirect, true) => "Média (somente superfície)",
        (SmartSource::UsbDirect, false) => "Baixa (sem dados)",
        (SmartSource::None, true) => "Média (somente superfície)",
        (SmartSource::None, false) => "Baixa (sem dados)",
        (_, true) => "Média (SMART parcial + superfície)",
        (_, false) => "Média (SMART parcial)",
    };
    c.f.sort_by(|a, b| b.severity.cmp(&a.severity));
    Assessment { health, life_left, status, verdict, confidence, findings: c.f }
}

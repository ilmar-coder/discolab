//! Verificação de novas versões nas Releases do GitHub.
//!
//! * Windows: baixa o `DiscoLab-portable.exe`, confere o SHA-256 publicado pelo GitHub,
//!   troca o executável atual e reinicia.
//! * Demais sistemas: abre a página da versão para baixar o pacote certo.
//!
//! Funciona só com repositório (ou espelho de releases) público: um app instalado não
//! deve carregar credenciais para ler um repositório privado.

use serde_json::Value;
use std::io::{Read, Write};
use std::path::PathBuf;

/// Repositório consultado (pode ser trocado na compilação com DISCOLAB_UPDATE_REPO).
pub const REPO: &str = match option_env!("DISCOLAB_UPDATE_REPO") {
    Some(r) => r,
    None => "ilmar-coder/discolab",
};

#[derive(Debug, Clone)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub sha256: Option<String>,
    pub size: u64,
}

#[derive(Debug, Clone)]
pub struct Release {
    pub version: String,
    pub page: String,
    pub notes: String,
    pub assets: Vec<Asset>,
}

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn parse_ver(v: &str) -> Vec<u64> {
    v.trim_start_matches(['v', 'V']).split(['.', '-']).map(|p| p.parse().unwrap_or(0)).collect()
}

/// `a` é mais nova que `b`?
pub fn newer(a: &str, b: &str) -> bool {
    let (a, b) = (parse_ver(a), parse_ver(b));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(20)))
        .user_agent(concat!("DiscoLab/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

pub fn parse_release(v: &Value) -> Option<Release> {
    let tag = v["tag_name"].as_str()?;
    let assets = v["assets"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    Some(Asset {
                        name: x["name"].as_str()?.to_string(),
                        url: x["browser_download_url"].as_str()?.to_string(),
                        sha256: x["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).map(|d| d.to_lowercase()),
                        size: x["size"].as_u64().unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Release {
        version: tag.trim_start_matches('v').to_string(),
        page: v["html_url"].as_str().unwrap_or("").to_string(),
        notes: v["body"].as_str().unwrap_or("").to_string(),
        assets,
    })
}

/// Consulta a última versão publicada. Ok(None) = já está atualizado.
pub fn check() -> Result<Option<Release>, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let mut resp = agent().get(&url).header("Accept", "application/vnd.github+json").call().map_err(|e| match e {
        ureq::Error::StatusCode(404) => "não há versões públicas disponíveis (repositório privado ou sem releases)".to_string(),
        other => format!("sem conexão com o GitHub ({other})"),
    })?;
    let text = resp.body_mut().read_to_string().map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let r = parse_release(&v).ok_or("resposta inesperada do GitHub")?;
    Ok(newer(&r.version, current_version()).then_some(r))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Baixa um arquivo verificando o SHA-256 (quando o GitHub informa).
pub fn download(asset: &Asset, dest: &std::path::Path, progress: &mut dyn FnMut(u64, u64)) -> Result<(), String> {
    use sha2::Digest;
    let mut resp = agent().get(&asset.url).call().map_err(|e| format!("download falhou: {e}"))?;
    let mut reader = resp.body_mut().with_config().limit(512 << 20).reader();
    let tmp = dest.with_extension("partial");
    let mut f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
    let mut hasher = sha2::Sha256::new();
    let mut buf = vec![0u8; 256 << 10];
    let mut done = 0u64;
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("download interrompido: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        f.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        progress(done, asset.size);
    }
    f.sync_all().map_err(|e| e.to_string())?;
    drop(f);
    if let Some(expected) = &asset.sha256 {
        let got = hex(&hasher.finalize());
        if &got != expected {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("arquivo corrompido (SHA-256 {got} diferente de {expected})"));
        }
    }
    std::fs::rename(&tmp, dest).map_err(|e| e.to_string())
}

/// Windows: substitui o executável em uso pela nova versão e a inicia.
/// (O Windows permite renomear um .exe em execução, mas não sobrescrevê-lo.)
pub fn replace_and_restart(new_exe: &std::path::Path) -> Result<(), String> {
    let me: PathBuf = std::env::current_exe().map_err(|e| e.to_string())?;
    let old = me.with_extension("old.exe");
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&me, &old).map_err(|e| format!("não foi possível trocar o executável: {e}"))?;
    if let Err(e) = std::fs::copy(new_exe, &me) {
        let _ = std::fs::rename(&old, &me);
        return Err(format!("não foi possível gravar a nova versão: {e}"));
    }
    let _ = std::fs::remove_file(new_exe);
    std::process::Command::new(&me).spawn().map_err(|e| e.to_string())?;
    std::process::exit(0);
}

/// Apaga o executável antigo deixado por uma atualização anterior.
pub fn cleanup_old() {
    if let Ok(me) = std::env::current_exe() {
        let _ = std::fs::remove_file(me.with_extension("old.exe"));
    }
}

/// Atualização automática disponível nesta plataforma?
pub fn can_self_update() -> bool {
    cfg!(windows)
}

/// Arquivo da release adequado a esta plataforma (Windows).
pub fn platform_asset(r: &Release) -> Option<&Asset> {
    r.assets.iter().find(|a| a.name == "DiscoLab-portable.exe")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn versions() {
        assert!(newer("0.2.0", "0.1.1"));
        assert!(newer("v1.0.0", "0.9.9"));
        assert!(newer("0.1.10", "0.1.9"));
        assert!(!newer("0.1.1", "0.1.1"));
        assert!(!newer("0.1.0", "0.1.1"));
    }
    #[test]
    fn parses_github_release() {
        let v = serde_json::json!({
            "tag_name": "v0.2.0", "html_url": "https://github.com/x/y/releases/tag/v0.2.0", "body": "notas",
            "assets": [{"name": "DiscoLab-portable.exe", "browser_download_url": "https://x/p.exe", "size": 10,
                        "digest": "sha256:ABCDEF"}]
        });
        let r = parse_release(&v).unwrap();
        assert_eq!(r.version, "0.2.0");
        let a = platform_asset(&r).unwrap();
        assert_eq!(a.sha256.as_deref(), Some("abcdef"));
    }
}

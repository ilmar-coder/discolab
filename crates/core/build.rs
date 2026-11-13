//! Embute o smartctl.exe no binário do Windows quando DISCOLAB_SMARTCTL aponta para ele
//! (o CI baixa e verifica o executável oficial do smartmontools).
fn main() {
    println!("cargo::rustc-check-cfg=cfg(embedded_smartctl)");
    println!("cargo::rerun-if-env-changed=DISCOLAB_SMARTCTL");
    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    if let (true, Ok(p)) = (windows, std::env::var("DISCOLAB_SMARTCTL")) {
        let path = std::path::Path::new(&p);
        if path.is_file() {
            let abs = std::fs::canonicalize(path).expect("caminho do smartctl");
            println!("cargo::rerun-if-changed={}", abs.display());
            println!("cargo::rustc-cfg=embedded_smartctl");
            println!("cargo::rustc-env=DISCOLAB_SMARTCTL_PATH={}", abs.display());
        } else {
            println!("cargo::warning=DISCOLAB_SMARTCTL={p} não existe; smartctl não será embutido");
        }
    }
}

//! Windows: ícone e manifesto que pede administrador ao abrir (SMART e superfície exigem).
fn main() {
    println!("cargo::rerun-if-changed=../../packaging/windows/discolab.manifest");
    println!("cargo::rerun-if-changed=../../packaging/windows/discolab.ico");
    let target_windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    // O compilador de recursos (rc.exe) só existe quando o build roda no próprio Windows.
    if target_windows && cfg!(windows) {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../packaging/windows/discolab.ico")
            .set_manifest_file("../../packaging/windows/discolab.manifest")
            .set("FileDescription", "DiscoLab - teste de discos")
            .set("ProductName", "DiscoLab");
        if let Err(e) = res.compile_for("discolab") {
            println!("cargo::warning=recursos do Windows não embutidos: {e}");
        }
    } else if target_windows {
        println!("cargo::warning=compilação cruzada: ícone e manifesto do Windows não embutidos");
    }
}

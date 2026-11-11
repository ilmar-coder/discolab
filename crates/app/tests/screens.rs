//! Renderiza as abas fora da tela: `SHOTS=/pasta cargo test -p discolab --test screens -- --ignored`
use egui_kittest::Harness;

#[test]
#[ignore]
fn screenshots() {
    let out = std::path::PathBuf::from(std::env::var("SHOTS").unwrap_or("target/shots".into()));
    std::fs::create_dir_all(&out).unwrap();
    let w: f32 = std::env::var("SHOT_W").ok().and_then(|v| v.parse().ok()).unwrap_or(1180.0);
    let h: f32 = std::env::var("SHOT_H").ok().and_then(|v| v.parse().ok()).unwrap_or(780.0);
    let mut hs = Harness::builder().with_size([w, h]).wgpu().build_eframe(|cc| discolab_app::App::new(cc));
    let wait = |hs: &mut Harness<discolab_app::App>| {
        for _ in 0..600 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            hs.run_steps(2);
            if !hs.state().test_busy() { break; }
        }
        hs.run_steps(4);
    };
    wait(&mut hs);
    for tab in ["health", "smart"] {
        hs.state_mut().test_hook(tab);
        hs.run_steps(4);
        hs.render().unwrap().save(out.join(format!("{tab}.png"))).unwrap();
    }
    hs.state_mut().test_hook("bench");
    hs.state_mut().test_hook("quickbench");
    wait(&mut hs);
    hs.render().unwrap().save(out.join("bench.png")).unwrap();
    for tab in ["surface", "report"] {
        hs.state_mut().test_hook(tab);
        hs.run_steps(4);
        hs.render().unwrap().save(out.join(format!("{tab}.png"))).unwrap();
    }
}

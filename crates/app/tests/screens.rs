//! Renderiza as abas fora da tela:
//! `SHOTS=/pasta [SHOT_W=400 SHOT_H=860] cargo test --release -p discolab --test screens -- --ignored`
use egui_kittest::Harness;

#[test]
#[ignore]
fn screenshots() {
    let out = std::path::PathBuf::from(std::env::var("SHOTS").unwrap_or("target/shots".into()));
    std::fs::create_dir_all(&out).unwrap();
    let w: f32 = std::env::var("SHOT_W").ok().and_then(|v| v.parse().ok()).unwrap_or(1200.0);
    let h: f32 = std::env::var("SHOT_H").ok().and_then(|v| v.parse().ok()).unwrap_or(800.0);
    let mut hs = Harness::builder().with_size([w, h]).wgpu().build_eframe(|cc| discolab_app::App::new(cc));
    if w < 760.0 {
        // simula as barras de status/navegação do Android
        hs.state_mut().test_hook("insets");
    }
    let wait = |hs: &mut Harness<discolab_app::App>| {
        for _ in 0..600 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            hs.run_steps(2);
            if !hs.state().test_busy() {
                break;
            }
        }
        hs.run_steps(4);
    };
    let shot = |hs: &mut Harness<discolab_app::App>, name: &str| {
        hs.run_steps(4);
        hs.render().unwrap().save(out.join(format!("{name}.png"))).unwrap();
    };
    wait(&mut hs);
    for tab in ["health", "smart"] {
        hs.state_mut().test_hook(tab);
        shot(&mut hs, tab);
    }
    hs.state_mut().test_hook("picker");
    shot(&mut hs, "picker");
    hs.state_mut().test_hook("nopicker");
    hs.state_mut().test_hook("bench");
    hs.state_mut().test_hook("quickbench");
    wait(&mut hs);
    shot(&mut hs, "bench");
    eprintln!("TEMPS {:?}", hs.state().test_temps());
    for tab in ["surface", "report", "capacity"] {
        hs.state_mut().test_hook(tab);
        shot(&mut hs, tab);
    }
    hs.state_mut().test_hook("perf");
    hs.state_mut().test_hook("light");
    for tab in ["health", "bench"] {
        hs.state_mut().test_hook(tab);
        shot(&mut hs, &format!("{tab}_claro"));
    }
}

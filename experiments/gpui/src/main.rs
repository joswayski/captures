#[cfg(not(target_os = "linux"))]
compile_error!(
    "This GPUI integration currently targets Linux; other platforms use the shipping desktop application."
);

mod app;
mod capture;
mod desktop;
mod editor;
mod history;
mod preferences;
mod preview;
mod recording;
mod settings;
mod ui;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help") {
        println!(
            "Captures GPUI\n\n--preferences | --history | --capture | --window | --display\n--record | --gif | --canvas | --open FILE... | --previews FILE...\n--background | --appearance light|dark|system\n\nRequires a Linux X11 session. Settings use CAPTURES_GPUI_DATA or the captures-gpui XDG data directory."
        );
        return Ok(());
    }
    anyhow::ensure!(
        std::env::var_os("DISPLAY").is_some()
            && std::env::var("XDG_SESSION_TYPE").as_deref() != Ok("wayland"),
        "GPUI capture integration requires X11. Use the shipping desktop application on Wayland."
    );
    let Some(instance) = desktop::Instance::acquire(&args)? else {
        return Ok(());
    };
    gpui::Application::new()
        .with_assets(ui::Assets)
        .run(move |cx| app::run(instance, args, cx));
    Ok(())
}

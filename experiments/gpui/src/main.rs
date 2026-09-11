mod app;
mod capture;
mod desktop;
mod editor;
mod history;
mod media;
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
            "Captures GPUI\n\n--preferences | --history | --capture | --window | --display\n--record | --gif | --canvas | --open FILE... | --previews FILE...\n--background | --appearance light|dark|system\n\nSettings use CAPTURES_GPUI_DATA or the platform captures-gpui data directory."
        );
        return Ok(());
    }
    desktop::ensure_supported_session()?;
    let Some(instance) = desktop::Instance::acquire(&args)? else {
        return Ok(());
    };
    let application = gpui::Application::new().with_assets(ui::Assets);
    application.on_open_urls(desktop::open_urls);
    application.on_reopen(|cx| app::command(&[], cx));
    application.run(move |cx| app::run(instance, args, cx));
    Ok(())
}

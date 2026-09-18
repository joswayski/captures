mod countdown;
mod live;
mod options;
mod preferences;
mod selector;
mod tokens;
mod workbench;

use eframe::egui;
use options::{Options, Scene};
use serde_json::json;

fn emit(event: &str, detail: serde_json::Value) {
    println!(
        "{}",
        json!({"schema": 1, "pid": std::process::id(), "event": event, "detail": detail})
    );
}

fn main() -> eframe::Result {
    let options = Options::parse(std::env::args().skip(1)).unwrap_or_else(|error| {
        eprintln!("{error}\n{}", options::USAGE);
        std::process::exit(2);
    });
    let floating = options.floating;
    let idle = options.scene == Scene::Idle;
    let size = if floating {
        [640., 620.]
    } else {
        [1000., 720.]
    };
    let native = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            .with_title(if options.live {
                "Captures"
            } else {
                "Captures — wgpu fixture workbench"
            })
            .with_inner_size(size)
            .with_min_inner_size(size)
            .with_visible(!idle)
            // eframe's wgpu painter takes its alpha capability from the root,
            // including for the transparent countdown child viewport.
            .with_transparent(floating || options.live)
            .with_decorations(!floating),
        ..Default::default()
    };
    emit(
        "starting",
        json!({"scene": if options.live { "live" } else { options.scene.name() },
        "phase": "before native event loop and renderer initialization"}),
    );
    eframe::run_native(
        "Captures renderer experiment",
        native,
        Box::new(move |cc| Ok(Box::new(workbench::Workbench::new(cc, options)))),
    )
}

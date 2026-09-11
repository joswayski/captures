#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!(
        "The GTK widget prototype is Linux-only; no macOS/Windows native frontend is implemented."
    );
    std::process::exit(1);
}

#[cfg(target_os = "linux")]
fn main() {
    use captures_ui_probe::CaptureStore;
    use gtk::{
        gdk_pixbuf::{InterpType, PixbufLoader},
        glib,
        prelude::*,
    };
    use std::{
        cell::Cell,
        rc::Rc,
        sync::{Arc, mpsc},
        time::Duration,
    };

    gtk::init().expect("GTK requires a graphical desktop");
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title("Captures UI probe");
    window.set_default_size(800, 600);
    window.set_resizable(false);
    let column = gtk::Box::new(gtk::Orientation::Vertical, 16);
    column.set_border_width(24);
    let title = gtk::Label::new(Some("Captures · native GTK experiment"));
    title.set_xalign(0.0);
    let note = gtk::Label::new(Some(
        "Display screenshot only. Includes this window. No recording or editor.",
    ));
    note.set_xalign(0.0);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let capture = gtk::Button::with_label("Capture display");
    let save = gtk::Button::with_label("Save PNG");
    save.set_sensitive(false);
    row.pack_start(&capture, false, false, 0);
    row.pack_start(&save, false, false, 0);
    let preview = gtk::Image::new();
    preview.set_size_request(720, 360);
    let status = gtk::Label::new(Some("Ready to capture"));
    status.set_xalign(0.0);
    status.set_line_wrap(true);
    column.pack_start(&title, false, false, 0);
    column.pack_start(&note, false, false, 0);
    column.pack_start(&row, false, false, 0);
    column.pack_start(&preview, true, true, 0);
    column.pack_start(&status, false, false, 0);
    window.add(&column);
    let store = Arc::new(CaptureStore::default());
    capture.connect_clicked({
        let store = store.clone();
        let save = save.clone();
        let status = status.clone();
        let preview = preview.clone();
        move |button| {
            button.set_sensitive(false);
            save.set_sensitive(false);
            status.set_text("Capturing…");
            let store = store.clone();
            let (sender, receiver) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = sender.send(store.capture());
            });
            let capture = button.clone();
            let save = save.clone();
            let status = status.clone();
            let preview = preview.clone();
            // Poll only during a capture; no permanent timer in the idle app.
            glib::timeout_add_local(Duration::from_millis(16), move || {
                let result = match receiver.try_recv() {
                    Ok(result) => result,
                    Err(mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                    Err(mpsc::TryRecvError::Disconnected) => Err("Capture worker stopped".into()),
                };
                capture.set_sensitive(true);
                let result = result.and_then(|frame| {
                    let loader = PixbufLoader::new();
                    loader.write(&frame.png).map_err(|e| e.to_string())?;
                    loader.close().map_err(|e| e.to_string())?;
                    let pixbuf = loader.pixbuf().ok_or("No decoded preview")?;
                    let scale =
                        (720.0 / f64::from(frame.width)).min(360.0 / f64::from(frame.height));
                    let scaled = pixbuf
                        .scale_simple(
                            (f64::from(frame.width) * scale).round().max(1.0) as i32,
                            (f64::from(frame.height) * scale).round().max(1.0) as i32,
                            InterpType::Bilinear,
                        )
                        .ok_or("Cannot scale preview")?;
                    preview.set_from_pixbuf(Some(&scaled));
                    Ok(format!(
                        "{} × {} · capture + PNG {:.1} ms",
                        frame.width, frame.height, frame.capture_encode_ms
                    ))
                });
                save.set_sensitive(result.is_ok());
                status.set_text(&result.unwrap_or_else(|e| format!("Error: {e}")));
                glib::ControlFlow::Break
            });
        }
    });
    save.connect_clicked({
        let status = status.clone();
        move |_| status.set_text(&store.save().unwrap_or_else(|e| format!("Error: {e}")))
    });
    let signalled = Rc::new(Cell::new(false));
    window.connect_draw(move |_, _| {
        if !signalled.replace(true) {
            glib::idle_add_local_once(|| {
                let _ = captures_ui_probe::ready();
            });
        }
        glib::Propagation::Proceed
    });
    window.connect_delete_event(|_, _| {
        gtk::main_quit();
        glib::Propagation::Proceed
    });
    window.show_all();
    gtk::main();
}

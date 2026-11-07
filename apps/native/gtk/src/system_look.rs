//! The desktop's light or dark preference, for the System appearance. The
//! XDG Settings portal's `org.freedesktop.appearance` `color-scheme` (GNOME
//! 42 and later, KDE Plasma) decides when it states one. Otherwise GTK's
//! prefer-dark setting or a `-dark` theme name decides, the same signals
//! WebKitGTK reads for the shipping app's `prefers-color-scheme`. All three
//! are watched, so a System window follows the desktop while it runs.
use std::cell::RefCell;
use std::sync::atomic::{AtomicU32, Ordering};

use gtk::prelude::*;
use gtk::{gio, glib};

const NAMESPACE: &str = "org.freedesktop.appearance";
const KEY: &str = "color-scheme";
/// The portal's 0 (no preference), 1 (dark) or 2 (light); `UNKNOWN` until
/// it answers or when there is no portal.
const UNKNOWN: u32 = u32::MAX;
static PORTAL: AtomicU32 = AtomicU32::new(UNKNOWN);

thread_local! {
    static CHANGED: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
    /// Kept alive for its `SettingChanged` signal.
    static PROXY: RefCell<Option<gio::DBusProxy>> = const { RefCell::new(None) };
}

/// Whether the desktop prefers a dark look now.
pub fn prefers_dark() -> bool {
    match PORTAL.load(Ordering::Relaxed) {
        1 => true,
        2 => false,
        _ => gtk::Settings::default().is_some_and(|settings| {
            settings.is_gtk_application_prefer_dark_theme()
                || settings
                    .gtk_theme_name()
                    .is_some_and(|name| name.to_lowercase().ends_with("-dark"))
        }),
    }
}

/// Read the portal's preference before the first window is styled, so a
/// dark desktop does not open light and flip. Without a portal (or after a
/// one-second wait) GTK's settings decide.
pub fn start() {
    let Ok(proxy) = gio::DBusProxy::for_bus_sync(
        gio::BusType::Session,
        gio::DBusProxyFlags::DO_NOT_LOAD_PROPERTIES,
        None,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.Settings",
        gio::Cancellable::NONE,
    ) else {
        return;
    };
    // `Read` (every portal version) wraps the value in one or two variants.
    let reply = proxy.call_sync(
        "Read",
        Some(&(NAMESPACE, KEY).to_variant()),
        gio::DBusCallFlags::NONE,
        1_000,
        gio::Cancellable::NONE,
    );
    if let Some(scheme) = reply.ok().and_then(|reply| scheme(&reply)) {
        PORTAL.store(scheme, Ordering::Relaxed);
    }
    proxy.connect_g_signal(|_, _, signal, parameters| {
        if signal != "SettingChanged" {
            return;
        }
        if let Some((namespace, key, value)) = parameters.get::<(String, String, glib::Variant)>()
            && namespace == NAMESPACE
            && key == KEY
            && let Some(scheme) = scheme(&value)
        {
            PORTAL.store(scheme, Ordering::Relaxed);
            glib::MainContext::default().invoke(notify);
        }
    });
    PROXY.with(|slot| *slot.borrow_mut() = Some(proxy));
}

/// Call `changed` on the main loop whenever the preference may have changed.
pub fn watch(changed: impl Fn() + 'static) {
    CHANGED.with(|slot| *slot.borrow_mut() = Some(Box::new(changed)));
    if let Some(settings) = gtk::Settings::default() {
        settings.connect_gtk_application_prefer_dark_theme_notify(|_| notify());
        settings.connect_gtk_theme_name_notify(|_| notify());
    }
}

/// The `u32` inside however many variant and tuple layers wrap it.
fn scheme(value: &glib::Variant) -> Option<u32> {
    let mut value = value.clone();
    loop {
        if let Some(scheme) = value.get::<u32>() {
            return Some(scheme);
        }
        value = if value.is_type(glib::VariantTy::VARIANT) {
            value.as_variant()?
        } else if value.is_container() && value.n_children() == 1 {
            value.child_value(0)
        } else {
            return None;
        };
    }
}

fn notify() {
    CHANGED.with(|slot| {
        if let Some(changed) = slot.borrow().as_ref() {
            changed();
        }
    });
}

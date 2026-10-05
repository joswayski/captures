//! Portal-only still acquisition. Native host visibility/placement is a separate gate.

use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

use dbus::{
    Path,
    arg::{PropMap, Variant},
    blocking::Connection,
    message::MatchRule,
};
use image::RgbaImage;

use crate::{CaptureError, CaptureResult};

const DESKTOP: &str = "org.freedesktop.portal.Desktop";
const DESKTOP_PATH: &str = "/org/freedesktop/portal/desktop";
const SCREENSHOT: &str = "org.freedesktop.portal.Screenshot";
const REQUEST: &str = "org.freedesktop.portal.Request";
const CALL_TIMEOUT: Duration = Duration::from_secs(5);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Ask the desktop portal for a still, without X11 enumeration or direct-capture fallbacks.
///
/// Run on a worker. The caller must first unmap its capture-excluded windows: the
/// portal has no own-window exclusion API. There is deliberately no parent window.
/// The portal controls consent, image extent and cursor inclusion; this is not a
/// named-display/window capture and supplies no assumed desktop geometry.
///
/// `Ok(None)` means local or portal cancellation. Cancellation/timeout closes the
/// request without waiting for a Response (Close does not emit one). The timeout
/// is checked around bus setup; it cannot interrupt connection setup or image
/// decoding. Screenshot and owner lookup use at most five seconds or the remaining
/// deadline, whichever is shorter; subscription uses five seconds, Close one second.
/// The returned local image is read, never deleted or changed: the portal owns it.
pub fn portal_screenshot(
    timeout: Duration,
    cancelled: impl Fn() -> bool,
) -> CaptureResult<Option<RgbaImage>> {
    let started = Instant::now();
    if cancelled() {
        return Ok(None);
    }
    if timeout.is_zero() {
        return Err(timed_out());
    }
    let connection = Connection::new_session().map_err(backend_error)?;
    let token = format!("captures_{}", uuid::Uuid::new_v4().simple());
    let sender = connection
        .unique_name()
        .to_string()
        .trim_start_matches(':')
        .replace('.', "_");
    let expected = format!("{DESKTOP_PATH}/request/{sender}/{token}");
    let (tx, rx) = mpsc::channel();
    // Subscribe before Screenshot. Do not restrict the path until its method
    // reply: older portals can return a different handle and respond immediately.
    let rule = MatchRule::new_signal(REQUEST, "Response").with_sender(DESKTOP);
    connection
        .add_match(rule, move |_: (), _, message| {
            if let (Some(path), Some(sender)) = (message.path(), message.sender()) {
                let response = message
                    .read2::<u32, PropMap>()
                    .map_err(|error| error.to_string());
                let _ = tx.send((path.to_string(), sender.to_string(), response));
            }
            true
        })
        .map_err(backend_error)?;
    if cancelled() {
        return Ok(None);
    }
    let remaining = timeout.saturating_sub(started.elapsed());
    if remaining.is_zero() {
        return Err(timed_out());
    }
    let proxy = connection.with_proxy(DESKTOP, DESKTOP_PATH, remaining.min(CALL_TIMEOUT));
    let options: PropMap = [
        (
            "handle_token".into(),
            Variant(Box::new(token) as Box<dyn dbus::arg::RefArg>),
        ),
        (
            "modal".into(),
            Variant(Box::new(false) as Box<dyn dbus::arg::RefArg>),
        ),
        (
            "interactive".into(),
            Variant(Box::new(false) as Box<dyn dbus::arg::RefArg>),
        ),
    ]
    .into();
    let handle: Result<(Path<'static>,), _> =
        proxy.method_call(SCREENSHOT, "Screenshot", ("", options));
    let handle = match handle {
        Ok((handle,)) => handle,
        Err(error) => {
            // A failed/lost method reply does not prove the request wasn't made.
            close_request(&connection, &expected);
            return if cancelled() {
                Ok(None)
            } else if started.elapsed() >= timeout {
                Err(timed_out())
            } else {
                Err(backend_error(error))
            };
        }
    };
    // Directed signals bypass bus match rules. Check the unique portal owner as
    // well as the returned handle, never trusting a same-path signal from a peer.
    if cancelled() {
        close_request(&connection, &handle);
        return Ok(None);
    }
    let remaining = timeout.saturating_sub(started.elapsed());
    if remaining.is_zero() {
        close_request(&connection, &handle);
        return Err(timed_out());
    }
    let owner: Result<(String,), _> = connection
        .with_proxy(
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            remaining.min(CALL_TIMEOUT),
        )
        .method_call("org.freedesktop.DBus", "GetNameOwner", (DESKTOP,));
    let owner = match owner {
        Ok((owner,)) => owner,
        Err(error) => {
            close_request(&connection, &handle);
            return Err(backend_error(error));
        }
    };
    loop {
        if cancelled() {
            close_request(&connection, &handle);
            return Ok(None);
        }
        let remaining = timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            close_request(&connection, &handle);
            return Err(timed_out());
        }
        for (path, sender, response) in rx.try_iter() {
            if path == handle.as_ref() && sender == owner {
                return decode_response(response.map_err(backend_error)?);
            }
        }
        if let Err(error) = connection.process(remaining.min(POLL_INTERVAL)) {
            close_request(&connection, &handle);
            return Err(backend_error(error));
        }
    }
}

fn close_request(connection: &Connection, path: &str) {
    // Closing a disappeared/completed request can fail. Keep the original outcome.
    let _: Result<(), _> = connection
        .with_proxy(DESKTOP, path, CLOSE_TIMEOUT)
        .method_call(REQUEST, "Close", ());
}

fn timed_out() -> CaptureError {
    CaptureError::Backend("Timed out waiting for the desktop screenshot portal.".into())
}

fn backend_error(error: impl std::fmt::Display) -> CaptureError {
    CaptureError::Backend(format!("Desktop screenshot portal: {error}"))
}

fn decode_response((status, results): (u32, PropMap)) -> CaptureResult<Option<RgbaImage>> {
    match status {
        1 => return Ok(None),
        0 => {}
        _ => return Err(backend_error("the request did not succeed")),
    }
    let uri = results
        .get("uri")
        .and_then(|value| value.0.as_str())
        .ok_or_else(|| backend_error("success response has no screenshot URI"))?;
    let uri = url::Url::parse(uri).map_err(backend_error)?;
    if uri.query().is_some() || uri.fragment().is_some() {
        return Err(backend_error(
            "screenshot URI must be a local file without a query or fragment",
        ));
    }
    let path = uri
        .to_file_path()
        .map_err(|_| backend_error("screenshot URI is not a local file"))?;
    let image = image::ImageReader::open(path)
        .map_err(backend_error)?
        .with_guessed_format()
        .map_err(backend_error)?
        .decode()
        .map_err(|error| CaptureError::Image(error.to_string()))?
        .into_rgba8();
    Ok(Some(image))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(status: u32, uri: &str) -> (u32, PropMap) {
        (
            status,
            [(
                "uri".into(),
                Variant(Box::new(uri.to_owned()) as Box<dyn dbus::arg::RefArg>),
            )]
            .into(),
        )
    }

    #[test]
    fn cancellation_and_failure_do_not_follow_the_uri() {
        assert!(
            decode_response(response(1, "file:///missing.png"))
                .unwrap()
                .is_none()
        );
        assert!(decode_response(response(2, "file:///missing.png")).is_err());
        assert!(decode_response(response(91, "file:///missing.png")).is_err());
        assert!(decode_response((0, PropMap::new())).is_err());
    }

    #[test]
    fn only_local_unambiguous_file_uris_are_read() {
        for uri in [
            "https://example.com/capture.png",
            "file://remote/capture.png",
            "file:///capture.png?query",
            "file:///capture.png#fragment",
            "not a URI",
        ] {
            assert!(decode_response(response(0, uri)).is_err(), "{uri}");
        }
    }

    #[test]
    fn encoded_unicode_file_is_read_exactly_and_never_removed() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("still é #1.png");
        let image = RgbaImage::from_raw(
            3,
            2,
            vec![
                1, 2, 3, 4, 250, 17, 99, 255, 0, 127, 255, 63, 19, 211, 7, 128, 88, 44, 222, 200,
                5, 6, 7, 8,
            ],
        )
        .unwrap();
        image.save(&path).unwrap();
        let before = std::fs::read(&path).unwrap();
        let captured = decode_response(response(
            0,
            url::Url::from_file_path(&path).unwrap().as_str(),
        ))
        .unwrap()
        .unwrap();
        assert_eq!(captured.dimensions(), (3, 2));
        assert_eq!(captured.as_raw(), image.as_raw());
        assert_eq!(std::fs::read(path).unwrap(), before);
    }
}

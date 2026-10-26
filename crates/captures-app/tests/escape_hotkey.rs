//! Cross-client ownership, not just the hotkey library's internal registry.
#![cfg(target_os = "linux")]

use std::time::{Duration, Instant};

use global_hotkey::{
    GlobalHotKeyManager,
    hotkey::{Code, HotKey},
};

#[test]
#[ignore = "requires CAPTURES_TEST_PRIVATE_X11=1 on a disposable Xvfb display"]
fn unregister_returns_escape_to_other_applications_without_another_capture() {
    assert_eq!(
        std::env::var("CAPTURES_TEST_PRIVATE_X11").as_deref(),
        Ok("1")
    );
    let captures = GlobalHotKeyManager::new().unwrap();
    let other_app = GlobalHotKeyManager::new().unwrap();
    let escape = HotKey::new(None, Code::Escape);

    for _ in 0..3 {
        captures.register(escape).unwrap();
        assert!(other_app.register(escape).is_err(), "capture owns Escape");
        captures.unregister(escape).unwrap();

        // Never send another request through `captures`: that could flush an
        // incorrectly buffered ungrab and hide the actual regression.
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match other_app.register(escape) {
                Ok(()) => break,
                Err(error) => {
                    assert!(
                        Instant::now() < deadline,
                        "Escape is still grabbed: {error}"
                    );
                    std::thread::sleep(Duration::from_millis(25));
                }
            }
        }
        other_app.unregister(escape).unwrap();
    }
}

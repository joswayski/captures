#![allow(non_upper_case_globals)]
//! UI Automation for the recording controls, following shipping's
//! `RecordingHud` DOM: the `<main>` window, the privacy line, the elapsed
//! time and state, each button by its `aria-label` (Invoke, disabled when
//! the button is), the microphone level and the error line
//! (`role="alert"`). The window never activates (`WS_EX_NOACTIVATE`), so no
//! part reports keyboard focus; Invoke presses a button through the same
//! path as a click.
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Com::*;
use windows::Win32::System::Ole::*;
use windows::Win32::System::Variant::*;
use windows::Win32::UI::Accessibility::*;
use windows::core::*;

use crate::hud::{Hud, Part};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Id {
    Root,
    Part(Part),
}

/// What changed since the last refresh, raised outside the recorder borrow.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A button's name flipped (Pause/Resume, Mute/Unmute, Restart/Retry).
    Name(Part, String),
    /// Parts appeared or went away (the meter, the error line).
    Structure,
    /// The error line appeared or changed.
    Alert(String),
}

/// The parts whose changes are announced, with their names. The time and
/// level change continuously and are read on demand instead.
pub fn announced(h: &Hud) -> Vec<(Part, String)> {
    h.parts()
        .into_iter()
        .map(|(part, _)| part)
        .filter(|part| !matches!(part, Part::Time | Part::Level))
        .map(|part| (part, h.part_name(part)))
        .collect()
}

/// The events between two `announced` lists.
pub fn diff(before: &[(Part, String)], after: &[(Part, String)]) -> Vec<Event> {
    let mut events = Vec::new();
    let kinds = |list: &[(Part, String)]| list.iter().map(|(p, _)| *p).collect::<Vec<_>>();
    if kinds(before) != kinds(after) {
        events.push(Event::Structure);
    }
    for (part, name) in after {
        let old = before.iter().find(|(p, _)| p == part).map(|(_, n)| n);
        match part {
            Part::Error if old != Some(name) => events.push(Event::Alert(name.clone())),
            Part::Button(_) if old.is_some_and(|old| old != name) => {
                events.push(Event::Name(*part, name.clone()));
            }
            _ => {}
        }
    }
    events
}

fn unavailable() -> Error {
    HRESULT(UIA_E_ELEMENTNOTAVAILABLE as _).into()
}

fn not_enabled() -> Error {
    HRESULT(UIA_E_ELEMENTNOTENABLED as _).into()
}

fn with<R>(f: impl FnOnce(&Hud, HWND, f32) -> Result<R>) -> Result<R> {
    crate::recorder::with_hud(f).unwrap_or_else(|| Err(unavailable()))
}

fn children(h: &Hud, id: Id) -> Vec<Id> {
    match id {
        Id::Root => h.parts().into_iter().map(|(p, _)| Id::Part(p)).collect(),
        Id::Part(_) => Vec::new(),
    }
}

fn enabled(h: &Hud, id: Id) -> bool {
    match id {
        Id::Part(Part::Button(control)) => h.view().control(control).enabled,
        _ => true,
    }
}

fn runtime_id(id: Id) -> [i32; 4] {
    let (kind, a) = match id {
        Id::Root => (0, 0),
        Id::Part(Part::Privacy) => (1, 0),
        Id::Part(Part::Time) => (2, 0),
        Id::Part(Part::Status) => (3, 0),
        Id::Part(Part::Button(control)) => (
            4,
            captures_app::recording_hud::Control::ALL
                .iter()
                .position(|c| *c == control)
                .unwrap_or(0) as i32,
        ),
        Id::Part(Part::Level) => (5, 0),
        Id::Part(Part::Error) => (6, 0),
    };
    // A distinct first part keeps these apart from the other windows' ids.
    [UiaAppendRuntimeId as i32, 400 + kind, a, 0]
}

fn screen_rect(
    hwnd: HWND,
    scale: f32,
    r: windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F,
) -> UiaRect {
    let mut origin = POINT::default();
    unsafe {
        let _ = ClientToScreen(hwnd, &mut origin);
    }
    let s = f64::from(scale);
    UiaRect {
        left: f64::from(origin.x) + f64::from(r.left) * s,
        top: f64::from(origin.y) + f64::from(r.top) * s,
        width: f64::from(r.right - r.left) * s,
        height: f64::from(r.bottom - r.top) * s,
    }
}

fn safe_array_i32(values: &[i32]) -> *mut SAFEARRAY {
    unsafe {
        let array = SafeArrayCreateVector(VT_I4, 0, values.len() as u32);
        for (i, value) in values.iter().enumerate() {
            let _ = SafeArrayPutElement(array, &(i as i32), value as *const i32 as _);
        }
        array
    }
}

#[implement(
    IRawElementProviderSimple,
    IRawElementProviderFragment,
    IRawElementProviderFragmentRoot,
    IInvokeProvider
)]
pub struct Node {
    id: Id,
}

pub fn node(id: Id) -> IRawElementProviderSimple {
    Node { id }.into()
}

fn fragment(id: Id) -> IRawElementProviderFragment {
    Node { id }.into()
}

impl IRawElementProviderSimple_Impl for Node_Impl {
    fn ProviderOptions(&self) -> Result<ProviderOptions> {
        Ok(ProviderOptions_ServerSideProvider | ProviderOptions_UseComThreading)
    }

    fn GetPatternProvider(&self, pattern: UIA_PATTERN_ID) -> Result<IUnknown> {
        if matches!(self.id, Id::Part(Part::Button(_))) && pattern == UIA_InvokePatternId {
            Ok(self.to_interface::<IUnknown>())
        } else {
            Err(Error::empty())
        }
    }

    fn GetPropertyValue(&self, property: UIA_PROPERTY_ID) -> Result<VARIANT> {
        let id = self.id;
        with(|h, _, _| {
            Ok(match property {
                UIA_NamePropertyId => VARIANT::from(BSTR::from(match id {
                    Id::Root => "Recording controls".to_string(),
                    Id::Part(part) => h.part_name(part),
                })),
                UIA_ControlTypePropertyId => VARIANT::from(match id {
                    Id::Root => UIA_PaneControlTypeId.0,
                    Id::Part(Part::Button(_)) => UIA_ButtonControlTypeId.0,
                    Id::Part(_) => UIA_TextControlTypeId.0,
                }),
                // `<main>` is the main landmark.
                UIA_LandmarkTypePropertyId if id == Id::Root => {
                    VARIANT::from(UIA_MainLandmarkTypeId.0)
                }
                UIA_LiveSettingPropertyId if id == Id::Part(Part::Error) => {
                    VARIANT::from(Assertive.0)
                }
                UIA_AutomationIdPropertyId => {
                    VARIANT::from(BSTR::from(format!("recording-controls-{id:?}")))
                }
                UIA_FrameworkIdPropertyId => VARIANT::from(BSTR::from("Captures")),
                UIA_IsKeyboardFocusablePropertyId | UIA_HasKeyboardFocusPropertyId => {
                    VARIANT::from(false)
                }
                UIA_IsEnabledPropertyId => VARIANT::from(enabled(h, id)),
                UIA_IsOffscreenPropertyId => VARIANT::from(false),
                UIA_IsControlElementPropertyId | UIA_IsContentElementPropertyId => {
                    VARIANT::from(true)
                }
                _ => VARIANT::default(),
            })
        })
    }

    fn HostRawElementProvider(&self) -> Result<IRawElementProviderSimple> {
        if self.id == Id::Root {
            let hwnd = with(|_, hwnd, _| Ok(hwnd))?;
            unsafe { UiaHostProviderFromHwnd(hwnd) }
        } else {
            Err(Error::empty())
        }
    }
}

impl IRawElementProviderFragment_Impl for Node_Impl {
    fn Navigate(&self, direction: NavigateDirection) -> Result<IRawElementProviderFragment> {
        let id = self.id;
        let next = with(|h, _, _| {
            let siblings = children(h, Id::Root);
            if id != Id::Root && !siblings.contains(&id) {
                return Err(unavailable());
            }
            Ok(match (id, direction) {
                (Id::Part(_), NavigateDirection_Parent) => Some(Id::Root),
                (Id::Root, NavigateDirection_FirstChild) => siblings.first().copied(),
                (Id::Root, NavigateDirection_LastChild) => siblings.last().copied(),
                (
                    Id::Part(_),
                    NavigateDirection_NextSibling | NavigateDirection_PreviousSibling,
                ) => {
                    let i = siblings.iter().position(|s| *s == id).unwrap_or(0);
                    if direction == NavigateDirection_NextSibling {
                        siblings.get(i + 1).copied()
                    } else {
                        i.checked_sub(1).map(|j| siblings[j])
                    }
                }
                _ => None,
            })
        })?;
        next.map(fragment).ok_or_else(Error::empty)
    }

    fn GetRuntimeId(&self) -> Result<*mut SAFEARRAY> {
        if self.id == Id::Root {
            return Ok(std::ptr::null_mut());
        }
        Ok(safe_array_i32(&runtime_id(self.id)))
    }

    fn BoundingRectangle(&self) -> Result<UiaRect> {
        let Id::Part(part) = self.id else {
            return Ok(UiaRect::default());
        };
        with(|h, hwnd, scale| {
            let (_, r) = h
                .parts()
                .into_iter()
                .find(|(p, _)| *p == part)
                .ok_or_else(unavailable)?;
            Ok(screen_rect(hwnd, scale, r))
        })
    }

    fn GetEmbeddedFragmentRoots(&self) -> Result<*mut SAFEARRAY> {
        Ok(std::ptr::null_mut())
    }

    fn SetFocus(&self) -> Result<()> {
        // The controls never take focus over the app being recorded.
        Ok(())
    }

    fn FragmentRoot(&self) -> Result<IRawElementProviderFragmentRoot> {
        Ok(Node { id: Id::Root }.into())
    }
}

impl IRawElementProviderFragmentRoot_Impl for Node_Impl {
    fn ElementProviderFromPoint(&self, x: f64, y: f64) -> Result<IRawElementProviderFragment> {
        let hit = with(|h, hwnd, scale| {
            let mut origin = POINT::default();
            unsafe {
                let _ = ClientToScreen(hwnd, &mut origin);
            }
            let s = f64::from(scale);
            let (px, py) = (
                ((x - f64::from(origin.x)) / s) as f32,
                ((y - f64::from(origin.y)) / s) as f32,
            );
            Ok(h.parts()
                .into_iter()
                .find(|(_, r)| px >= r.left && px < r.right && py >= r.top && py < r.bottom)
                .map_or(Id::Root, |(p, _)| Id::Part(p)))
        })?;
        Ok(fragment(hit))
    }

    fn GetFocus(&self) -> Result<IRawElementProviderFragment> {
        Err(Error::empty())
    }
}

impl IInvokeProvider_Impl for Node_Impl {
    fn Invoke(&self) -> Result<()> {
        let Id::Part(Part::Button(control)) = self.id else {
            return Err(Error::empty());
        };
        let hwnd = with(|h, hwnd, _| {
            if h.view().control(control).enabled {
                Ok(hwnd)
            } else {
                Err(not_enabled())
            }
        })?;
        crate::recorder::post_invoke(hwnd, control);
        Ok(())
    }
}

/// Raise the controls' accessibility events, outside the recorder borrow.
pub fn raise(events: &[Event]) {
    unsafe {
        if events.is_empty() || !UiaClientsAreListening().as_bool() {
            return;
        }
        for event in events {
            match event {
                Event::Name(part, name) => {
                    let _ = UiaRaiseAutomationPropertyChangedEvent(
                        &node(Id::Part(*part)),
                        UIA_NamePropertyId,
                        &VARIANT::default(),
                        &VARIANT::from(BSTR::from(name.as_str())),
                    );
                }
                Event::Structure => {
                    let _ = UiaRaiseStructureChangedEvent(
                        &node(Id::Root),
                        StructureChangeType_ChildrenInvalidated,
                        runtime_id(Id::Root).as_mut_ptr(),
                        4,
                    );
                }
                Event::Alert(text) => {
                    let alert = node(Id::Part(Part::Error));
                    let _ = UiaRaiseAutomationEvent(&alert, UIA_SystemAlertEventId);
                    let _ = UiaRaiseNotificationEvent(
                        &alert,
                        NotificationKind_Other,
                        NotificationProcessing_ImportantMostRecent,
                        &BSTR::from(text.as_str()),
                        &BSTR::from("recording-controls-error"),
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use captures_app::recording_hud::Control;
    use captures_recording::RecordingState;

    #[test]
    fn diff_announces_flipped_names_new_parts_and_errors_only() {
        let mut h = Hud::new(true);
        h.input.state = RecordingState::Recording;
        h.input.busy = false;
        let before = announced(&h);
        // The clock ticking is not announced.
        h.elapsed_ms = 5_000;
        assert_eq!(diff(&before, &announced(&h)), vec![]);
        h.input.state = RecordingState::Paused;
        assert_eq!(
            diff(&before, &announced(&h)),
            vec![Event::Name(
                Part::Button(Control::PauseResume),
                "Resume recording".into()
            )]
        );
        let paused = announced(&h);
        h.error = Some("ffmpeg exited".into());
        let events = diff(&paused, &announced(&h));
        assert_eq!(events[0], Event::Structure);
        assert!(matches!(&events[1], Event::Alert(text) if text.contains("ffmpeg")));
        assert_eq!(events.len(), 2);
    }
}

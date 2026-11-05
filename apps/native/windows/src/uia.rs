#![allow(non_upper_case_globals)]
//! UI Automation for Preferences: the window, the section navigation (Invoke
//! buttons), the header, and each settings card as a group of switches with
//! the Toggle pattern, their title as name and their description as help
//! text. Providers are node ids that read live scene state on the UI thread.
use captures_app::preferences::{self, SECTIONS};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Com::*;
use windows::Win32::System::Ole::*;
use windows::Win32::System::Variant::*;
use windows::Win32::UI::Accessibility::*;
use windows::core::*;

use crate::host::{Host, with_host};
use crate::scene::{CARDS, Event, NAV_WIDTH, Rect, Target};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Id {
    Root,
    Nav,
    NavItem(usize),
    Title,
    Subtitle,
    History,
    Card(usize),
    Switch(usize),
}

fn unavailable() -> Error {
    HRESULT(UIA_E_ELEMENTNOTAVAILABLE as _).into()
}

fn with<R>(f: impl FnOnce(&mut Host) -> Result<R>) -> Result<R> {
    with_host(f).unwrap_or_else(|| Err(unavailable()))
}

fn target(id: Id) -> Option<Target> {
    match id {
        Id::NavItem(i) => Some(Target::Nav(i)),
        Id::History => Some(Target::History),
        Id::Switch(row) => Some(Target::Row(row)),
        _ => None,
    }
}

fn id_of(target: Target) -> Id {
    match target {
        Target::Nav(i) => Id::NavItem(i),
        Target::History => Id::History,
        Target::Row(row) => Id::Switch(row),
    }
}

fn children(id: Id) -> Vec<Id> {
    match id {
        Id::Root => {
            let mut list = vec![Id::Nav, Id::Title, Id::Subtitle, Id::History];
            list.extend((0..CARDS.len()).map(Id::Card));
            list
        }
        Id::Nav => (0..SECTIONS.len()).map(Id::NavItem).collect(),
        Id::Card(card) => CARDS[card].1.iter().map(|&row| Id::Switch(row)).collect(),
        _ => Vec::new(),
    }
}

fn parent(id: Id) -> Option<Id> {
    Some(match id {
        Id::Root => return None,
        Id::NavItem(_) => Id::Nav,
        Id::Switch(row) => Id::Card(CARDS.iter().position(|(_, rows)| rows.contains(&row))?),
        _ => Id::Root,
    })
}

fn rect(host: &Host, id: Id) -> Rect {
    let scene = &host.scene;
    match id {
        Id::Root => Rect::new(0., 0., scene.w, scene.h),
        Id::Nav => Rect::new(0., 0., NAV_WIDTH, scene.h),
        Id::Title => scene.header_text_rect(false),
        Id::Subtitle => scene.header_text_rect(true),
        Id::Card(card) => scene.card_rect(card),
        Id::Switch(row) => scene.row_rect(row),
        Id::NavItem(_) | Id::History => scene.target_rect(target(id).expect("target")),
    }
}

fn name(host: &Host, id: Id) -> String {
    match id {
        Id::Root => captures_app::app_windows::PREFERENCES.title.into(),
        Id::Nav => "Preferences sections".into(),
        Id::NavItem(i) => SECTIONS[i].title.into(),
        Id::Title => preferences::TITLE.into(),
        Id::Subtitle => preferences::SUBTITLE.into(),
        Id::History => preferences::HISTORY_ACTION.into(),
        Id::Card(card) => SECTIONS[CARDS[card].0].title.into(),
        Id::Switch(row) => host.scene.rows[row].title.into(),
    }
}

fn help(host: &Host, id: Id) -> String {
    match id {
        Id::Card(card) => SECTIONS[CARDS[card].0].description.into(),
        Id::Switch(row) => host.scene.rows[row].detail.clone(),
        _ => String::new(),
    }
}

fn control_type(id: Id) -> UIA_CONTROLTYPE_ID {
    match id {
        Id::Root => UIA_WindowControlTypeId,
        Id::Nav | Id::Card(_) => UIA_GroupControlTypeId,
        Id::Title | Id::Subtitle => UIA_TextControlTypeId,
        // Like WinUI's ToggleSwitch: a button with the Toggle pattern.
        Id::NavItem(_) | Id::History | Id::Switch(_) => UIA_ButtonControlTypeId,
    }
}

fn runtime_id(id: Id) -> [i32; 3] {
    let (kind, index) = match id {
        Id::Root => (0, 0),
        Id::Nav => (1, 0),
        Id::NavItem(i) => (2, i),
        Id::Title => (3, 0),
        Id::Subtitle => (4, 0),
        Id::History => (5, 0),
        Id::Card(i) => (6, i),
        Id::Switch(i) => (7, i),
    };
    [UiaAppendRuntimeId as i32, kind, index as i32]
}

fn screen_rect(host: &Host, r: Rect) -> UiaRect {
    let mut origin = POINT::default();
    unsafe {
        let _ = ClientToScreen(host.hwnd, &mut origin);
    }
    let s = f64::from(host.scene.scale);
    UiaRect {
        left: f64::from(origin.x) + f64::from(r.x) * s,
        top: f64::from(origin.y) + f64::from(r.y) * s,
        width: f64::from(r.w) * s,
        height: f64::from(r.h) * s,
    }
}

fn offscreen(host: &Host, id: Id) -> bool {
    match id {
        Id::Card(_) | Id::Switch(_) => {
            let (r, v) = (rect(host, id), host.scene.viewport());
            r.y + r.h <= v.y || r.y >= v.y + v.h
        }
        _ => false,
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
    IInvokeProvider,
    IToggleProvider
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
        let supported = match self.id {
            Id::NavItem(_) | Id::History => pattern == UIA_InvokePatternId,
            Id::Switch(_) => pattern == UIA_TogglePatternId,
            _ => false,
        };
        if supported {
            Ok(self.to_interface::<IUnknown>())
        } else {
            Err(Error::empty())
        }
    }

    fn GetPropertyValue(&self, property: UIA_PROPERTY_ID) -> Result<VARIANT> {
        let id = self.id;
        with(|host| {
            let scene = &host.scene;
            let enabled = match id {
                Id::Switch(row) => scene.rows[row].enabled,
                _ => true,
            };
            Ok(match property {
                UIA_NamePropertyId => VARIANT::from(BSTR::from(name(host, id))),
                UIA_HelpTextPropertyId => VARIANT::from(BSTR::from(help(host, id))),
                UIA_ControlTypePropertyId => VARIANT::from(control_type(id).0),
                UIA_LocalizedControlTypePropertyId if matches!(id, Id::Switch(_)) => {
                    VARIANT::from(BSTR::from("toggle switch"))
                }
                UIA_LandmarkTypePropertyId if id == Id::Nav => {
                    VARIANT::from(UIA_NavigationLandmarkTypeId.0)
                }
                UIA_AutomationIdPropertyId => VARIANT::from(BSTR::from(format!("{id:?}"))),
                UIA_FrameworkIdPropertyId => VARIANT::from(BSTR::from("Captures")),
                UIA_IsKeyboardFocusablePropertyId => VARIANT::from(target(id).is_some() && enabled),
                UIA_HasKeyboardFocusPropertyId => {
                    VARIANT::from(target(id).is_some() && scene.focus == target(id))
                }
                UIA_IsEnabledPropertyId => VARIANT::from(enabled),
                UIA_IsOffscreenPropertyId => VARIANT::from(offscreen(host, id)),
                UIA_IsControlElementPropertyId | UIA_IsContentElementPropertyId => {
                    VARIANT::from(true)
                }
                _ => VARIANT::default(),
            })
        })
    }

    fn HostRawElementProvider(&self) -> Result<IRawElementProviderSimple> {
        if self.id == Id::Root {
            let hwnd = with(|host| Ok(host.hwnd))?;
            unsafe { UiaHostProviderFromHwnd(hwnd) }
        } else {
            Err(Error::empty())
        }
    }
}

impl IRawElementProviderFragment_Impl for Node_Impl {
    fn Navigate(&self, direction: NavigateDirection) -> Result<IRawElementProviderFragment> {
        let id = self.id;
        let next = match direction {
            NavigateDirection_Parent => parent(id),
            NavigateDirection_FirstChild => children(id).first().copied(),
            NavigateDirection_LastChild => children(id).last().copied(),
            NavigateDirection_NextSibling | NavigateDirection_PreviousSibling => parent(id)
                .and_then(|p| {
                    let siblings = children(p);
                    let i = siblings.iter().position(|s| *s == id)?;
                    if direction == NavigateDirection_NextSibling {
                        siblings.get(i + 1).copied()
                    } else {
                        i.checked_sub(1).map(|j| siblings[j])
                    }
                }),
            _ => None,
        };
        next.map(fragment).ok_or_else(Error::empty)
    }

    fn GetRuntimeId(&self) -> Result<*mut SAFEARRAY> {
        if self.id == Id::Root {
            return Ok(std::ptr::null_mut());
        }
        Ok(safe_array_i32(&runtime_id(self.id)))
    }

    fn BoundingRectangle(&self) -> Result<UiaRect> {
        let id = self.id;
        if id == Id::Root {
            return Ok(UiaRect::default());
        }
        with(|host| Ok(screen_rect(host, rect(host, id))))
    }

    fn GetEmbeddedFragmentRoots(&self) -> Result<*mut SAFEARRAY> {
        Ok(std::ptr::null_mut())
    }

    fn SetFocus(&self) -> Result<()> {
        let id = self.id;
        with(|host| {
            if let Some(target) = target(id) {
                host.scene.set_focus(Some(target), true);
                host.need = true;
            }
            Ok(())
        })
    }

    fn FragmentRoot(&self) -> Result<IRawElementProviderFragmentRoot> {
        Ok(Node { id: Id::Root }.into())
    }
}

impl IRawElementProviderFragmentRoot_Impl for Node_Impl {
    fn ElementProviderFromPoint(&self, x: f64, y: f64) -> Result<IRawElementProviderFragment> {
        let hit = with(|host| {
            let mut origin = POINT::default();
            unsafe {
                let _ = ClientToScreen(host.hwnd, &mut origin);
            }
            let s = f64::from(host.scene.scale);
            let (px, py) = (
                ((x - f64::from(origin.x)) / s) as f32,
                ((y - f64::from(origin.y)) / s) as f32,
            );
            if let Some(target) = host.scene.hit(px, py) {
                return Ok(id_of(target));
            }
            for id in children(Id::Root) {
                if rect(host, id).contains(px, py) {
                    return Ok(id);
                }
            }
            Ok(Id::Root)
        })?;
        Ok(fragment(hit))
    }

    fn GetFocus(&self) -> Result<IRawElementProviderFragment> {
        let focus = with(|host| Ok(host.scene.focus))?;
        focus
            .map(|target| fragment(id_of(target)))
            .ok_or_else(Error::empty)
    }
}

impl IInvokeProvider_Impl for Node_Impl {
    fn Invoke(&self) -> Result<()> {
        let id = self.id;
        with(|host| {
            if let Some(target) = target(id) {
                host.activate(target);
            }
            Ok(())
        })
    }
}

impl IToggleProvider_Impl for Node_Impl {
    fn Toggle(&self) -> Result<()> {
        let id = self.id;
        with(|host| {
            if let Id::Switch(row) = id {
                if !host.scene.rows[row].enabled {
                    return Err(HRESULT(UIA_E_ELEMENTNOTENABLED as _).into());
                }
                host.activate(Target::Row(row));
            }
            Ok(())
        })
    }

    fn ToggleState(&self) -> Result<ToggleState> {
        let id = self.id;
        with(|host| match id {
            Id::Switch(row) if host.scene.rows[row].on => Ok(ToggleState_On),
            _ => Ok(ToggleState_Off),
        })
    }
}

/// Raise scene events; called with the host released, since clients may call
/// straight back into the providers.
pub fn raise(events: &[(Event, bool, String)]) {
    unsafe {
        if !UiaClientsAreListening().as_bool() {
            return;
        }
        for (event, on, detail) in events {
            match *event {
                Event::Focus(target) => {
                    let _ = UiaRaiseAutomationEvent(
                        &node(id_of(target)),
                        UIA_AutomationFocusChangedEventId,
                    );
                }
                Event::Toggled(row) => {
                    let state = |on: bool| VARIANT::from(if on { 1i32 } else { 0i32 });
                    let _ = UiaRaiseAutomationPropertyChangedEvent(
                        &node(Id::Switch(row)),
                        UIA_ToggleToggleStatePropertyId,
                        &state(!on),
                        &state(*on),
                    );
                }
                Event::Detail(row) => {
                    let _ = UiaRaiseAutomationPropertyChangedEvent(
                        &node(Id::Switch(row)),
                        UIA_HelpTextPropertyId,
                        &VARIANT::default(),
                        &VARIANT::from(BSTR::from(detail.as_str())),
                    );
                }
            }
        }
    }
}

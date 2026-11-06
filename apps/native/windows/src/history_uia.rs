#![allow(non_upper_case_globals)]
//! UI Automation for Capture History, following the shipping DOM: Back to
//! Preferences, the `h1` and lede, Cancel and Delete all, the "Filter
//! captures" group of pressed-state buttons (Toggle), the page error
//! (`role="alert"`), the empty state, and the "Recent captures" region of
//! `article` cards. Each card has its open button (disabled: only Edit opens
//! the editor in this host so far), "File missing", the trash button, the
//! date and details, the dropped-frame warning, its two actions and its
//! error. Every card is in the tree, virtualised or not; cards outside the
//! viewport report IsOffscreen.
use captures_app::history_view;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Com::*;
use windows::Win32::System::Ole::*;
use windows::Win32::System::Variant::*;
use windows::Win32::UI::Accessibility::*;
use windows::core::*;

use crate::history::{
    Event, FILTERED_BODY, FILTERED_TITLE, FILTERS, History, Target, with_history,
};
use crate::scene::Rect;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Id {
    Root,
    Back,
    Title,
    Lede,
    Cancel,
    DeleteAll,
    Filters,
    Filter(usize),
    Error,
    Empty,
    EmptyTitle,
    EmptyBody,
    Grid,
    Card(u32),
    /// `.history-image-open`.
    Open(u32),
    Missing(u32),
    Delete(u32),
    Date(u32),
    Details(u32),
    Warning(u32),
    Action(u32, usize),
    CardError(u32),
}

fn unavailable() -> Error {
    HRESULT(UIA_E_ELEMENTNOTAVAILABLE as _).into()
}

fn not_enabled() -> Error {
    HRESULT(UIA_E_ELEMENTNOTENABLED as _).into()
}

fn with<R>(f: impl FnOnce(&mut History) -> Result<R>) -> Result<R> {
    with_history(f).unwrap_or_else(|| Err(unavailable()))
}

fn target(id: Id) -> Option<Target> {
    Some(match id {
        Id::Back => Target::Back,
        Id::Cancel => Target::Cancel,
        Id::DeleteAll => Target::DeleteAll,
        Id::Filter(i) => Target::Filter(i),
        Id::Delete(serial) => Target::Delete(serial),
        Id::Action(serial, i) => Target::Action(serial, i),
        _ => return None,
    })
}

fn id_of(target: Target) -> Id {
    match target {
        Target::Back => Id::Back,
        Target::Cancel => Id::Cancel,
        Target::DeleteAll => Id::DeleteAll,
        Target::Filter(i) => Id::Filter(i),
        Target::Delete(serial) => Id::Delete(serial),
        Target::Action(serial, i) => Id::Action(serial, i),
    }
}

fn empty_shown(h: &History) -> bool {
    h.loading || h.shown.is_empty()
}

fn children(h: &History, id: Id) -> Vec<Id> {
    match id {
        Id::Root => {
            let mut list = vec![Id::Back, Id::Title, Id::Lede];
            if h.has_entries() {
                if h.confirm_all.is_some() {
                    list.push(Id::Cancel);
                }
                list.extend([Id::DeleteAll, Id::Filters]);
            }
            if h.error.is_some() {
                list.push(Id::Error);
            }
            list.push(if empty_shown(h) { Id::Empty } else { Id::Grid });
            list
        }
        Id::Filters => (0..FILTERS.len()).map(Id::Filter).collect(),
        Id::Empty if h.loading => vec![Id::EmptyTitle],
        Id::Empty => vec![Id::EmptyTitle, Id::EmptyBody],
        Id::Grid => h
            .shown
            .iter()
            .map(|&i| Id::Card(h.items[i].serial))
            .collect(),
        Id::Card(serial) => {
            let Some(item) = h.item(serial) else {
                return Vec::new();
            };
            let mut list = vec![Id::Open(serial)];
            if item.card.missing {
                list.push(Id::Missing(serial));
            }
            list.extend([Id::Delete(serial), Id::Date(serial), Id::Details(serial)]);
            if item.card.warning.is_some() {
                list.push(Id::Warning(serial));
            }
            list.extend((0..item.card.actions.len()).map(|i| Id::Action(serial, i)));
            if item.error.is_some() {
                list.push(Id::CardError(serial));
            }
            list
        }
        _ => Vec::new(),
    }
}

fn parent(id: Id) -> Option<Id> {
    Some(match id {
        Id::Root => return None,
        Id::Filter(_) => Id::Filters,
        Id::EmptyTitle | Id::EmptyBody => Id::Empty,
        Id::Card(_) => Id::Grid,
        Id::Open(s)
        | Id::Missing(s)
        | Id::Delete(s)
        | Id::Date(s)
        | Id::Details(s)
        | Id::Warning(s)
        | Id::Action(s, _)
        | Id::CardError(s) => Id::Card(s),
        _ => Id::Root,
    })
}

/// Client-area rect in DIPs.
fn rect(h: &History, id: Id) -> Rect {
    let l = &h.l;
    let card = |serial: u32| {
        h.shown_index(serial)
            .map_or_else(Rect::default, |k| h.client(h.card_rect(k)))
    };
    let inside = |serial: u32, x: f32, y: f32, w: f32, hh: f32| {
        let c = card(serial);
        Rect::new(c.x + x, c.y + y, w, hh)
    };
    let image_h = history_view::CARD_IMAGE_HEIGHT as f32;
    let body = (history_view::CARD_IMAGE_HEIGHT + history_view::CARD_DIVIDER) as f32 + 12.;
    let width = |serial: u32| card(serial).w - 26.;
    match id {
        Id::Root => Rect::new(0., 0., h.w, h.h),
        Id::Title => h.client(Rect::new(l.heading.x, l.heading.y, l.heading.w, l.title_h)),
        Id::Lede => h.client(Rect::new(
            l.heading.x,
            l.heading.y + l.title_h + 6.,
            l.heading.w,
            l.heading.h - l.title_h - 6.,
        )),
        Id::Filters => h.client(Rect::new(
            l.toolbar.x,
            l.toolbar.y,
            l.toolbar.w,
            l.filters[0].h,
        )),
        Id::Error => h.client(l.error),
        Id::Empty | Id::EmptyTitle | Id::EmptyBody => h.client(l.empty),
        Id::Grid => {
            let count = h.shown.len();
            let last = count.saturating_sub(1);
            let bottom = if count == 0 {
                l.grid_y
            } else {
                let r = h.card_rect(last);
                r.y + r.h
            };
            h.client(Rect::new(l.shell.x, l.grid_y, l.shell.w, bottom - l.grid_y))
        }
        Id::Card(serial) => card(serial),
        Id::Open(serial) | Id::Missing(serial) => {
            inside(serial, 1., 1., card(serial).w - 2., image_h - 1.)
        }
        Id::Date(serial) => inside(serial, 13., body, width(serial), 20.),
        Id::Details(serial) => inside(serial, 13., body + 24., width(serial), 14.),
        Id::Warning(serial) => inside(serial, 13., body + 42., width(serial), 14.),
        Id::CardError(serial) => {
            let c = card(serial);
            let actions = h.target_rect(Target::Action(serial, 0));
            Rect::new(
                c.x + 13.,
                actions.y + actions.h + 6.,
                width(serial),
                (c.y + c.h - 12. - (actions.y + actions.h + 6.)).max(0.),
            )
        }
        _ => target(id).map_or_else(Rect::default, |t| h.target_rect(t)),
    }
}

fn name(h: &History, id: Id) -> String {
    let copy = history_view::copy();
    let item = |serial| h.item(serial);
    match id {
        Id::Root => captures_app::app_windows::HISTORY.title.into(),
        Id::Title => copy.title.into(),
        Id::Lede => copy.lede.into(),
        Id::Filters => copy.filter_group_label.into(),
        Id::Error => h.error.clone().unwrap_or_default(),
        Id::EmptyTitle if h.loading => copy.loading.into(),
        Id::EmptyTitle if h.items.is_empty() => copy.empty_title.into(),
        Id::EmptyTitle => FILTERED_TITLE.into(),
        Id::EmptyBody if h.items.is_empty() => copy.empty_body.into(),
        Id::EmptyBody => FILTERED_BODY.into(),
        Id::Grid => copy.grid_label.into(),
        Id::Open(serial) => item(serial)
            .and_then(|it| it.card.open_label)
            .unwrap_or_default()
            .into(),
        Id::Missing(_) => copy.missing.into(),
        Id::Date(serial) => item(serial).map_or_else(String::new, |it| it.card.date.clone()),
        Id::Details(serial) => item(serial).map_or_else(String::new, |it| it.card.details.clone()),
        Id::Warning(serial) => item(serial)
            .and_then(|it| it.card.warning.clone())
            .unwrap_or_default(),
        Id::CardError(serial) => item(serial)
            .and_then(|it| it.error.clone())
            .unwrap_or_default(),
        _ => target(id).map_or_else(String::new, |t| h.name(t)),
    }
}

fn control_type(id: Id) -> UIA_CONTROLTYPE_ID {
    match id {
        Id::Root => UIA_WindowControlTypeId,
        Id::Back | Id::Cancel | Id::DeleteAll | Id::Filter(_) | Id::Open(_) | Id::Delete(_) => {
            UIA_ButtonControlTypeId
        }
        Id::Action(..) => UIA_ButtonControlTypeId,
        Id::Filters | Id::Empty | Id::Grid | Id::Card(_) => UIA_GroupControlTypeId,
        _ => UIA_TextControlTypeId,
    }
}

fn runtime_id(id: Id) -> [i32; 4] {
    let (kind, a, b) = match id {
        Id::Root => (0, 0, 0),
        Id::Back => (1, 0, 0),
        Id::Title => (2, 0, 0),
        Id::Lede => (3, 0, 0),
        Id::Cancel => (4, 0, 0),
        Id::DeleteAll => (5, 0, 0),
        Id::Filters => (6, 0, 0),
        Id::Filter(i) => (7, i as u32, 0),
        Id::Error => (8, 0, 0),
        Id::Empty => (9, 0, 0),
        Id::EmptyTitle => (10, 0, 0),
        Id::EmptyBody => (11, 0, 0),
        Id::Grid => (12, 0, 0),
        Id::Card(s) => (13, s, 0),
        Id::Open(s) => (14, s, 0),
        Id::Missing(s) => (15, s, 0),
        Id::Delete(s) => (16, s, 0),
        Id::Date(s) => (17, s, 0),
        Id::Details(s) => (18, s, 0),
        Id::Warning(s) => (19, s, 0),
        Id::Action(s, i) => (20, s, i as u32),
        Id::CardError(s) => (21, s, 0),
    };
    // A distinct first part keeps these apart from Preferences' ids.
    [UiaAppendRuntimeId as i32, 100 + kind, a as i32, b as i32]
}

fn screen_rect(h: &History, r: Rect) -> UiaRect {
    let mut origin = POINT::default();
    unsafe {
        let _ = ClientToScreen(h.hwnd, &mut origin);
    }
    let s = f64::from(h.scale);
    UiaRect {
        left: f64::from(origin.x) + f64::from(r.x) * s,
        top: f64::from(origin.y) + f64::from(r.y) * s,
        width: f64::from(r.w) * s,
        height: f64::from(r.h) * s,
    }
}

fn offscreen(h: &History, id: Id) -> bool {
    if id == Id::Root {
        return false;
    }
    let r = rect(h, id);
    r.y + r.h <= 0. || r.y >= h.h
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
            // `aria-pressed` buttons.
            Id::Filter(_) => pattern == UIA_TogglePatternId,
            Id::Back
            | Id::Cancel
            | Id::DeleteAll
            | Id::Open(_)
            | Id::Delete(_)
            | Id::Action(..) => pattern == UIA_InvokePatternId,
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
        with(|h| {
            let enabled = match id {
                // The thumbnail button isn't wired to the editor yet; Edit is.
                Id::Open(_) => false,
                _ => target(id).is_none_or(|t| h.enabled(t)),
            };
            let focusable = target(id).is_some();
            let focused = target(id).is_some_and(|t| h.focus == Some(t));
            Ok(match property {
                UIA_NamePropertyId => VARIANT::from(BSTR::from(name(h, id))),
                UIA_ControlTypePropertyId => VARIANT::from(control_type(id).0),
                UIA_LocalizedControlTypePropertyId if matches!(id, Id::Card(_)) => {
                    VARIANT::from(BSTR::from("article"))
                }
                UIA_LocalizedControlTypePropertyId if id == Id::Grid => {
                    VARIANT::from(BSTR::from("region"))
                }
                // `<section aria-label>` is a region landmark.
                UIA_LandmarkTypePropertyId if id == Id::Grid => {
                    VARIANT::from(UIA_CustomLandmarkTypeId.0)
                }
                UIA_LocalizedLandmarkTypePropertyId if id == Id::Grid => {
                    VARIANT::from(BSTR::from("region"))
                }
                UIA_HeadingLevelPropertyId if id == Id::Title => VARIANT::from(HeadingLevel1.0),
                UIA_HeadingLevelPropertyId if id == Id::EmptyTitle => {
                    VARIANT::from(HeadingLevel2.0)
                }
                UIA_LiveSettingPropertyId => match id {
                    Id::Error | Id::CardError(_) => VARIANT::from(Assertive.0),
                    // The loading section is `aria-live="polite"`.
                    Id::Empty if h.loading => VARIANT::from(Polite.0),
                    _ => VARIANT::default(),
                },
                UIA_AutomationIdPropertyId => VARIANT::from(BSTR::from(format!("history-{id:?}"))),
                UIA_FrameworkIdPropertyId => VARIANT::from(BSTR::from("Captures")),
                UIA_IsKeyboardFocusablePropertyId => VARIANT::from(focusable && enabled),
                UIA_HasKeyboardFocusPropertyId => VARIANT::from(focused),
                UIA_IsEnabledPropertyId => VARIANT::from(enabled),
                UIA_IsOffscreenPropertyId => VARIANT::from(offscreen(h, id)),
                UIA_IsControlElementPropertyId | UIA_IsContentElementPropertyId => {
                    VARIANT::from(true)
                }
                UIA_ToggleToggleStatePropertyId => match id {
                    Id::Filter(i) => VARIANT::from(toggle_state(h, i).0),
                    _ => VARIANT::default(),
                },
                _ => VARIANT::default(),
            })
        })
    }

    fn HostRawElementProvider(&self) -> Result<IRawElementProviderSimple> {
        if self.id == Id::Root {
            let hwnd = with(|h| Ok(h.hwnd))?;
            unsafe { UiaHostProviderFromHwnd(hwnd) }
        } else {
            Err(Error::empty())
        }
    }
}

fn toggle_state(h: &History, i: usize) -> ToggleState {
    if FILTERS[i] == h.filter {
        ToggleState_On
    } else {
        ToggleState_Off
    }
}

impl IRawElementProviderFragment_Impl for Node_Impl {
    fn Navigate(&self, direction: NavigateDirection) -> Result<IRawElementProviderFragment> {
        let id = self.id;
        let next = with(|h| {
            let exists = |id: Id| parent(id).is_none_or(|p| children(h, p).contains(&id));
            if !exists(id) {
                return Err(unavailable());
            }
            Ok(match direction {
                NavigateDirection_Parent => parent(id),
                NavigateDirection_FirstChild => children(h, id).first().copied(),
                NavigateDirection_LastChild => children(h, id).last().copied(),
                NavigateDirection_NextSibling | NavigateDirection_PreviousSibling => parent(id)
                    .and_then(|p| {
                        let siblings = children(h, p);
                        let i = siblings.iter().position(|s| *s == id)?;
                        if direction == NavigateDirection_NextSibling {
                            siblings.get(i + 1).copied()
                        } else {
                            i.checked_sub(1).map(|j| siblings[j])
                        }
                    }),
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
        let id = self.id;
        if id == Id::Root {
            return Ok(UiaRect::default());
        }
        with(|h| Ok(screen_rect(h, rect(h, id))))
    }

    fn GetEmbeddedFragmentRoots(&self) -> Result<*mut SAFEARRAY> {
        Ok(std::ptr::null_mut())
    }

    fn SetFocus(&self) -> Result<()> {
        let id = self.id;
        with(|h| {
            if let Some(target) = target(id).filter(|&t| h.enabled(t)) {
                h.set_focus(Some(target), true);
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
        let hit = with(|h| {
            let mut origin = POINT::default();
            unsafe {
                let _ = ClientToScreen(h.hwnd, &mut origin);
            }
            let s = f64::from(h.scale);
            let (px, py) = (
                ((x - f64::from(origin.x)) / s) as f32,
                ((y - f64::from(origin.y)) / s) as f32,
            );
            let (control, card) = h.hit(px, py);
            if let Some(control) = control {
                return Ok(id_of(control));
            }
            if let Some(serial) = card {
                for child in children(h, Id::Card(serial)) {
                    if rect(h, child).contains(px, py) {
                        return Ok(child);
                    }
                }
                return Ok(Id::Card(serial));
            }
            for id in children(h, Id::Root) {
                if rect(h, id).contains(px, py) {
                    return Ok(id);
                }
            }
            Ok(Id::Root)
        })?;
        Ok(fragment(hit))
    }

    fn GetFocus(&self) -> Result<IRawElementProviderFragment> {
        let focus = with(|h| Ok(h.focus))?;
        focus
            .map(|target| fragment(id_of(target)))
            .ok_or_else(Error::empty)
    }
}

impl IInvokeProvider_Impl for Node_Impl {
    fn Invoke(&self) -> Result<()> {
        let id = self.id;
        with(|h| {
            let Some(target) = target(id).filter(|&t| h.enabled(t)) else {
                return Err(not_enabled());
            };
            h.activate(target);
            Ok(())
        })
    }
}

impl IToggleProvider_Impl for Node_Impl {
    fn Toggle(&self) -> Result<()> {
        self.Invoke()
    }

    fn ToggleState(&self) -> Result<ToggleState> {
        let id = self.id;
        with(|h| match id {
            Id::Filter(i) => Ok(toggle_state(h, i)),
            _ => Err(Error::empty()),
        })
    }
}

/// Raise History's accessibility events, outside its borrow; `detail` is
/// the new name or alert text.
pub fn raise(events: &[(Event, String)]) {
    unsafe {
        if !UiaClientsAreListening().as_bool() {
            return;
        }
        let root = node(Id::Root);
        for (event, detail) in events {
            match *event {
                Event::Focus(target) => {
                    let _ = UiaRaiseAutomationEvent(
                        &node(id_of(target)),
                        UIA_AutomationFocusChangedEventId,
                    );
                }
                Event::Name(target) => {
                    let _ = UiaRaiseAutomationPropertyChangedEvent(
                        &node(id_of(target)),
                        UIA_NamePropertyId,
                        &VARIANT::default(),
                        &VARIANT::from(BSTR::from(detail.as_str())),
                    );
                }
                Event::Pressed(i, on) => {
                    let state = |on: bool| VARIANT::from(if on { 1i32 } else { 0i32 });
                    let _ = UiaRaiseAutomationPropertyChangedEvent(
                        &node(Id::Filter(i)),
                        UIA_ToggleToggleStatePropertyId,
                        &state(!on),
                        &state(on),
                    );
                }
                Event::Structure => {
                    let _ = UiaRaiseStructureChangedEvent(
                        &root,
                        StructureChangeType_ChildrenInvalidated,
                        runtime_id(Id::Root).as_mut_ptr(),
                        4,
                    );
                }
                Event::Alert(card) => {
                    let alert = node(card.map_or(Id::Error, Id::CardError));
                    let _ = UiaRaiseAutomationEvent(&alert, UIA_SystemAlertEventId);
                    let _ = UiaRaiseNotificationEvent(
                        &alert,
                        NotificationKind_Other,
                        NotificationProcessing_ImportantMostRecent,
                        &BSTR::from(detail.as_str()),
                        &BSTR::from("history-error"),
                    );
                }
            }
        }
    }
}

/// Events with the text they report, read while the window is borrowed.
pub fn describe(h: &History, events: Vec<Event>) -> Vec<(Event, String)> {
    events
        .into_iter()
        .map(|event| {
            let detail = match event {
                Event::Name(target) => h.name(target),
                Event::Alert(None) => h.error.clone().unwrap_or_default(),
                Event::Alert(Some(serial)) => h
                    .item(serial)
                    .and_then(|it| it.error.clone())
                    .unwrap_or_default(),
                _ => String::new(),
            };
            (event, detail)
        })
        .collect()
}

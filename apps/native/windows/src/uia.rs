#![allow(non_upper_case_globals)]
//! UI Automation for Preferences: the window, the section navigation (Invoke
//! buttons), the header, and each settings card as a group of its controls:
//! switches (Toggle), the Interface theme and accent choices (radio buttons
//! with SelectionItem inside a Selection group), the read-only capture folder
//! (Value) and Choose… (Invoke). Names come from the shipping titles and
//! `aria-label`s; descriptions are help text. Providers are node ids that read
//! live scene state on the UI thread.
use captures_app::preferences::{self, APPEARANCE_MODES, SECTIONS, THEMES};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Com::*;
use windows::Win32::System::Ole::*;
use windows::Win32::System::Variant::*;
use windows::Win32::UI::Accessibility::*;
use windows::core::*;

use crate::host::{Host, with_host};
use crate::scene::{ACCENT_GROUP_LABEL, CARDS, Event, Item, NAV_WIDTH, Rect, Target};

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
    /// Interface theme: the segment group and its segments.
    Modes,
    Mode(usize),
    /// Accent color: the chip group and its chips.
    Accents,
    Accent(usize),
    /// Save captures to: the read-only folder and Choose….
    Folder,
    Choose,
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
        Id::Mode(i) => Some(Target::Mode(i)),
        Id::Accent(i) => Some(Target::Theme(i)),
        Id::Choose => Some(Target::Choose),
        _ => None,
    }
}

fn id_of(target: Target) -> Id {
    match target {
        Target::Nav(i) => Id::NavItem(i),
        Target::History => Id::History,
        Target::Row(row) => Id::Switch(row),
        Target::Mode(i) => Id::Mode(i),
        Target::Theme(i) => Id::Accent(i),
        Target::Choose => Id::Choose,
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
        Id::Card(card) => CARDS[card]
            .1
            .iter()
            .flat_map(|item| match *item {
                Item::Switch(row) => vec![Id::Switch(row)],
                Item::Mode => vec![Id::Modes],
                Item::Accent => vec![Id::Accents],
                Item::Folder => vec![Id::Folder, Id::Choose],
            })
            .collect(),
        Id::Modes => (0..APPEARANCE_MODES.len()).map(Id::Mode).collect(),
        Id::Accents => (0..THEMES.len()).map(Id::Accent).collect(),
        _ => Vec::new(),
    }
}

fn parent(id: Id) -> Option<Id> {
    let card_of = |item: Item| CARDS.iter().position(|(_, items)| items.contains(&item));
    Some(match id {
        Id::Root => return None,
        Id::NavItem(_) => Id::Nav,
        Id::Mode(_) => Id::Modes,
        Id::Accent(_) => Id::Accents,
        Id::Switch(row) => Id::Card(card_of(Item::Switch(row))?),
        Id::Modes => Id::Card(card_of(Item::Mode)?),
        Id::Accents => Id::Card(card_of(Item::Accent)?),
        Id::Folder | Id::Choose => Id::Card(card_of(Item::Folder)?),
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
        // The whole `label` toggles.
        Id::Switch(row) => scene.row_rect(row),
        Id::Modes => scene.mode_rect(),
        Id::Accents => scene.accent_rect(),
        Id::Folder => scene.field_rect(),
        _ => scene.target_rect(target(id).expect("target")),
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
        Id::Modes => preferences::row("appearance").title.into(),
        Id::Mode(i) => APPEARANCE_MODES[i].1.into(),
        Id::Accents => ACCENT_GROUP_LABEL.into(),
        Id::Accent(i) => preferences::theme_accessibility_label(&THEMES[i]),
        Id::Folder => preferences::row("output_directory").title.into(),
        Id::Choose => "Choose…".into(),
    }
}

fn help(host: &Host, id: Id) -> String {
    match id {
        Id::Card(card) => SECTIONS[CARDS[card].0].description.into(),
        Id::Switch(row) => host.scene.rows[row].detail.clone(),
        Id::Modes => preferences::row("appearance").description.into(),
        Id::Accents => preferences::row("theme").description.into(),
        _ => String::new(),
    }
}

fn control_type(id: Id) -> UIA_CONTROLTYPE_ID {
    match id {
        Id::Root => UIA_WindowControlTypeId,
        Id::Nav | Id::Card(_) | Id::Modes | Id::Accents => UIA_GroupControlTypeId,
        Id::Title | Id::Subtitle => UIA_TextControlTypeId,
        Id::Mode(_) | Id::Accent(_) => UIA_RadioButtonControlTypeId,
        Id::Folder => UIA_EditControlTypeId,
        // Like WinUI's ToggleSwitch, switches are buttons with Toggle.
        Id::NavItem(_) | Id::History | Id::Switch(_) | Id::Choose => UIA_ButtonControlTypeId,
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
        Id::Modes => (8, 0),
        Id::Mode(i) => (9, i),
        Id::Accents => (10, 0),
        Id::Accent(i) => (11, i),
        Id::Folder => (12, 0),
        Id::Choose => (13, 0),
    };
    [UiaAppendRuntimeId as i32, kind, index as i32]
}

fn selected(host: &Host, id: Id) -> bool {
    match id {
        Id::Mode(i) => host.scene.mode_index() == i,
        Id::Accent(i) => host.scene.theme_index() == i,
        _ => false,
    }
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
    if matches!(
        id,
        Id::Root | Id::Nav | Id::NavItem(_) | Id::Title | Id::Subtitle | Id::History
    ) {
        return false;
    }
    let (r, v) = (rect(host, id), host.scene.viewport());
    r.y + r.h <= v.y || r.y >= v.y + v.h
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

fn safe_array_unknown(values: &[IUnknown]) -> *mut SAFEARRAY {
    unsafe {
        let array = SafeArrayCreateVector(VT_UNKNOWN, 0, values.len() as u32);
        for (i, value) in values.iter().enumerate() {
            // SafeArrayPutElement AddRefs interface elements.
            let _ = SafeArrayPutElement(array, &(i as i32), value.as_raw());
        }
        array
    }
}

#[implement(
    IRawElementProviderSimple,
    IRawElementProviderFragment,
    IRawElementProviderFragmentRoot,
    IInvokeProvider,
    IToggleProvider,
    ISelectionProvider,
    ISelectionItemProvider,
    IValueProvider
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
            Id::NavItem(_) | Id::History | Id::Choose => pattern == UIA_InvokePatternId,
            Id::Switch(_) => pattern == UIA_TogglePatternId,
            Id::Modes | Id::Accents => pattern == UIA_SelectionPatternId,
            Id::Mode(_) | Id::Accent(_) => pattern == UIA_SelectionItemPatternId,
            Id::Folder => pattern == UIA_ValuePatternId,
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
                UIA_SelectionItemIsSelectedPropertyId => VARIANT::from(selected(host, id)),
                UIA_ValueValuePropertyId if id == Id::Folder => {
                    VARIANT::from(BSTR::from(scene.settings().output_directory.as_str()))
                }
                UIA_ValueIsReadOnlyPropertyId if id == Id::Folder => VARIANT::from(true),
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
            if rect(host, Id::Folder).contains(px, py) {
                return Ok(Id::Folder);
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

impl ISelectionProvider_Impl for Node_Impl {
    fn GetSelection(&self) -> Result<*mut SAFEARRAY> {
        let id = self.id;
        let chosen = with(|host| {
            Ok(match id {
                Id::Modes => Id::Mode(host.scene.mode_index()),
                _ => Id::Accent(host.scene.theme_index()),
            })
        })?;
        Ok(safe_array_unknown(&[node(chosen).cast()?]))
    }

    fn CanSelectMultiple(&self) -> Result<BOOL> {
        Ok(false.into())
    }

    fn IsSelectionRequired(&self) -> Result<BOOL> {
        Ok(true.into())
    }
}

impl ISelectionItemProvider_Impl for Node_Impl {
    fn Select(&self) -> Result<()> {
        let id = self.id;
        with(|host| {
            if let Some(target) = target(id) {
                host.activate(target);
            }
            Ok(())
        })
    }

    fn AddToSelection(&self) -> Result<()> {
        let id = self.id;
        if with(|host| Ok(selected(host, id)))? {
            Ok(())
        } else {
            Err(HRESULT(UIA_E_INVALIDOPERATION as _).into())
        }
    }

    fn RemoveFromSelection(&self) -> Result<()> {
        // One choice is always selected.
        Err(HRESULT(UIA_E_INVALIDOPERATION as _).into())
    }

    fn IsSelected(&self) -> Result<BOOL> {
        let id = self.id;
        with(|host| Ok(selected(host, id).into()))
    }

    fn SelectionContainer(&self) -> Result<IRawElementProviderSimple> {
        parent(self.id).map(node).ok_or_else(Error::empty)
    }
}

impl IValueProvider_Impl for Node_Impl {
    fn SetValue(&self, _: &PCWSTR) -> Result<()> {
        // Read-only: Choose… changes the folder.
        Err(HRESULT(UIA_E_ELEMENTNOTENABLED as _).into())
    }

    fn Value(&self) -> Result<BSTR> {
        with(|host| Ok(BSTR::from(host.scene.settings().output_directory.as_str())))
    }

    fn IsReadOnly(&self) -> Result<BOOL> {
        Ok(true.into())
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
                Event::Selected(target) => {
                    let _ = UiaRaiseAutomationEvent(
                        &node(id_of(target)),
                        UIA_SelectionItem_ElementSelectedEventId,
                    );
                }
                Event::Folder => {
                    let _ = UiaRaiseAutomationPropertyChangedEvent(
                        &node(Id::Folder),
                        UIA_ValueValuePropertyId,
                        &VARIANT::default(),
                        &VARIANT::from(BSTR::from(detail.as_str())),
                    );
                }
            }
        }
    }
}

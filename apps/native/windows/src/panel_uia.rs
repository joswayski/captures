#![allow(non_upper_case_globals)]
//! UI Automation for the small windows that draw themselves as one surface:
//! first-run setup, Send Feedback, the launch and update notices and the mini previews. Each window
//! describes its parts as a list of [`Item`]s in shipping's DOM order and
//! roles (`<h1>`, `role="radio"` in a `radiogroup`, `<textarea>`/`<input>`,
//! `<button>`, `role="status"`/`role="alert"`), and applies [`Act`]s through
//! the same paths as clicks and typing. Providers are `(window, key)` pairs
//! that read the window's state on the UI thread on every call.
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Com::*;
use windows::Win32::System::Ole::*;
use windows::Win32::System::Variant::*;
use windows::Win32::UI::Accessibility::*;
use windows::core::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Window {
    Setup,
    Feedback,
    Notice,
    Previews,
    Update,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    /// `<h1>`/`<h2>`, with its level.
    Heading(i32),
    Text,
    /// A labelled group, such as `role="radiogroup"`; one choice is selected.
    Group,
    Button,
    Radio,
    /// `<input>` (single line) or `<textarea>` (multiline).
    Edit,
    /// `role="status"`: announced politely when it appears or changes.
    Status,
    /// `role="alert"`: announced assertively.
    Alert,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// Stable within the window; also the automation id.
    pub key: u32,
    pub parent: Option<u32>,
    pub role: Role,
    pub name: String,
    /// Help text, such as a category's description or a field's hint.
    pub help: String,
    /// In client coordinates (DIPs).
    pub rect: D2D_RECT_F,
    pub enabled: bool,
    pub focused: bool,
    /// A radio's `aria-checked`.
    pub selected: bool,
    /// An Edit's text.
    pub value: Option<String>,
}

impl Item {
    pub fn new(key: u32, role: Role, name: impl Into<String>, rect: D2D_RECT_F) -> Self {
        Self {
            key,
            parent: None,
            role,
            name: name.into(),
            help: String::new(),
            rect,
            enabled: true,
            focused: false,
            selected: false,
            value: None,
        }
    }
}

/// A window's accessible state, read inside its borrow.
pub struct Snapshot {
    pub hwnd: HWND,
    pub scale: f32,
    /// The client size in DIPs, for `IsOffscreen`.
    pub size: (f32, f32),
    pub name: &'static str,
    /// The window takes keyboard focus (the launch notice never does).
    pub focusable: bool,
    pub items: Vec<Item>,
}

/// What an assistive client asks a part to do.
pub enum Act {
    Invoke,
    Select,
    SetValue(String),
    Focus,
}

fn unavailable() -> Error {
    HRESULT(UIA_E_ELEMENTNOTAVAILABLE as _).into()
}

fn not_enabled() -> Error {
    HRESULT(UIA_E_ELEMENTNOTENABLED as _).into()
}

fn snapshot(window: Window) -> Result<Snapshot> {
    match window {
        Window::Setup => crate::setup::uia_snapshot(),
        Window::Feedback => crate::feedback::uia_snapshot(),
        Window::Notice => crate::startup_notice::uia_snapshot(),
        Window::Previews => crate::previews::uia_snapshot(),
        Window::Update => crate::update_notice::uia_snapshot(),
    }
    .ok_or_else(unavailable)
}

/// Apply `act` to `key`; the window repaints itself.
fn act(window: Window, key: u32, act: Act) -> Result<()> {
    let item = item(window, key)?;
    if !item.enabled {
        return Err(not_enabled());
    }
    let before = watch(window);
    match window {
        Window::Setup => crate::setup::uia_act(key, act),
        Window::Feedback => crate::feedback::uia_act(key, act),
        Window::Notice => crate::startup_notice::uia_act(key, act),
        Window::Previews => crate::previews::uia_act(key, act),
        Window::Update => crate::update_notice::uia_act(key, act),
    }
    .ok_or_else(unavailable)?;
    raise(window, before);
    Ok(())
}

fn item(window: Window, key: u32) -> Result<Item> {
    snapshot(window)?
        .items
        .into_iter()
        .find(|i| i.key == key)
        .ok_or_else(unavailable)
}

fn interactive(role: Role) -> bool {
    matches!(role, Role::Button | Role::Radio | Role::Edit)
}

fn runtime_id(window: Window, key: Option<u32>) -> [i32; 4] {
    let w = match window {
        Window::Setup => 0,
        Window::Feedback => 1,
        Window::Notice => 2,
        Window::Previews => 3,
        Window::Update => 4,
    };
    // A distinct first part keeps these apart from the other windows' ids.
    [
        UiaAppendRuntimeId as i32,
        500 + w,
        key.map_or(0, |k| k as i32 + 1),
        0,
    ]
}

fn screen_rect(s: &Snapshot, r: D2D_RECT_F) -> UiaRect {
    let mut origin = POINT::default();
    unsafe {
        let _ = ClientToScreen(s.hwnd, &mut origin);
    }
    let scale = f64::from(s.scale);
    UiaRect {
        left: f64::from(origin.x) + f64::from(r.left) * scale,
        top: f64::from(origin.y) + f64::from(r.top) * scale,
        width: f64::from(r.right - r.left) * scale,
        height: f64::from(r.bottom - r.top) * scale,
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

fn children(items: &[Item], parent: Option<u32>) -> Vec<u32> {
    items
        .iter()
        .filter(|i| i.parent == parent)
        .map(|i| i.key)
        .collect()
}

#[implement(
    IRawElementProviderSimple,
    IRawElementProviderFragment,
    IRawElementProviderFragmentRoot,
    IInvokeProvider,
    ISelectionProvider,
    ISelectionItemProvider,
    IValueProvider
)]
pub struct Node {
    window: Window,
    /// `None` is the window's root.
    key: Option<u32>,
}

pub fn node(window: Window, key: Option<u32>) -> IRawElementProviderSimple {
    Node { window, key }.into()
}

fn fragment(window: Window, key: Option<u32>) -> IRawElementProviderFragment {
    Node { window, key }.into()
}

impl Node_Impl {
    fn item(&self) -> Result<Item> {
        item(self.window, self.key.ok_or_else(unavailable)?)
    }
}

impl IRawElementProviderSimple_Impl for Node_Impl {
    fn ProviderOptions(&self) -> Result<ProviderOptions> {
        Ok(ProviderOptions_ServerSideProvider | ProviderOptions_UseComThreading)
    }

    fn GetPatternProvider(&self, pattern: UIA_PATTERN_ID) -> Result<IUnknown> {
        let Some(key) = self.key else {
            return Err(Error::empty());
        };
        let role = item(self.window, key)?.role;
        let supported = matches!(
            (role, pattern),
            (Role::Button, UIA_InvokePatternId)
                | (Role::Group, UIA_SelectionPatternId)
                | (Role::Radio, UIA_SelectionItemPatternId)
                | (Role::Edit, UIA_ValuePatternId)
        );
        if supported {
            Ok(self.to_interface::<IUnknown>())
        } else {
            Err(Error::empty())
        }
    }

    fn GetPropertyValue(&self, property: UIA_PROPERTY_ID) -> Result<VARIANT> {
        let s = snapshot(self.window)?;
        let Some(key) = self.key else {
            return Ok(match property {
                UIA_NamePropertyId => VARIANT::from(BSTR::from(s.name)),
                UIA_ControlTypePropertyId => VARIANT::from(UIA_PaneControlTypeId.0),
                UIA_AutomationIdPropertyId => {
                    VARIANT::from(BSTR::from(format!("{:?}", self.window).to_lowercase()))
                }
                UIA_FrameworkIdPropertyId => VARIANT::from(BSTR::from("Captures")),
                // Shipping's setup, feedback and preview pages are `<main>`.
                UIA_LandmarkTypePropertyId
                    if !matches!(self.window, Window::Notice | Window::Update) =>
                {
                    VARIANT::from(UIA_MainLandmarkTypeId.0)
                }
                _ => VARIANT::default(),
            });
        };
        let i = s
            .items
            .iter()
            .find(|i| i.key == key)
            .ok_or_else(unavailable)?;
        Ok(match property {
            UIA_NamePropertyId => VARIANT::from(BSTR::from(i.name.as_str())),
            UIA_HelpTextPropertyId if !i.help.is_empty() => {
                VARIANT::from(BSTR::from(i.help.as_str()))
            }
            UIA_ControlTypePropertyId => VARIANT::from(match i.role {
                Role::Heading(_) | Role::Text | Role::Status | Role::Alert => {
                    UIA_TextControlTypeId.0
                }
                Role::Group => UIA_GroupControlTypeId.0,
                Role::Button => UIA_ButtonControlTypeId.0,
                Role::Radio => UIA_RadioButtonControlTypeId.0,
                Role::Edit => UIA_EditControlTypeId.0,
            }),
            UIA_HeadingLevelPropertyId => VARIANT::from(match i.role {
                Role::Heading(level) => HeadingLevel1.0 + level.clamp(1, 9) - 1,
                _ => HeadingLevel_None.0,
            }),
            UIA_LiveSettingPropertyId if matches!(i.role, Role::Status | Role::Alert) => {
                VARIANT::from(if i.role == Role::Alert {
                    Assertive.0
                } else {
                    Polite.0
                })
            }
            UIA_AutomationIdPropertyId => VARIANT::from(BSTR::from(format!(
                "{}-{}",
                format!("{:?}", self.window).to_lowercase(),
                i.key
            ))),
            UIA_FrameworkIdPropertyId => VARIANT::from(BSTR::from("Captures")),
            UIA_IsKeyboardFocusablePropertyId => {
                VARIANT::from(s.focusable && interactive(i.role) && i.enabled)
            }
            UIA_HasKeyboardFocusPropertyId => VARIANT::from(i.focused),
            UIA_IsEnabledPropertyId => VARIANT::from(i.enabled),
            UIA_IsOffscreenPropertyId => {
                VARIANT::from(i.rect.bottom <= 0. || i.rect.top >= s.size.1)
            }
            UIA_IsControlElementPropertyId | UIA_IsContentElementPropertyId => VARIANT::from(true),
            _ => VARIANT::default(),
        })
    }

    fn HostRawElementProvider(&self) -> Result<IRawElementProviderSimple> {
        if self.key.is_none() {
            let hwnd = snapshot(self.window)?.hwnd;
            unsafe { UiaHostProviderFromHwnd(hwnd) }
        } else {
            Err(Error::empty())
        }
    }
}

impl IRawElementProviderFragment_Impl for Node_Impl {
    fn Navigate(&self, direction: NavigateDirection) -> Result<IRawElementProviderFragment> {
        let s = snapshot(self.window)?;
        let items = &s.items;
        let parent = match self.key {
            None => None,
            Some(key) => Some(
                items
                    .iter()
                    .find(|i| i.key == key)
                    .ok_or_else(unavailable)?
                    .parent,
            ),
        };
        let next = match direction {
            NavigateDirection_Parent => parent,
            NavigateDirection_FirstChild => children(items, self.key).first().map(|k| Some(*k)),
            NavigateDirection_LastChild => children(items, self.key).last().map(|k| Some(*k)),
            NavigateDirection_NextSibling | NavigateDirection_PreviousSibling => {
                match (self.key, parent) {
                    (Some(key), Some(parent)) => {
                        let siblings = children(items, parent);
                        let i = siblings.iter().position(|k| *k == key).unwrap_or(0);
                        if direction == NavigateDirection_NextSibling {
                            siblings.get(i + 1).map(|k| Some(*k))
                        } else {
                            i.checked_sub(1).map(|j| Some(siblings[j]))
                        }
                    }
                    _ => None,
                }
            }
            _ => None,
        };
        next.map(|key| fragment(self.window, key))
            .ok_or_else(Error::empty)
    }

    fn GetRuntimeId(&self) -> Result<*mut SAFEARRAY> {
        if self.key.is_none() {
            return Ok(std::ptr::null_mut());
        }
        Ok(safe_array_i32(&runtime_id(self.window, self.key)))
    }

    fn BoundingRectangle(&self) -> Result<UiaRect> {
        let Some(key) = self.key else {
            return Ok(UiaRect::default());
        };
        let s = snapshot(self.window)?;
        let i = s
            .items
            .iter()
            .find(|i| i.key == key)
            .ok_or_else(unavailable)?;
        Ok(screen_rect(&s, i.rect))
    }

    fn GetEmbeddedFragmentRoots(&self) -> Result<*mut SAFEARRAY> {
        Ok(std::ptr::null_mut())
    }

    fn SetFocus(&self) -> Result<()> {
        let Some(key) = self.key else { return Ok(()) };
        let s = snapshot(self.window)?;
        let i = s
            .items
            .iter()
            .find(|i| i.key == key)
            .ok_or_else(unavailable)?;
        if s.focusable && interactive(i.role) {
            act(self.window, key, Act::Focus)?;
        }
        Ok(())
    }

    fn FragmentRoot(&self) -> Result<IRawElementProviderFragmentRoot> {
        Ok(Node {
            window: self.window,
            key: None,
        }
        .into())
    }
}

impl IRawElementProviderFragmentRoot_Impl for Node_Impl {
    fn ElementProviderFromPoint(&self, x: f64, y: f64) -> Result<IRawElementProviderFragment> {
        let s = snapshot(self.window)?;
        let mut origin = POINT::default();
        unsafe {
            let _ = ClientToScreen(s.hwnd, &mut origin);
        }
        let scale = f64::from(s.scale);
        let (px, py) = (
            ((x - f64::from(origin.x)) / scale) as f32,
            ((y - f64::from(origin.y)) / scale) as f32,
        );
        // The innermost (last listed) part under the point.
        let hit = s
            .items
            .iter()
            .rev()
            .find(|i| {
                let r = i.rect;
                px >= r.left && px < r.right && py >= r.top && py < r.bottom
            })
            .map(|i| i.key);
        Ok(fragment(self.window, hit))
    }

    fn GetFocus(&self) -> Result<IRawElementProviderFragment> {
        let s = snapshot(self.window)?;
        s.items
            .iter()
            .find(|i| i.focused)
            .map(|i| fragment(self.window, Some(i.key)))
            .ok_or_else(Error::empty)
    }
}

impl IInvokeProvider_Impl for Node_Impl {
    fn Invoke(&self) -> Result<()> {
        act(self.window, self.key.ok_or_else(Error::empty)?, Act::Invoke)
    }
}

impl ISelectionProvider_Impl for Node_Impl {
    fn GetSelection(&self) -> Result<*mut SAFEARRAY> {
        let s = snapshot(self.window)?;
        let chosen: Vec<IUnknown> = s
            .items
            .iter()
            .filter(|i| i.parent == self.key && i.selected)
            .map(|i| node(self.window, Some(i.key)).cast())
            .collect::<Result<_>>()?;
        Ok(crate::uia::safe_array_unknown(&chosen))
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
        act(self.window, self.key.ok_or_else(Error::empty)?, Act::Select)
    }

    fn AddToSelection(&self) -> Result<()> {
        if self.item()?.selected {
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
        Ok(self.item()?.selected.into())
    }

    fn SelectionContainer(&self) -> Result<IRawElementProviderSimple> {
        let parent = self.item()?.parent.ok_or_else(Error::empty)?;
        Ok(node(self.window, Some(parent)))
    }
}

impl IValueProvider_Impl for Node_Impl {
    fn SetValue(&self, value: &PCWSTR) -> Result<()> {
        let text = unsafe { value.to_string() }.unwrap_or_default();
        act(
            self.window,
            self.key.ok_or_else(Error::empty)?,
            Act::SetValue(text),
        )
    }

    fn Value(&self) -> Result<BSTR> {
        Ok(BSTR::from(self.item()?.value.unwrap_or_default()))
    }

    fn IsReadOnly(&self) -> Result<BOOL> {
        Ok((!self.item()?.enabled).into())
    }
}

/// What a client hears about, between two snapshots of one window.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Keyboard focus moved to this part.
    Focus(u32),
    /// Parts appeared or went away.
    Structure,
    Name(u32, String),
    Enabled(u32, bool),
    /// A radio became the selected choice.
    Selected(u32),
    Value(u32, String),
    /// A status or alert line appeared or changed.
    Announce(u32, Role, String),
}

/// The events between two item lists of one window.
pub fn diff(before: &[Item], after: &[Item]) -> Vec<Event> {
    let mut events = Vec::new();
    let keys = |items: &[Item]| items.iter().map(|i| i.key).collect::<Vec<_>>();
    if keys(before) != keys(after) {
        events.push(Event::Structure);
    }
    let focus = |items: &[Item]| items.iter().find(|i| i.focused).map(|i| i.key);
    if let Some(key) = focus(after)
        && focus(before) != Some(key)
    {
        events.push(Event::Focus(key));
    }
    for i in after {
        let old = before.iter().find(|o| o.key == i.key);
        if matches!(i.role, Role::Status | Role::Alert) {
            if old.is_none_or(|o| o.name != i.name) {
                events.push(Event::Announce(i.key, i.role, i.name.clone()));
            }
            continue;
        }
        let Some(old) = old else { continue };
        if old.name != i.name {
            events.push(Event::Name(i.key, i.name.clone()));
        }
        if old.enabled != i.enabled {
            events.push(Event::Enabled(i.key, i.enabled));
        }
        if i.selected && !old.selected {
            events.push(Event::Selected(i.key));
        }
        if let Some(value) = &i.value
            && old.value.as_ref() != Some(value)
        {
            events.push(Event::Value(i.key, value.clone()));
        }
    }
    events
}

/// The window's items while a client listens, to diff after a message.
pub fn watch(window: Window) -> Option<Vec<Item>> {
    unsafe { UiaClientsAreListening() }
        .as_bool()
        .then(|| snapshot(window).ok().map(|s| s.items))
        .flatten()
}

/// Raise what changed since `before`; call outside the window's borrow.
pub fn raise(window: Window, before: Option<Vec<Item>>) {
    let Some(before) = before else { return };
    let Ok(after) = snapshot(window) else { return };
    for event in diff(&before, &after.items) {
        unsafe {
            let _ = match &event {
                Event::Focus(key) => UiaRaiseAutomationEvent(
                    &node(window, Some(*key)),
                    UIA_AutomationFocusChangedEventId,
                ),
                Event::Structure => UiaRaiseStructureChangedEvent(
                    &node(window, None),
                    StructureChangeType_ChildrenInvalidated,
                    runtime_id(window, None).as_mut_ptr(),
                    4,
                ),
                Event::Name(key, name) => UiaRaiseAutomationPropertyChangedEvent(
                    &node(window, Some(*key)),
                    UIA_NamePropertyId,
                    &VARIANT::default(),
                    &VARIANT::from(BSTR::from(name.as_str())),
                ),
                Event::Enabled(key, enabled) => UiaRaiseAutomationPropertyChangedEvent(
                    &node(window, Some(*key)),
                    UIA_IsEnabledPropertyId,
                    &VARIANT::from(!enabled),
                    &VARIANT::from(*enabled),
                ),
                Event::Selected(key) => UiaRaiseAutomationEvent(
                    &node(window, Some(*key)),
                    UIA_SelectionItem_ElementSelectedEventId,
                ),
                Event::Value(key, value) => UiaRaiseAutomationPropertyChangedEvent(
                    &node(window, Some(*key)),
                    UIA_ValueValuePropertyId,
                    &VARIANT::default(),
                    &VARIANT::from(BSTR::from(value.as_str())),
                ),
                Event::Announce(key, role, text) => {
                    let provider = node(window, Some(*key));
                    if *role == Role::Alert {
                        let _ = UiaRaiseAutomationEvent(&provider, UIA_SystemAlertEventId);
                    }
                    UiaRaiseNotificationEvent(
                        &provider,
                        NotificationKind_Other,
                        if *role == Role::Alert {
                            NotificationProcessing_ImportantMostRecent
                        } else {
                            NotificationProcessing_MostRecent
                        },
                        &BSTR::from(text.as_str()),
                        &BSTR::from(format!("{window:?}-{key}").to_lowercase()),
                    )
                }
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gfx::rect;

    fn item(key: u32, role: Role, name: &str) -> Item {
        Item::new(key, role, name, rect(0., 0., 10., 10.))
    }

    #[test]
    fn diff_reports_focus_selection_values_and_new_status_lines() {
        let mut radio_a = item(1, Role::Radio, "Bug");
        radio_a.selected = true;
        let radio_b = item(2, Role::Radio, "Idea");
        let mut edit = item(3, Role::Edit, "Message");
        edit.value = Some(String::new());
        let mut send = item(4, Role::Button, "Send feedback");
        send.enabled = false;
        let before = vec![radio_a.clone(), radio_b.clone(), edit.clone(), send.clone()];

        // Nothing changed: nothing to say.
        assert_eq!(diff(&before, &before), vec![]);

        let mut after = before.clone();
        after[0].selected = false;
        after[1].selected = true;
        after[2].value = Some("Hi".into());
        after[2].focused = true;
        after[3].enabled = true;
        after.push(item(5, Role::Status, "Thanks — feedback sent."));
        assert_eq!(
            diff(&before, &after),
            vec![
                Event::Structure,
                Event::Focus(3),
                Event::Selected(2),
                Event::Value(3, "Hi".into()),
                Event::Enabled(4, true),
                Event::Announce(5, Role::Status, "Thanks — feedback sent.".into()),
            ]
        );

        // Focus staying put and an unchanged status are not repeated.
        let again = after.clone();
        assert_eq!(diff(&after, &again), vec![]);
        let mut renamed = after.clone();
        renamed[3].name = "Sending…".into();
        assert_eq!(
            diff(&after, &renamed),
            vec![Event::Name(4, "Sending…".into())]
        );
    }
}

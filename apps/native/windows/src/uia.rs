#![allow(non_upper_case_globals)]
//! UI Automation for Preferences: the window, the section navigation (Invoke
//! buttons that also report the section in view through SelectionItem, like
//! shipping `aria-current`), the header, and each settings card as a group of
//! its controls: switches (Toggle), the Interface theme and accent choices (radio buttons
//! with SelectionItem inside a Selection group), the read-only capture folder
//! (Value) and Choose… (Invoke), and the Find bar (a search landmark with an
//! Edit that has Value and Text patterns, a live match count and Invoke
//! buttons), and the custom color editor (a group with Reset colors, color
//! wells as buttons with a read-only Value, and hex Edits with Value and
//! Text). Names come from the shipping titles and
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
use crate::scene::{
    ACCENT_GROUP_LABEL, CARDS, CUSTOM_GROUP_LABEL, Event, FIND_BUTTONS, FieldId, FindPart, Item,
    NAV_WIDTH, Rect, SelectKey, Target, Utility,
};
use crate::textfield::TextField;
use crate::uia_text::range;

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
    /// A utility row's Open: Windows keyboard settings or Send feedback.
    Open(Utility),
    /// Updates: the build identity text, the disabled Check Now and the
    /// status text (`role="status"`).
    UpdateVersion,
    UpdateAction,
    UpdateStatus,
    /// A shortcut recorder: a toggle button whose value is its keys.
    Shortcut(usize),
    /// A select: ComboBox with ExpandCollapse, Selection and Value.
    Select(SelectKey),
    /// An option of the open listbox (ListItem with SelectionItem).
    Option(SelectKey, usize),
    /// The header save status (`role="status"`).
    SaveStatus,
    /// Find (`role="search"`), its input and buttons, and the live count.
    Find,
    FindPart(FindPart),
    FindCount,
    /// `.custom-theme-editor` (`role="group"`).
    CustomEditor,
    /// Its copy: title, description, then each field's label and note.
    CustomText(usize),
    ResetColors,
    /// A color's `input[type="color"]`: a button whose value is the color.
    Well(usize),
    /// A color's hex `input`.
    Hex(usize),
}

/// The editor's children in DOM order.
const CUSTOM_CHILDREN: [Id; 11] = [
    Id::CustomText(0),
    Id::CustomText(1),
    Id::ResetColors,
    Id::CustomText(2),
    Id::Well(0),
    Id::Hex(0),
    Id::CustomText(3),
    Id::CustomText(4),
    Id::Well(1),
    Id::Hex(1),
    Id::CustomText(5),
];

/// Tree state that changes which children exist.
#[derive(Clone, Copy, Default)]
struct Live {
    /// The open select and its option count, for the combo box's children.
    list: Option<(SelectKey, usize)>,
    /// The find bar is shown.
    find: bool,
    /// The custom color editor is shown.
    custom: bool,
    /// The save status is shown.
    status: bool,
}

fn live(host: &Host) -> Live {
    Live {
        list: host
            .scene
            .open
            .map(|(key, _)| (key, host.scene.select_options(key).len())),
        find: host.scene.find.open,
        custom: host.scene.custom_shown(),
        status: host.scene.save_shown(),
    }
}

fn unavailable() -> Error {
    HRESULT(UIA_E_ELEMENTNOTAVAILABLE as _).into()
}

pub(crate) fn with<R>(f: impl FnOnce(&mut Host) -> Result<R>) -> Result<R> {
    with_host(f).unwrap_or_else(|| Err(unavailable()))
}

/// The scene field an Edit element is.
pub(crate) fn field_id(id: Id) -> Option<FieldId> {
    match id {
        Id::FindPart(FindPart::Input) => Some(FieldId::Find),
        Id::Hex(i) => Some(FieldId::Hex(i)),
        _ => None,
    }
}

/// The text field behind an Edit element, while it is shown.
pub(crate) fn field(host: &Host, id: Id) -> Option<&TextField> {
    let field = field_id(id).filter(|&f| host.scene.field_shown(f))?;
    Some(host.scene.field(field))
}

/// The Edit element of a scene field.
fn field_uia(field: FieldId) -> Id {
    match field {
        FieldId::Find => Id::FindPart(FindPart::Input),
        FieldId::Hex(i) => Id::Hex(i),
    }
}

/// A text field's text box on screen, and pixels per DIP.
pub(crate) fn field_screen_rect(host: &Host, id: Id) -> (UiaRect, f64) {
    let r = field_id(id).map_or_else(Default::default, |f| host.scene.field_box(f));
    (screen_rect(host, r), f64::from(host.scene.scale))
}

fn target(id: Id) -> Option<Target> {
    match id {
        Id::NavItem(i) => Some(Target::Nav(i)),
        Id::History => Some(Target::History),
        Id::Switch(row) => Some(Target::Row(row)),
        Id::Mode(i) => Some(Target::Mode(i)),
        Id::Accent(i) => Some(Target::Theme(i)),
        Id::Choose => Some(Target::Choose),
        Id::Open(which) => Some(Target::Open(which)),
        Id::Shortcut(i) => Some(Target::Shortcut(i)),
        Id::Select(key) => Some(Target::Select(key)),
        Id::Option(key, i) => Some(Target::Option(key, i)),
        Id::FindPart(part) => Some(Target::Find(part)),
        Id::ResetColors => Some(Target::ResetColors),
        Id::Well(i) => Some(Target::Well(i)),
        Id::Hex(i) => Some(Target::Hex(i)),
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
        Target::Open(which) => Id::Open(which),
        Target::Shortcut(i) => Id::Shortcut(i),
        Target::Select(key) => Id::Select(key),
        Target::Option(key, i) => Id::Option(key, i),
        Target::Find(part) => Id::FindPart(part),
        Target::ResetColors => Id::ResetColors,
        Target::Well(i) => Id::Well(i),
        Target::Hex(i) => Id::Hex(i),
    }
}

fn children(id: Id, live: Live) -> Vec<Id> {
    match id {
        Id::Root => {
            let mut list = vec![Id::Nav, Id::Title, Id::Subtitle, Id::History];
            if live.status {
                list.push(Id::SaveStatus);
            }
            if live.find {
                list.push(Id::Find);
            }
            list.extend((0..CARDS.len()).map(Id::Card));
            list
        }
        Id::CustomEditor => CUSTOM_CHILDREN.to_vec(),
        Id::Find => {
            let mut list = vec![Id::FindPart(FindPart::Input), Id::FindCount];
            list.extend(FIND_BUTTONS.map(Id::FindPart));
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
                Item::Utility(which) => vec![Id::Open(which)],
                Item::Updates => vec![Id::UpdateVersion, Id::UpdateAction, Id::UpdateStatus],
                Item::CustomTheme if live.custom => vec![Id::CustomEditor],
                Item::CustomTheme => Vec::new(),
                Item::Shortcuts => (0..preferences::SHORTCUT_ROWS.len())
                    .map(Id::Shortcut)
                    .collect(),
                Item::Select(key) => vec![Id::Select(key)],
                Item::SelectGrid(keys) => keys.iter().map(|&k| Id::Select(k)).collect(),
            })
            .collect(),
        // Like a Win32 combo box, the open list's items are its children.
        Id::Select(key) => match live.list {
            Some((open, count)) if open == key => (0..count).map(|i| Id::Option(key, i)).collect(),
            _ => Vec::new(),
        },
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
        Id::Open(which) => Id::Card(card_of(Item::Utility(which))?),
        Id::UpdateVersion | Id::UpdateAction | Id::UpdateStatus => {
            Id::Card(card_of(Item::Updates)?)
        }
        Id::Shortcut(_) => Id::Card(card_of(Item::Shortcuts)?),
        Id::Select(key) => Id::Card(card_of(Item::Select(key)).or_else(|| {
            CARDS.iter().position(|(_, items)| {
                items
                    .iter()
                    .any(|item| matches!(item, Item::SelectGrid(keys) if keys.contains(&key)))
            })
        })?),
        Id::Option(key, _) => Id::Select(key),
        Id::FindPart(_) | Id::FindCount => Id::Find,
        Id::CustomEditor => Id::Card(card_of(Item::CustomTheme)?),
        Id::CustomText(_) | Id::ResetColors | Id::Well(_) | Id::Hex(_) => Id::CustomEditor,
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
        Id::UpdateVersion => scene.update_copy_rect(),
        Id::UpdateAction => scene.update_rects().0,
        Id::UpdateStatus => scene.update_rects().1,
        Id::Find => scene.find_rects().0,
        Id::SaveStatus => scene.save_rect(),
        Id::CustomEditor => scene.custom_rect(),
        Id::CustomText(k) => scene.custom_text_rect(k),
        Id::FindCount => scene.find_rects().1,
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
        // Shipping repeats "Open"; the row title tells the two apart.
        Id::Open(which) => format!("{} {}", which.copy().2, which.copy().0),
        Id::UpdateVersion => preferences::UPDATES_VERSION.into(),
        Id::UpdateAction => preferences::UPDATES_ACTION.into(),
        Id::UpdateStatus => preferences::UPDATES_TITLE.into(),
        Id::Shortcut(i) => preferences::SHORTCUT_ROWS[i].0.into(),
        Id::Select(key) => key.label().into(),
        Id::Option(key, i) => host
            .scene
            .select_options(key)
            .into_iter()
            .nth(i)
            .map_or_else(String::new, |(label, _)| label),
        // `role="search"` has no label in shipping.
        Id::Find => String::new(),
        Id::FindPart(part) => part.label().into(),
        Id::FindCount => host.scene.find_count_label(),
        Id::SaveStatus => host.scene.save_message(),
        Id::CustomEditor => CUSTOM_GROUP_LABEL.into(),
        Id::CustomText(0) => preferences::CUSTOM_THEME.title.into(),
        Id::CustomText(1) => preferences::CUSTOM_THEME.description.into(),
        Id::CustomText(k) => {
            let (_, label, note) = preferences::CUSTOM_THEME.fields[(k - 2) / 2];
            if k.is_multiple_of(2) { label } else { note }.into()
        }
        Id::ResetColors => preferences::CUSTOM_THEME.reset.into(),
        // Shipping `aria-label`s.
        Id::Well(i) => format!("{} color picker", preferences::CUSTOM_THEME.fields[i].1),
        Id::Hex(i) => format!("{} hex value", preferences::CUSTOM_THEME.fields[i].1),
    }
}

fn help(host: &Host, id: Id) -> String {
    match id {
        Id::Card(card) => crate::scene::card_description(CARDS[card].0),
        Id::Switch(row) => host.scene.rows[row].detail.clone(),
        Id::Modes => preferences::row("appearance").description.into(),
        Id::Accents => preferences::row("theme").description.into(),
        Id::Open(which) => which.copy().1.into(),
        Id::UpdateVersion => preferences::UPDATES_CHANNEL.into(),
        Id::UpdateAction | Id::UpdateStatus => preferences::UPDATES_DETAIL.into(),
        Id::Shortcut(i) => host.scene.shortcut_error(i).into(),
        Id::Select(key) => preferences::row(key.setting()).description.into(),
        Id::Well(i) | Id::Hex(i) => preferences::CUSTOM_THEME.fields[i].2.into(),
        _ => String::new(),
    }
}

fn control_type(id: Id) -> UIA_CONTROLTYPE_ID {
    match id {
        Id::Root => UIA_WindowControlTypeId,
        Id::Nav | Id::Card(_) | Id::Modes | Id::Accents => UIA_GroupControlTypeId,
        Id::Title | Id::Subtitle | Id::UpdateVersion | Id::UpdateStatus => UIA_TextControlTypeId,
        Id::Mode(_) | Id::Accent(_) => UIA_RadioButtonControlTypeId,
        Id::Folder => UIA_EditControlTypeId,
        Id::Select(_) => UIA_ComboBoxControlTypeId,
        Id::Option(..) => UIA_ListItemControlTypeId,
        Id::Find => UIA_GroupControlTypeId,
        Id::FindPart(FindPart::Input) => UIA_EditControlTypeId,
        Id::FindPart(_) => UIA_ButtonControlTypeId,
        Id::FindCount | Id::SaveStatus | Id::CustomText(_) => UIA_TextControlTypeId,
        Id::CustomEditor => UIA_GroupControlTypeId,
        Id::Hex(_) => UIA_EditControlTypeId,
        // Chromium exposes `input[type="color"]` as a button with a value.
        Id::ResetColors | Id::Well(_) => UIA_ButtonControlTypeId,
        Id::Open(_) | Id::Shortcut(_) | Id::UpdateAction => UIA_ButtonControlTypeId,
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
        Id::Open(which) => (14, which.index()),
        Id::UpdateVersion => (18, 0),
        Id::UpdateAction => (19, 0),
        Id::UpdateStatus => (20, 0),
        Id::Shortcut(i) => (15, i),
        Id::Select(key) => (16, key.index()),
        Id::Option(key, i) => (17, key.index() * 1000 + i),
        Id::Find => (21, 0),
        Id::FindPart(part) => (22, part.index()),
        Id::FindCount => (23, 0),
        Id::SaveStatus => (24, 0),
        Id::CustomEditor => (25, 0),
        Id::CustomText(k) => (26, k),
        Id::ResetColors => (27, 0),
        Id::Well(i) => (28, i),
        Id::Hex(i) => (29, i),
    };
    [UiaAppendRuntimeId as i32, kind, index as i32]
}

fn selected(host: &Host, id: Id) -> bool {
    match id {
        Id::NavItem(i) => host.scene.active == i,
        Id::Mode(i) => host.scene.mode_index() == i,
        Id::Accent(i) => host.scene.theme_index() == i,
        Id::Option(key, i) => host.scene.select_index(key) == i,
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
        Id::Root
            | Id::Nav
            | Id::NavItem(_)
            | Id::Title
            | Id::Subtitle
            | Id::History
            | Id::Find
            | Id::FindPart(_)
            | Id::FindCount
            | Id::SaveStatus
    ) {
        return false;
    }
    if let Id::Option(..) = id {
        let (r, v) = (rect(host, id), host.scene.list_rect());
        return r.y + r.h <= v.y || r.y >= v.y + v.h;
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

pub(crate) fn safe_array_unknown(values: &[IUnknown]) -> *mut SAFEARRAY {
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
    IExpandCollapseProvider,
    ISelectionProvider,
    ISelectionItemProvider,
    IValueProvider,
    ITextProvider,
    ITextProvider2
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
            // Sections also report the one in view (shipping `aria-current`).
            Id::NavItem(_) => {
                pattern == UIA_InvokePatternId || pattern == UIA_SelectionItemPatternId
            }
            Id::Nav => pattern == UIA_SelectionPatternId,
            Id::History | Id::Choose | Id::Open(_) | Id::UpdateAction => {
                pattern == UIA_InvokePatternId
            }
            Id::Shortcut(_) => pattern == UIA_TogglePatternId || pattern == UIA_ValuePatternId,
            Id::Select(_) => [
                UIA_ExpandCollapsePatternId,
                UIA_SelectionPatternId,
                UIA_ValuePatternId,
            ]
            .contains(&pattern),
            Id::Option(..) => pattern == UIA_SelectionItemPatternId,
            Id::Switch(_) => pattern == UIA_TogglePatternId,
            Id::Modes | Id::Accents => pattern == UIA_SelectionPatternId,
            Id::Mode(_) | Id::Accent(_) => pattern == UIA_SelectionItemPatternId,
            Id::Folder => pattern == UIA_ValuePatternId,
            Id::ResetColors => pattern == UIA_InvokePatternId,
            Id::Well(_) => pattern == UIA_InvokePatternId || pattern == UIA_ValuePatternId,
            Id::FindPart(FindPart::Input) | Id::Hex(_) => {
                [UIA_ValuePatternId, UIA_TextPatternId, UIA_TextPattern2Id].contains(&pattern)
            }
            Id::FindPart(_) => pattern == UIA_InvokePatternId,
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
                // No update worker in this host.
                Id::UpdateAction => false,
                Id::Option(key, i) => scene
                    .select_options(key)
                    .get(i)
                    .is_some_and(|(_, disabled)| !disabled),
                Id::FindPart(part) => scene.find_enabled(part),
                _ => true,
            };
            let value = match id {
                Id::Folder => Some(scene.settings().output_directory.clone()),
                Id::FindPart(FindPart::Input) => Some(scene.find_query()),
                Id::Hex(i) => Some(scene.field_value(FieldId::Hex(i))),
                Id::Well(i) => Some(scene.custom_value(i).to_owned()),
                Id::Select(key) => Some(scene.select_value(key)),
                Id::Shortcut(i) => Some(scene.shortcut_value(i)),
                _ => None,
            };
            // Options are `tabIndex={-1}`; the active one is the trigger's
            // `aria-activedescendant`.
            let focusable = target(id).is_some() && !matches!(id, Id::Option(..));
            let focused = match id {
                Id::Option(key, i) => scene
                    .open
                    .is_some_and(|(open, state)| open == key && state.active == i),
                _ => target(id).is_some() && scene.focus == target(id),
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
                UIA_LandmarkTypePropertyId if id == Id::Find => {
                    VARIANT::from(UIA_SearchLandmarkTypeId.0)
                }
                // `aria-live="polite"`.
                UIA_LiveSettingPropertyId if matches!(id, Id::FindCount | Id::SaveStatus) => {
                    VARIANT::from(Polite.0)
                }
                UIA_AutomationIdPropertyId => VARIANT::from(BSTR::from(format!("{id:?}"))),
                UIA_FrameworkIdPropertyId => VARIANT::from(BSTR::from("Captures")),
                UIA_IsKeyboardFocusablePropertyId => VARIANT::from(focusable && enabled),
                UIA_HasKeyboardFocusPropertyId => VARIANT::from(focused),
                UIA_IsEnabledPropertyId => VARIANT::from(enabled),
                UIA_IsOffscreenPropertyId => VARIANT::from(offscreen(host, id)),
                UIA_IsControlElementPropertyId | UIA_IsContentElementPropertyId => {
                    VARIANT::from(true)
                }
                UIA_SelectionItemIsSelectedPropertyId => VARIANT::from(selected(host, id)),
                UIA_AriaPropertiesPropertyId if matches!(id, Id::NavItem(_)) => {
                    let current = if selected(host, id) { "true" } else { "false" };
                    VARIANT::from(BSTR::from(format!("current={current}")))
                }
                UIA_ValueValuePropertyId if value.is_some() => {
                    VARIANT::from(BSTR::from(value.unwrap_or_default()))
                }
                UIA_ValueIsReadOnlyPropertyId if value.is_some() => {
                    VARIANT::from(field_id(id).is_none())
                }
                UIA_ExpandCollapseExpandCollapseStatePropertyId => match id {
                    Id::Select(key) => VARIANT::from(expand_state(host, key).0),
                    _ => VARIANT::default(),
                },
                UIA_ToggleToggleStatePropertyId => match id {
                    Id::Shortcut(i) => VARIANT::from(toggle_state(host, i).0),
                    _ => VARIANT::default(),
                },
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
        let live = with(|host| Ok(live(host)))?;
        let children = |id| children(id, live);
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
            let live = live(host);
            for id in children(Id::Root, live) {
                if rect(host, id).contains(px, py) {
                    for child in children(id, live) {
                        if id == Id::Find && rect(host, child).contains(px, py) {
                            return Ok(child);
                        }
                    }
                    return Ok(id);
                }
            }
            Ok(Id::Root)
        })?;
        Ok(fragment(hit))
    }

    fn GetFocus(&self) -> Result<IRawElementProviderFragment> {
        let focus = with(|host| {
            Ok(match host.scene.open {
                Some((key, state)) => Some(Target::Option(key, state.active)),
                None => host.scene.focus,
            })
        })?;
        focus
            .map(|target| fragment(id_of(target)))
            .ok_or_else(Error::empty)
    }
}

impl IInvokeProvider_Impl for Node_Impl {
    fn Invoke(&self) -> Result<()> {
        let id = self.id;
        if id == Id::UpdateAction {
            return Err(HRESULT(UIA_E_ELEMENTNOTENABLED as _).into());
        }
        with(|host| {
            if let Id::FindPart(part) = id
                && !host.scene.find_enabled(part)
            {
                return Err(HRESULT(UIA_E_ELEMENTNOTENABLED as _).into());
            }
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
            if let Id::Shortcut(i) = id {
                // `aria-pressed`: a second toggle stops recording.
                if host.scene.recording == Some(i) {
                    host.scene.stop_recording();
                } else {
                    host.scene.set_focus(Some(Target::Shortcut(i)), true);
                    host.scene.start_recording(i);
                }
                host.need = true;
            }
            Ok(())
        })
    }

    fn ToggleState(&self) -> Result<ToggleState> {
        let id = self.id;
        with(|host| match id {
            Id::Switch(row) if host.scene.rows[row].on => Ok(ToggleState_On),
            Id::Shortcut(i) => Ok(toggle_state(host, i)),
            _ => Ok(ToggleState_Off),
        })
    }
}

impl ISelectionProvider_Impl for Node_Impl {
    fn GetSelection(&self) -> Result<*mut SAFEARRAY> {
        let id = self.id;
        let chosen = with(|host| {
            Ok(match id {
                Id::Select(key) => Id::Option(key, host.scene.select_index(key)),
                Id::Nav => Id::NavItem(host.scene.active),
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
    fn SetValue(&self, value: &PCWSTR) -> Result<()> {
        let id = self.id;
        let Some(field) = field_id(id) else {
            // Read-only: Choose… changes the folder, the picker a color.
            return Err(HRESULT(UIA_E_ELEMENTNOTENABLED as _).into());
        };
        let text = unsafe { value.to_string() }.unwrap_or_default();
        with(|host| {
            if !host.scene.field_shown(field) {
                return Err(unavailable());
            }
            let Host { scene, g, .. } = host;
            scene.field_edit_in(g, field, |editor| editor.set_text(&text));
            // A hex input commits as if typed and left.
            if let FieldId::Hex(i) = field {
                scene.commit_typed_hex(i);
            }
            host.need = true;
            Ok(())
        })
    }

    fn Value(&self) -> Result<BSTR> {
        let id = self.id;
        with(|host| {
            let scene = &host.scene;
            Ok(BSTR::from(match id {
                Id::Select(key) => scene.select_value(key),
                Id::Shortcut(i) => scene.shortcut_value(i),
                Id::FindPart(FindPart::Input) => scene.find_query(),
                Id::Hex(i) => scene.field_value(FieldId::Hex(i)),
                Id::Well(i) => scene.custom_value(i).to_owned(),
                _ => scene.settings().output_directory.clone(),
            }))
        })
    }

    fn IsReadOnly(&self) -> Result<BOOL> {
        Ok(field_id(self.id).is_none().into())
    }
}

impl ITextProvider_Impl for Node_Impl {
    fn GetSelection(&self) -> Result<*mut SAFEARRAY> {
        let id = self.id;
        let (a, b) = with(|host| {
            field(host, id)
                .map(|f| f.editor.selection())
                .ok_or_else(unavailable)
        })?;
        Ok(safe_array_unknown(&[range(id, a, b).cast()?]))
    }

    fn GetVisibleRanges(&self) -> Result<*mut SAFEARRAY> {
        let id = self.id;
        let n = with(|host| {
            field(host, id)
                .map(|f| f.editor.len())
                .ok_or_else(unavailable)
        })?;
        Ok(safe_array_unknown(&[range(id, 0, n).cast()?]))
    }

    fn RangeFromChild(&self, _child: Ref<IRawElementProviderSimple>) -> Result<ITextRangeProvider> {
        Err(E_INVALIDARG.into())
    }

    fn RangeFromPoint(&self, point: &UiaPoint) -> Result<ITextRangeProvider> {
        let id = self.id;
        let position = with(|host| {
            let (box_, scale) = field_screen_rect(host, id);
            let x = ((point.x - box_.left) / scale) as f32;
            Ok(field(host, id).map_or(0, |f| f.hit(x)))
        })?;
        Ok(range(id, position, position))
    }

    fn DocumentRange(&self) -> Result<ITextRangeProvider> {
        let id = self.id;
        let n = with(|host| {
            field(host, id)
                .map(|f| f.editor.len())
                .ok_or_else(unavailable)
        })?;
        Ok(range(id, 0, n))
    }

    fn SupportedTextSelection(&self) -> Result<SupportedTextSelection> {
        Ok(SupportedTextSelection_Single)
    }
}

impl ITextProvider2_Impl for Node_Impl {
    fn RangeFromAnnotation(
        &self,
        _annotation: Ref<IRawElementProviderSimple>,
    ) -> Result<ITextRangeProvider> {
        Err(E_INVALIDARG.into())
    }

    fn GetCaretRange(&self, active: *mut BOOL) -> Result<ITextRangeProvider> {
        let id = self.id;
        let (caret, focused) = with(|host| {
            let caret = field(host, id)
                .map(|f| f.editor.caret())
                .ok_or_else(unavailable)?;
            Ok((caret, host.scene.focused_field() == field_id(id)))
        })?;
        if !active.is_null() {
            unsafe { *active = focused.into() };
        }
        Ok(range(id, caret, caret))
    }
}

fn expand_state(host: &Host, key: SelectKey) -> ExpandCollapseState {
    if host.scene.open.is_some_and(|(open, _)| open == key) {
        ExpandCollapseState_Expanded
    } else {
        ExpandCollapseState_Collapsed
    }
}

fn toggle_state(host: &Host, index: usize) -> ToggleState {
    if host.scene.recording == Some(index) {
        ToggleState_On
    } else {
        ToggleState_Off
    }
}

impl IExpandCollapseProvider_Impl for Node_Impl {
    fn Expand(&self) -> Result<()> {
        let id = self.id;
        with(|host| {
            if let Id::Select(key) = id {
                host.scene.set_focus(Some(Target::Select(key)), true);
                host.scene.open_select(key);
                host.need = true;
            }
            Ok(())
        })
    }

    fn Collapse(&self) -> Result<()> {
        with(|host| {
            host.scene.close_select();
            host.need = true;
            Ok(())
        })
    }

    fn ExpandCollapseState(&self) -> Result<ExpandCollapseState> {
        let id = self.id;
        with(|host| match id {
            Id::Select(key) => Ok(expand_state(host, key)),
            _ => Ok(ExpandCollapseState_LeafNode),
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
                Event::Expanded(key, open) => {
                    let state = |open: bool| {
                        VARIANT::from(if open {
                            ExpandCollapseState_Expanded.0
                        } else {
                            ExpandCollapseState_Collapsed.0
                        })
                    };
                    let combo = node(Id::Select(key));
                    let _ = UiaRaiseAutomationPropertyChangedEvent(
                        &combo,
                        UIA_ExpandCollapseExpandCollapseStatePropertyId,
                        &state(!open),
                        &state(open),
                    );
                    let _ = UiaRaiseStructureChangedEvent(
                        &combo,
                        StructureChangeType_ChildrenInvalidated,
                        runtime_id(Id::Select(key)).as_mut_ptr(),
                        3,
                    );
                    if !open {
                        // Focus returns to the trigger.
                        let _ = UiaRaiseAutomationEvent(&combo, UIA_AutomationFocusChangedEventId);
                    }
                }
                Event::Active(key, index) => {
                    let _ = UiaRaiseAutomationEvent(
                        &node(Id::Option(key, index)),
                        UIA_AutomationFocusChangedEventId,
                    );
                }
                Event::Value(target) => {
                    let _ = UiaRaiseAutomationPropertyChangedEvent(
                        &node(id_of(target)),
                        UIA_ValueValuePropertyId,
                        &VARIANT::default(),
                        &VARIANT::from(BSTR::from(detail.as_str())),
                    );
                }
                Event::Recording(index, recording) => {
                    let state = |on: bool| VARIANT::from(if on { 1i32 } else { 0i32 });
                    let recorder = node(Id::Shortcut(index));
                    let _ = UiaRaiseAutomationPropertyChangedEvent(
                        &recorder,
                        UIA_ToggleToggleStatePropertyId,
                        &state(!recording),
                        &state(recording),
                    );
                    let _ = UiaRaiseAutomationPropertyChangedEvent(
                        &recorder,
                        UIA_ValueValuePropertyId,
                        &VARIANT::default(),
                        &VARIANT::from(BSTR::from(detail.as_str())),
                    );
                }
                Event::SaveStatus => {
                    let root = node(Id::Root);
                    let _ = UiaRaiseStructureChangedEvent(
                        &root,
                        StructureChangeType_ChildrenInvalidated,
                        runtime_id(Id::Root).as_mut_ptr(),
                        3,
                    );
                    if !detail.is_empty() {
                        let status = node(Id::SaveStatus);
                        let _ = UiaRaiseAutomationEvent(&status, UIA_LiveRegionChangedEventId);
                        let _ = UiaRaiseNotificationEvent(
                            &status,
                            NotificationKind_ActionCompleted,
                            NotificationProcessing_MostRecent,
                            &BSTR::from(detail.as_str()),
                            &BSTR::from("save-status"),
                        );
                    }
                }
                Event::FindOpen(_) => {
                    let root = node(Id::Root);
                    let _ = UiaRaiseStructureChangedEvent(
                        &root,
                        StructureChangeType_ChildrenInvalidated,
                        runtime_id(Id::Root).as_mut_ptr(),
                        3,
                    );
                }
                Event::FieldText(field) => {
                    let input = node(field_uia(field));
                    let _ = UiaRaiseAutomationPropertyChangedEvent(
                        &input,
                        UIA_ValueValuePropertyId,
                        &VARIANT::default(),
                        &VARIANT::from(BSTR::from(detail.as_str())),
                    );
                    let _ = UiaRaiseAutomationEvent(&input, UIA_Text_TextChangedEventId);
                }
                Event::FieldCaret(field) => {
                    let _ = UiaRaiseAutomationEvent(
                        &node(field_uia(field)),
                        UIA_Text_TextSelectionChangedEventId,
                    );
                }
                Event::FindCount => {
                    let count = node(Id::FindCount);
                    let _ = UiaRaiseAutomationPropertyChangedEvent(
                        &count,
                        UIA_NamePropertyId,
                        &VARIANT::default(),
                        &VARIANT::from(BSTR::from(detail.as_str())),
                    );
                    let _ = UiaRaiseAutomationEvent(&count, UIA_LiveRegionChangedEventId);
                }
                Event::ShortcutError(index) => {
                    let recorder = node(Id::Shortcut(index));
                    let _ = UiaRaiseAutomationPropertyChangedEvent(
                        &recorder,
                        UIA_HelpTextPropertyId,
                        &VARIANT::default(),
                        &VARIANT::from(BSTR::from(detail.as_str())),
                    );
                    // `role="status"`: announce a new error.
                    if !detail.is_empty() {
                        let _ = UiaRaiseNotificationEvent(
                            &recorder,
                            NotificationKind_Other,
                            NotificationProcessing_ImportantMostRecent,
                            &BSTR::from(detail.as_str()),
                            &BSTR::from("shortcut-error"),
                        );
                    }
                }
            }
        }
    }
}

#![allow(non_upper_case_globals)]
//! UI Automation for the screenshot editor, following shipping
//! `ScreenshotEditor.tsx`: the restored-draft status, the "Canvas" group,
//! Undo and Redo, the "Canvas zoom controls" group (Fit, Zoom out, the
//! "Canvas zoom" slider, Zoom in, the preset select), Add images, the
//! "Screenshot tools" navigation of `aria-pressed` buttons, the "Shapes" menu
//! of `menuitemradio` items, the editing canvas region, the "Resize
//! properties panel" separator, the Layers region with its rows and quick
//! actions, the "Tool properties" region, and the export bar.
//!
//! Every editor window has its own tree: nodes carry their window handle.
//! Controls this host draws but doesn't implement yet are present and
//! disabled, as they are drawn.
use captures_app::editor_chrome::{self, RAIL_TOOLS, SHAPE_TOOLS, header};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Com::*;
use windows::Win32::System::Ole::*;
use windows::Win32::System::Variant::*;
use windows::Win32::UI::Accessibility::*;
use windows::core::*;

use crate::editor::{
    CUSTOM, Check, Editor, Event, Field, MenuKind, Num, Prop, Stub, Target, with_editor,
};
use crate::scene::Rect;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Id {
    Root,
    /// `.screenshot-editor-draft-banner` (`role="status"`) and its text.
    Banner,
    BannerText,
    /// `role="group" aria-label="Canvas"`.
    CanvasGroup,
    Zoom,
    /// `nav aria-label="Screenshot tools"`.
    Tools,
    /// `role="menu" aria-label="Shapes"`.
    Flyout,
    /// `section aria-label="Screenshot editing canvas"`.
    Canvas,
    /// `section aria-label="Layers"`, its heading and its list.
    Layers,
    LayersTitle,
    List,
    Row(usize),
    /// The inline label editor (`textarea aria-label="Edit text on canvas"`).
    InlineText,
    /// A row's open rename input (`input aria-label="Rename layer"`), in
    /// place of the row button (`role="group"` while renaming).
    Rename,
    /// `section aria-label="Tool properties"`.
    Properties,
    /// `.screenshot-shape-picker` (`role="group" aria-label="Shape"`).
    ShapePicker,
    /// A `ColorField` fieldset, named by its legend.
    ColorField(Field),
    /// `role="group" aria-label="Eraser mode"`.
    ModeGroup,
    /// `output aria-label="Save location"`.
    Location,
    /// `input aria-label="Saved filename"`.
    Filename,
    /// `.screenshot-export-notice` and `.screenshot-export-hint`.
    Notice,
    Hint,
    Control(Target),
    Stub(Stub),
    /// The open Background color card (`role="dialog"`).
    BgMenu,
    /// The open Text style menu (`role="menu"` of `menuitemradio`) or Font
    /// list (`listbox` of options), and its rows.
    Menu,
    MenuItem(usize),
}

fn unavailable() -> Error {
    HRESULT(UIA_E_ELEMENTNOTAVAILABLE as _).into()
}

fn not_enabled() -> Error {
    HRESULT(UIA_E_ELEMENTNOTENABLED as _).into()
}

fn read_only() -> Error {
    HRESULT(UIA_E_INVALIDOPERATION as _).into()
}

fn with<R>(hwnd: HWND, f: impl FnOnce(&mut Editor) -> Result<R>) -> Result<R> {
    with_editor(hwnd, f).unwrap_or_else(|| Err(unavailable()))
}

fn shapes() -> usize {
    RAIL_TOOLS
        .iter()
        .position(|t| t.key == editor_chrome::SHAPES_KEY)
        .unwrap_or(0)
}

fn children(e: &Editor, id: Id) -> Vec<Id> {
    let c = Id::Control;
    match id {
        Id::Root => {
            let mut list = Vec::new();
            if e.banner {
                list.push(Id::Banner);
            }
            list.push(Id::CanvasGroup);
            if e.exists(Target::Undo) {
                list.extend([c(Target::Undo), c(Target::Redo)]);
            }
            list.extend([Id::Zoom, Id::Stub(Stub::AddImages), Id::Tools]);
            if e.flyout {
                list.push(Id::Flyout);
            }
            list.extend([
                Id::Canvas,
                c(Target::Divider),
                Id::Layers,
                Id::Properties,
                Id::Stub(Stub::ExportSettings),
                Id::Location,
                Id::Stub(Stub::Change),
                Id::Filename,
                Id::Stub(Stub::Format),
                c(Target::Copy),
            ]);
            if e.notice_text().is_some() {
                list.push(Id::Notice);
            }
            if notice_error(e).is_none() && !e.hint_text().is_empty() {
                list.push(Id::Hint);
            }
            if e.exists(Target::SaveAsNew) {
                list.push(c(Target::SaveAsNew));
            }
            list.push(c(Target::Save));
            // Shipping portals the card and the menu to the end of `body`.
            if e.bg_open() {
                list.push(Id::BgMenu);
            }
            if e.open_menu_state().is_some() {
                list.push(Id::Menu);
            }
            list
        }
        Id::BgMenu if e.bg_open() => vec![
            c(Target::Prop(Prop::Check(Check::Solid))),
            Id::ColorField(Field::Canvas),
        ],
        Id::ColorField(Field::Canvas) if e.bg_open() => (0..=CUSTOM)
            .map(|i| c(Target::Prop(Prop::Swatch(Field::Canvas, i))))
            .collect(),
        Id::Menu => match e.open_menu_state() {
            Some((kind, _)) => (0..e.menu_items(kind).len()).map(Id::MenuItem).collect(),
            None => Vec::new(),
        },
        Id::Control(Target::Prop(Prop::Number(num))) => [true, false]
            .into_iter()
            .map(|up| c(Target::Prop(Prop::Step(num, up))))
            .filter(|id| matches!(id, Id::Control(t) if e.exists(*t)))
            .collect(),
        Id::Banner => vec![Id::BannerText, c(Target::Dismiss)],
        Id::CanvasGroup => vec![
            c(Target::Prop(Prop::Number(Num::CanvasWidth))),
            c(Target::Prop(Prop::Number(Num::CanvasHeight))),
            c(Target::Trim),
            c(Target::Background),
        ],
        Id::Zoom => vec![
            c(Target::Fit),
            c(Target::ZoomOut),
            c(Target::Slider),
            c(Target::ZoomIn),
            c(Target::Prop(Prop::Preset)),
        ],
        Id::Tools => (0..RAIL_TOOLS.len()).map(|i| c(Target::Tool(i))).collect(),
        Id::Flyout => (0..SHAPE_TOOLS.len())
            .map(|i| c(Target::Shape(i)))
            .collect(),
        Id::Layers => vec![Id::LayersTitle, Id::Stub(Stub::AddLayer), Id::List],
        Id::List => (0..e.layer_count()).map(Id::Row).collect(),
        Id::Row(i) if i < e.layer_count() => vec![
            if e.renaming() == Some(i) {
                Id::Rename
            } else {
                c(Target::Layer(i))
            },
            c(Target::Eye(i)),
            c(Target::Lock(i)),
            Id::Stub(Stub::LayerMenu(i)),
        ],
        Id::Properties => {
            let mut list = Vec::new();
            for prop in e.prop_order() {
                let id = match prop {
                    Prop::Shape(_) => Id::ShapePicker,
                    Prop::Mode(_) => Id::ModeGroup,
                    Prop::Swatch(field, _) => Id::ColorField(field),
                    prop => c(Target::Prop(prop)),
                };
                if !list.contains(&id) {
                    list.push(id);
                }
            }
            list
        }
        Id::Canvas if e.text_active() => vec![Id::InlineText],
        Id::ShapePicker | Id::ColorField(_) | Id::ModeGroup => e
            .prop_order()
            .into_iter()
            .filter(|&prop| prop_parent(prop) == id)
            .map(|prop| c(Target::Prop(prop)))
            .collect(),
        _ => Vec::new(),
    }
}

fn prop_parent(prop: Prop) -> Id {
    match prop {
        Prop::Check(Check::Solid) => Id::BgMenu,
        Prop::Shape(_) => Id::ShapePicker,
        Prop::Mode(_) => Id::ModeGroup,
        Prop::Swatch(field, _) => Id::ColorField(field),
        Prop::Step(num, _) => Id::Control(Target::Prop(Prop::Number(num))),
        _ => Id::Properties,
    }
}

fn parent(e: &Editor, id: Id) -> Option<Id> {
    Some(match id {
        Id::Root => return None,
        Id::BannerText | Id::Control(Target::Dismiss) => Id::Banner,
        Id::Control(Target::Trim | Target::Background) => Id::CanvasGroup,
        Id::ColorField(Field::Canvas) => Id::BgMenu,
        Id::Control(Target::Prop(Prop::Number(num))) if num.is_canvas() => Id::CanvasGroup,
        Id::Control(Target::Fit | Target::ZoomOut | Target::Slider | Target::ZoomIn)
        | Id::Control(Target::Prop(Prop::Preset)) => Id::Zoom,
        Id::Control(Target::Tool(_)) => Id::Tools,
        Id::Control(Target::Shape(_)) => Id::Flyout,
        Id::LayersTitle | Id::Stub(Stub::AddLayer) | Id::List => Id::Layers,
        Id::Row(_) => Id::List,
        Id::ShapePicker | Id::ColorField(_) | Id::ModeGroup => Id::Properties,
        Id::InlineText => Id::Canvas,
        Id::Rename => Id::Row(e.renaming()?),
        Id::MenuItem(_) => Id::Menu,
        Id::Control(Target::Prop(prop)) => prop_parent(prop),
        Id::Control(Target::Layer(i) | Target::Eye(i) | Target::Lock(i))
        | Id::Stub(Stub::LayerMenu(i)) => Id::Row(i),
        _ => Id::Root,
    })
}

fn notice_error(e: &Editor) -> Option<String> {
    e.notice_text()
        .filter(|(_, error)| *error)
        .map(|(text, _)| text)
}

/// Client-area rect in DIPs.
fn rect(e: &Editor, id: Id) -> Rect {
    let l = &e.l;
    match id {
        Id::Root => Rect::new(0., 0., e.w, e.h),
        Id::Banner | Id::BannerText => l.banner,
        Id::CanvasGroup => l.canvas_group,
        Id::Zoom => l.zoom,
        Id::Tools => l.rail,
        Id::Flyout => l.flyout,
        Id::Canvas => l.well,
        Id::Layers => l.layers,
        Id::LayersTitle => Rect::new(l.layers.x, l.layers.y, l.layers.w * 0.5, 48.),
        Id::List => l.list,
        Id::Row(i) => {
            let r = e.target_rect(Target::Layer(i));
            Rect::new(l.list.x, r.y, l.list.w, r.h)
        }
        Id::Properties => l.properties,
        Id::InlineText => e.text_rect(),
        Id::Rename => e.rename_rect().unwrap_or_default(),
        Id::ShapePicker => e.picker_rect(),
        Id::ModeGroup => e.modes_rect(),
        Id::ColorField(field) => e.color_field_rect(field),
        Id::Location => l.folder,
        Id::Filename => l.filename,
        Id::Notice => l.notice_line,
        Id::Hint => l.hint_line,
        Id::Control(target) => e.target_rect(target),
        Id::Stub(stub) => e.stub_rect(stub),
        Id::BgMenu => e.bg_menu_rect(),
        Id::Menu => e.menu_rect().unwrap_or_default(),
        Id::MenuItem(i) => e.menu_item_rect(i),
    }
}

fn name(e: &Editor, id: Id) -> String {
    match id {
        Id::Root => captures_app::app_windows::SCREENSHOT_EDITOR_TITLE.into(),
        Id::BannerText => header::DRAFT_RESTORED.into(),
        Id::CanvasGroup => header::CANVAS.into(),
        Id::Zoom => header::ZOOM_GROUP.into(),
        Id::Tools => "Screenshot tools".into(),
        Id::Flyout => editor_chrome::SHAPES_LABEL.into(),
        Id::Canvas => "Screenshot editing canvas".into(),
        Id::Layers | Id::LayersTitle => editor_chrome::layers::TITLE.into(),
        Id::Row(i) => e.name(Target::Layer(i)),
        Id::Properties => "Tool properties".into(),
        Id::InlineText => editor_chrome::text_format::INLINE_LABEL.into(),
        Id::Rename => captures_app::editor_layers::menu::RENAME_LABEL.into(),
        Id::ShapePicker => "Shape".into(),
        Id::ModeGroup => "Eraser mode".into(),
        Id::ColorField(field) => e.field_label(field),
        Id::Location => "Save location".into(),
        Id::Filename => "Saved filename".into(),
        Id::Notice => e.notice_text().map(|(text, _)| text).unwrap_or_default(),
        Id::Hint => e.hint_text(),
        Id::Control(target) => e.name(target),
        Id::Stub(stub) => e.stub_name(stub),
        Id::BgMenu => editor_chrome::colors::CANVAS_BACKGROUND.into(),
        Id::Menu => match e.open_menu_state() {
            Some((MenuKind::Style, _)) => e.style_label().into(),
            Some((MenuKind::Font, _)) => "Font".into(),
            Some((MenuKind::Aspect, _)) => crate::editor::ASPECT_LABEL.into(),
            Some((MenuKind::Preset, _)) => editor_chrome::header::ZOOM_PRESET.into(),
            None => String::new(),
        },
        Id::MenuItem(i) => e
            .open_menu_state()
            .and_then(|(kind, _)| e.menu_items(kind).into_iter().nth(i))
            .unwrap_or_default(),
        Id::Banner | Id::List => String::new(),
    }
}

fn menu_kind(e: &Editor) -> Option<MenuKind> {
    e.open_menu_state().map(|(kind, _)| kind)
}

/// A popup trigger's kind.
fn trigger_kind(id: Id) -> Option<MenuKind> {
    match id {
        Id::Control(Target::Prop(Prop::StyleTrigger)) => Some(MenuKind::Style),
        Id::Control(Target::Prop(Prop::Font)) => Some(MenuKind::Font),
        Id::Control(Target::Prop(Prop::Aspect)) => Some(MenuKind::Aspect),
        Id::Control(Target::Prop(Prop::Preset)) => Some(MenuKind::Preset),
        _ => None,
    }
}

fn control_type(e: &Editor, id: Id) -> UIA_CONTROLTYPE_ID {
    match id {
        Id::Menu if menu_kind(e).is_some_and(MenuKind::is_select) => UIA_ListControlTypeId,
        Id::Menu => UIA_MenuControlTypeId,
        // Chromium maps `role="dialog"` to a pane.
        Id::BgMenu => UIA_PaneControlTypeId,
        Id::MenuItem(_) if menu_kind(e).is_some_and(MenuKind::is_select) => {
            UIA_ListItemControlTypeId
        }
        Id::MenuItem(_) => UIA_MenuItemControlTypeId,
        Id::Control(Target::Prop(Prop::Font | Prop::Aspect | Prop::Preset)) => {
            UIA_ComboBoxControlTypeId
        }
        Id::Control(Target::Prop(Prop::Number(_) | Prop::Textarea)) => UIA_EditControlTypeId,
        Id::Root => UIA_WindowControlTypeId,
        Id::Banner
        | Id::CanvasGroup
        | Id::Zoom
        | Id::Tools
        | Id::Canvas
        | Id::Layers
        | Id::Properties
        | Id::ShapePicker
        | Id::ModeGroup
        | Id::ColorField(_) => UIA_GroupControlTypeId,
        Id::Flyout => UIA_MenuControlTypeId,
        Id::List => UIA_ListControlTypeId,
        Id::Row(_) => UIA_ListItemControlTypeId,
        Id::Filename | Id::InlineText | Id::Rename => UIA_EditControlTypeId,
        Id::Stub(Stub::Format) => UIA_ComboBoxControlTypeId,
        Id::Stub(_) => UIA_ButtonControlTypeId,
        Id::Control(Target::Shape(_)) => UIA_MenuItemControlTypeId,
        Id::Control(Target::Slider) => UIA_SliderControlTypeId,
        Id::Control(Target::Divider) => UIA_SeparatorControlTypeId,
        Id::Control(Target::SaveAsNew | Target::Prop(Prop::Check(_))) => UIA_CheckBoxControlTypeId,
        Id::Control(Target::Prop(Prop::Range(_))) => UIA_SliderControlTypeId,
        Id::Control(_) => UIA_ButtonControlTypeId,
        Id::BannerText | Id::LayersTitle | Id::Location | Id::Notice | Id::Hint => {
            UIA_TextControlTypeId
        }
    }
}

fn runtime_id(id: Id) -> [i32; 4] {
    let target = |t: Target| -> (i32, i32) {
        match t {
            Target::Dismiss => (0, 0),
            Target::Undo => (1, 0),
            Target::Redo => (2, 0),
            Target::Fit => (3, 0),
            Target::ZoomOut => (4, 0),
            Target::Slider => (5, 0),
            Target::ZoomIn => (6, 0),
            Target::Tool(i) => (7, i as i32),
            Target::Shape(i) => (8, i as i32),
            Target::Divider => (9, 0),
            Target::Layer(i) => (10, i as i32),
            Target::Eye(i) => (11, i as i32),
            Target::Lock(i) => (12, i as i32),
            Target::Copy => (13, 0),
            Target::SaveAsNew => (14, 0),
            Target::Save => (15, 0),
            Target::Trim => (21, 0),
            Target::Background => (30, 0),
            Target::Prop(Prop::Shape(i)) => (16, i as i32),
            Target::Prop(Prop::Check(check)) => (17, check as i32),
            Target::Prop(Prop::Swatch(field, i)) => (18, field as i32 * 16 + i as i32),
            Target::Prop(Prop::Range(range)) => (19, range as i32),
            Target::Prop(Prop::Mode(i)) => (20, i as i32),
            Target::Prop(Prop::StyleTrigger) => (22, 0),
            Target::Prop(Prop::Font) => (23, 0),
            Target::Prop(Prop::Number(num)) => (24, num as i32),
            Target::Prop(Prop::Step(num, up)) => (25, num as i32 * 2 + i32::from(up)),
            Target::Prop(Prop::Textarea) => (26, 0),
            Target::Prop(Prop::Format(i)) => (27, i as i32),
            Target::Prop(Prop::Aspect) => (28, 0),
            Target::Prop(Prop::CropAction(apply)) => (29, i32::from(apply)),
            Target::Prop(Prop::Preset) => (31, 0),
            Target::Prop(Prop::Straighten) => (32, 0),
        }
    };
    let stub = |s: Stub| -> (i32, i32) {
        match s {
            Stub::AddImages => (5, 0),
            Stub::AddLayer => (6, 0),
            Stub::LayerMenu(i) => (7, i as i32),
            Stub::ExportSettings => (8, 0),
            Stub::Change => (9, 0),
            Stub::Format => (10, 0),
        }
    };
    let (kind, a) = match id {
        Id::Root => (0, 0),
        Id::Banner => (1, 0),
        Id::BannerText => (2, 0),
        Id::CanvasGroup => (3, 0),
        Id::Zoom => (4, 0),
        Id::Tools => (5, 0),
        Id::Flyout => (6, 0),
        Id::Canvas => (7, 0),
        Id::Layers => (8, 0),
        Id::LayersTitle => (9, 0),
        Id::List => (10, 0),
        Id::Row(i) => (11, i as i32),
        Id::Properties => (12, 0),
        Id::Location => (13, 0),
        Id::Filename => (14, 0),
        Id::Notice => (15, 0),
        Id::Hint => (16, 0),
        Id::ShapePicker => (17, 0),
        Id::ColorField(field) => (18, field as i32),
        Id::InlineText => (19, 0),
        Id::Rename => (24, 0),
        Id::ModeGroup => (20, 0),
        Id::Menu => (21, 0),
        Id::BgMenu => (23, 0),
        Id::MenuItem(i) => (22, i as i32),
        Id::Control(t) => {
            let (k, a) = target(t);
            (100 + k, a)
        }
        Id::Stub(s) => {
            let (k, a) = stub(s);
            (200 + k, a)
        }
    };
    // A distinct second part keeps these apart from Preferences' and
    // History's ids; the host provider's prefix keeps windows apart.
    [UiaAppendRuntimeId as i32, 1000 + kind, a, 0]
}

fn screen_rect(e: &Editor, r: Rect) -> UiaRect {
    let mut origin = POINT::default();
    unsafe {
        let _ = ClientToScreen(e.hwnd, &mut origin);
    }
    let s = f64::from(e.scale);
    UiaRect {
        left: f64::from(origin.x) + f64::from(r.x) * s,
        top: f64::from(origin.y) + f64::from(r.y) * s,
        width: f64::from(r.w) * s,
        height: f64::from(r.h) * s,
    }
}

fn offscreen(e: &Editor, id: Id) -> bool {
    match id {
        // Rows scrolled out of the list.
        Id::Row(_)
        | Id::Rename
        | Id::Control(Target::Layer(_) | Target::Eye(_) | Target::Lock(_))
        | Id::Stub(Stub::LayerMenu(_)) => {
            let (r, list) = (rect(e, id), e.l.list);
            r.y + r.h <= list.y || r.y >= list.y + list.h
        }
        Id::ShapePicker | Id::ColorField(_) | Id::ModeGroup | Id::Control(Target::Prop(_)) => {
            let (r, view) = (rect(e, id), e.props_view());
            r.y + r.h <= view.y || r.y >= view.y + view.h
        }
        _ => false,
    }
}

/// The Properties body's vertical scroll position, 0 to 100, or
/// `UIA_ScrollPatternNoScroll` when it fits.
fn scroll_percent(e: &Editor) -> f64 {
    let max = e.props_max();
    if max <= 0. {
        UIA_ScrollPatternNoScroll
    } else {
        f64::from(e.props_scroll / max * 100.)
    }
}

/// A control inside the scrolled Properties body: the property its
/// `ScrollIntoView` reveals.
fn scroll_item(id: Id) -> Option<Prop> {
    match id {
        Id::Control(Target::Prop(prop)) => Some(prop),
        _ => None,
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

fn toggle(on: bool) -> ToggleState {
    if on { ToggleState_On } else { ToggleState_Off }
}

/// A control's toggle state: `aria-pressed` buttons, the layer rows'
/// selection and the Save as new file checkbox.
fn toggle_state(e: &Editor, id: Id) -> Option<ToggleState> {
    let Id::Control(target) = id else {
        return None;
    };
    match target {
        Target::Layer(i) => {
            let layer = e.layer(i)?;
            Some(toggle(e.selected.as_deref() == Some(layer.id.as_str())))
        }
        Target::Shape(_) => None,
        _ => e.pressed(target).map(toggle),
    }
}

/// `(value, min, max, small step, large step)` of the slider and divider.
fn range(e: &Editor, id: Id) -> Option<(f64, f64, f64, f64, f64)> {
    match id {
        Id::Control(Target::Slider) => Some((e.slider_value(), 5., 800., 10., 50.)),
        Id::Control(Target::Divider) => {
            let (min, max) = e.sidebar_bounds();
            Some((
                f64::from(e.sidebar_width()),
                f64::from(min),
                f64::from(max),
                10.,
                40.,
            ))
        }
        Id::Control(Target::Prop(Prop::Range(range))) => {
            let (min, max) = crate::editor::range_bounds(range);
            Some((
                e.range_value(range),
                min,
                max,
                1.,
                ((max - min) / 10.).round(),
            ))
        }
        _ => None,
    }
}

/// `aria-valuetext` or an input's value.
fn value(e: &Editor, id: Id) -> Option<String> {
    Some(match id {
        Id::Control(Target::Slider) => e.slider_text(),
        Id::Control(Target::Divider) => format!("{} pixels wide", e.sidebar_width()),
        Id::Control(Target::Prop(Prop::Range(range))) => {
            crate::editor::range_text(range, e.range_value(range))
        }
        Id::Filename => e.filename(),
        Id::InlineText => e.text_value(),
        Id::Rename => e.rename_text(),
        Id::Control(Target::Prop(prop @ (Prop::Number(_) | Prop::Textarea))) => e.field_text(prop),
        Id::Control(Target::Prop(prop @ (Prop::Font | Prop::Aspect | Prop::Preset))) => {
            let kind = trigger_kind(Id::Control(Target::Prop(prop)))?;
            e.menu_displayed(kind)
                .and_then(|i| e.menu_items(kind).into_iter().nth(i))
                .unwrap_or_default()
        }
        Id::Location => e.save_location(),
        _ => return None,
    })
}

#[implement(
    IRawElementProviderSimple,
    IRawElementProviderFragment,
    IRawElementProviderFragmentRoot,
    IInvokeProvider,
    IToggleProvider,
    IExpandCollapseProvider,
    ISelectionItemProvider,
    IRangeValueProvider,
    IValueProvider,
    IScrollProvider,
    IScrollItemProvider
)]
pub struct Node {
    hwnd: HWND,
    id: Id,
}

pub fn node(hwnd: HWND, id: Id) -> IRawElementProviderSimple {
    Node { hwnd, id }.into()
}

fn fragment(hwnd: HWND, id: Id) -> IRawElementProviderFragment {
    Node { hwnd, id }.into()
}

fn enabled(e: &Editor, id: Id) -> bool {
    match id {
        Id::Control(target) => e.enabled(target),
        // Drawn disabled: not in this host yet.
        Id::Stub(_) => false,
        _ => true,
    }
}

fn focusable(e: &Editor, id: Id) -> bool {
    matches!(id, Id::Control(t) if e.focusable().contains(&t))
}

impl IRawElementProviderSimple_Impl for Node_Impl {
    fn ProviderOptions(&self) -> Result<ProviderOptions> {
        Ok(ProviderOptions_ServerSideProvider | ProviderOptions_UseComThreading)
    }

    fn GetPatternProvider(&self, pattern: UIA_PATTERN_ID) -> Result<IUnknown> {
        let id = self.id;
        let supported = with(self.hwnd, |e| {
            Ok(match id {
                // `.screenshot-properties` is `overflow-y: auto`.
                Id::Properties => pattern == UIA_ScrollPatternId,
                _ if pattern == UIA_ScrollItemPatternId => scroll_item(id).is_some(),
                Id::Control(Target::Slider | Target::Divider | Target::Prop(Prop::Range(_))) => {
                    pattern == UIA_RangeValuePatternId || pattern == UIA_ValuePatternId
                }
                Id::Control(Target::Shape(_)) => {
                    pattern == UIA_SelectionItemPatternId || pattern == UIA_InvokePatternId
                }
                Id::Control(Target::Prop(Prop::StyleTrigger) | Target::Background) => {
                    pattern == UIA_ExpandCollapsePatternId || pattern == UIA_InvokePatternId
                }
                Id::Control(Target::Prop(Prop::Font | Prop::Aspect | Prop::Preset)) => {
                    pattern == UIA_ExpandCollapsePatternId || pattern == UIA_ValuePatternId
                }
                Id::Control(Target::Prop(Prop::Number(_) | Prop::Textarea)) => {
                    pattern == UIA_ValuePatternId
                }
                Id::MenuItem(_) => {
                    pattern == UIA_SelectionItemPatternId || pattern == UIA_InvokePatternId
                }
                Id::Control(Target::Tool(i)) if i == shapes() => {
                    pattern == UIA_TogglePatternId
                        || pattern == UIA_ExpandCollapsePatternId
                        || pattern == UIA_InvokePatternId
                }
                Id::Control(_) if toggle_state(e, id).is_some() => pattern == UIA_TogglePatternId,
                Id::Control(_) => pattern == UIA_InvokePatternId,
                Id::Filename | Id::Location | Id::InlineText | Id::Rename => {
                    pattern == UIA_ValuePatternId
                }
                Id::Stub(_) => pattern == UIA_InvokePatternId,
                _ => false,
            })
        })?;
        if supported {
            Ok(self.to_interface::<IUnknown>())
        } else {
            Err(Error::empty())
        }
    }

    fn GetPropertyValue(&self, property: UIA_PROPERTY_ID) -> Result<VARIANT> {
        let id = self.id;
        with(self.hwnd, |e| {
            let enabled = enabled(e, id);
            // The popup's active row is focused while it is open (Preferences
            // selects do the same).
            let active = e.open_menu_state().and_then(|(_, active)| active);
            let focused = match id {
                Id::MenuItem(i) => active == Some(i),
                Id::Control(t) => e.focus == Some(t) && active.is_none(),
                Id::InlineText => e.text_active(),
                Id::Rename => e.renaming().is_some(),
                _ => false,
            };
            let region = matches!(id, Id::Canvas | Id::Layers | Id::Properties);
            Ok(match property {
                UIA_NamePropertyId => VARIANT::from(BSTR::from(name(e, id))),
                UIA_ControlTypePropertyId => VARIANT::from(control_type(e, id).0),
                UIA_HelpTextPropertyId => match id {
                    Id::Control(target) => VARIANT::from(BSTR::from(e.help(target))),
                    Id::Location => VARIANT::from(BSTR::from(e.save_location())),
                    _ => VARIANT::default(),
                },
                UIA_LocalizedControlTypePropertyId if region => VARIANT::from(BSTR::from("region")),
                UIA_LocalizedControlTypePropertyId if id == Id::Tools => {
                    VARIANT::from(BSTR::from("navigation"))
                }
                UIA_LandmarkTypePropertyId if id == Id::Tools => {
                    VARIANT::from(UIA_NavigationLandmarkTypeId.0)
                }
                UIA_LandmarkTypePropertyId if region => VARIANT::from(UIA_CustomLandmarkTypeId.0),
                UIA_LocalizedLandmarkTypePropertyId if region => {
                    VARIANT::from(BSTR::from("region"))
                }
                UIA_OrientationPropertyId if id == Id::Control(Target::Divider) => {
                    VARIANT::from(OrientationType_Vertical.0)
                }
                UIA_LiveSettingPropertyId => match id {
                    Id::Banner => VARIANT::from(Polite.0),
                    Id::Notice if notice_error(e).is_some() => VARIANT::from(Assertive.0),
                    Id::Notice => VARIANT::from(Polite.0),
                    _ => VARIANT::default(),
                },
                UIA_AutomationIdPropertyId => VARIANT::from(BSTR::from(format!("editor-{id:?}"))),
                UIA_FrameworkIdPropertyId => VARIANT::from(BSTR::from("Captures")),
                UIA_IsKeyboardFocusablePropertyId => {
                    VARIANT::from(focusable(e, id) || matches!(id, Id::InlineText | Id::Rename))
                }
                UIA_HasKeyboardFocusPropertyId => VARIANT::from(focused),
                UIA_IsEnabledPropertyId => VARIANT::from(enabled),
                UIA_IsOffscreenPropertyId => VARIANT::from(offscreen(e, id)),
                UIA_IsControlElementPropertyId | UIA_IsContentElementPropertyId => {
                    VARIANT::from(true)
                }
                UIA_ToggleToggleStatePropertyId => {
                    toggle_state(e, id).map_or_else(VARIANT::default, |s| VARIANT::from(s.0))
                }
                UIA_ValueValuePropertyId => {
                    value(e, id).map_or_else(VARIANT::default, |v| VARIANT::from(BSTR::from(v)))
                }
                UIA_ValueIsReadOnlyPropertyId if writable(id) => VARIANT::from(false),
                UIA_ValueIsReadOnlyPropertyId => VARIANT::from(!matches!(
                    id,
                    Id::Control(Target::Slider | Target::Divider | Target::Prop(Prop::Range(_)))
                )),
                UIA_RangeValueValuePropertyId => {
                    range(e, id).map_or_else(VARIANT::default, |r| VARIANT::from(r.0))
                }
                UIA_SelectionItemIsSelectedPropertyId => match id {
                    Id::Control(target @ Target::Shape(_)) => {
                        VARIANT::from(e.pressed(target).unwrap_or(false))
                    }
                    Id::MenuItem(i) => VARIANT::from(item_selected(e, i)),
                    _ => VARIANT::default(),
                },
                UIA_ExpandCollapseExpandCollapseStatePropertyId => match id {
                    Id::Control(Target::Tool(i)) if i == shapes() => {
                        VARIANT::from(expand_state(e, id).0)
                    }
                    _ if trigger_kind(id).is_some() || id == Id::Control(Target::Background) => {
                        VARIANT::from(expand_state(e, id).0)
                    }
                    _ => VARIANT::default(),
                },
                _ => VARIANT::default(),
            })
        })
    }

    fn HostRawElementProvider(&self) -> Result<IRawElementProviderSimple> {
        if self.id == Id::Root {
            unsafe { UiaHostProviderFromHwnd(self.hwnd) }
        } else {
            Err(Error::empty())
        }
    }
}

fn expand_state(e: &Editor, id: Id) -> ExpandCollapseState {
    let open = match trigger_kind(id) {
        Some(kind) => e.menu_open(kind),
        None if id == Id::Control(Target::Background) => e.bg_open(),
        None => e.flyout,
    };
    if open {
        ExpandCollapseState_Expanded
    } else {
        ExpandCollapseState_Collapsed
    }
}

/// Inputs that take `Value.SetValue`.
fn writable(id: Id) -> bool {
    match id {
        // The crop's Width and Height are `readOnly`.
        Id::Control(Target::Prop(Prop::Number(num))) => !num.read_only(),
        Id::InlineText | Id::Rename | Id::Control(Target::Prop(Prop::Textarea)) => true,
        _ => false,
    }
}

fn item_selected(e: &Editor, index: usize) -> bool {
    menu_kind(e).is_some_and(|kind| e.menu_displayed(kind) == Some(index))
}

impl IRawElementProviderFragment_Impl for Node_Impl {
    fn Navigate(&self, direction: NavigateDirection) -> Result<IRawElementProviderFragment> {
        let id = self.id;
        let next = with(self.hwnd, |e| {
            let exists = |id: Id| parent(e, id).is_none_or(|p| children(e, p).contains(&id));
            if !exists(id) {
                return Err(unavailable());
            }
            Ok(match direction {
                NavigateDirection_Parent => parent(e, id),
                NavigateDirection_FirstChild => children(e, id).first().copied(),
                NavigateDirection_LastChild => children(e, id).last().copied(),
                NavigateDirection_NextSibling | NavigateDirection_PreviousSibling => parent(e, id)
                    .and_then(|p| {
                        let siblings = children(e, p);
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
        next.map(|id| fragment(self.hwnd, id))
            .ok_or_else(Error::empty)
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
        with(self.hwnd, |e| Ok(screen_rect(e, rect(e, id))))
    }

    fn GetEmbeddedFragmentRoots(&self) -> Result<*mut SAFEARRAY> {
        Ok(std::ptr::null_mut())
    }

    fn SetFocus(&self) -> Result<()> {
        let id = self.id;
        with(self.hwnd, |e| {
            if let Id::Control(target) = id
                && focusable(e, id)
            {
                e.set_focus(Some(target), true);
            }
            Ok(())
        })
    }

    fn FragmentRoot(&self) -> Result<IRawElementProviderFragmentRoot> {
        Ok(Node {
            hwnd: self.hwnd,
            id: Id::Root,
        }
        .into())
    }
}

impl IRawElementProviderFragmentRoot_Impl for Node_Impl {
    fn ElementProviderFromPoint(&self, x: f64, y: f64) -> Result<IRawElementProviderFragment> {
        let hit = with(self.hwnd, |e| {
            let mut origin = POINT::default();
            unsafe {
                let _ = ClientToScreen(e.hwnd, &mut origin);
            }
            let s = f64::from(e.scale);
            let (px, py) = (
                ((x - f64::from(origin.x)) / s) as f32,
                ((y - f64::from(origin.y)) / s) as f32,
            );
            if let Some(target) = e.hit(px, py) {
                return Ok(Id::Control(target));
            }
            // The deepest element under the point, groups last.
            let mut at = Id::Root;
            loop {
                let inner = children(e, at)
                    .into_iter()
                    .find(|&child| rect(e, child).contains(px, py) && !offscreen(e, child));
                match inner {
                    Some(child) => at = child,
                    None => return Ok(at),
                }
            }
        })?;
        Ok(fragment(self.hwnd, hit))
    }

    fn GetFocus(&self) -> Result<IRawElementProviderFragment> {
        if with(self.hwnd, |e| Ok(e.renaming().is_some()))? {
            return Ok(fragment(self.hwnd, Id::Rename));
        }
        if with(self.hwnd, |e| Ok(e.text_active()))? {
            return Ok(fragment(self.hwnd, Id::InlineText));
        }
        if let Some(active) = with(self.hwnd, |e| {
            Ok(e.open_menu_state().and_then(|(_, active)| active))
        })? {
            return Ok(fragment(self.hwnd, Id::MenuItem(active)));
        }
        let focus = with(self.hwnd, |e| Ok(e.focus))?;
        focus
            .map(|target| fragment(self.hwnd, Id::Control(target)))
            .ok_or_else(Error::empty)
    }
}

impl Node_Impl {
    fn activate(&self) -> Result<()> {
        let id = self.id;
        with(self.hwnd, |e| match id {
            Id::Control(target) if e.exists(target) && e.enabled(target) => {
                e.blur_field_for(target);
                e.activate(target);
                Ok(())
            }
            Id::MenuItem(i) if menu_kind(e).is_some() => {
                e.choose_menu(i);
                Ok(())
            }
            _ => Err(not_enabled()),
        })
    }
}

impl IInvokeProvider_Impl for Node_Impl {
    fn Invoke(&self) -> Result<()> {
        self.activate()
    }
}

impl IToggleProvider_Impl for Node_Impl {
    fn Toggle(&self) -> Result<()> {
        self.activate()
    }

    fn ToggleState(&self) -> Result<ToggleState> {
        let id = self.id;
        with(self.hwnd, |e| toggle_state(e, id).ok_or_else(Error::empty))
    }
}

impl Node_Impl {
    fn expand(&self, open: bool) -> Result<()> {
        let id = self.id;
        with(self.hwnd, |e| {
            match trigger_kind(id) {
                Some(kind) if e.exists(Target::Prop(kind.trigger())) => e.set_menu(kind, open),
                Some(_) => return Err(not_enabled()),
                None if id == Id::Control(Target::Background) => {
                    if !e.enabled(Target::Background) {
                        return Err(not_enabled());
                    }
                    e.set_bg_menu(open);
                }
                None => e.set_flyout(open),
            }
            Ok(())
        })
    }
}

impl IExpandCollapseProvider_Impl for Node_Impl {
    fn Expand(&self) -> Result<()> {
        self.expand(true)
    }

    fn Collapse(&self) -> Result<()> {
        self.expand(false)
    }

    fn ExpandCollapseState(&self) -> Result<ExpandCollapseState> {
        let id = self.id;
        with(self.hwnd, |e| Ok(expand_state(e, id)))
    }
}

impl ISelectionItemProvider_Impl for Node_Impl {
    fn Select(&self) -> Result<()> {
        self.activate()
    }

    fn AddToSelection(&self) -> Result<()> {
        self.activate()
    }

    fn RemoveFromSelection(&self) -> Result<()> {
        Err(read_only())
    }

    fn IsSelected(&self) -> Result<BOOL> {
        let id = self.id;
        with(self.hwnd, |e| match id {
            Id::Control(target) => Ok(e.pressed(target).unwrap_or(false).into()),
            Id::MenuItem(i) => Ok(item_selected(e, i).into()),
            _ => Ok(false.into()),
        })
    }

    fn SelectionContainer(&self) -> Result<IRawElementProviderSimple> {
        Ok(node(
            self.hwnd,
            if matches!(self.id, Id::MenuItem(_)) {
                Id::Menu
            } else {
                Id::Flyout
            },
        ))
    }
}

impl IRangeValueProvider_Impl for Node_Impl {
    fn SetValue(&self, value: f64) -> Result<()> {
        let id = self.id;
        with(self.hwnd, |e| {
            match id {
                Id::Control(Target::Slider) if e.enabled(Target::Slider) => {
                    e.zoom_to(value.clamp(5., 800.), None)
                }
                Id::Control(Target::Divider) => e.resize_sidebar(Some(value as f32)),
                Id::Control(Target::Prop(Prop::Range(range)))
                    if e.exists(Target::Prop(Prop::Range(range))) =>
                {
                    e.set_range(range, value, false)
                }
                _ => return Err(not_enabled()),
            }
            Ok(())
        })
    }

    fn Value(&self) -> Result<f64> {
        let id = self.id;
        with(self.hwnd, |e| {
            range(e, id).map(|r| r.0).ok_or_else(Error::empty)
        })
    }

    fn IsReadOnly(&self) -> Result<BOOL> {
        Ok(false.into())
    }

    fn Maximum(&self) -> Result<f64> {
        let id = self.id;
        with(self.hwnd, |e| {
            range(e, id).map(|r| r.2).ok_or_else(Error::empty)
        })
    }

    fn Minimum(&self) -> Result<f64> {
        let id = self.id;
        with(self.hwnd, |e| {
            range(e, id).map(|r| r.1).ok_or_else(Error::empty)
        })
    }

    fn LargeChange(&self) -> Result<f64> {
        let id = self.id;
        with(self.hwnd, |e| {
            range(e, id).map(|r| r.4).ok_or_else(Error::empty)
        })
    }

    fn SmallChange(&self) -> Result<f64> {
        let id = self.id;
        with(self.hwnd, |e| {
            range(e, id).map(|r| r.3).ok_or_else(Error::empty)
        })
    }
}

impl IValueProvider_Impl for Node_Impl {
    fn SetValue(&self, value: &PCWSTR) -> Result<()> {
        // The inline label and the property inputs take text; the filename
        // field is read-only in this host and ranges take RangeValue.SetValue.
        if !writable(self.id) {
            return Err(read_only());
        }
        let value = unsafe { value.to_string() }.unwrap_or_default();
        if self.id == Id::Rename {
            return with(self.hwnd, |e| {
                if e.renaming().is_none() {
                    return Err(unavailable());
                }
                e.set_rename_value(&value);
                Ok(())
            });
        }
        if let Id::Control(Target::Prop(prop)) = self.id {
            let id = self.id;
            return with(self.hwnd, |e| {
                if !e.exists(Target::Prop(prop)) {
                    return Err(unavailable());
                }
                if !writable(id) || !e.enabled(Target::Prop(prop)) {
                    return Err(not_enabled());
                }
                e.set_field_value(prop, &value);
                Ok(())
            });
        }
        with(self.hwnd, |e| {
            if !e.text_active() {
                return Err(unavailable());
            }
            e.set_text_value(&value);
            Ok(())
        })
    }

    fn Value(&self) -> Result<BSTR> {
        let id = self.id;
        with(self.hwnd, |e| {
            Ok(BSTR::from(value(e, id).unwrap_or_default()))
        })
    }

    fn IsReadOnly(&self) -> Result<BOOL> {
        Ok((!writable(self.id)).into())
    }
}

/// Raise an editor window's accessibility events, outside its borrow;
/// `detail` is the new value or notice text.
pub fn raise(hwnd: HWND, events: &[(Event, String)]) {
    unsafe {
        if !UiaClientsAreListening().as_bool() {
            return;
        }
        for (event, detail) in events {
            match *event {
                Event::Focus(target) => {
                    let _ = UiaRaiseAutomationEvent(
                        &node(hwnd, Id::Control(target)),
                        UIA_AutomationFocusChangedEventId,
                    );
                }
                Event::Toggled(target @ (Target::Undo | Target::Redo)) => {
                    // Undo and Redo report availability, not a state.
                    let _ = target;
                    for t in [Target::Undo, Target::Redo] {
                        let _ = UiaRaiseAutomationPropertyChangedEvent(
                            &node(hwnd, Id::Control(t)),
                            UIA_IsEnabledPropertyId,
                            &VARIANT::default(),
                            &VARIANT::default(),
                        );
                    }
                }
                Event::Toggled(target @ Target::Shape(_)) => {
                    let _ = UiaRaiseAutomationEvent(
                        &node(hwnd, Id::Control(target)),
                        UIA_SelectionItem_ElementSelectedEventId,
                    );
                }
                Event::Toggled(target) => {
                    let on = detail == "on";
                    let state = |on: bool| VARIANT::from(toggle(on).0);
                    let _ = UiaRaiseAutomationPropertyChangedEvent(
                        &node(hwnd, Id::Control(target)),
                        UIA_ToggleToggleStatePropertyId,
                        &state(!on),
                        &state(on),
                    );
                }
                Event::Value(target) => {
                    let element = node(hwnd, Id::Control(target));
                    let _ = UiaRaiseAutomationPropertyChangedEvent(
                        &element,
                        UIA_RangeValueValuePropertyId,
                        &VARIANT::default(),
                        &VARIANT::default(),
                    );
                    let _ = UiaRaiseAutomationPropertyChangedEvent(
                        &element,
                        UIA_ValueValuePropertyId,
                        &VARIANT::default(),
                        &VARIANT::from(BSTR::from(detail.as_str())),
                    );
                }
                Event::Expanded(target) => {
                    let trigger = node(hwnd, Id::Control(target));
                    let open = detail == "open";
                    let state = |open: bool| {
                        VARIANT::from(
                            if open {
                                ExpandCollapseState_Expanded
                            } else {
                                ExpandCollapseState_Collapsed
                            }
                            .0,
                        )
                    };
                    let _ = UiaRaiseAutomationPropertyChangedEvent(
                        &trigger,
                        UIA_ExpandCollapseExpandCollapseStatePropertyId,
                        &state(!open),
                        &state(open),
                    );
                    // Focus returns to a menu's trigger; the Background card
                    // moves focus itself, only when it held it.
                    if !open && target != Target::Background {
                        // Focus returns to the trigger.
                        let _ =
                            UiaRaiseAutomationEvent(&trigger, UIA_AutomationFocusChangedEventId);
                    }
                }
                Event::MenuActive(index) => {
                    let _ = UiaRaiseAutomationEvent(
                        &node(hwnd, Id::MenuItem(index)),
                        UIA_AutomationFocusChangedEventId,
                    );
                }
                Event::Scrolled => {
                    let _ = UiaRaiseAutomationPropertyChangedEvent(
                        &node(hwnd, Id::Properties),
                        UIA_ScrollVerticalScrollPercentPropertyId,
                        &VARIANT::default(),
                        &VARIANT::from(detail.parse::<f64>().unwrap_or(0.)),
                    );
                }
                Event::Structure => {
                    let _ = UiaRaiseStructureChangedEvent(
                        &node(hwnd, Id::Root),
                        StructureChangeType_ChildrenInvalidated,
                        runtime_id(Id::Root).as_mut_ptr(),
                        4,
                    );
                }
                Event::RenameFocus => {
                    let _ = UiaRaiseAutomationEvent(
                        &node(hwnd, Id::Rename),
                        UIA_AutomationFocusChangedEventId,
                    );
                }
                Event::RenameText => {
                    let _ = UiaRaiseAutomationPropertyChangedEvent(
                        &node(hwnd, Id::Rename),
                        UIA_ValueValuePropertyId,
                        &VARIANT::default(),
                        &VARIANT::from(BSTR::from(detail.as_str())),
                    );
                }
                Event::InlineFocus => {
                    let _ = UiaRaiseAutomationEvent(
                        &node(hwnd, Id::InlineText),
                        UIA_AutomationFocusChangedEventId,
                    );
                }
                Event::InlineText => {
                    let _ = UiaRaiseAutomationPropertyChangedEvent(
                        &node(hwnd, Id::InlineText),
                        UIA_ValueValuePropertyId,
                        &VARIANT::default(),
                        &VARIANT::from(BSTR::from(detail.as_str())),
                    );
                }
                Event::Notice => {
                    if detail.is_empty() {
                        continue;
                    }
                    let notice = node(hwnd, Id::Notice);
                    let _ = UiaRaiseAutomationEvent(&notice, UIA_LiveRegionChangedEventId);
                    let _ = UiaRaiseNotificationEvent(
                        &notice,
                        NotificationKind_ActionCompleted,
                        NotificationProcessing_ImportantMostRecent,
                        &BSTR::from(detail.as_str()),
                        &BSTR::from("editor-notice"),
                    );
                }
            }
        }
    }
}

/// Events with the text they report, read while the window is borrowed.
pub fn describe(e: &Editor, events: Vec<Event>) -> Vec<(Event, String)> {
    events
        .into_iter()
        .map(|event| {
            let detail = match event {
                Event::Toggled(target) => {
                    let on = toggle_state(e, Id::Control(target)) == Some(ToggleState_On);
                    if on { "on" } else { "off" }.into()
                }
                Event::Value(target) => value(e, Id::Control(target)).unwrap_or_default(),
                Event::Notice => e.notice_text().map(|(text, _)| text).unwrap_or_default(),
                Event::InlineText => e.text_value(),
                Event::RenameText => e.rename_text(),
                Event::Scrolled => scroll_percent(e).to_string(),
                Event::Expanded(target) => {
                    let open = match target {
                        Target::Background => e.bg_open(),
                        _ => trigger_kind(Id::Control(target)).is_some_and(|k| e.menu_open(k)),
                    };
                    if open { "open" } else { "closed" }.into()
                }
                _ => String::new(),
            };
            (event, detail)
        })
        .collect()
}

impl IScrollProvider_Impl for Node_Impl {
    fn Scroll(&self, horizontal: ScrollAmount, vertical: ScrollAmount) -> Result<()> {
        if horizontal != ScrollAmount_NoAmount {
            return Err(HRESULT(UIA_E_INVALIDOPERATION as _).into());
        }
        with(self.hwnd, |e| {
            let page = (e.props_view().h - 40.).max(40.);
            let delta = match vertical {
                ScrollAmount_LargeDecrement => -page,
                ScrollAmount_LargeIncrement => page,
                ScrollAmount_SmallDecrement => -40.,
                ScrollAmount_SmallIncrement => 40.,
                _ => 0.,
            };
            e.scroll_by(delta);
            Ok(())
        })
    }

    fn SetScrollPercent(&self, horizontal: f64, vertical: f64) -> Result<()> {
        if horizontal != UIA_ScrollPatternNoScroll {
            return Err(HRESULT(UIA_E_INVALIDOPERATION as _).into());
        }
        with(self.hwnd, |e| {
            if vertical != UIA_ScrollPatternNoScroll {
                e.set_props_percent(vertical as f32);
            }
            Ok(())
        })
    }

    fn HorizontalScrollPercent(&self) -> Result<f64> {
        Ok(UIA_ScrollPatternNoScroll)
    }

    fn VerticalScrollPercent(&self) -> Result<f64> {
        with(self.hwnd, |e| Ok(scroll_percent(e)))
    }

    fn HorizontalViewSize(&self) -> Result<f64> {
        Ok(100.)
    }

    fn VerticalViewSize(&self) -> Result<f64> {
        with(self.hwnd, |e| {
            let (view, max) = (e.props_view().h, e.props_max());
            Ok(f64::from(if max <= 0. {
                100.
            } else {
                view / (view + max) * 100.
            }))
        })
    }

    fn HorizontallyScrollable(&self) -> Result<BOOL> {
        Ok(false.into())
    }

    fn VerticallyScrollable(&self) -> Result<BOOL> {
        with(self.hwnd, |e| Ok((e.props_max() > 0.).into()))
    }
}

impl IScrollItemProvider_Impl for Node_Impl {
    fn ScrollIntoView(&self) -> Result<()> {
        let id = self.id;
        with(self.hwnd, |e| {
            let prop = scroll_item(id).ok_or_else(not_enabled)?;
            e.reveal(prop);
            Ok(())
        })
    }
}

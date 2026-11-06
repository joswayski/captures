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

use crate::editor::{Editor, Event, Field, Prop, Stub, Target, with_editor};
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
    /// `section aria-label="Tool properties"`.
    Properties,
    /// `.screenshot-shape-picker` (`role="group" aria-label="Shape"`).
    ShapePicker,
    /// A `ColorField` fieldset, named by its legend.
    ColorField(Field),
    /// `output aria-label="Save location"`.
    Location,
    /// `input aria-label="Saved filename"`.
    Filename,
    /// `.screenshot-export-notice` and `.screenshot-export-hint`.
    Notice,
    Hint,
    Control(Target),
    Stub(Stub),
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
            list
        }
        Id::Banner => vec![Id::BannerText, c(Target::Dismiss)],
        Id::CanvasGroup => [Stub::Width, Stub::Height, Stub::Trim, Stub::Background]
            .map(Id::Stub)
            .to_vec(),
        Id::Zoom => vec![
            c(Target::Fit),
            c(Target::ZoomOut),
            c(Target::Slider),
            c(Target::ZoomIn),
            Id::Stub(Stub::Preset),
        ],
        Id::Tools => (0..RAIL_TOOLS.len()).map(|i| c(Target::Tool(i))).collect(),
        Id::Flyout => (0..SHAPE_TOOLS.len())
            .map(|i| c(Target::Shape(i)))
            .collect(),
        Id::Layers => vec![Id::LayersTitle, Id::Stub(Stub::AddLayer), Id::List],
        Id::List => (0..e.layer_count()).map(Id::Row).collect(),
        Id::Row(i) if i < e.layer_count() => vec![
            c(Target::Layer(i)),
            c(Target::Eye(i)),
            c(Target::Lock(i)),
            Id::Stub(Stub::LayerMenu(i)),
        ],
        Id::Properties => {
            let mut list = Vec::new();
            for prop in e.prop_order() {
                let id = match prop {
                    Prop::Shape(_) => Id::ShapePicker,
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
        Id::ShapePicker | Id::ColorField(_) => e
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
        Prop::Shape(_) => Id::ShapePicker,
        Prop::Swatch(field, _) => Id::ColorField(field),
        _ => Id::Properties,
    }
}

fn parent(id: Id) -> Option<Id> {
    Some(match id {
        Id::Root => return None,
        Id::BannerText | Id::Control(Target::Dismiss) => Id::Banner,
        Id::Stub(Stub::Width | Stub::Height | Stub::Trim | Stub::Background) => Id::CanvasGroup,
        Id::Control(Target::Fit | Target::ZoomOut | Target::Slider | Target::ZoomIn)
        | Id::Stub(Stub::Preset) => Id::Zoom,
        Id::Control(Target::Tool(_)) => Id::Tools,
        Id::Control(Target::Shape(_)) => Id::Flyout,
        Id::LayersTitle | Id::Stub(Stub::AddLayer) | Id::List => Id::Layers,
        Id::Row(_) => Id::List,
        Id::ShapePicker | Id::ColorField(_) => Id::Properties,
        Id::InlineText => Id::Canvas,
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
        Id::ShapePicker => e.picker_rect(),
        Id::ColorField(field) => e.color_field_rect(field),
        Id::Location => l.folder,
        Id::Filename => l.filename,
        Id::Notice | Id::Hint => l.hint,
        Id::Control(target) => e.target_rect(target),
        Id::Stub(stub) => e.stub_rect(stub),
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
        Id::ShapePicker => "Shape".into(),
        Id::ColorField(field) => e.field_label(field),
        Id::Location => "Save location".into(),
        Id::Filename => "Saved filename".into(),
        Id::Notice => e.notice_text().map(|(text, _)| text).unwrap_or_default(),
        Id::Hint => e.hint_text(),
        Id::Control(target) => e.name(target),
        Id::Stub(stub) => e.stub_name(stub),
        Id::Banner | Id::List => String::new(),
    }
}

fn control_type(id: Id) -> UIA_CONTROLTYPE_ID {
    match id {
        Id::Root => UIA_WindowControlTypeId,
        Id::Banner
        | Id::CanvasGroup
        | Id::Zoom
        | Id::Tools
        | Id::Canvas
        | Id::Layers
        | Id::Properties
        | Id::ShapePicker
        | Id::ColorField(_) => UIA_GroupControlTypeId,
        Id::Flyout => UIA_MenuControlTypeId,
        Id::List => UIA_ListControlTypeId,
        Id::Row(_) => UIA_ListItemControlTypeId,
        Id::Filename | Id::InlineText | Id::Stub(Stub::Width | Stub::Height) => {
            UIA_EditControlTypeId
        }
        Id::Stub(Stub::Preset | Stub::Format) => UIA_ComboBoxControlTypeId,
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
            Target::Prop(Prop::Shape(i)) => (16, i as i32),
            Target::Prop(Prop::Check(check)) => (17, check as i32),
            Target::Prop(Prop::Swatch(field, i)) => (18, field as i32 * 16 + i as i32),
            Target::Prop(Prop::Range(range)) => (19, range as i32),
        }
    };
    let stub = |s: Stub| -> (i32, i32) {
        match s {
            Stub::Width => (0, 0),
            Stub::Height => (1, 0),
            Stub::Trim => (2, 0),
            Stub::Background => (3, 0),
            Stub::Preset => (4, 0),
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
        | Id::Control(Target::Layer(_) | Target::Eye(_) | Target::Lock(_))
        | Id::Stub(Stub::LayerMenu(_)) => {
            let (r, list) = (rect(e, id), e.l.list);
            r.y + r.h <= list.y || r.y >= list.y + list.h
        }
        Id::ShapePicker | Id::ColorField(_) | Id::Control(Target::Prop(_)) => {
            let (r, view) = (rect(e, id), e.props_view());
            r.y + r.h <= view.y || r.y >= view.y + view.h
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
        Id::Location => e.save_location(),
        Id::Stub(stub) => e.stub_value(stub)?,
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
    IValueProvider
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
                Id::Control(Target::Slider | Target::Divider | Target::Prop(Prop::Range(_))) => {
                    pattern == UIA_RangeValuePatternId || pattern == UIA_ValuePatternId
                }
                Id::Control(Target::Shape(_)) => {
                    pattern == UIA_SelectionItemPatternId || pattern == UIA_InvokePatternId
                }
                Id::Control(Target::Tool(i)) if i == shapes() => {
                    pattern == UIA_TogglePatternId
                        || pattern == UIA_ExpandCollapsePatternId
                        || pattern == UIA_InvokePatternId
                }
                Id::Control(_) if toggle_state(e, id).is_some() => pattern == UIA_TogglePatternId,
                Id::Control(_) => pattern == UIA_InvokePatternId,
                Id::Filename | Id::Location | Id::InlineText => pattern == UIA_ValuePatternId,
                Id::Stub(stub) if e.stub_value(stub).is_some() => pattern == UIA_ValuePatternId,
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
            let focused = matches!(id, Id::Control(t) if e.focus == Some(t))
                || (id == Id::InlineText && e.text_active());
            let region = matches!(id, Id::Canvas | Id::Layers | Id::Properties);
            Ok(match property {
                UIA_NamePropertyId => VARIANT::from(BSTR::from(name(e, id))),
                UIA_ControlTypePropertyId => VARIANT::from(control_type(id).0),
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
                    VARIANT::from(focusable(e, id) || id == Id::InlineText)
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
                UIA_ValueIsReadOnlyPropertyId if id == Id::InlineText => VARIANT::from(false),
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
                    _ => VARIANT::default(),
                },
                UIA_ExpandCollapseExpandCollapseStatePropertyId => match id {
                    Id::Control(Target::Tool(i)) if i == shapes() => {
                        VARIANT::from(expand_state(e).0)
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

fn expand_state(e: &Editor) -> ExpandCollapseState {
    if e.flyout {
        ExpandCollapseState_Expanded
    } else {
        ExpandCollapseState_Collapsed
    }
}

impl IRawElementProviderFragment_Impl for Node_Impl {
    fn Navigate(&self, direction: NavigateDirection) -> Result<IRawElementProviderFragment> {
        let id = self.id;
        let next = with(self.hwnd, |e| {
            let exists = |id: Id| parent(id).is_none_or(|p| children(e, p).contains(&id));
            if !exists(id) {
                return Err(unavailable());
            }
            Ok(match direction {
                NavigateDirection_Parent => parent(id),
                NavigateDirection_FirstChild => children(e, id).first().copied(),
                NavigateDirection_LastChild => children(e, id).last().copied(),
                NavigateDirection_NextSibling | NavigateDirection_PreviousSibling => parent(id)
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
        if with(self.hwnd, |e| Ok(e.text_active()))? {
            return Ok(fragment(self.hwnd, Id::InlineText));
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
                e.activate(target);
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

impl IExpandCollapseProvider_Impl for Node_Impl {
    fn Expand(&self) -> Result<()> {
        with(self.hwnd, |e| {
            e.set_flyout(true);
            Ok(())
        })
    }

    fn Collapse(&self) -> Result<()> {
        with(self.hwnd, |e| {
            e.set_flyout(false);
            Ok(())
        })
    }

    fn ExpandCollapseState(&self) -> Result<ExpandCollapseState> {
        with(self.hwnd, |e| Ok(expand_state(e)))
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
            _ => Ok(false.into()),
        })
    }

    fn SelectionContainer(&self) -> Result<IRawElementProviderSimple> {
        Ok(node(self.hwnd, Id::Flyout))
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
        // Only the inline label takes text; the filename field is read-only
        // in this host and ranges take RangeValue.SetValue.
        if self.id != Id::InlineText {
            return Err(read_only());
        }
        let value = unsafe { value.to_string() }.unwrap_or_default();
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
        Ok((self.id != Id::InlineText).into())
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
                Event::Structure => {
                    let _ = UiaRaiseStructureChangedEvent(
                        &node(hwnd, Id::Root),
                        StructureChangeType_ChildrenInvalidated,
                        runtime_id(Id::Root).as_mut_ptr(),
                        4,
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
                _ => String::new(),
            };
            (event, detail)
        })
        .collect()
}

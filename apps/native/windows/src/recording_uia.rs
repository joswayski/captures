#![allow(non_upper_case_globals)]
//! Recording shell UIA: the heading and warning, footer Edit/Value,
//! Copy/Toggle, save actions/Invoke and live export/error status.
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Com::*;
use windows::Win32::System::Ole::*;
use windows::Win32::System::Variant::*;
use windows::Win32::UI::Accessibility::*;
use windows::core::*;

use crate::recording::{Event, Recording, TARGETS, Target, with_recording};
use crate::scene::Rect;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Id {
    Root,
    Title,
    Warning,
    Location,
    Status,
    MaximumEstimate,
    Control(Target),
    Option(Target, usize),
}

fn with<R>(hwnd: HWND, f: impl FnOnce(&mut Recording) -> Result<R>) -> Result<R> {
    with_recording(hwnd, f).unwrap_or_else(|| Err(HRESULT(UIA_E_ELEMENTNOTAVAILABLE as _).into()))
}

fn range(w: &Recording, target: Target) -> Option<(f64, f64, f64)> {
    w.crop_range(target)
        .or_else(|| w.compare_range(target))
        .or_else(|| w.volume_range(target))
        .or_else(|| {
            w.trim_range(target)
                .map(|(v, min, max)| (v as f64, min as f64, max as f64))
        })
}

fn children(w: &Recording) -> Vec<Id> {
    let mut ids = vec![Id::Title];
    if w.warning().is_some() {
        ids.push(Id::Warning);
    }
    ids.push(Id::Location);
    ids.extend(TARGETS.into_iter().filter(|&t| w.shown(t)).map(Id::Control));
    if w.shown(Target::MaximumSize) {
        ids.push(Id::MaximumEstimate);
    }
    ids.push(Id::Status);
    ids
}

fn name(w: &Recording, id: Id) -> String {
    match id {
        Id::Root => captures_app::app_windows::RECORDING_EDITOR_TITLE.into(),
        Id::Title => w.title().into(),
        Id::Warning => w.warning().unwrap_or_default(),
        Id::Location => "Save location".into(),
        Id::Status => w.status.clone(),
        Id::MaximumEstimate => format!("Est. size: {}", w.maximum_estimate()),
        Id::Control(t) => w.target_label(t).into(),
        Id::Option(t, i) => w
            .output_choices(t)
            .get(i)
            .map_or(String::new(), |c| c.label.clone()),
    }
}

fn value(w: &Recording, id: Id) -> String {
    match id {
        Id::Control(Target::Filename) => w.text(),
        Id::Control(t) if t.is_output() => w.output_value(t),
        Id::Location => w.directory.to_string_lossy().into_owned(),
        _ => String::new(),
    }
}

fn rect(w: &Recording, id: Id) -> Rect {
    match id {
        Id::Control(t) => w.visible_target_rect(t),
        Id::Option(t, i) => {
            if w.menu_target() == Some(t) {
                w.output_menu_row(i)
            } else {
                Rect::default()
            }
        }
        Id::Root => Rect::new(0., 0., w.width, w.height),
        Id::Title => w.page_visible_rect(w.title_rect()),
        Id::Warning => w.page_visible_rect(w.warning_rect()),
        Id::MaximumEstimate => w.page_visible_rect(w.maximum_estimate_rect()),
        Id::Location => {
            let r = w.target_rect(Target::Filename);
            Rect::new(r.x, r.y - 20., r.w, 16.)
        }
        Id::Status => {
            let r = w.target_rect(Target::Save);
            Rect::new(r.x - 240., r.y - 19., 340., 16.)
        }
    }
}

fn screen_rect(w: &Recording, r: Rect) -> UiaRect {
    let mut origin = POINT::default();
    unsafe {
        let _ = ClientToScreen(w.hwnd, &mut origin);
    }
    let s = f64::from(w.scale);
    UiaRect {
        left: f64::from(origin.x) + f64::from(r.x) * s,
        top: f64::from(origin.y) + f64::from(r.y) * s,
        width: f64::from(r.w) * s,
        height: f64::from(r.h) * s,
    }
}

fn runtime(id: Id) -> [i32; 3] {
    let target_id = |t| {
        TARGETS
            .iter()
            .position(|&target| target == t)
            .expect("target") as i32
    };
    let kind = match id {
        Id::Root => 0,
        Id::Title => 1,
        Id::Warning => 2,
        Id::Location => 3,
        Id::Status => 4,
        Id::MaximumEstimate => 5,
        Id::Control(t) => 10 + target_id(t),
        Id::Option(t, i) => 1000 + target_id(t) * 100 + i as i32,
    };
    [UiaAppendRuntimeId as i32, 400, kind]
}

#[implement(
    IRawElementProviderSimple,
    IRawElementProviderFragment,
    IRawElementProviderFragmentRoot,
    IInvokeProvider,
    IToggleProvider,
    IValueProvider,
    IRangeValueProvider,
    IExpandCollapseProvider,
    ISelectionItemProvider
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

impl IRawElementProviderSimple_Impl for Node_Impl {
    fn ProviderOptions(&self) -> Result<ProviderOptions> {
        Ok(ProviderOptions_ServerSideProvider | ProviderOptions_UseComThreading)
    }
    fn GetPatternProvider(&self, pattern: UIA_PATTERN_ID) -> Result<IUnknown> {
        let supported = match self.id {
            Id::Control(t) if t.is_select() => {
                pattern == UIA_ExpandCollapsePatternId || pattern == UIA_ValuePatternId
            }
            Id::Control(t) if t.is_number() => pattern == UIA_ValuePatternId,
            Id::Control(t) if t.is_volume() => pattern == UIA_RangeValuePatternId,
            Id::Control(t) if t.is_check() => pattern == UIA_TogglePatternId,
            Id::Option(_, _) => {
                pattern == UIA_SelectionItemPatternId || pattern == UIA_InvokePatternId
            }
            Id::Control(
                Target::TrimStart | Target::TrimEnd | Target::CropHandle(_) | Target::CompareSplit,
            ) => pattern == UIA_RangeValuePatternId,
            Id::Control(Target::Filename) | Id::Location => pattern == UIA_ValuePatternId,
            Id::Control(Target::Copy | Target::Loop | Target::Fit | Target::Actual) => {
                pattern == UIA_TogglePatternId
            }
            Id::Control(_) => pattern == UIA_InvokePatternId,
            _ => false,
        };
        if supported {
            Ok(self.to_interface::<IUnknown>())
        } else {
            Err(Error::empty())
        }
    }
    fn GetPropertyValue(&self, property: UIA_PROPERTY_ID) -> Result<VARIANT> {
        with(self.hwnd, |w| {
            let enabled = match self.id {
                Id::Control(t) => w.enabled(t),
                Id::Option(t, _) => w.menu_target() == Some(t) && w.enabled(t),
                _ => true,
            };
            Ok(match property {
                UIA_NamePropertyId => VARIANT::from(BSTR::from(name(w, self.id))),
                UIA_ControlTypePropertyId => VARIANT::from(
                    match self.id {
                        Id::Root => UIA_WindowControlTypeId,
                        Id::Control(t) if t.is_select() => UIA_ComboBoxControlTypeId,
                        Id::Control(t) if t.is_number() => UIA_EditControlTypeId,
                        Id::Control(t) if t.is_volume() => UIA_SliderControlTypeId,
                        Id::Control(t) if t.is_check() => UIA_CheckBoxControlTypeId,
                        Id::Option(_, _) => UIA_ListItemControlTypeId,
                        Id::Control(Target::Filename) => UIA_EditControlTypeId,
                        Id::Control(Target::Copy) => UIA_CheckBoxControlTypeId,
                        Id::Control(
                            Target::TrimStart
                            | Target::TrimEnd
                            | Target::CropHandle(_)
                            | Target::CompareSplit,
                        ) => UIA_SliderControlTypeId,
                        Id::Control(_) => UIA_ButtonControlTypeId,
                        _ => UIA_TextControlTypeId,
                    }
                    .0,
                ),
                UIA_AutomationIdPropertyId => {
                    VARIANT::from(BSTR::from(format!("recording-{:?}", self.id)))
                }
                UIA_FrameworkIdPropertyId => VARIANT::from(BSTR::from("Captures")),
                UIA_IsEnabledPropertyId => VARIANT::from(enabled),
                UIA_IsKeyboardFocusablePropertyId => {
                    VARIANT::from(enabled && matches!(self.id, Id::Control(_)))
                }
                UIA_HasKeyboardFocusPropertyId => {
                    VARIANT::from(matches!(self.id,Id::Control(t) if w.focus==Some(t)))
                }
                UIA_IsControlElementPropertyId | UIA_IsContentElementPropertyId => {
                    VARIANT::from(true)
                }
                UIA_IsOffscreenPropertyId => {
                    let r = rect(w, self.id);
                    VARIANT::from(
                        r.w <= 0. || r.h <= 0. || matches!(self.id,Id::Control(t) if !w.shown(t)),
                    )
                }
                UIA_HeadingLevelPropertyId if self.id == Id::Title => {
                    VARIANT::from(HeadingLevel1.0)
                }
                UIA_LiveSettingPropertyId if self.id == Id::Status => {
                    VARIANT::from(if w.error { Assertive.0 } else { Polite.0 })
                }
                UIA_LiveSettingPropertyId if self.id == Id::MaximumEstimate => {
                    VARIANT::from(Polite.0)
                }
                UIA_ValueValuePropertyId => VARIANT::from(BSTR::from(value(w, self.id))),
                UIA_HelpTextPropertyId => {
                    let description = match self.id {
                        Id::Option(t, i) => {
                            w.output_choices(t).get(i).map(|c| c.description.clone())
                        }
                        Id::MaximumEstimate => {
                            Some(captures_app::recording_editor_ui::ESTIMATE_HELP.into())
                        }
                        _ => None,
                    }
                    .unwrap_or_default();
                    VARIANT::from(BSTR::from(description))
                }
                UIA_ExpandCollapseExpandCollapseStatePropertyId => VARIANT::from(
                    if matches!(self.id, Id::Control(t) if w.menu_target() == Some(t)) {
                        1i32
                    } else {
                        0i32
                    },
                ),
                UIA_SelectionItemIsSelectedPropertyId => {
                    VARIANT::from(matches!(self.id, Id::Option(t,i) if w.output_selected(t) == i))
                }
                UIA_RangeValueValuePropertyId
                | UIA_RangeValueMinimumPropertyId
                | UIA_RangeValueMaximumPropertyId => {
                    if let Id::Control(target) = self.id
                        && let Some((value, min, max)) = range(w, target)
                    {
                        VARIANT::from(if property == UIA_RangeValueValuePropertyId {
                            value
                        } else if property == UIA_RangeValueMinimumPropertyId {
                            min
                        } else {
                            max
                        })
                    } else {
                        VARIANT::default()
                    }
                }
                UIA_RangeValueIsReadOnlyPropertyId => VARIANT::from(!enabled),
                UIA_ValueIsReadOnlyPropertyId => VARIANT::from(
                    !matches!(self.id, Id::Control(t) if t == Target::Filename || t.is_number())
                        || !enabled,
                ),
                UIA_ToggleToggleStatePropertyId => {
                    if let Id::Control(t) = self.id
                        && let Some(on) = w.toggled(t)
                    {
                        VARIANT::from(i32::from(on))
                    } else {
                        VARIANT::default()
                    }
                }
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

impl IRawElementProviderFragment_Impl for Node_Impl {
    fn Navigate(&self, direction: NavigateDirection) -> Result<IRawElementProviderFragment> {
        let next = with(self.hwnd, |w| {
            let list = if let Id::Option(t, _) = self.id {
                (0..w.output_choices(t).len())
                    .map(|i| Id::Option(t, i))
                    .collect()
            } else {
                children(w)
            };
            Ok(match direction {
                NavigateDirection_Parent if matches!(self.id, Id::Option(_, _)) => {
                    if let Id::Option(t, _) = self.id {
                        Some(Id::Control(t))
                    } else {
                        None
                    }
                }
                NavigateDirection_Parent if self.id != Id::Root => Some(Id::Root),
                NavigateDirection_FirstChild | NavigateDirection_LastChild if matches!(self.id,Id::Control(t) if w.menu_target() == Some(t)) => {
                    if let Id::Control(t) = self.id {
                        Some(Id::Option(
                            t,
                            if direction == NavigateDirection_FirstChild {
                                0
                            } else {
                                w.output_choices(t).len() - 1
                            },
                        ))
                    } else {
                        None
                    }
                }
                NavigateDirection_FirstChild if self.id == Id::Root => list.first().copied(),
                NavigateDirection_LastChild if self.id == Id::Root => list.last().copied(),
                NavigateDirection_NextSibling | NavigateDirection_PreviousSibling => {
                    list.iter().position(|&id| id == self.id).and_then(|i| {
                        if direction == NavigateDirection_NextSibling {
                            list.get(i + 1).copied()
                        } else {
                            i.checked_sub(1).and_then(|i| list.get(i).copied())
                        }
                    })
                }
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
        unsafe {
            let values = runtime(self.id);
            let array = SafeArrayCreateVector(VT_I4, 0, values.len() as u32);
            for (i, value) in values.iter().enumerate() {
                let _ = SafeArrayPutElement(array, &(i as i32), value as *const i32 as _);
            }
            Ok(array)
        }
    }
    fn BoundingRectangle(&self) -> Result<UiaRect> {
        with(self.hwnd, |w| Ok(screen_rect(w, rect(w, self.id))))
    }
    fn GetEmbeddedFragmentRoots(&self) -> Result<*mut SAFEARRAY> {
        Ok(std::ptr::null_mut())
    }
    fn SetFocus(&self) -> Result<()> {
        with(self.hwnd, |w| {
            if let Id::Control(t) = self.id
                && w.enabled(t)
            {
                w.set_focus(Some(t), true);
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
        let id = with(self.hwnd, |w| {
            let mut origin = POINT::default();
            unsafe {
                let _ = ClientToScreen(w.hwnd, &mut origin);
            }
            let (x, y) = (
                ((x - f64::from(origin.x)) / f64::from(w.scale)) as f32,
                ((y - f64::from(origin.y)) / f64::from(w.scale)) as f32,
            );
            Ok(w.output_menu_hit(x, y)
                .and_then(|i| w.menu_target().map(|t| Id::Option(t, i)))
                .or_else(|| {
                    w.hit(x, y).map(Id::Control).or_else(|| {
                        children(w)
                            .into_iter()
                            .find(|&id| rect(w, id).contains(x, y))
                    })
                })
                .unwrap_or(Id::Root))
        })?;
        Ok(fragment(self.hwnd, id))
    }
    fn GetFocus(&self) -> Result<IRawElementProviderFragment> {
        with(self.hwnd, |w| Ok(w.focus))?
            .map(|t| fragment(self.hwnd, Id::Control(t)))
            .ok_or_else(Error::empty)
    }
}

impl IInvokeProvider_Impl for Node_Impl {
    fn Invoke(&self) -> Result<()> {
        with(self.hwnd, |w| {
            if let Id::Option(t, i) = self.id {
                if w.menu_target() != Some(t) || !w.enabled(t) {
                    return Err(HRESULT(UIA_E_ELEMENTNOTAVAILABLE as _).into());
                }
                w.choose_output(t, i);
                return Ok(());
            }
            let Id::Control(t) = self.id else {
                return Err(Error::empty());
            };
            if !w.enabled(t) {
                return Err(HRESULT(UIA_E_ELEMENTNOTENABLED as _).into());
            }
            w.activate(t);
            Ok(())
        })
    }
}

impl IToggleProvider_Impl for Node_Impl {
    fn Toggle(&self) -> Result<()> {
        self.Invoke()
    }
    fn ToggleState(&self) -> Result<ToggleState> {
        with(self.hwnd, |w| {
            let Id::Control(t) = self.id else {
                return Err(Error::from(E_INVALIDARG));
            };
            let on = w.toggled(t).ok_or_else(|| Error::from(E_INVALIDARG))?;
            Ok(if on { ToggleState_On } else { ToggleState_Off })
        })
    }
}

impl IExpandCollapseProvider_Impl for Node_Impl {
    fn Expand(&self) -> Result<()> {
        self.expand_output(true)
    }
    fn Collapse(&self) -> Result<()> {
        self.expand_output(false)
    }
    fn ExpandCollapseState(&self) -> Result<ExpandCollapseState> {
        with(self.hwnd, |w| {
            Ok(
                if matches!(self.id,Id::Control(t) if w.menu_target() == Some(t)) {
                    ExpandCollapseState_Expanded
                } else {
                    ExpandCollapseState_Collapsed
                },
            )
        })
    }
}

impl Node_Impl {
    fn expand_output(&self, open: bool) -> Result<()> {
        with(self.hwnd, |w| {
            let Id::Control(t) = self.id else {
                return Err(E_INVALIDARG.into());
            };
            if !t.is_select() {
                return Err(E_INVALIDARG.into());
            }
            if !w.enabled(t) {
                return Err(HRESULT(UIA_E_ELEMENTNOTENABLED as _).into());
            }
            w.set_output_menu(t, open);
            Ok(())
        })
    }
}

impl ISelectionItemProvider_Impl for Node_Impl {
    fn Select(&self) -> Result<()> {
        self.Invoke()
    }
    fn AddToSelection(&self) -> Result<()> {
        self.Invoke()
    }
    fn RemoveFromSelection(&self) -> Result<()> {
        Err(E_INVALIDARG.into())
    }
    fn IsSelected(&self) -> Result<BOOL> {
        with(self.hwnd, |w| {
            Ok(matches!(self.id,Id::Option(t,i) if w.output_selected(t) == i).into())
        })
    }
    fn SelectionContainer(&self) -> Result<IRawElementProviderSimple> {
        let Id::Option(t, _) = self.id else {
            return Err(E_INVALIDARG.into());
        };
        Ok(node(self.hwnd, Id::Control(t)))
    }
}

impl IValueProvider_Impl for Node_Impl {
    fn SetValue(&self, value: &PCWSTR) -> Result<()> {
        let value = unsafe { value.to_string() }.map_err(|_| Error::from(E_INVALIDARG))?;
        with(self.hwnd, |w| {
            let Id::Control(t) = self.id else {
                return Err(E_INVALIDARG.into());
            };
            if !w.enabled(t) {
                return Err(HRESULT(UIA_E_ELEMENTNOTENABLED as _).into());
            }
            if t == Target::Filename {
                w.set_value(&value);
            } else if t.is_number() {
                let (min, max) = w.number_bounds(t);
                if !value
                    .trim()
                    .parse::<f64>()
                    .is_ok_and(|v| v.is_finite() && v >= min && v <= max)
                    || (t == Target::MaximumSize
                        && w.output_size_bytes(&value)
                            .is_none_or(|bytes| bytes < 100_000))
                {
                    return Err(E_INVALIDARG.into());
                }
                w.set_output_number(t, &value);
            } else {
                return Err(E_INVALIDARG.into());
            }
            Ok(())
        })
    }
    fn Value(&self) -> Result<BSTR> {
        with(self.hwnd, |w| Ok(BSTR::from(value(w, self.id))))
    }
    fn IsReadOnly(&self) -> Result<BOOL> {
        with(self.hwnd, |w| {
            Ok((!matches!(self.id,Id::Control(t) if (t == Target::Filename || t.is_number()) && w.enabled(t))).into())
        })
    }
}

impl IRangeValueProvider_Impl for Node_Impl {
    fn SetValue(&self, value: f64) -> Result<()> {
        with(self.hwnd, |w| {
            let Id::Control(target) = self.id else {
                return Err(E_INVALIDARG.into());
            };
            let Some((_, min, max)) = range(w, target) else {
                return Err(E_INVALIDARG.into());
            };
            if !w.enabled(target) {
                return Err(HRESULT(UIA_E_ELEMENTNOTENABLED as _).into());
            }
            if !value.is_finite() || value < min || value > max {
                return Err(E_INVALIDARG.into());
            }
            if target.is_volume() {
                w.set_volume(target, value);
            } else if matches!(target, Target::CropHandle(_)) {
                w.set_crop_range(target, value);
            } else if target == Target::CompareSplit {
                w.set_compare_split(value);
            } else {
                w.set_trim(target, value);
            }
            Ok(())
        })
    }
    fn Value(&self) -> Result<f64> {
        self.trim_component(0)
    }
    fn Minimum(&self) -> Result<f64> {
        self.trim_component(1)
    }
    fn Maximum(&self) -> Result<f64> {
        self.trim_component(2)
    }
    fn IsReadOnly(&self) -> Result<BOOL> {
        with(self.hwnd, |w| {
            Ok((!matches!(self.id, Id::Control(t) if w.enabled(t))).into())
        })
    }
    fn SmallChange(&self) -> Result<f64> {
        if self.id == Id::Control(Target::CompareSplit) {
            return Ok(captures_app::compression_compare::KEY_STEP * 100.);
        }
        with(self.hwnd, |w| {
            Ok(
                if matches!(self.id, Id::Control(Target::TrimStart | Target::TrimEnd))
                    && w.trim_range(Target::TrimEnd)
                        .is_some_and(|(_, _, max)| max >= 60_000)
                {
                    10.
                } else {
                    1.
                },
            )
        })
    }
    fn LargeChange(&self) -> Result<f64> {
        if self.id == Id::Control(Target::CompareSplit) {
            return Ok(captures_app::compression_compare::PAGE_STEP * 100.);
        }
        Ok(if matches!(self.id,Id::Control(t) if t.is_volume()) {
            20.
        } else if matches!(self.id, Id::Control(Target::CropHandle(_))) {
            10.
        } else {
            1000.
        })
    }
}

impl Node_Impl {
    fn trim_component(&self, part: usize) -> Result<f64> {
        with(self.hwnd, |w| {
            let Id::Control(target) = self.id else {
                return Err(E_INVALIDARG.into());
            };
            let Some((value, min, max)) = range(w, target) else {
                return Err(E_INVALIDARG.into());
            };
            Ok([value, min, max][part])
        })
    }
}

pub fn describe(w: &Recording, events: Vec<Event>) -> Vec<(Event, String)> {
    events
        .into_iter()
        .map(|event| {
            let text = match event {
                Event::Value => w.text(),
                Event::Copy => w.make_copy.to_string(),
                Event::Status => w.status.clone(),
                Event::Name(t) => w.target_label(t).into(),
                Event::OutputValue(t) => w.output_value(t),
                Event::Expanded(t) => (w.menu_target() == Some(t)).to_string(),
                Event::Toggle(t) => w.toggled(t).unwrap_or(false).to_string(),
                Event::Range(t) => {
                    range(w, t).map_or(String::new(), |(value, _, _)| value.to_string())
                }
                _ => String::new(),
            };
            (event, text)
        })
        .collect()
}

pub fn raise(hwnd: HWND, events: &[(Event, String)]) {
    unsafe {
        if !UiaClientsAreListening().as_bool() {
            return;
        }
        for (event, text) in events {
            match *event {
                Event::OutputValue(t) => {
                    let _ = UiaRaiseAutomationPropertyChangedEvent(
                        &node(hwnd, Id::Control(t)),
                        UIA_ValueValuePropertyId,
                        &VARIANT::default(),
                        &VARIANT::from(BSTR::from(text)),
                    );
                    if t == Target::MaximumSize {
                        let _ = UiaRaiseAutomationEvent(
                            &node(hwnd, Id::MaximumEstimate),
                            UIA_LiveRegionChangedEventId,
                        );
                    }
                }
                Event::Expanded(t) => {
                    let _ = UiaRaiseAutomationPropertyChangedEvent(
                        &node(hwnd, Id::Control(t)),
                        UIA_ExpandCollapseExpandCollapseStatePropertyId,
                        &VARIANT::default(),
                        &VARIANT::from(if text == "true" { 1i32 } else { 0i32 }),
                    );
                }
                Event::Focus(target) => {
                    let _ = UiaRaiseAutomationEvent(
                        &node(hwnd, Id::Control(target)),
                        UIA_AutomationFocusChangedEventId,
                    );
                }
                Event::Value => {
                    let _ = UiaRaiseAutomationPropertyChangedEvent(
                        &node(hwnd, Id::Control(Target::Filename)),
                        UIA_ValueValuePropertyId,
                        &VARIANT::default(),
                        &VARIANT::from(BSTR::from(text)),
                    );
                }
                Event::Copy | Event::Toggle(_) => {
                    let target = if let Event::Toggle(t) = *event {
                        t
                    } else {
                        Target::Copy
                    };
                    let on = text == "true";
                    let _ = UiaRaiseAutomationPropertyChangedEvent(
                        &node(hwnd, Id::Control(target)),
                        UIA_ToggleToggleStatePropertyId,
                        &VARIANT::from(if on { 0i32 } else { 1i32 }),
                        &VARIANT::from(if on { 1i32 } else { 0i32 }),
                    );
                }
                Event::Name(target) => {
                    let _ = UiaRaiseAutomationPropertyChangedEvent(
                        &node(hwnd, Id::Control(target)),
                        UIA_NamePropertyId,
                        &VARIANT::default(),
                        &VARIANT::from(BSTR::from(text)),
                    );
                }
                Event::Range(target) => {
                    let _ = UiaRaiseAutomationPropertyChangedEvent(
                        &node(hwnd, Id::Control(target)),
                        UIA_RangeValueValuePropertyId,
                        &VARIANT::default(),
                        &VARIANT::from(text.parse::<f64>().unwrap_or(0.)),
                    );
                }
                Event::Status => {
                    let _ = UiaRaiseAutomationEvent(
                        &node(hwnd, Id::Status),
                        UIA_LiveRegionChangedEventId,
                    );
                }
                Event::Structure => {
                    let _ = UiaRaiseStructureChangedEvent(
                        &node(hwnd, Id::Root),
                        StructureChangeType_ChildrenInvalidated,
                        runtime(Id::Root).as_mut_ptr(),
                        3,
                    );
                }
            }
        }
    }
}

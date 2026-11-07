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

use crate::recording::{Event, Recording, Target, with_recording};
use crate::scene::Rect;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Id {
    Root,
    Title,
    Warning,
    Location,
    Status,
    Control(Target),
}

fn with<R>(hwnd: HWND, f: impl FnOnce(&mut Recording) -> Result<R>) -> Result<R> {
    with_recording(hwnd, f).unwrap_or_else(|| Err(HRESULT(UIA_E_ELEMENTNOTAVAILABLE as _).into()))
}

fn children(w: &Recording) -> Vec<Id> {
    let mut ids = vec![Id::Title];
    if w.warning().is_some() {
        ids.push(Id::Warning);
    }
    ids.push(Id::Location);
    ids.extend(
        [
            Target::Change,
            Target::Filename,
            Target::Copy,
            Target::Cancel,
            Target::Reveal,
            Target::Save,
        ]
        .into_iter()
        .filter(|&t| w.shown(t))
        .map(Id::Control),
    );
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
        Id::Control(t) => t.label().into(),
    }
}

fn value(w: &Recording, id: Id) -> String {
    match id {
        Id::Control(Target::Filename) => w.text(),
        Id::Location => w.directory.to_string_lossy().into_owned(),
        _ => String::new(),
    }
}

fn rect(w: &Recording, id: Id) -> Rect {
    match id {
        Id::Control(t) => w.target_rect(t),
        Id::Root => Rect::new(0., 0., w.width, w.height),
        Id::Title => w.title_rect(),
        Id::Warning => w.warning_rect(),
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
    let kind = match id {
        Id::Root => 0,
        Id::Title => 1,
        Id::Warning => 2,
        Id::Location => 3,
        Id::Status => 4,
        Id::Control(t) => 10 + t as i32,
    };
    [UiaAppendRuntimeId as i32, 400, kind]
}

#[implement(
    IRawElementProviderSimple,
    IRawElementProviderFragment,
    IRawElementProviderFragmentRoot,
    IInvokeProvider,
    IToggleProvider,
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

impl IRawElementProviderSimple_Impl for Node_Impl {
    fn ProviderOptions(&self) -> Result<ProviderOptions> {
        Ok(ProviderOptions_ServerSideProvider | ProviderOptions_UseComThreading)
    }
    fn GetPatternProvider(&self, pattern: UIA_PATTERN_ID) -> Result<IUnknown> {
        let supported = match self.id {
            Id::Control(Target::Filename) | Id::Location => pattern == UIA_ValuePatternId,
            Id::Control(Target::Copy) => pattern == UIA_TogglePatternId,
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
                _ => true,
            };
            Ok(match property {
                UIA_NamePropertyId => VARIANT::from(BSTR::from(name(w, self.id))),
                UIA_ControlTypePropertyId => VARIANT::from(
                    match self.id {
                        Id::Root => UIA_WindowControlTypeId,
                        Id::Control(Target::Filename) => UIA_EditControlTypeId,
                        Id::Control(Target::Copy) => UIA_CheckBoxControlTypeId,
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
                    VARIANT::from(matches!(self.id,Id::Control(t) if !w.shown(t)))
                }
                UIA_HeadingLevelPropertyId if self.id == Id::Title => {
                    VARIANT::from(HeadingLevel1.0)
                }
                UIA_LiveSettingPropertyId if self.id == Id::Status => {
                    VARIANT::from(if w.error { Assertive.0 } else { Polite.0 })
                }
                UIA_ValueValuePropertyId => VARIANT::from(BSTR::from(value(w, self.id))),
                UIA_ValueIsReadOnlyPropertyId => {
                    VARIANT::from(self.id != Id::Control(Target::Filename) || !enabled)
                }
                UIA_ToggleToggleStatePropertyId if self.id == Id::Control(Target::Copy) => {
                    VARIANT::from(if w.make_copy { 1i32 } else { 0i32 })
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
            let list = children(w);
            Ok(match direction {
                NavigateDirection_Parent if self.id != Id::Root => Some(Id::Root),
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
            Ok(w.hit(x, y)
                .map(Id::Control)
                .or_else(|| {
                    children(w)
                        .into_iter()
                        .find(|&id| rect(w, id).contains(x, y))
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
            Ok(if w.make_copy {
                ToggleState_On
            } else {
                ToggleState_Off
            })
        })
    }
}

impl IValueProvider_Impl for Node_Impl {
    fn SetValue(&self, value: &PCWSTR) -> Result<()> {
        let value = unsafe { value.to_string() }.map_err(|_| Error::from(E_INVALIDARG))?;
        with(self.hwnd, |w| {
            if self.id != Id::Control(Target::Filename) || !w.enabled(Target::Filename) {
                return Err(HRESULT(UIA_E_ELEMENTNOTENABLED as _).into());
            }
            w.set_value(&value);
            Ok(())
        })
    }
    fn Value(&self) -> Result<BSTR> {
        with(self.hwnd, |w| Ok(BSTR::from(value(w, self.id))))
    }
    fn IsReadOnly(&self) -> Result<BOOL> {
        with(self.hwnd, |w| {
            Ok((self.id != Id::Control(Target::Filename) || !w.enabled(Target::Filename)).into())
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
                Event::Copy => {
                    let on = text == "true";
                    let _ = UiaRaiseAutomationPropertyChangedEvent(
                        &node(hwnd, Id::Control(Target::Copy)),
                        UIA_ToggleToggleStatePropertyId,
                        &VARIANT::from(if on { 0i32 } else { 1i32 }),
                        &VARIANT::from(if on { 1i32 } else { 0i32 }),
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

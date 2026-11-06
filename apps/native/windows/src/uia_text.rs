//! UI Automation text ranges over a [`TextField`](crate::textfield::TextField):
//! the Text pattern's `ITextRangeProvider`, so Narrator reads characters and
//! words as the caret moves and can select by range. Single-line fields
//! treat lines, paragraphs, pages and the document as the whole text.
#![allow(non_upper_case_globals)]
use std::cell::Cell;

use windows::Win32::Foundation::*;
use windows::Win32::System::Com::SAFEARRAY;
use windows::Win32::System::Ole::*;
use windows::Win32::System::Variant::*;
use windows::Win32::UI::Accessibility::*;
use windows::core::*;

use crate::host::Host;
use crate::textfield::Editor;
use crate::uia::{Id, field, field_screen_rect, node, safe_array_unknown, with};

#[implement(ITextRangeProvider)]
pub struct Range {
    owner: Id,
    start: Cell<usize>,
    end: Cell<usize>,
}

pub fn range(owner: Id, start: usize, end: usize) -> ITextRangeProvider {
    Range {
        owner,
        start: Cell::new(start),
        end: Cell::new(end),
    }
    .into()
}

fn editor(host: &Host, owner: Id) -> Result<&Editor> {
    field(host, owner)
        .map(|f| &f.editor)
        .ok_or_else(|| HRESULT(UIA_E_ELEMENTNOTAVAILABLE as _).into())
}

/// Unit boundaries, sorted, always including 0 and the end.
fn boundaries(editor: &Editor, unit: TextUnit) -> Vec<usize> {
    let n = editor.len();
    let mut b = vec![0];
    match unit {
        TextUnit_Character | TextUnit_Format => {
            let mut i = 0;
            while i < n {
                i = editor.next(i);
                b.push(i);
            }
        }
        TextUnit_Word => {
            let mut i = 0;
            while i < n {
                i = editor.word_right(i);
                b.push(i);
            }
        }
        _ => {}
    }
    b.push(n);
    b.sort_unstable();
    b.dedup();
    b
}

/// Move `pos` by `count` boundaries; the new position and how far it moved.
fn step(b: &[usize], pos: usize, count: i32) -> (usize, i32) {
    let (mut p, mut moved) = (pos, 0);
    while moved < count {
        let Some(&x) = b.iter().find(|x| **x > p) else {
            break;
        };
        p = x;
        moved += 1;
    }
    while moved > count {
        let Some(&x) = b.iter().rev().find(|x| **x < p) else {
            break;
        };
        p = x;
        moved -= 1;
    }
    (p, moved)
}

fn other(r: Ref<ITextRangeProvider>) -> Result<(usize, usize)> {
    let r = r.ok().map_err(|_| Error::from(E_INVALIDARG))?;
    let r: &Range = unsafe { r.as_impl() };
    Ok((r.start.get(), r.end.get()))
}

fn safe_array_f64(values: &[f64]) -> *mut SAFEARRAY {
    unsafe {
        let array = SafeArrayCreateVector(VT_R8, 0, values.len() as u32);
        for (i, value) in values.iter().enumerate() {
            let _ = SafeArrayPutElement(array, &(i as i32), value as *const f64 as _);
        }
        array
    }
}

impl Range {
    fn bounds(&self, n: usize) -> (usize, usize) {
        let s = self.start.get().min(n);
        (s, self.end.get().min(n).max(s))
    }

    fn set(&self, start: usize, end: usize) {
        self.start.set(start);
        self.end.set(end.max(start));
    }
}

impl ITextRangeProvider_Impl for Range_Impl {
    fn Clone(&self) -> Result<ITextRangeProvider> {
        Ok(range(self.owner, self.start.get(), self.end.get()))
    }

    fn Compare(&self, r: Ref<ITextRangeProvider>) -> Result<BOOL> {
        Ok((other(r)? == (self.start.get(), self.end.get())).into())
    }

    fn CompareEndpoints(
        &self,
        endpoint: TextPatternRangeEndpoint,
        r: Ref<ITextRangeProvider>,
        target: TextPatternRangeEndpoint,
    ) -> Result<i32> {
        let (start, end) = other(r)?;
        let a = if endpoint == TextPatternRangeEndpoint_Start {
            self.start.get()
        } else {
            self.end.get()
        };
        let b = if target == TextPatternRangeEndpoint_Start {
            start
        } else {
            end
        };
        Ok(a as i32 - b as i32)
    }

    fn ExpandToEnclosingUnit(&self, unit: TextUnit) -> Result<()> {
        with(|host| {
            let editor = editor(host, self.owner)?;
            let b = boundaries(editor, unit);
            let (s, _) = self.bounds(editor.len());
            let lo = *b.iter().rev().find(|x| **x <= s).unwrap_or(&0);
            let hi = *b.iter().find(|x| **x > lo).unwrap_or(&editor.len());
            self.set(lo, hi);
            Ok(())
        })
    }

    fn FindAttribute(
        &self,
        _id: UIA_TEXTATTRIBUTE_ID,
        _value: &VARIANT,
        _backward: BOOL,
    ) -> Result<ITextRangeProvider> {
        Err(Error::empty())
    }

    fn FindText(
        &self,
        text: &BSTR,
        backward: BOOL,
        ignore_case: BOOL,
    ) -> Result<ITextRangeProvider> {
        let found = with(|host| {
            let editor = editor(host, self.owner)?;
            let (s, e) = self.bounds(editor.len());
            let units = &editor.units()[s..e];
            let fold = |v: &[u16]| -> Vec<u16> {
                if ignore_case.as_bool() {
                    String::from_utf16_lossy(v)
                        .to_lowercase()
                        .encode_utf16()
                        .collect()
                } else {
                    v.to_vec()
                }
            };
            let (hay, needle) = (fold(units), fold(text));
            if needle.is_empty() || needle.len() > hay.len() || hay.len() != units.len() {
                return Ok(None);
            }
            let mut starts = 0..=hay.len() - needle.len();
            let at = |i: &usize| hay[*i..*i + needle.len()] == needle[..];
            let found = if backward.as_bool() {
                starts.rev().find(at)
            } else {
                starts.find(at)
            };
            Ok(found.map(|i| (s + i, s + i + needle.len())))
        })?;
        found
            .map(|(s, e)| range(self.owner, s, e))
            .ok_or_else(Error::empty)
    }

    fn GetAttributeValue(&self, id: UIA_TEXTATTRIBUTE_ID) -> Result<VARIANT> {
        Ok(match id {
            UIA_IsReadOnlyAttributeId => VARIANT::from(false),
            _ => VARIANT::from(unsafe { UiaGetReservedNotSupportedValue()? }),
        })
    }

    fn GetBoundingRectangles(&self) -> Result<*mut SAFEARRAY> {
        with(|host| {
            let Some(field) = field(host, self.owner) else {
                return Ok(safe_array_f64(&[]));
            };
            let (s, e) = self.bounds(field.editor.len());
            let (box_, scale) = field_screen_rect(host, self.owner);
            let top = box_.top + (box_.height - f64::from(field.line_height()) * scale) / 2.;
            let mut values = Vec::new();
            for (left, width) in field.range_rects(s, e) {
                // Clipped to the visible box.
                let x0 = (f64::from(left) * scale).max(0.);
                let x1 = (f64::from(left + width.max(1.)) * scale).min(box_.width);
                if x1 > x0 {
                    values.extend_from_slice(&[
                        box_.left + x0,
                        top,
                        x1 - x0,
                        f64::from(field.line_height()) * scale,
                    ]);
                }
            }
            Ok(safe_array_f64(&values))
        })
    }

    fn GetEnclosingElement(&self) -> Result<IRawElementProviderSimple> {
        Ok(node(self.owner))
    }

    fn GetText(&self, max: i32) -> Result<BSTR> {
        with(|host| {
            let editor = editor(host, self.owner)?;
            let (s, e) = self.bounds(editor.len());
            let e = if max >= 0 { e.min(s + max as usize) } else { e };
            Ok(BSTR::from_wide(&editor.units()[s..e]))
        })
    }

    fn Move(&self, unit: TextUnit, count: i32) -> Result<i32> {
        with(|host| {
            let editor = editor(host, self.owner)?;
            let n = editor.len();
            let b = boundaries(editor, unit);
            let (s, e) = self.bounds(n);
            let base = *b.iter().rev().find(|x| **x <= s).unwrap_or(&0);
            let (p, moved) = step(&b, base, count);
            let end = if s == e {
                p
            } else {
                *b.iter().find(|x| **x > p).unwrap_or(&n)
            };
            self.set(p, end);
            Ok(moved)
        })
    }

    fn MoveEndpointByUnit(
        &self,
        endpoint: TextPatternRangeEndpoint,
        unit: TextUnit,
        count: i32,
    ) -> Result<i32> {
        with(|host| {
            let editor = editor(host, self.owner)?;
            let b = boundaries(editor, unit);
            let (s, e) = self.bounds(editor.len());
            let start = endpoint == TextPatternRangeEndpoint_Start;
            let (p, moved) = step(&b, if start { s } else { e }, count);
            if start {
                self.start.set(p);
                self.end.set(e.max(p));
            } else {
                self.end.set(p);
                self.start.set(s.min(p));
            }
            Ok(moved)
        })
    }

    fn MoveEndpointByRange(
        &self,
        endpoint: TextPatternRangeEndpoint,
        r: Ref<ITextRangeProvider>,
        target: TextPatternRangeEndpoint,
    ) -> Result<()> {
        let (start, end) = other(r)?;
        let p = if target == TextPatternRangeEndpoint_Start {
            start
        } else {
            end
        };
        if endpoint == TextPatternRangeEndpoint_Start {
            self.start.set(p);
            self.end.set(self.end.get().max(p));
        } else {
            self.end.set(p);
            self.start.set(self.start.get().min(p));
        }
        Ok(())
    }

    fn Select(&self) -> Result<()> {
        let owner = self.owner;
        with(|host| {
            let n = editor(host, owner)?.len();
            let (s, e) = self.bounds(n);
            host.scene.field_edit(&host.g, |editor| editor.select(s, e));
            host.need = true;
            Ok(())
        })
    }

    fn AddToSelection(&self) -> Result<()> {
        Err(HRESULT(UIA_E_INVALIDOPERATION as _).into())
    }

    fn RemoveFromSelection(&self) -> Result<()> {
        Err(HRESULT(UIA_E_INVALIDOPERATION as _).into())
    }

    fn ScrollIntoView(&self, _top: BOOL) -> Result<()> {
        Ok(())
    }

    fn GetChildren(&self) -> Result<*mut SAFEARRAY> {
        Ok(safe_array_unknown(&[]))
    }
}

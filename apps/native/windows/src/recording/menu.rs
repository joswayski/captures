//! Shipping CustomSelect menus. Focus remains on the trigger; highlighted
//! rows do not change the session until Enter/Space or a pointer selection.
use super::*;
use captures_app::controls::select;

#[derive(Clone, Copy)]
pub(super) struct Menu {
    pub target: Target,
    pub active: usize,
    pub scroll: f32,
}

impl Recording {
    pub fn menu_target(&self) -> Option<Target> {
        self.menu.map(|m| m.target)
    }

    pub fn set_output_menu(&mut self, target: Target, open: bool) {
        if !target.is_select() || !self.enabled(target) {
            return;
        }
        if open {
            self.set_focus(Some(target), true);
            self.menu = Some(Menu {
                target,
                active: self.output_selected(target),
                scroll: 0.,
            });
            self.place_output_menu();
        } else {
            self.close_output_menu();
        }
        self.events.push(Event::Structure);
        self.events.push(Event::Expanded(target));
        self.dirty[MENU] = true;
    }

    pub(super) fn toggle_output_menu(&mut self, t: Target) {
        self.set_output_menu(t, self.menu_target() != Some(t));
    }

    pub(super) fn close_output_menu(&mut self) {
        if let Some(m) = self.menu.take() {
            self.events.push(Event::Structure);
            self.events.push(Event::Expanded(m.target));
            self.dirty[MENU] = true;
        }
    }

    fn menu_row_height(&self, t: Target) -> f32 {
        if matches!(
            t,
            Target::Resolution | Target::Quality | Target::QualityMode
        ) {
            64.
        } else {
            self.t.n("h-md") + 2. * self.t.n("s-2")
        }
    }

    pub(super) fn place_output_menu(&mut self) {
        let Some(menu) = self.menu else {
            return;
        };
        let trigger = self.visible_target_rect(menu.target);
        if trigger.h <= 0. || !self.enabled(menu.target) {
            self.close_output_menu();
            return;
        }
        let count = self.output_choices(menu.target).len();
        let row = self.menu_row_height(menu.target);
        let pad = self.t.n("s-2");
        let layout = select::layout(
            select::Rect {
                left: trigger.x.into(),
                top: trigger.y.into(),
                width: trigger.w.into(),
                height: trigger.h.into(),
            },
            360.,
            f64::from(row * count as f32 + 2. * pad + 2.),
            self.width.into(),
            self.height.into(),
            count,
        );
        let r = Rect::new(
            layout.left as f32,
            layout.top as f32,
            layout.width as f32,
            (layout.max_height as f32).min(row * count as f32 + 2. * pad + 2.),
        );
        self.rects[MENU] = r;
        let height = r.h - 2. * pad - 2.;
        let top = menu.active as f32 * row;
        let scroll = if top < menu.scroll {
            top
        } else if top + row > menu.scroll + height {
            top + row - height
        } else {
            menu.scroll
        };
        self.menu.as_mut().expect("menu").scroll = scroll.max(0.);
        self.dirty[MENU] = true;
    }

    pub fn output_menu_row(&self, index: usize) -> Rect {
        let Some(m) = self.menu else {
            return Rect::default();
        };
        let r = self.rects[MENU];
        let pad = self.t.n("s-2") + 1.;
        let h = self.menu_row_height(m.target);
        let y = r.y + pad + index as f32 * h - m.scroll;
        let top = y.max(r.y + pad);
        let bottom = (y + h).min(r.y + r.h - pad);
        Rect::new(r.x + pad, top, r.w - 2. * pad, (bottom - top).max(0.))
    }

    pub fn output_menu_hit(&self, x: f32, y: f32) -> Option<usize> {
        let m = self.menu?;
        (0..self.output_choices(m.target).len()).find(|&i| {
            let r = self.output_menu_row(i);
            r.h > 0. && r.contains(x, y)
        })
    }

    pub(super) fn output_menu_pointer(&mut self, msg: u32, x: f32, y: f32) -> bool {
        let Some(m) = self.menu else {
            return false;
        };
        let hit = self.output_menu_hit(x, y);
        if let Some(i) = hit {
            if msg == WM_MOUSEMOVE && i != m.active {
                self.menu.as_mut().expect("menu").active = i;
                self.dirty[MENU] = true;
            }
            if msg == WM_LBUTTONDOWN {
                self.choose_output(m.target, i);
            }
            return true;
        }
        if msg == WM_LBUTTONDOWN {
            self.close_output_menu();
            return true;
        }
        self.rects[MENU].contains(x, y)
    }

    pub(super) fn output_menu_wheel(&mut self, delta: f32) -> bool {
        let Some(m) = self.menu else {
            return false;
        };
        let total = self.output_choices(m.target).len() as f32 * self.menu_row_height(m.target);
        let max = (total - self.rects[MENU].h + 2. * self.t.n("s-2") + 2.).max(0.);
        self.menu.as_mut().expect("menu").scroll = (m.scroll - delta).clamp(0., max);
        self.dirty[MENU] = true;
        true
    }

    pub(super) fn output_menu_key(&mut self, key: VIRTUAL_KEY) -> bool {
        if key == VK_TAB {
            self.close_output_menu();
            return false;
        }
        let Some(t) = self.focus.filter(|&t| t.is_select() && self.enabled(t)) else {
            return false;
        };
        let key = match key {
            VK_DOWN => select::Key::ArrowDown,
            VK_UP => select::Key::ArrowUp,
            VK_HOME => select::Key::Home,
            VK_END => select::Key::End,
            VK_RETURN => select::Key::Enter,
            VK_SPACE => select::Key::Space,
            VK_ESCAPE => select::Key::Escape,
            _ => return false,
        };
        let state = self
            .menu
            .map_or(select::State::default(), |m| select::State {
                open: true,
                active: m.active,
            });
        let out = select::key(
            state,
            &vec![false; self.output_choices(t).len()],
            self.output_selected(t),
            key,
        );
        if !out.handled {
            return false;
        }
        if let Some(i) = out.chosen {
            self.choose_output(t, i);
        } else if out.state.open {
            if self.menu.is_none() {
                self.set_output_menu(t, true);
            }
            self.menu.as_mut().expect("menu").active = out.state.active;
            self.place_output_menu();
        } else {
            self.close_output_menu();
        }
        true
    }

    pub(super) fn draw_output_menu(&self, dc: &ID2D1DeviceContext) -> Result<()> {
        let Some(m) = self.menu else {
            return Ok(());
        };
        let t = &self.t;
        let r = self.rects[MENU];
        let all = gfx::rect(0., 0., r.w, r.h);
        gfx::shadow(dc, all, t.n("r-lg"), t.shadow("shadow-lg"))?;
        gfx::fill_round(dc, all, t.n("r-lg"), t.color("surface-overlay"))?;
        gfx::border(dc, all, t.n("r-lg"), 1., t.color("border"))?;
        let pad = t.n("s-2") + 1.;
        let h = self.menu_row_height(m.target);
        unsafe {
            dc.PushAxisAlignedClip(
                &gfx::rect(pad, pad, r.w - pad, r.h - pad),
                D2D1_ANTIALIAS_MODE_ALIASED,
            );
        }
        let result = (|| {
            for (i, choice) in self.output_choices(m.target).iter().enumerate() {
                let y = pad + i as f32 * h - m.scroll;
                if y + h < pad || y > r.h - pad {
                    continue;
                }
                if i == m.active {
                    gfx::fill_round(
                        dc,
                        gfx::rect(pad, y, r.w - pad, y + h),
                        t.n("r-sm"),
                        t.color("surface-hover"),
                    )?;
                }
                let x = pad + t.n("s-4");
                let label_y = if choice.description.is_empty() {
                    y
                } else {
                    y + 7.
                };
                let label_h = if choice.description.is_empty() {
                    h
                } else {
                    20.
                };
                self.draw_text(
                    dc,
                    Style::Option,
                    &choice.label,
                    Rect::new(x, label_y, r.w - x - 36., label_h),
                    "text",
                    false,
                )?;
                if !choice.description.is_empty() {
                    let text = self
                        .g
                        .text(t, Style::Tiny, &choice.description, r.w - x - 30.)?;
                    gfx::text(dc, &text, x, y + 29., t.color("text-subtle"))?;
                }
                if self.output_selected(m.target) == i {
                    gfx::icon(
                        dc,
                        &self.g.factory,
                        "check",
                        &[],
                        (r.w - pad - 24., y + 12., 14.),
                        2.,
                        t.color("theme-accent"),
                    )?;
                }
            }
            Ok(())
        })();
        unsafe {
            dc.PopAxisAlignedClip();
        }
        result
    }
}

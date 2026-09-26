//! AccessKit names for custom-painted surfaces that have no egui widget of
//! their own (capture overlays, setup cards and statuses).
use eframe::egui::{
    self,
    accesskit::{Live, Role},
};

/// A child `Ui` over `rect`, exposed as a group named `label`. Widgets added
/// to the returned `Ui` become the group's children. The parent `Ui` still
/// advances past `rect`, as if the child's contents were allocated there.
pub fn group(ui: &mut egui::Ui, rect: egui::Rect, salt: &str, label: &str) -> egui::Ui {
    let child = ui.new_child(egui::UiBuilder::new().max_rect(rect).id_salt(salt));
    ui.advance_cursor_after_rect(rect);
    set_group(&child, Role::Group, label);
    child
}

/// Names `ui`'s own container node, e.g. a permission card (`<article>`).
pub fn set_group(ui: &egui::Ui, role: Role, label: &str) {
    ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
        node.set_role(role);
        node.set_label(label);
    });
}

/// Gives `ui`'s container node a role whose content names it, e.g. an alert.
pub fn set_role(ui: &egui::Ui, role: Role) {
    ui.ctx()
        .accesskit_node_builder(ui.unique_id(), |node| node.set_role(role));
}

/// The current value of a group made by [`group`], e.g. its target.
pub fn set_value(ui: &egui::Ui, value: &str) {
    ui.ctx()
        .accesskit_node_builder(ui.unique_id(), |node| node.set_value(value));
}

/// Static text under `ui`'s node, read by screen readers but not painted by
/// egui (the host paints it). `live` announces changes politely, like an
/// `aria-live="polite"` region.
pub fn text(ui: &egui::Ui, salt: &str, rect: egui::Rect, text: &str, live: bool) {
    let response = ui.interact(rect, ui.unique_id().with(salt), egui::Sense::hover());
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_role(Role::Label);
        node.set_value(text);
        node.set_bounds(egui::accesskit::Rect {
            x0: rect.min.x.into(),
            y0: rect.min.y.into(),
            x1: rect.max.x.into(),
            y1: rect.max.y.into(),
        });
        if live {
            node.set_live(Live::Polite);
        }
    });
}

/// Turns a label into a heading (`<h1>`…`<h6>`) named `text`.
pub fn heading(response: &egui::Response, level: usize, text: &str) {
    response.ctx.accesskit_node_builder(response.id, |node| {
        node.set_role(Role::Heading);
        node.set_level(level);
        node.set_label(text);
    });
}

/// Marks `ui`'s container as a polite live region (`aria-live="polite"`).
pub fn set_live(ui: &egui::Ui) {
    ui.ctx()
        .accesskit_node_builder(ui.unique_id(), |node| node.set_live(Live::Polite));
}

#[cfg(test)]
pub mod tests {
    use eframe::egui::{
        self,
        accesskit::{Node, NodeId, TreeUpdate},
    };

    /// The latest AccessKit tree from one pass of `add` at `size`.
    pub fn tree(
        ctx: &egui::Context,
        size: egui::Vec2,
        events: Vec<egui::Event>,
        add: impl FnMut(&mut egui::Ui),
    ) -> TreeUpdate {
        ctx.enable_accesskit();
        let mut add = add;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                events,
                ..Default::default()
            },
            |ui| add(ui),
        );
        output.textures_delta.clear();
        output
            .platform_output
            .accesskit_update
            .take()
            .expect("accesskit tree")
    }

    pub fn find<'a>(tree: &'a TreeUpdate, label: &str) -> Option<(NodeId, &'a Node)> {
        tree.nodes
            .iter()
            .find(|(_, node)| node.label() == Some(label))
            .map(|(id, node)| (*id, node))
    }

    pub fn find_role<'a>(
        tree: &'a TreeUpdate,
        role: egui::accesskit::Role,
        label: &str,
    ) -> Option<(NodeId, &'a Node)> {
        tree.nodes
            .iter()
            .find(|(_, node)| node.role() == role && node.label() == Some(label))
            .map(|(id, node)| (*id, node))
    }

    pub fn find_value<'a>(tree: &'a TreeUpdate, value: &str) -> Option<(NodeId, &'a Node)> {
        tree.nodes
            .iter()
            .find(|(_, node)| node.value() == Some(value))
            .map(|(id, node)| (*id, node))
    }

    /// Whether `child` is inside `ancestor`.
    pub fn contains(tree: &TreeUpdate, ancestor: NodeId, child: NodeId) -> bool {
        let Some((_, node)) = tree.nodes.iter().find(|(id, _)| *id == ancestor) else {
            return false;
        };
        node.children()
            .iter()
            .any(|&next| next == child || contains(tree, next, child))
    }
}

//! Applies mount items to the views, in order. Each kind of content has
//! its module ([`crate::text`], [`crate::image`], [`crate::scroll`],
//! [`crate::text_input`], [`crate::navigation`], ...); this file only
//! keeps the view table and the tree.

use std::rc::Rc;

use gtk4::prelude::*;
use nsh_core::{Command, LayoutAnimation, MountItem, NodeId, PointerEvents, ROOT_ID, ViewKind};
use rustc_hash::FxHashMap;

use crate::{surface::Surface, view::NshView};

/// The views of a surface.
pub(crate) struct Mounter {
    /// Every live view by node id (the root included).
    pub(crate) views: FxHashMap<NodeId, NshView>,
    /// The root view.
    pub(crate) root: NshView,
    /// Animates the frame changes, insertions and removals of the rest of
    /// the batch.
    pub(crate) layout_animation: Option<LayoutAnimation>,
    /// The surface lays out right to left.
    pub(crate) rtl: bool,
}

impl Mounter {
    pub(crate) fn new(root: NshView) -> Self {
        let mut views = FxHashMap::default();
        views.insert(ROOT_ID, root.clone());
        Mounter {
            views,
            root,
            layout_animation: None,
            rtl: false,
        }
    }

    /// Removes every view but the root.
    pub(crate) fn clear(&mut self) {
        for child in self.root.child_views() {
            child.unparent();
        }
        let root = self.root.clone();
        for (_, view) in self.views.drain() {
            if view != root && view.parent().is_some() {
                view.unparent();
            }
        }
        // A screen effect's or transition's layer is the root's: it goes
        // with the views that mounted it.
        root.state_mut().shader = None;
        self.views.insert(ROOT_ID, root);
        self.layout_animation = None;
    }

    /// The batch is applied.
    pub(crate) fn end_batch(&mut self, surface: &Rc<Surface>) {
        self.layout_animation = None;
        crate::navigation::end_batch(surface);
        crate::system::end_batch(surface);
        crate::selectable_view::end_batch(surface);
    }

    fn get(&self, id: NodeId) -> Option<&NshView> {
        let view = self.views.get(&id);
        if view.is_none() {
            log::debug!("mount: no view {id}");
        }
        view
    }

    pub(crate) fn apply(&mut self, surface: &Rc<Surface>, item: &MountItem) {
        match item {
            MountItem::Create { id, kind } => {
                let view = NshView::new(*id, *kind);
                crate::access::created(&view);
                match kind {
                    ViewKind::Input => crate::text_input::create(surface, &view),
                    ViewKind::Canvas => crate::canvas::create(&view),
                    ViewKind::Image => crate::image::create(&view),
                    ViewKind::WebView | ViewKind::Video | ViewKind::Native => {
                        crate::native::create(surface, &view)
                    }
                    _ => {}
                }
                self.views.insert(*id, view);
            }
            MountItem::TrackStates { id, states } => {
                if let Some(view) = self.get(*id) {
                    crate::input::track_states(surface, view, *states);
                }
            }
            MountItem::Insert {
                parent,
                child,
                before,
            } => {
                let (Some(parent), Some(child)) = (self.get(*parent), self.get(*child)) else {
                    return;
                };
                match child.kind() {
                    ViewKind::Modal => return crate::navigation::present(surface, child),
                    ViewKind::HeaderItem => {
                        return crate::navigation::header_item(surface, parent, child);
                    }
                    ViewKind::InputAccessory => {
                        return crate::text_input::accessory(surface, parent, child);
                    }
                    _ => {}
                }
                let before = before
                    .and_then(|b| self.views.get(&b))
                    .filter(|b| b.parent().as_ref() == Some(parent.upcast_ref()));
                // GTK reorders an existing child without unrooting its subtree.
                // Unparenting first would interrupt a focused descendant's editor.
                if child
                    .parent()
                    .as_ref()
                    .is_some_and(|old| old != parent.upcast_ref::<gtk4::Widget>())
                {
                    child.unparent();
                }
                child.insert_before(parent, before);
                parent.queue_allocate();
                crate::transition::inserted(surface, child, self.layout_animation.as_ref());
                if parent.kind() == ViewKind::Stack {
                    crate::navigation::stack_changed(surface, parent, Some(child));
                }
                if parent.kind() == ViewKind::Scroll {
                    crate::scroll::child_inserted(parent);
                }
                crate::selectable_view::touched(surface, child);
                // An attachment's cursor depends on its text's.
                crate::input::style_changed(child);
            }
            MountItem::Remove { id } => {
                if let Some(view) = self.get(*id) {
                    crate::selectable_view::touched(surface, view);
                    // A removal animation detaches the view when it ends.
                    if !crate::transition::removing(surface, view, self.layout_animation.as_ref()) {
                        self.detach(surface, view);
                    }
                }
            }
            MountItem::Delete { id } => {
                crate::file_drop::deleted(surface, *id);
                crate::navigation::deleted(surface, *id);
                if let Some(view) = self.views.remove(id) {
                    crate::selectable_view::touched(surface, &view);
                    crate::native::deleted(surface, &view);
                    crate::shader::deleted(surface, &view);
                    // A removal animation detaches the view when it ends.
                    if !crate::transition::removing(surface, &view, self.layout_animation.as_ref())
                    {
                        crate::transition::deleted(&view);
                        self.detach(surface, &view);
                    }
                }
            }
            MountItem::Frame { id, frame } => {
                let Some(view) = self.get(*id) else { return };
                // A window's size is the window's (it reports it with
                // `set_root_size`).
                if *id == ROOT_ID || crate::window::is_window(surface, *id) {
                    return;
                }
                let mut frame = *frame;
                if view.kind() == ViewKind::Modal {
                    // Where a modal sits is the platform's (a sheet is a
                    // card inside the window): the core only sizes it.
                    let old = view.state().frame;
                    (frame.origin.x.0, frame.origin.y.0) = (old.origin.x.0, old.origin.y.0);
                }
                let frame = &frame;
                // Compared with where a running layout animation is headed,
                // not with the in-between frame it shows: a frame set back
                // to the shown one must still stop the animation.
                let target = crate::transition::frame_target(view);
                if target.unwrap_or(view.state().frame) == *frame {
                    return;
                }
                let old = std::mem::replace(&mut view.state_mut().frame, *frame);
                if old.size.width.0 != frame.size.width.0
                    || old.size.height.0 != frame.size.height.0
                {
                    view.queue_allocate();
                }
                view.moved();
                crate::transition::frame_changed(
                    surface,
                    view,
                    old,
                    self.layout_animation.as_ref(),
                );
                match view.kind() {
                    ViewKind::Stack => crate::navigation::stack_resized(surface, view),
                    ViewKind::Canvas => view.changed(),
                    ViewKind::Image => {
                        crate::shader::resized(view);
                        crate::image::resized(surface, view);
                    }
                    _ => crate::shader::resized(view),
                }
            }
            MountItem::ViewStyle { id, style } => {
                let Some(view) = self.get(*id) else { return };
                let old = std::mem::replace(&mut view.state_mut().style, style.clone());
                crate::input::style_changed(view);
                crate::transition::style_changed(surface, view, &old);
                crate::access::style_changed(view, &old);
                view.moved();
            }
            MountItem::Text {
                id,
                text,
                layout,
                content,
            } => {
                if let Some(view) = self.get(*id) {
                    let old = view.state().text.as_ref().map(|t| t.text.clone());
                    crate::text::set(view, text, layout, *content);
                    if let Some(old) = old {
                        crate::transition::text_changed(surface, view, &old, text);
                    }
                    crate::selectable_view::touched(surface, view);
                    crate::access::text_changed(view);
                }
            }
            MountItem::Events { id, events } => {
                if let Some(view) = self.get(*id) {
                    view.state_mut().events = *events;
                    crate::file_drop::events_changed(surface, view);
                    crate::input::events_changed(surface, view);
                    crate::access::events_changed(surface, view);
                }
            }
            MountItem::Image {
                id,
                source,
                resize_mode,
                options,
            } => {
                if let Some(view) = self.get(*id) {
                    crate::image::set(surface, view, source.as_ref(), *resize_mode, options);
                }
            }
            MountItem::ScrollConfig {
                id,
                horizontal,
                scrollbar_visible,
                replaced,
                mirrored,
                ..
            } => {
                if let Some(view) = self.get(*id) {
                    crate::scroll::config(
                        surface,
                        view,
                        *horizontal,
                        *scrollbar_visible,
                        *replaced,
                        *mirrored,
                    );
                }
            }
            MountItem::ScrollContent { id, size } => {
                if let Some(view) = self.get(*id) {
                    crate::scroll::content(surface, view, *size);
                }
            }
            MountItem::ScrollBy { id, dx, dy } => {
                if let Some(view) = self.get(*id) {
                    crate::scroll::scroll_by(surface, view, dx.0, dy.0);
                }
            }
            MountItem::LayoutDirection { rtl } => self.rtl = *rtl,
            MountItem::Overflow { id, extent } => {
                if let Some(view) = self.get(*id) {
                    view.state_mut().overflow = *extent;
                }
            }
            MountItem::Appearance { appearance } => {
                crate::system::set_appearance(surface, *appearance);
            }
            MountItem::Keymap(keymap) => crate::keymap::set(surface, keymap.clone()),
            MountItem::AccessibilityLabel { id, label } => {
                if let Some(view) = self.get(*id) {
                    crate::access::label(view, label);
                }
            }
            MountItem::StickyCells { id, cells } => {
                if let Some(view) = self.get(*id) {
                    crate::scroll::sticky(surface, view, cells.clone());
                }
            }
            MountItem::ScrollBehavior { id, behavior } => {
                if let Some(view) = self.get(*id) {
                    crate::scroll::behavior(surface, view, behavior);
                }
            }
            MountItem::Refreshing { id, refreshing } => {
                if let Some(view) = self.get(*id) {
                    crate::scroll::refreshing(surface, view, *refreshing);
                }
            }
            MountItem::Canvas { id, drawing } => {
                if let Some(view) = self.get(*id) {
                    crate::canvas::set(surface, view, drawing.clone());
                }
            }
            MountItem::Shader { id, layer } => {
                if let Some(view) = self.get(*id) {
                    crate::shader::set(surface, view, layer.clone());
                }
            }
            MountItem::ShaderInput { id } => {
                if let Some(view) = self.get(*id) {
                    crate::shader::input_changed(view);
                }
            }
            MountItem::ShaderTransition { id, duration } => {
                if let Some(view) = self.get(*id) {
                    crate::shader::start_transition(view, *duration);
                }
            }
            MountItem::NavShader { id, shader } => {
                crate::navigation::nav_shader(surface, *id, shader.clone());
            }
            MountItem::Gestures { id, values } => {
                if let Some(view) = self.get(*id) {
                    crate::input::gestures(surface, view, *values);
                }
            }
            MountItem::ViewProps { id, props } => {
                let Some(view) = self.get(*id) else { return };
                view.set_can_target(!matches!(props.pointer_events, PointerEvents::None));
                view.state_mut().props = props.clone();
                crate::input::props_changed(surface, view);
                crate::selectable_view::props_changed(surface, view);
                crate::access::props_changed(surface, view);
            }
            MountItem::LayoutAnimation(a) => self.layout_animation = Some(*a),
            MountItem::Input { id, config } => {
                if let Some(view) = self.get(*id) {
                    crate::text_input::config(surface, view, config);
                }
            }
            MountItem::InputValue { id, text, count } => {
                if let Some(view) = self.get(*id) {
                    crate::text_input::value(view, text, *count);
                }
            }
            MountItem::InputContent { id, content } => {
                if let Some(view) = self.get(*id) {
                    crate::text_input::content(view, *content);
                }
            }
            MountItem::ScreenConfig { id, config } => {
                if let Some(view) = self.get(*id) {
                    crate::navigation::screen_config(surface, view, config);
                }
            }
            MountItem::ModalConfig { id, config } => {
                if let Some(view) = self.get(*id) {
                    crate::navigation::modal_config(surface, view, config);
                }
            }
            MountItem::NativeProps { id, props } => {
                if let Some(view) = self.get(*id) {
                    crate::native::props(surface, view, props);
                }
            }
            MountItem::Component { id, name } => {
                if let Some(view) = self.get(*id) {
                    crate::native::component(surface, view, name);
                }
            }
            MountItem::ComponentProps { id, props } => {
                if let Some(view) = self.get(*id) {
                    crate::native::component_props(surface, view, props);
                }
            }
            MountItem::Command { id, command } => self.command(surface, *id, command),
            // The animator's (`Surface::apply`).
            MountItem::Animation(_)
            | MountItem::AnimationBindings { .. }
            | MountItem::ComponentPropSlots { .. }
            | MountItem::TreeOrder(_)
            | MountItem::ProjectStates { .. } => {}
        }
    }

    /// Takes `view` out of its parent (a modal is dismissed).
    pub(crate) fn detach(&self, surface: &Rc<Surface>, view: &NshView) {
        match view.kind() {
            ViewKind::Modal => return crate::navigation::dismiss(surface, view),
            ViewKind::HeaderItem => return crate::navigation::header_item_removed(surface, view),
            _ => {}
        }
        let parent = view.parent_view();
        if view.parent().is_some() {
            view.unparent();
        }
        if let Some(parent) = parent {
            parent.queue_allocate();
            parent.queue_draw();
            if parent.kind() == ViewKind::Stack {
                crate::navigation::stack_changed(surface, &parent, None);
            }
        }
    }

    fn command(&self, surface: &Rc<Surface>, id: NodeId, command: &Command) {
        let view = self.views.get(&id);
        match command {
            Command::Focus => match view {
                Some(view) if view.kind() == ViewKind::Input => crate::text_input::focus(view),
                Some(view) => crate::key_focus::focus(view),
                None => {}
            },
            Command::Blur => match view {
                Some(view) if view.kind() == ViewKind::Input => crate::text_input::blur(view),
                Some(view) if crate::key_focus::blur(view) => {}
                _ => crate::text_input::blur_all(surface),
            },
            Command::SetSelection { start, end } => {
                if let Some(view) = view {
                    crate::text_input::set_selection(view, *start, *end);
                }
            }
            Command::InsertText(text) => {
                if let Some(view) = view {
                    crate::text_input::insert_text(view, text);
                }
            }
            Command::ScrollTo { y, .. } if view.is_some_and(|v| v.kind() == ViewKind::Input) => {
                crate::text_input::scroll_to(view.expect("an input"), *y);
            }
            Command::ScrollTo { .. }
            | Command::ScrollToEnd { .. }
            | Command::ScrollToIndex { .. }
            | Command::ZoomTo { .. } => {
                if let Some(view) = view {
                    crate::scroll::command(surface, view, command);
                }
            }
            // A window of the app's own (`Window.open`) hosts dialog root
            // `id`.
            Command::Desktop(desktop) if id != ROOT_ID => {
                crate::window::command(surface, id, view, desktop)
            }
            // Here, with the view at hand: `Surface::view` would borrow the
            // mounter again while a batch is applied.
            Command::AccessibilityFocus => {
                if let Some(view) = view {
                    view.grab_focus();
                }
            }
            Command::ImageInfo { .. } => crate::image::command(surface, id, command),
            Command::SetJsResponder { .. } | Command::ClearJsResponder => {
                crate::input::command(surface, id, command);
            }
            Command::Reload
            | Command::GoBack
            | Command::GoForward
            | Command::StopLoading
            | Command::InjectJavaScript(_)
            | Command::PostMessage(_)
            | Command::Load(_)
            | Command::Seek(_)
            | Command::Native { .. } => {
                if let Some(view) = view {
                    crate::native::command(surface, view, command);
                }
            }
            other => crate::system::command(surface, id, other),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use gtk4::{self as gtk, glib, prelude::*};
    use nsh_core::{MountItem, ROOT_ID, ViewKind};

    use crate::gtk_test;

    fn focused_reorder(nested: bool) {
        gtk_test::run(move || {
            let surface = gtk_test::surface();
            let kind = if nested {
                ViewKind::View
            } else {
                ViewKind::Input
            };
            gtk_test::apply(
                &surface,
                &[
                    MountItem::Create { id: 2, kind },
                    MountItem::Create {
                        id: 4,
                        kind: ViewKind::View,
                    },
                    MountItem::Frame {
                        id: 2,
                        frame: gtk_test::frame(0.0, 0.0, 180.0, 40.0),
                    },
                    MountItem::Frame {
                        id: 4,
                        frame: gtk_test::frame(0.0, 50.0, 180.0, 40.0),
                    },
                    MountItem::Insert {
                        parent: ROOT_ID,
                        child: 2,
                        before: None,
                    },
                    MountItem::Insert {
                        parent: ROOT_ID,
                        child: 4,
                        before: None,
                    },
                ],
            );
            let input_id = if nested {
                gtk_test::apply(
                    &surface,
                    &[
                        MountItem::Create {
                            id: 6,
                            kind: ViewKind::Input,
                        },
                        MountItem::Frame {
                            id: 6,
                            frame: gtk_test::frame(0.0, 0.0, 180.0, 40.0),
                        },
                        MountItem::Insert {
                            parent: 2,
                            child: 6,
                            before: None,
                        },
                    ],
                );
                6
            } else {
                2
            };
            gtk_test::apply(
                &surface,
                &[
                    MountItem::Input {
                        id: input_id,
                        config: Box::default(),
                    },
                    MountItem::InputValue {
                        id: input_id,
                        text: "retained draft".into(),
                        count: 0,
                    },
                    MountItem::InputContent {
                        id: input_id,
                        content: gtk_test::frame(0.0, 0.0, 180.0, 40.0),
                    },
                ],
            );
            let input = gtk_test::view(&surface, input_id);
            let moving = gtk_test::view(&surface, 2);
            let editor = input
                .state()
                .natives
                .iter()
                .find_map(|native| native.widget.clone().downcast::<gtk::Text>().ok())
                .expect("the input hosts its native GTK editor");
            let window = gtk_test::show(&surface);
            assert!(gtk_test::wait_until(|| editor.is_mapped()));
            assert!(editor.grab_focus());
            editor.select_region(2, 7);
            let focus = || editor.root().and_then(|root| root.focus());
            assert_eq!(focus().as_ref(), Some(editor.upcast_ref::<gtk::Widget>()));

            let detached = Rc::new(Cell::new(0));
            for widget in [
                moving.clone().upcast::<gtk::Widget>(),
                editor.clone().upcast(),
            ] {
                let seen = detached.clone();
                widget.connect_root_notify(move |_| seen.set(seen.get() + 1));
                let seen = detached.clone();
                widget.connect_unrealize(move |_| seen.set(seen.get() + 1));
            }
            let parents = Rc::new(Cell::new(0));
            let seen = parents.clone();
            moving.connect_parent_notify(move |_| seen.set(seen.get() + 1));

            // Forward, backward, already in place, and a self anchor all retain
            // the live GTK subtree. No focus command repairs a move afterward.
            for (before, expected) in [
                (None, [4, 2]),
                (Some(4), [2, 4]),
                (Some(4), [2, 4]),
                (Some(2), [2, 4]),
            ] {
                gtk_test::apply(
                    &surface,
                    &[MountItem::Insert {
                        parent: ROOT_ID,
                        child: 2,
                        before,
                    }],
                );
                assert_eq!(parents.get(), 0, "same-parent insertion never unparents");
                assert_eq!(detached.get(), 0, "the editor stays rooted and realized");
                // Let deferred GTK focus reconciliation run across a frame.
                let frames = Rc::new(Cell::new(0));
                let seen = frames.clone();
                window.add_tick_callback(move |_, _| {
                    seen.set(seen.get() + 1);
                    if seen.get() < 2 {
                        glib::ControlFlow::Continue
                    } else {
                        glib::ControlFlow::Break
                    }
                });
                assert!(gtk_test::wait_until(|| frames.get() == 2));
                assert_eq!(focus().as_ref(), Some(editor.upcast_ref::<gtk::Widget>()));
                assert_eq!(editor.selection_bounds(), Some((2, 7)));
                assert_eq!(editor.text(), "retained draft");
                assert_eq!(gtk_test::view(&surface, input_id), input);
                assert_eq!(
                    surface
                        .root
                        .child_views()
                        .iter()
                        .map(|child| child.id())
                        .collect::<Vec<_>>(),
                    expected,
                );
                assert_eq!(detached.get(), 0, "no delayed detach notification");
            }
            window.destroy();
        });
    }

    #[test]
    fn same_parent_insert_keeps_a_focused_editor_rooted() {
        focused_reorder(false);
    }

    #[test]
    fn same_parent_insert_keeps_a_focused_descendant_rooted() {
        focused_reorder(true);
    }

    #[test]
    fn cross_parent_insert_still_detaches_from_the_old_parent() {
        gtk_test::run(|| {
            let surface = gtk_test::surface();
            for id in [2, 4, 6] {
                gtk_test::apply(
                    &surface,
                    &[MountItem::Create {
                        id,
                        kind: ViewKind::View,
                    }],
                );
            }
            gtk_test::apply(
                &surface,
                &[
                    MountItem::Insert {
                        parent: ROOT_ID,
                        child: 2,
                        before: None,
                    },
                    MountItem::Insert {
                        parent: ROOT_ID,
                        child: 4,
                        before: None,
                    },
                    MountItem::Insert {
                        parent: 2,
                        child: 6,
                        before: None,
                    },
                ],
            );
            let child = gtk_test::view(&surface, 6);
            let detached = Rc::new(Cell::new(false));
            let seen = detached.clone();
            child.connect_parent_notify(move |child| {
                if child.parent().is_none() {
                    seen.set(true);
                }
            });
            gtk_test::apply(
                &surface,
                &[MountItem::Insert {
                    parent: 4,
                    child: 6,
                    before: None,
                }],
            );
            assert!(detached.get(), "a genuine reparent still detaches first");
            assert!(gtk_test::view(&surface, 2).child_views().is_empty());
            assert_eq!(gtk_test::view(&surface, 4).child_views(), [child.clone()]);
            assert_eq!(child.parent_view(), Some(gtk_test::view(&surface, 4)));
        });
    }
}

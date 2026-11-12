//! `Checkbox`: `hitSlop` widens where presses count past its bounds, as a
//! view's does: the platforms get it on the checkbox's pressable view, it
//! follows a binding, and a checkbox without one gets none.

use std::sync::{Arc, Mutex};

use nsh_base::checkbox;
use nsh_core::{EdgeInsets, MountBatch, MountItem, NodeId, Size, element::Root, prelude::*};
use nsh_engine::EngineConfig;
use nsh_testing::Headless;

/// Every batch's items.
type Seen = Arc<Mutex<Vec<MountItem>>>;

fn start<C: Component<Props = ()>>() -> (Headless, Seen) {
    let config = EngineConfig {
        viewport: Size::new(px(390.0), px(844.0)),
        ..EngineConfig::new(Root::of::<C>())
    };
    let mut app = Headless::start(config).unwrap();
    let seen = Seen::default();
    let log = seen.clone();
    app.observe(move |_: &str, batch: &MountBatch| {
        log.lock().unwrap().extend(batch.items.iter().cloned());
    });
    app.first_render().unwrap();
    (app, seen)
}

/// The hit slop the platforms last got for view `id` (none: zero).
fn hit_slop(seen: &Seen, id: NodeId) -> EdgeInsets {
    seen.lock()
        .unwrap()
        .iter()
        .filter_map(|item| match item {
            MountItem::ViewProps { id: of, props } if *of == id => Some(props.hit_slop),
            _ => None,
        })
        .next_back()
        .unwrap_or_default()
}

fn target(app: &Headless, selector: &str) -> NodeId {
    app.screen()
        .press_target(selector)
        .unwrap()
        .unwrap_or_else(|| panic!("no pressable {selector}"))
}

/// A small checkbox with a slop on every side, a checkbox whose slop the
/// app widens, and a checkbox with none.
#[component]
fn SmallCheckboxes() -> impl IntoElement {
    let changes = signal(0u32);
    let wide = signal(false);
    view()
        .child(
            checkbox()
                .hit_slop(EdgeInsets::all(px(12.0)))
                .accessibility_label("Agree")
                .on_change(move |_: &bool| changes.update(|n| *n += 1))
                // Laid out to a size, as a view with no area takes no
                // presses.
                .child(text("Agree")),
        )
        .child(
            checkbox()
                .hit_slop(Prop::bind(move || match wide.get() {
                    true => EdgeInsets {
                        top: px(4.0),
                        left: px(2.0),
                        ..EdgeInsets::default()
                    },
                    false => EdgeInsets::default(),
                }))
                .child(text("Grows")),
        )
        .child(
            checkbox()
                .on_change(move |_: &bool| wide.set(true))
                .child(text("Widen")),
        )
        .child(text(move || format!("changes:{}", changes.get())))
}

#[test]
fn hit_slop_reaches_the_pressable_view() {
    let (mut app, seen) = start::<SmallCheckboxes>();
    let small = target(&app, "label:Agree");
    assert_eq!(hit_slop(&seen, small), EdgeInsets::all(px(12.0)));
    // The slop changes nothing of what a press does.
    app.press("label:Agree").unwrap();
    app.wait_for("text:changes:1").unwrap();
}

#[test]
fn hit_slop_follows_a_binding_and_is_none_by_default() {
    let (mut app, seen) = start::<SmallCheckboxes>();
    let grows = target(&app, "text:Grows");
    let widen = target(&app, "text:Widen");
    assert_eq!(hit_slop(&seen, grows), EdgeInsets::default());
    app.press("text:Widen").unwrap();
    let expected = EdgeInsets {
        top: px(4.0),
        left: px(2.0),
        ..EdgeInsets::default()
    };
    assert_eq!(hit_slop(&seen, grows), expected);
    // A checkbox without `hitSlop` never gets one.
    assert_eq!(hit_slop(&seen, widen), EdgeInsets::default());
}

/// Both event subscriptions observe one proposal from the native Checkbox.
/// Changing controlled state in `on_change` must not change detail's proposal.
#[component]
fn CheckboxDetails() -> impl IntoElement {
    let checked = signal(false);
    let changes = signal(0u32);
    let details = signal(0u32);
    let last = signal(String::new());
    view()
        .child(
            checkbox()
                .accessibility_label("Details")
                .checked(checked)
                .on_change(move |value: &bool| {
                    changes.update(|count| *count += 1);
                    checked.set(*value);
                })
                .on_change_detail(move |event: &nsh_base::CheckboxChangeDetail| {
                    details.update(|count| *count += 1);
                    last.set(format!(
                        "proposal:{} modifiers:{}{}{}{}",
                        event.checked,
                        event.control as u8,
                        event.shift as u8,
                        event.platform as u8,
                        event.alt as u8,
                    ));
                })
                .child(text("Details")),
        )
        .child(
            checkbox()
                .accessibility_label("Disabled details")
                .disabled(true)
                .on_change(move |_: &bool| changes.update(|count| *count += 100))
                .on_change_detail(move |_: &nsh_base::CheckboxChangeDetail| {
                    details.update(|count| *count += 100)
                })
                .child(text("Disabled details")),
        )
        .child(text(move || {
            format!("changes:{} details:{}", changes.get(), details.get())
        }))
        .child(text(move || last.get()))
}

#[test]
fn detail_uses_the_same_pointer_proposal_and_all_modifiers_once() {
    let (mut app, _) = start::<CheckboxDetails>();
    app.hold_keys("ctrl+shift+meta+alt", true).unwrap();
    app.press("label:Details").unwrap();
    app.wait_for("text:changes:1 details:1").unwrap();
    app.wait_for("text:proposal:true modifiers:1111").unwrap();
    app.hold_keys("ctrl+shift+meta+alt", false).unwrap();
    app.press("label:Details").unwrap();
    app.wait_for("text:changes:2 details:2").unwrap();
    app.wait_for("text:proposal:false modifiers:0000").unwrap();
    assert!(app.press("label:Disabled details").is_err());
    app.wait_for("text:changes:2 details:2").unwrap();
}

#[test]
fn detail_preserves_focused_keyboard_activation_modifiers_once() {
    let mut config = EngineConfig::new(Root::of::<CheckboxDetails>());
    config.environment.os = nsh_core::platform::Os::MacOs;
    let mut app = Headless::start(config).unwrap();
    app.first_render().unwrap();
    app.press_key("tab").unwrap();
    app.press_key("shift+space").unwrap();
    app.wait_for("text:changes:1 details:1").unwrap();
    app.wait_for("text:proposal:true modifiers:0100").unwrap();
    app.press_key("return").unwrap();
    app.wait_for("text:changes:2 details:2").unwrap();
    app.wait_for("text:proposal:false modifiers:0000").unwrap();
}

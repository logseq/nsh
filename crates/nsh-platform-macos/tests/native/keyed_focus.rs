//! A native input's keyed ancestor moves during its own change callback.
//! AppKit's field editor, selection and both input views must survive the move;
//! the next window-local native key event reaches it without focusing again.
//! Background-only mode keeps the app prohibited from activating and the window
//! non-key, while retaining the same actual field-editor assertions.

use std::{
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use nsh_core::{element::input, prelude::*};
use nsh_platform_macos as platform;
use platform::{
    objc2::{MainThreadMarker, MainThreadOnly, Message, rc::Retained},
    objc2_app_kit::{
        NSAccessibility, NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSEvent,
        NSEventModifierFlags, NSEventType, NSResponder, NSTextField, NSView, NSWindow,
        NSWindowStyleMask,
    },
    objc2_foundation::{NSDate, NSPoint, NSRange, NSRect, NSRunLoop, NSSize, NSString},
};

static HOLDER: AtomicUsize = AtomicUsize::new(0);
static ARMED: AtomicBool = AtomicBool::new(false);
static CHANGES: Mutex<Vec<(u32, String)>> = Mutex::new(Vec::new());

#[component]
fn Fields() -> impl IntoElement {
    let order = signal(vec![1_u32, 2]);
    let rows = each(
        order,
        |id| *id,
        move |id, _| {
            let id = id.get_untracked();
            let value = signal(if id == 1 { "abcd" } else { "other" }.to_owned());
            view().test_id(&format!("row-{id}")).h(px(56.0)).child(
                input()
                    .test_id(&format!("field-{id}"))
                    .w(px(200.0))
                    .h(px(40.0))
                    .value(move || value.get())
                    .on_change(move |text: &str| {
                        value.set(text.to_owned());
                        CHANGES.lock().unwrap().push((id, text.to_owned()));
                        if ARMED.swap(false, Ordering::SeqCst) {
                            order.set(vec![2, 1]);
                        }
                    }),
            )
        },
    );
    match HOLDER.load(Ordering::SeqCst) {
        0 => view()
            .test_id("holder")
            .size_full()
            .child(rows)
            .into_element(),
        1 => view()
            .test_id("holder")
            .size_full()
            .overflow_hidden()
            .shadow_md()
            .child(rows)
            .into_element(),
        2 => view()
            .test_id("holder")
            .size_full()
            .backdrop_blur(px(8.0))
            .child(rows)
            .into_element(),
        _ => scroll()
            .test_id("holder")
            .size_full()
            .child(rows)
            .into_element(),
    }
}

fn find(view: &NSView, id: &str) -> Option<Retained<NSView>> {
    if view
        .accessibilityIdentifier()
        .is_some_and(|value| value.to_string() == id)
    {
        return Some(view.retain());
    }
    view.subviews().iter().find_map(|child| find(&child, id))
}

fn field(view: &NSView) -> Option<Retained<NSView>> {
    if view.downcast_ref::<NSTextField>().is_some() {
        return Some(view.retain());
    }
    view.subviews().iter().find_map(|child| field(&child))
}

fn pump() {
    NSRunLoop::currentRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.005));
}

fn until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(Instant::now() < deadline, "{what} never happened");
        pump();
    }
}

fn changed(value: &str) -> bool {
    CHANGES
        .lock()
        .unwrap()
        .iter()
        .any(|(id, text)| *id == 1 && text == value)
}

fn rows(parent: &NSView) -> Vec<String> {
    parent
        .subviews()
        .iter()
        .filter_map(|view| {
            let id = view.accessibilityIdentifier()?.to_string();
            id.starts_with("row-").then_some(id)
        })
        .collect()
}

fn character(window: &NSWindow, value: &str, code: u16) {
    let value = NSString::from_str(value);
    for kind in [NSEventType::KeyDown, NSEventType::KeyUp] {
        let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            kind, NSPoint::new(0.0, 0.0), NSEventModifierFlags::empty(), 1.0,
            window.windowNumber(), None, &value, &value, false, code,
        ).expect("a native key event");
        window.sendEvent(&event);
    }
}

fn assert_background(mtm: MainThreadMarker, window: Option<&NSWindow>, background: bool) {
    if !background {
        return;
    }
    let app = NSApplication::sharedApplication(mtm);
    assert_eq!(
        app.activationPolicy(),
        NSApplicationActivationPolicy::Prohibited
    );
    assert!(
        !app.isActive(),
        "the keyed focus probe never activates its app"
    );
    if let Some(window) = window {
        assert!(
            !window.isKeyWindow(),
            "the probe never takes the global keyboard focus"
        );
    }
}

fn check(mtm: MainThreadMarker, holder: usize, hosted: bool, background: bool) {
    assert_background(mtm, None, background);
    HOLDER.store(holder, Ordering::SeqCst);
    ARMED.store(false, Ordering::SeqCst);
    CHANGES.lock().unwrap().clear();
    // SAFETY: AppKit objects are created on the process main thread.
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(400.0, 240.0)),
            NSWindowStyleMask::Titled,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    // SAFETY: This scope retains the window until after it closes.
    unsafe { window.setReleasedWhenClosed(false) };
    let surface = platform::AppView::new(mtm, platform::AppOptions::default());
    window.setContentView(Some(&surface.view()));
    surface.load(platform::App::Rust(platform::Root::of::<Fields>()));
    if background {
        // The public nonactivating path also used by NSH's background runner.
        window.orderFrontRegardless();
    } else {
        window.makeKeyAndOrderFront(None);
    }
    assert_background(mtm, Some(&window), background);
    let root = surface.view();
    until("both keyed inputs", || find(&root, "field-2").is_some());
    let first_row = find(&root, "row-1").unwrap();
    let second_row = find(&root, "row-2").unwrap();
    let first = find(&root, "field-1").unwrap();
    let second = find(&root, "field-2").unwrap();
    // SAFETY: Both rows are attached to a retained main-thread container.
    let parent = unsafe { first_row.superview() }.unwrap();
    assert_eq!(rows(&parent), ["row-1", "row-2"]);
    let foreign = hosted.then(|| {
        let view = NSView::initWithFrame(NSView::alloc(mtm), NSRect::ZERO);
        parent.addSubview(&view);
        view
    });

    let native_field = field(&first).expect("the first field's native editor");
    let field = native_field.downcast_ref::<NSTextField>().unwrap();
    assert!(window.makeFirstResponder(Some(field)));
    let editor = field.currentEditor().expect("the active field editor");
    assert_background(mtm, Some(&window), background);
    editor.setSelectedRange(NSRange::new(2, 0));
    character(&window, "z", 6);
    until("the native-key control before reorder", || changed("abzcd"));
    assert_eq!(editor.selectedRange(), NSRange::new(3, 0));
    assert_background(mtm, Some(&window), background);

    ARMED.store(true, Ordering::SeqCst);
    character(&window, "q", 12);
    until("the focused ancestor's keyed move", || {
        rows(&parent) == ["row-2", "row-1"]
    });
    assert!(changed("abzqcd"));
    assert_background(mtm, Some(&window), background);
    for (id, before) in [
        ("row-1", &first_row),
        ("row-2", &second_row),
        ("field-1", &first),
        ("field-2", &second),
    ] {
        assert!(
            std::ptr::eq(&*find(&root, id).unwrap(), &**before),
            "{id} keeps its native identity"
        );
    }
    assert!(std::ptr::eq(
        &*field.currentEditor().expect("the field still edits"),
        &*editor
    ));
    assert!(std::ptr::eq::<NSResponder>(
        &*window.firstResponder().unwrap(),
        &***editor
    ));
    assert_eq!(
        editor.selectedRange(),
        NSRange::new(4, 0),
        "the caret survives ancestor movement"
    );
    if let Some(foreign) = foreign {
        assert!(
            std::ptr::eq(&*parent.subviews().lastObject().unwrap(), &*foreign),
            "the hosted sibling stays above reordered children"
        );
    }

    character(&window, "x", 7);
    until("typing after reorder without refocusing", || {
        changed("abzqxcd")
    });
    assert_eq!(editor.selectedRange(), NSRange::new(5, 0));
    assert_background(mtm, Some(&window), background);
    surface.stop();
    window.close();
    assert_background(mtm, None, background);
}

pub(super) fn run() {
    run_with_background(false);
}

/// Runs only this probe in a fresh process, without activation or global input.
/// No alternate input or field-editor fallback is used if AppKit refuses it.
pub(super) fn run_background() {
    run_with_background(true);
}

fn run_with_background(background: bool) {
    let mtm = MainThreadMarker::new().expect("the process main thread");
    let app = NSApplication::sharedApplication(mtm);
    if background {
        assert!(
            !app.isActive(),
            "start the background probe in a fresh inactive process"
        );
        let before = app.activationPolicy();
        let requested = (before != NSApplicationActivationPolicy::Prohibited)
            .then(|| app.setActivationPolicy(NSApplicationActivationPolicy::Prohibited));
        // The state is authoritative: an already prohibited app needs no change.
        // Report one bounded diagnostic before any native view is constructed.
        eprintln!(
            "background keyed focus: policy before={before:?}, request={requested:?}, actual={:?}, active={}",
            app.activationPolicy(),
            app.isActive(),
        );
    } else {
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    }
    assert_background(mtm, None, background);
    app.finishLaunching();
    assert_background(mtm, None, background);
    for holder in 0..4 {
        for hosted in [false, true] {
            check(mtm, holder, hosted, background);
        }
    }
    assert_background(mtm, None, background);
    println!(
        "keyed focus: native editor identity, selection and window-local native key input survive ancestor reorders (background={background})"
    );
}

//! The query reads the mounted snapshot and delegates to its own engine.

use std::{cell::Cell, rc::Rc, sync::Arc};

use super::{Core, ROOT_ID};
use crate::{
    Affinity, AttributedText, MountItem, Op, Tag, TextEngine, TextGeometryQuery as Q,
    TextGeometryResult as R, TextLayout, TextLine, TextPosition,
    geometry::{Bounds, Point, Points, Size, point, px, rect, size_pt},
};

#[derive(Default)]
struct GeometryEngine {
    layouts: Rc<Cell<u32>>,
}

impl TextEngine for GeometryEngine {
    type Prepared = String;

    fn prepare(&self, text: &AttributedText) -> String {
        text.text.clone()
    }

    fn measure(&self, _: &String, _: Option<Points>) -> Size<Points> {
        size_pt(120.0, 40.0)
    }

    fn layout(&self, text: &String, _: Size<Points>) -> TextLayout {
        self.layouts.set(self.layouts.get() + 1);
        TextLayout(Arc::new(text.clone()))
    }

    fn lines(&self, layout: &TextLayout) -> Vec<TextLine> {
        let text = layout.downcast::<String>().unwrap();
        let end = text.encode_utf16().count() as u32;
        vec![
            TextLine {
                start: 0,
                end: 3,
                y: px(0.0),
                height: px(20.0),
                ..Default::default()
            },
            TextLine {
                start: 3,
                end,
                y: px(20.0),
                height: px(20.0),
                ..Default::default()
            },
        ]
    }

    fn caret_rect(&self, layout: &TextLayout, position: TextPosition) -> Option<Bounds<Points>> {
        assert!(layout.downcast::<String>().is_some());
        let y = if position.affinity == Affinity::Upstream {
            0.0
        } else {
            20.0
        };
        Some(rect(position.offset as f32 * 7.0, y, 0.0, 20.0))
    }

    fn range_rects(
        &self,
        layout: &TextLayout,
        _: std::ops::Range<u32>,
        out: &mut Vec<Bounds<Points>>,
    ) {
        assert!(layout.downcast::<String>().is_some());
        // Deliberately disjoint, as a bidirectional visual selection may be.
        out.extend([rect(4.0, 20.0, 9.0, 20.0), rect(25.0, 20.0, 7.0, 20.0)]);
    }

    fn hit_test(&self, layout: &TextLayout, _: Point<Points>) -> TextPosition {
        assert!(layout.downcast::<String>().is_some());
        TextPosition::new(3, Affinity::Upstream)
    }
}

fn mounted() -> Core<GeometryEngine> {
    let mut c = Core::new(GeometryEngine::default(), size_pt(320.0, 480.0));
    for op in [
        Op::Create {
            id: 2,
            tag: Tag::Text,
        },
        Op::CreateText {
            id: 3,
            text: "a😀e\u{301} אב".into(),
        },
        Op::Insert {
            parent: 2,
            child: 3,
            before: None,
        },
        Op::Insert {
            parent: ROOT_ID,
            child: 2,
            before: None,
        },
    ] {
        c.apply(op).unwrap();
    }
    c
}

#[test]
fn committed_text_geometry_uses_the_mounted_layout_and_attributed_snapshot() {
    let mut c = mounted();
    assert!(c.text_geometry(2, Q::Lines).is_none());
    let batch = c.commit().unwrap();
    let text = batch
        .items
        .iter()
        .find_map(|item| match item {
            MountItem::Text { id: 2, text, .. } => Some(text),
            _ => None,
        })
        .unwrap();
    let layouts = c.text_engine.layouts.get();
    let lines = c.text_geometry(2, Q::Lines).unwrap();
    assert!(Arc::ptr_eq(text, &lines.text));
    let R::Lines(lines) = lines.result else {
        panic!("visual lines")
    };
    assert_eq!(
        lines
            .iter()
            .map(|line| line.start..line.end)
            .collect::<Vec<_>>(),
        [0..3, 3..8]
    );
    for (affinity, y) in [(Affinity::Upstream, 0.0), (Affinity::Downstream, 20.0)] {
        let answer = c
            .text_geometry(2, Q::Caret(TextPosition::new(3, affinity)))
            .unwrap();
        assert_eq!(answer.result, R::Caret(Some(rect(21.0, y, 0.0, 20.0))));
    }
    assert_eq!(
        c.text_geometry(2, Q::Range(3..8)).unwrap().result,
        R::Range(vec![
            rect(4.0, 20.0, 9.0, 20.0),
            rect(25.0, 20.0, 7.0, 20.0)
        ])
    );
    assert_eq!(
        c.text_geometry(2, Q::HitTest(point(px(100.0), px(5.0))))
            .unwrap()
            .result,
        R::HitTest(TextPosition::new(3, Affinity::Upstream))
    );
    assert_eq!(
        c.text_engine.layouts.get(),
        layouts,
        "queries never typeset again"
    );
    assert!(c.commit().is_none(), "queries never dirty or mount views");
}

#[test]
fn committed_text_geometry_rejects_invalid_boundaries_and_non_finite_points() {
    let mut c = mounted();
    c.commit();
    for query in [
        Q::Caret(TextPosition::new(2, Affinity::Downstream)),
        Q::Caret(TextPosition::new(10, Affinity::Downstream)),
        Q::Range(2..3),
        Q::Range(0..2),
        Q::Range(std::ops::Range { start: 5, end: 4 }),
        Q::Range(0..10),
        Q::HitTest(point(px(f32::NAN), px(0.0))),
        Q::HitTest(point(px(0.0), px(f32::INFINITY))),
    ] {
        assert!(c.text_geometry(2, query).is_none());
    }
    assert!(
        c.text_geometry(2, Q::Caret(TextPosition::new(8, Affinity::Downstream)))
            .is_some()
    );
    assert!(c.text_geometry(2, Q::Range(3..3)).is_some());
}

#[test]
fn committed_text_geometry_answers_actions_after_the_same_turns_text_changes() {
    use crate::element::{Answer, task::Action};
    let mut c = mounted();
    c.commit();
    let old = c.text_geometry(2, Q::Lines).unwrap();
    let (answer, slot) = Answer::new();
    c.components
        .executor
        .act(Action::TextGeometry(2, Q::Lines, slot.clone()));
    c.apply(Op::SetText {
        id: 3,
        text: "replacement".into(),
    })
    .unwrap();
    assert!(slot.borrow().value.is_none());
    c.commit();
    let next = slot.borrow_mut().value.take().unwrap().unwrap();
    assert_eq!(next.text.text, "replacement");
    assert_eq!(old.text.text, "a😀e\u{301} אב");
    assert!(!Arc::ptr_eq(&old.text, &next.text));
    drop(answer);
}

#[test]
fn committed_text_geometry_returns_none_for_detached_released_and_non_text_targets() {
    let mut c = mounted();
    c.commit();
    assert!(c.text_geometry(ROOT_ID, Q::Lines).is_none());
    assert!(c.text_geometry(3, Q::Lines).is_none());
    c.apply(Op::Remove {
        parent: ROOT_ID,
        child: 2,
    })
    .unwrap();
    c.commit();
    assert!(c.text_geometry(2, Q::Lines).is_none());
    c.apply(Op::Release { id: 2 }).unwrap();
    c.commit();
    assert!(c.text_geometry(2, Q::Lines).is_none());
}

#[test]
fn committed_text_geometry_uses_current_span_metadata_without_retypesetting() {
    use crate::Prop;
    let mut c = mounted();
    c.apply(Op::Create {
        id: 4,
        tag: Tag::Text,
    })
    .unwrap();
    c.apply(Op::CreateText {
        id: 5,
        text: " span".into(),
    })
    .unwrap();
    c.apply(Op::Insert {
        parent: 4,
        child: 5,
        before: None,
    })
    .unwrap();
    c.apply(Op::Insert {
        parent: 2,
        child: 4,
        before: None,
    })
    .unwrap();
    c.apply(Op::SetProp {
        id: 4,
        prop: Prop::TestId(Some("first".into())),
    })
    .unwrap();
    c.commit();
    let old = c.text_geometry(2, Q::Lines).unwrap();
    let layouts = c.text_engine.layouts.get();
    c.apply(Op::SetProp {
        id: 4,
        prop: Prop::TestId(Some("second".into())),
    })
    .unwrap();
    c.commit();
    let current = c.text_geometry(2, Q::Lines).unwrap();
    assert_eq!(&*old.text.test_ids[0].test_id, "first");
    assert_eq!(&*current.text.test_ids[0].test_id, "second");
    assert_eq!(current.text.test_ids[0].start, 8);
    assert_eq!(current.text.test_ids[0].end, 13);
    assert_eq!(c.text_engine.layouts.get(), layouts);
    assert!(
        c.text_geometry(4, Q::Lines).is_none(),
        "an inline span has no independent layout"
    );
}

#[test]
fn committed_text_geometry_excludes_hidden_and_culled_layouts() {
    use crate::{Style, StylePatch, StyleValue};
    let mut c = mounted();
    c.commit();
    let key = c.key(2).unwrap();
    c.nodes[key as usize].culled = true;
    assert!(c.text_geometry(2, Q::Lines).is_none());
    c.nodes[key as usize].culled = false;
    let mut style = Box::<Style>::default();
    let mut patch = StylePatch::default();
    patch
        .set(&mut style, "display", StyleValue::Str("none"))
        .unwrap();
    c.apply(Op::SetStyle {
        id: 2,
        style,
        patch,
    })
    .unwrap();
    c.commit();
    assert!(c.text_geometry(2, Q::Lines).is_none());
}

#[test]
fn committed_text_geometry_keeps_content_padding_and_ancestor_scroll_coordinates() {
    use crate::{Style, StylePatch, StyleValue};
    let mut c = mounted();
    c.apply(Op::Create {
        id: 6,
        tag: Tag::Scroll,
    })
    .unwrap();
    c.apply(Op::Insert {
        parent: ROOT_ID,
        child: 6,
        before: None,
    })
    .unwrap();
    c.apply(Op::Insert {
        parent: 6,
        child: 2,
        before: None,
    })
    .unwrap();
    let mut style = Box::<Style>::default();
    let mut patch = StylePatch::default();
    for (key, value) in [("paddingLeft", 7.0), ("paddingTop", 11.0)] {
        patch
            .set(&mut style, key, StyleValue::Number(value))
            .unwrap();
    }
    c.apply(Op::SetStyle {
        id: 2,
        style,
        patch,
    })
    .unwrap();
    c.commit();
    let scroll = c.key(6).unwrap();
    let scroll = c.nodes[scroll as usize].scroll.as_mut().unwrap();
    scroll.zoom = 2.0;
    scroll.offset = (0.0, 5.0);
    let answer = c.text_geometry(2, Q::Lines).unwrap();
    assert_eq!(answer.measurement, c.measure(2).unwrap());
    assert_eq!(answer.page_scale, 2.0);
    assert_eq!(answer.content.origin, point(px(7.0), px(11.0)));
    assert_eq!(answer.measurement.page.y, px(-5.0));
}

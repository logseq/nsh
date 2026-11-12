//! Read-only geometry of the final attributed text layout a platform draws.
//!
//! [`crate::element::ElementRef::text_geometry`] answers after the turn's text
//! layout, using the running [`crate::TextEngine`]. It performs no shaping,
//! mutation, focus change or platform command of its own.

use std::{ops::Range, sync::Arc};

use crate::{
    AttributedText, Measurement, TextLine, TextPosition,
    geometry::{Bounds, Point, Points},
};

/// One query in text-content coordinates, in points, with y increasing down.
/// Offsets are UTF-16 units in the final [`AttributedText::text`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum TextGeometryQuery {
    /// Caret edge and visual line, including wrap affinity.
    Caret(TextPosition),
    /// Selection boxes, including separate stretches in bidirectional text.
    Range(Range<u32>),
    /// The character edge nearest a point in the content box.
    HitTest(Point<Points>),
    /// The visual lines of the final layout, in display order.
    Lines,
}

/// A query's answer in text-content coordinates. Glyph placement is supplied
/// by the active text engine, with the same precision as its own geometry API.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum TextGeometryResult {
    /// A zero-width box as high as the caret's visual line.
    Caret(Option<Bounds<Points>>),
    /// One box per adjacent selected stretch on each visual line.
    Range(Vec<Bounds<Points>>),
    /// UTF-16 offset and wrap affinity at the nearest character edge.
    HitTest(TextPosition),
    /// The engine's actual visual lines; their `text` field is empty, as in
    /// [`crate::TextEngine::lines`]. Slice `TextGeometry::text` by UTF-16 ranges
    /// when characters are needed.
    Lines(Vec<TextLine>),
}

/// Geometry and the exact attributed snapshot sent to the platform together.
/// A retained answer stays valid for this snapshot after later commits;
/// consumers discard it when their source revision or retained target changes.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct TextGeometry {
    /// The containing text block's frame and top left in its root view.
    pub measurement: Measurement,
    /// The text content box relative to the containing block. Geometry results
    /// and hit-test points are relative to this box's origin.
    pub content: Bounds<Points>,
    /// Product of ancestor scroll zoom scales. A content coordinate `p`
    /// maps to root-view coordinates as `measurement.page +
    /// (content.origin + p) * page_scale`. As with `measure()`, transforms
    /// and native animation positions are outside this layout snapshot.
    pub page_scale: f32,
    /// The rendered attributed text, including its UTF-16 span/test-id ranges
    /// and atomic attachment positions. Hidden spans retain their offsets.
    pub text: Arc<AttributedText>,
    /// The requested geometry, from the same final layout as `text`.
    pub result: TextGeometryResult,
}

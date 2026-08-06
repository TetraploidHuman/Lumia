//! Source spans.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct BytePos(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Span {
    pub start: BytePos,
    pub end: BytePos,
}

impl Span {
    pub fn new(start: u32, end: u32) -> Self {
        Self {
            start: BytePos(start),
            end: BytePos(end),
        }
    }

    pub fn merge(self, other: Span) -> Span {
        Span {
            start: BytePos(self.start.0.min(other.start.0)),
            end: BytePos(self.end.0.max(other.end.0)),
        }
    }

    pub fn dummy() -> Self {
        Span::new(0, 0)
    }
}

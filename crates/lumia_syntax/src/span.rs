//! Source spans (byte range within a file from the compilation SourceMap).

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct BytePos(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Span {
    /// Index into the compilation `SourceMap` / `LoadedProgram.files`.
    pub file: u32,
    pub start: BytePos,
    pub end: BytePos,
}

impl Span {
    pub fn new(start: u32, end: u32) -> Self {
        Self {
            file: 0,
            start: BytePos(start),
            end: BytePos(end),
        }
    }

    pub fn with_file(self, file: u32) -> Self {
        Self { file, ..self }
    }

    pub fn merge(self, other: Span) -> Span {
        debug_assert_eq!(
            self.file, other.file,
            "merge spans from different files"
        );
        Span {
            file: self.file,
            start: BytePos(self.start.0.min(other.start.0)),
            end: BytePos(self.end.0.max(other.end.0)),
        }
    }

    pub fn dummy() -> Self {
        Span::new(0, 0)
    }
}

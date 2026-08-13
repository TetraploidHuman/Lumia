//! Shared IR helpers for opt passes.

use lumia_core::{Local, Value};
use rustc_hash::FxHashMap as HashMap;

/// Known Int/Bool/Char/Float locals as i64 bit patterns (for call-site matching).
#[derive(Clone, Default)]
pub(crate) struct KnownScalars {
    map: HashMap<u32, i64>,
}

impl KnownScalars {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn insert(&mut self, local: u32, n: i64) {
        self.map.insert(local, n);
    }

    pub(crate) fn remove(&mut self, local: u32) {
        self.map.remove(&local);
    }

    pub(crate) fn get(&self, local: u32) -> Option<i64> {
        self.map.get(&local).copied()
    }

    pub(crate) fn contains(&self, local: u32) -> bool {
        self.map.contains_key(&local)
    }

    /// Track Int / Bool / Char / Float (and Local aliases) as i64 bit patterns.
    pub(crate) fn track(&mut self, local: u32, value: &Value) {
        match value {
            Value::Int(n) => {
                self.map.insert(local, *n);
            }
            Value::Bool(b) => {
                self.map.insert(local, if *b { 1 } else { 0 });
            }
            Value::Char(c) => {
                self.map.insert(local, u32::from(*c) as i64);
            }
            Value::Float(f) => {
                self.map.insert(local, f.to_bits() as i64);
            }
            Value::Local(Local(src)) => {
                if let Some(&n) = self.map.get(src) {
                    self.map.insert(local, n);
                }
            }
            _ => {}
        }
    }

    pub(crate) fn resolve_all(&self, args: &[Local]) -> Option<Vec<i64>> {
        let mut out = Vec::with_capacity(args.len());
        for a in args {
            out.push(self.get(a.0)?);
        }
        Some(out)
    }
}

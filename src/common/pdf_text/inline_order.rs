//! A PDF can draw inline fragments in a later text object. Repair nearby
//! fragments on the same baseline while keeping the page's stream line order.

use super::{number, MAX_STATE_DEPTH};
use lopdf::{Dictionary, Document, Object};
use std::collections::{BTreeMap, HashMap};
use std::ops::Range;

const MAX_SPANS: usize = 32_768;

pub(super) struct Widths {
    values: HashMap<u16, f32>,
    default: Option<f32>,
    two_bytes: bool,
}

impl Widths {
    pub(super) fn read(font: &Dictionary, doc: &Document) -> Option<Self> {
        let mut values = HashMap::new();
        if font.get(b"Subtype").ok()?.as_name().ok()? != b"Type0" {
            let first = u8::try_from(font.get(b"FirstChar").ok()?.as_i64().ok()?).ok()?;
            let widths = font.get_deref(b"Widths", doc).ok()?.as_array().ok()?;
            if widths.len() > 256 - usize::from(first) {
                return None;
            }
            for (i, width) in (u16::from(first)..).zip(widths) {
                values.insert(i, number(width)?);
            }
            return Some(Self {
                values,
                default: None,
                two_bytes: false,
            });
        }
        if font.get(b"Encoding").ok()?.as_name().ok()? != b"Identity-H" {
            return None;
        }
        let descendant = font.get_deref(b"DescendantFonts", doc).ok()?.as_array().ok()?.first()?;
        let font = doc.dereference(descendant).ok()?.1.as_dict().ok()?;
        if let Ok(widths) = font.get_deref(b"W", doc) {
            let mut widths = widths.as_array().ok()?.iter();
            let mut count = 0;
            while let Some(first) = widths.next() {
                let first = u16::try_from(first.as_i64().ok()?).ok()?;
                let entry = widths.next()?;
                if let Ok(list) = entry.as_array() {
                    count += list.len();
                    if list.len() > 65536 - usize::from(first) || count > 65536 {
                        return None;
                    }
                    for (cid, width) in (first..=u16::MAX).zip(list) {
                        values.insert(cid, number(width)?);
                    }
                } else {
                    let last = u16::try_from(entry.as_i64().ok()?).ok()?;
                    count += usize::from(last.checked_sub(first)?) + 1;
                    if count > 65536 {
                        return None;
                    }
                    let width = number(widths.next()?)?;
                    values.extend((first..=last).map(|cid| (cid, width)));
                }
            }
        }
        let default = font.get(b"DW").ok().and_then(number).unwrap_or(1000.0);
        Some(Self {
            values,
            default: Some(default),
            two_bytes: true,
        })
    }

    fn advance(&self, bytes: &[u8]) -> Option<f32> {
        let width = |cid| self.values.get(&cid).copied().or(self.default).filter(|w| *w >= 0.0);
        if self.two_bytes {
            let chunks = bytes.chunks_exact(2);
            if !chunks.remainder().is_empty() {
                return None;
            }
            chunks.map(|b| width(u16::from_be_bytes([b[0], b[1]]))).sum()
        } else {
            bytes.iter().map(|&b| width(u16::from(b))).sum()
        }
    }
}

struct Span {
    x: f32,
    end: f32,
    em: f32,
    text: Range<usize>,
}

#[derive(Default)]
pub(super) struct InlineOrder {
    lines: BTreeMap<i64, Vec<Span>>,
    line: Option<(f32, f32)>,
    x: Option<f32>,
    scale: (f32, f32),
    leading: f32,
    // Each transform gets a fresh identity. Restoring a saved graphics state
    // can change coordinates without another cm.
    frame: usize,
    next_frame: usize,
    saved_state: Vec<(usize, f32)>,
    span_frame: Option<usize>,
    spans: usize,
    disabled: bool,
}

impl InlineOrder {
    pub(super) fn apply(&mut self, operator: &str, operands: &[Object]) {
        let get = |i: usize| operands.get(i).and_then(number).unwrap_or(0.0);
        // shortcut: horizontal text under a common page transform, extend when
        // independently transformed text needs to be merged.
        match operator {
            "cm" if self.spans != 0 => self.disabled = true,
            "cm" if get(1) != 0.0 || get(2) != 0.0 || get(0) <= 0.0 => self.disabled = true,
            "cm" => {
                self.next_frame += 1;
                self.frame = self.next_frame;
            }
            "q" if self.saved_state.len() < MAX_STATE_DEPTH => self.saved_state.push((self.frame, self.leading)),
            "q" => self.disabled = true,
            "Q" => {
                if let Some((frame, leading)) = self.saved_state.pop() {
                    (self.frame, self.leading) = (frame, leading);
                }
            }
            "Tc" | "Tw" | "Ts" if get(0) != 0.0 => self.disabled = true,
            "\"" if get(0) != 0.0 || get(1) != 0.0 => self.disabled = true,
            "Tz" if get(0) != 100.0 => self.disabled = true,
            "BT" => {
                self.scale = (1.0, 1.0);
                self.line = None;
                self.x = None;
            }
            "Tm" if get(1) == 0.0 && get(2) == 0.0 && get(0) > 0.0 && get(3) != 0.0 => {
                self.scale = (get(0), get(3));
                self.line = Some((get(4), get(5)));
                self.x = Some(get(4));
            }
            "Tm" => self.disabled = true,
            "Td" | "TD" => {
                if operator == "TD" {
                    self.leading = -get(1);
                }
                self.move_line(get(0), get(1));
            }
            "TL" => self.leading = get(0),
            "T*" | "'" | "\"" => self.move_line(0.0, -self.leading),
            _ => {}
        }
    }

    fn move_line(&mut self, tx: f32, ty: f32) {
        let (x, y) = self.line.unwrap_or_default();
        let x = x + tx * self.scale.0;
        self.line = Some((x, y + ty * self.scale.1));
        self.x = Some(x);
    }

    pub(super) fn kern(&mut self, adjustment: f32, size: f32) {
        self.x = self.x.map(|x| x - adjustment * size * self.scale.0 / 1000.0);
    }

    pub(super) fn record(&mut self, font: Option<&Widths>, bytes: &[u8], size: f32, text: Range<usize>) {
        if self.disabled || text.is_empty() {
            return;
        }
        if *self.span_frame.get_or_insert(self.frame) != self.frame {
            self.disabled = true;
            return;
        }
        let (Some(x), Some((_, y)), Some(width)) = (self.x, self.line, font.and_then(|font| font.advance(bytes)))
        else {
            self.disabled = true;
            return;
        };
        let em = size * self.scale.0;
        let end = x + width * em / 1000.0;
        if self.spans == MAX_SPANS || !end.is_finite() || !y.is_finite() || y.abs() > 1e6 || em <= 0.0 {
            self.disabled = true;
            return;
        }
        self.spans += 1;
        self.x = Some(end);
        // Relative moves and absolute matrices can land a few hundredths of a
        // point apart on the same baseline after rounding.
        let key = (y * 100.0).round() as i64;
        let key = self.lines.range(key - 4..=key + 4).next().map_or(key, |(&key, _)| key);
        self.lines.entry(key).or_default().push(Span { x, end, em, text });
    }

    pub(super) fn finish(self, text: String) -> String {
        if self.disabled {
            return text;
        }
        let newlines: Vec<_> = text.match_indices('\n').map(|(i, _)| i).collect();
        let mut edits: Vec<(Range<usize>, String)> = Vec::new();
        for mut line in self.lines.into_values() {
            if !line.windows(2).any(|w| {
                w[1].x < w[0].x
                    || newlines
                        .get(newlines.partition_point(|&i| i < w[0].text.end))
                        .is_some_and(|&i| i < w[1].text.start)
            }) {
                continue;
            }
            let first = line[0].text.start;
            line.sort_by(|a, b| a.x.total_cmp(&b.x));
            // Keep independent columns and overlapping drawings in stream order.
            if line.windows(2).any(|w| {
                let gap = w[1].x - w[0].end;
                gap < -0.25 * w[0].em || gap > w[0].em.max(w[1].em)
            }) {
                continue;
            }
            let mut joined = String::new();
            for (i, span) in line.iter().enumerate() {
                let piece = &text[span.text.clone()];
                if i > 0
                    && span.x - line[i - 1].end > 0.12 * span.em
                    && !joined.ends_with(char::is_whitespace)
                    && !piece.starts_with(char::is_whitespace)
                {
                    joined.push(' ');
                }
                joined.push_str(piece);
                edits.push((span.text.clone(), String::new()));
            }
            edits.push((first..first, joined));
        }
        if edits.is_empty() {
            return text;
        }
        edits.sort_by_key(|(range, _)| (range.start, range.end));
        let mut out = String::with_capacity(text.len());
        let mut offset = 0;
        for (range, replacement) in edits {
            out.push_str(&text[offset..range.start]);
            out.push_str(&replacement);
            offset = range.end;
        }
        out.push_str(&text[offset..]);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_stream_order_when_positions_are_incomparable() {
        let font = Widths {
            values: HashMap::new(),
            default: Some(500.0),
            two_bytes: false,
        };
        let text = "First\nSecond".to_string();
        for restored_transform in [true, false] {
            let mut order = InlineOrder::default();
            if restored_transform {
                order.apply("q", &[]);
                order.apply("cm", &[1.into(), 0.into(), 0.into(), 1.into(), 200.into(), 0.into()]);
            }
            order.apply("Tm", &[1.into(), 0.into(), 0.into(), 1.into(), 30.into(), 700.into()]);
            order.record(Some(&font), b"First", 10.0, 0..5);
            if restored_transform {
                order.apply("Q", &[]);
            } else {
                order.apply("\"", &[0.into(), 3.into(), Object::string_literal("Second")]);
            }
            order.apply("Tm", &[1.into(), 0.into(), 0.into(), 1.into(), 0.into(), 700.into()]);
            order.record(Some(&font), b"Second", 10.0, 6..12);
            assert_eq!(order.finish(text.clone()), text);
        }
    }

    #[test]
    fn restores_inline_fragments_without_reordering_columns() {
        let mut order = InlineOrder::default();
        let font = Widths {
            values: HashMap::new(),
            default: Some(500.0),
            two_bytes: false,
        };
        let mut text = String::new();
        let mut draw = |x: i64, y: i64, fragment: &str| {
            order.apply("Tm", &[1.into(), 0.into(), 0.into(), 1.into(), x.into(), y.into()]);
            if !text.is_empty() {
                text.push('\n');
            }
            let start = text.len();
            text.push_str(fragment);
            order.record(Some(&font), fragment.as_bytes(), 10.0, start..text.len());
        };
        draw(0, 700, "Objects ");
        draw(80, 700, " exhibit duplicates.");
        draw(0, 686, "Another line.");
        draw(40, 700, "MUST NOT");
        draw(72, 672, "Right column");
        draw(0, 672, "Left column");
        let extracted = order.finish(text);
        assert_eq!(
            extracted.split_whitespace().collect::<Vec<_>>().join(" "),
            "Objects MUST NOT exhibit duplicates. Another line. Right column Left column"
        );
    }
}

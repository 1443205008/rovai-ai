//! Ephemeral, content-free estimates of output actually observed during a live Run.
//! This module has no database, transcript, evidence, log, or Usage write path.

use std::{collections::HashMap, time::Instant};

use icu_properties::{
    CodePointMapData,
    props::{GeneralCategory, Script},
    script::ScriptWithExtensions,
};
use serde::Serialize;
use uuid::Uuid;

pub const ALGORITHM_VERSION: &str = "observable-output-heuristic-v3";
pub const UNICODE_DATA_VERSION: &str = "icu4x-2.2.0";
const MAX_RUNS: usize = 128;
const MAX_ITEMS_PER_RUN: usize = 512;
const MAX_ITEM_ID_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputKind {
    PublicText,
    ReasoningText,
    ReasoningSummary,
}

#[derive(Clone, Copy, Debug)]
pub struct Fragment<'a> {
    pub run_id: &'a str,
    pub execution_epoch: i64,
    pub kind: OutputKind,
    pub item_id: &'a str,
    /// Native/Execution Text offset, in UTF-16 code units. Gaps establish a new baseline.
    pub offset_utf16: Option<usize>,
    /// A native ingress sequence may replace an offset for genuine delta protocols.
    pub source_sequence: Option<u64>,
    pub text: &'a str,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservableOutputSample {
    pub agent_run_id: String,
    pub execution_epoch: i64,
    pub counter_generation: String,
    pub sequence: u64,
    pub algorithm_version: &'static str,
    pub unicode_data_version: &'static str,
    pub sampled_at_ms: u64,
    pub last_output_at_ms: Option<u64>,
    pub public_text_units: u64,
    pub reasoning_units: u64,
    pub reasoning_source: &'static str,
    pub stream_confirmed: bool,
}

#[derive(Debug)]
struct ItemCursor {
    kind: OutputKind,
    end_utf16: Option<usize>,
    last_sequence: Option<u64>,
    fragments: u32,
}

#[derive(Debug)]
struct RunCounter {
    generation: String,
    sequence: u64,
    public_units: u64,
    reasoning_units: u64,
    reasoning_source: &'static str,
    stream_confirmed: bool,
    last_output_at_ms: Option<u64>,
    items: HashMap<String, ItemCursor>,
}

impl RunCounter {
    fn new() -> Self {
        Self {
            generation: Uuid::new_v4().to_string(),
            sequence: 0,
            public_units: 0,
            reasoning_units: 0,
            reasoning_source: "none",
            stream_confirmed: false,
            last_output_at_ms: None,
            items: HashMap::new(),
        }
    }
}

/// One instance belongs to one Core process. Dropping it also drops every estimate.
#[derive(Debug)]
pub struct ObservableOutputCounters {
    started_at: Instant,
    runs: HashMap<(String, i64), RunCounter>,
}

impl Default for ObservableOutputCounters {
    fn default() -> Self {
        Self {
            started_at: Instant::now(),
            runs: HashMap::new(),
        }
    }
}

impl ObservableOutputCounters {
    pub fn observe(&mut self, fragment: Fragment<'_>) {
        if fragment.text.is_empty()
            || fragment.item_id.is_empty()
            || fragment.item_id.len() > MAX_ITEM_ID_BYTES
            || (fragment.offset_utf16.is_none() && fragment.source_sequence.is_none())
        {
            return;
        }
        let key = (fragment.run_id.to_owned(), fragment.execution_epoch);
        if !self.runs.contains_key(&key) {
            if self.runs.len() >= MAX_RUNS {
                return;
            }
            self.runs.insert(key.clone(), RunCounter::new());
        }
        let run = self.runs.get_mut(&key).expect("inserted Run counter");
        if !run.items.contains_key(fragment.item_id) && run.items.len() >= MAX_ITEMS_PER_RUN {
            // A new generation makes truncation visible to the reader. Never silently
            // evict an item and later count its replay as fresh output.
            *run = RunCounter::new();
            return;
        }
        let cursor = run
            .items
            .entry(fragment.item_id.to_owned())
            .or_insert(ItemCursor {
                kind: fragment.kind,
                end_utf16: None,
                last_sequence: None,
                fragments: 0,
            });
        if cursor.kind != fragment.kind {
            return;
        } // raw thought and summary are exclusive per item

        let suffix = if let Some(offset) = fragment.offset_utf16 {
            let Some(end) = offset.checked_add(fragment.text.encode_utf16().count()) else {
                return;
            };
            let previous = cursor.end_utf16;
            if previous.is_some_and(|previous| end <= previous) {
                return;
            }
            if previous.is_some_and(|previous| offset > previous) {
                *run = RunCounter::new();
                run.items.insert(
                    fragment.item_id.to_owned(),
                    ItemCursor {
                        kind: fragment.kind,
                        end_utf16: Some(end),
                        last_sequence: None,
                        fragments: 0,
                    },
                );
                return;
            }
            cursor.end_utf16 = Some(end);
            match previous {
                Some(previous) => match suffix_after_utf16(fragment.text, previous - offset) {
                    Some(suffix) => suffix,
                    None => {
                        *run = RunCounter::new();
                        return;
                    }
                },
                // A newly observed nonzero offset is already in progress or replayed.
                None if offset > 0 => return,
                None => fragment.text,
            }
        } else {
            let sequence = fragment.source_sequence.expect("validated source sequence");
            if cursor
                .last_sequence
                .is_some_and(|previous| sequence <= previous)
            {
                return;
            }
            cursor.last_sequence = Some(sequence);
            fragment.text
        };
        let units = estimate_units(suffix);
        if units == 0 {
            return;
        }
        let total = match fragment.kind {
            OutputKind::PublicText => &mut run.public_units,
            OutputKind::ReasoningText | OutputKind::ReasoningSummary => &mut run.reasoning_units,
        };
        let Some(next) = total.checked_add(units) else {
            *run = RunCounter::new();
            return;
        };
        *total = next;
        cursor.fragments = cursor.fragments.saturating_add(1);
        if cursor.fragments >= 2 {
            run.stream_confirmed = true;
        }
        run.last_output_at_ms = Some(self.started_at.elapsed().as_millis() as u64);
        if fragment.kind == OutputKind::ReasoningText {
            run.reasoning_source = "stream_text";
        } else if fragment.kind == OutputKind::ReasoningSummary && run.reasoning_source == "none" {
            run.reasoning_source = "stream_summary";
        }
    }

    pub fn sample(&mut self, run_id: &str, execution_epoch: i64) -> Option<ObservableOutputSample> {
        let run = self.runs.get_mut(&(run_id.to_owned(), execution_epoch))?;
        run.sequence = run.sequence.saturating_add(1);
        Some(ObservableOutputSample {
            agent_run_id: run_id.to_owned(),
            execution_epoch,
            counter_generation: run.generation.clone(),
            sequence: run.sequence,
            algorithm_version: ALGORITHM_VERSION,
            unicode_data_version: UNICODE_DATA_VERSION,
            sampled_at_ms: self.started_at.elapsed().as_millis() as u64,
            last_output_at_ms: run.last_output_at_ms,
            public_text_units: run.public_units,
            reasoning_units: run.reasoning_units,
            reasoning_source: run.reasoning_source,
            stream_confirmed: run.stream_confirmed,
        })
    }

    pub fn remove(&mut self, run_id: &str, execution_epoch: i64) {
        self.runs.remove(&(run_id.to_owned(), execution_epoch));
    }
}

fn suffix_after_utf16(text: &str, code_units: usize) -> Option<&str> {
    if code_units == 0 {
        return Some(text);
    }
    let mut position = 0;
    for (byte, character) in text.char_indices() {
        if position == code_units {
            return Some(&text[byte..]);
        }
        position += character.len_utf16();
        if position > code_units {
            return None;
        }
    }
    (position == code_units).then_some("")
}

/// Integer units of 0.01 estimated token, counted once per Unicode scalar.
pub fn estimate_units(text: &str) -> u64 {
    let scripts = ScriptWithExtensions::new();
    let categories = CodePointMapData::<GeneralCategory>::new();
    text.chars()
        .map(|character| {
            if ignored(character) {
                return 0;
            }
            if character.is_ascii() {
                return 25;
            }
            let script = scripts.get_script_val(character);
            if script == Script::Han {
                return 60;
            }
            if matches!(script, Script::Hiragana | Script::Katakana | Script::Hangul) {
                return 60;
            }
            if script == Script::Latin {
                return 25;
            }
            // U+30FC and other shared Kana marks are assigned once, before the
            // shared punctuation table. Script_Extensions is membership, not a sum.
            if scripts.has_script(character, Script::Hiragana)
                || scripts.has_script(character, Script::Katakana)
            {
                return 60;
            }
            if cjk_punctuation(character) {
                return 60;
            }
            if matches!(
                categories.get(character),
                GeneralCategory::OtherSymbol
                    | GeneralCategory::MathSymbol
                    | GeneralCategory::CurrencySymbol
                    | GeneralCategory::ModifierSymbol
            ) {
                return 100;
            }
            50
        })
        .sum()
}

fn ignored(character: char) -> bool {
    character.is_whitespace()
        || matches!(
            character,
            '\u{200b}'..='\u{200f}' | '\u{2060}' | '\u{feff}' | '\u{fe0e}' | '\u{fe0f}'
        )
}

fn cjk_punctuation(character: char) -> bool {
    matches!(
        character,
        '。' | '、'
            | '，'
            | '．'
            | '？'
            | '！'
            | '：'
            | '；'
            | '「'
            | '」'
            | '『'
            | '』'
            | '（'
            | '）'
            | '【'
            | '】'
            | '《'
            | '》'
            | '〈'
            | '〉'
            | '・'
            | '…'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_classification_has_one_weight_per_scalar() {
        assert_eq!(estimate_units("abc 123!"), 175);
        assert_eq!(estimate_units("汉𠀀かなー한글"), 420);
        assert_eq!(estimate_units("é🙂\u{200d}💻"), 225);
        assert_eq!(estimate_units(" \t\n\u{fe0f}"), 0);
        assert_eq!(estimate_units("，."), 85);
        for text in [
            "English and code: let x = 42;",
            "中文，English 🙂",
            "かなー한글",
            "{\"ok\":true}",
            "## Markdown\n- item",
            " \t\n",
            "👩‍💻𠀀",
        ] {
            let expected = estimate_units(text);
            let mut counter = ObservableOutputCounters::default();
            let mut offset = 0;
            for character in text.chars() {
                let segment = character.to_string();
                let fragment = Fragment {
                    run_id: "partition",
                    execution_epoch: 1,
                    kind: OutputKind::PublicText,
                    item_id: "body",
                    offset_utf16: Some(offset),
                    source_sequence: None,
                    text: &segment,
                };
                counter.observe(fragment);
                counter.observe(fragment); // same offset is replay, not fresh text
                offset += character.len_utf16();
            }
            assert_eq!(
                counter.sample("partition", 1).unwrap().public_text_units,
                expected,
                "{text}"
            );
        }
    }

    #[test]
    fn overlap_replay_and_generation_reset_keep_content_out_of_samples() {
        let mut counter = ObservableOutputCounters::default();
        let fragment = |offset, text| Fragment {
            run_id: "run",
            execution_epoch: 2,
            kind: OutputKind::PublicText,
            item_id: "body",
            offset_utf16: Some(offset),
            source_sequence: None,
            text,
        };
        counter.observe(fragment(0, "A🙂"));
        counter.observe(fragment(1, "🙂B"));
        counter.observe(fragment(1, "🙂B"));
        let first = counter.sample("run", 2).unwrap();
        assert_eq!(first.public_text_units, 150);
        assert!(first.stream_confirmed);
        assert_eq!(first.reasoning_units, 0);
        assert!(!serde_json::to_string(&first).unwrap().contains("🙂"));
        counter.observe(fragment(8, "gap"));
        let next = counter.sample("run", 2).unwrap();
        assert_ne!(next.counter_generation, first.counter_generation);
        assert_eq!(next.public_text_units, 0);
    }

    #[test]
    fn reasoning_modes_are_exclusive_and_native_sequences_dedupe() {
        let mut counter = ObservableOutputCounters::default();
        let fragment = |kind, sequence, text| Fragment {
            run_id: "run",
            execution_epoch: 1,
            kind,
            item_id: "reasoning-1",
            offset_utf16: None,
            source_sequence: Some(sequence),
            text,
        };
        counter.observe(fragment(OutputKind::ReasoningSummary, 5, "考虑"));
        counter.observe(fragment(OutputKind::ReasoningSummary, 5, "考虑"));
        counter.observe(fragment(OutputKind::ReasoningText, 6, "完整思考"));
        let sample = counter.sample("run", 1).unwrap();
        assert_eq!(sample.reasoning_units, 120);
        assert_eq!(sample.reasoning_source, "stream_summary");
        assert!(!sample.stream_confirmed);
        assert!(counter.sample("run", 2).is_none());
    }
}

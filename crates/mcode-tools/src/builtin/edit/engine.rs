//! Private planning and apply engine for [`super::EditTool`].

// Rust guideline compliant 2026-08-27.

use aho_corasick::{AhoCorasick, MatchKind};
use memchr::memmem::Finder;
use regex::RegexBuilder;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::{
    EditArgs, EditOp, MAX_CAPTURE_BYTES, MAX_DIFF_SNIPPET, MAX_DIFF_SUMMARY_BYTES,
    MAX_LITERAL_PATTERNS, MAX_MATCHES, MAX_OPERATIONS, Occurrence, REGEX_NEST_LIMIT, UTF8_BOM,
};
use crate::builtin::fs_io::MAX_WRITE_BYTES;
use crate::builtin::fs_search::{MAX_PATTERN_BYTES, REGEX_DFA_SIZE_LIMIT, REGEX_SIZE_LIMIT};
use crate::tool::{ToolError, ToolResult};

pub(super) fn check_cancel(cancel: &CancellationToken) -> Result<(), ToolError> {
    if cancel.is_cancelled() {
        Err(ToolError::Execution(
            "file operation cancelled before completion".to_owned(),
        ))
    } else {
        Ok(())
    }
}

pub(super) fn edit_result(
    path_key: &str,
    revision: &str,
    applied: &Applied,
    detached_hardlink: bool,
) -> ToolResult {
    let mut text = format!(
        "Edited {path_key}: {} replacement{}, {} → {} bytes",
        applied.replacements,
        if applied.replacements == 1 { "" } else { "s" },
        applied.bytes_before,
        applied.bytes_after,
    );
    if detached_hardlink {
        text.push_str(" (detached_hardlink=true: this directory entry now names a new inode)");
    }
    text.push_str(&format!("\n[revision {revision}]"));
    ToolResult::text(text).with_details(json!({
        "path": path_key,
        "replacements": applied.replacements,
        "bytes_before": applied.bytes_before,
        "bytes_after": applied.bytes_after,
        "revision": revision,
        "detached_hardlink": detached_hardlink,
        "diff": applied.diff,
    }))
}

pub(super) enum PreparedOp {
    Literal {
        patterns: Vec<String>,
        replacements: Vec<String>,
        pick: Pick,
    },
    Regex {
        compiled: regex::Regex,
        replacement: String,
        pick: Pick,
    },
    LineRange {
        start_line: usize,
        end_line: usize,
        expected_text: Option<String>,
        expected_hash: Option<String>,
        replacement: String,
    },
}

#[derive(Clone, Copy)]
pub(super) enum Pick {
    Unique,
    All,
    Nth(usize),
}

pub(super) struct Planned {
    start: usize,
    end: usize,
    replacement: String,
}

pub(super) struct Applied {
    pub(super) text: String,
    pub(super) replacements: usize,
    pub(super) bytes_before: usize,
    pub(super) bytes_after: usize,
    pub(super) diff: String,
}

pub(super) fn normalize_args(args: &EditArgs) -> Result<Vec<PreparedOp>, ToolError> {
    match (&args.operations, &args.old_string, &args.new_string) {
        (Some(_), Some(_), _) | (Some(_), _, Some(_)) => Err(ToolError::InvalidArgs(
            "cannot combine old_string/new_string with operations".to_owned(),
        )),
        (Some(operations), None, None) => {
            if operations.is_empty() {
                return Err(ToolError::InvalidArgs(
                    "operations must not be empty".to_owned(),
                ));
            }
            if operations.len() > MAX_OPERATIONS {
                return Err(ToolError::InvalidArgs(format!(
                    "at most {MAX_OPERATIONS} operations are allowed"
                )));
            }
            operations.iter().map(prepare_op).collect()
        }
        (None, Some(old), Some(new)) => {
            if old.is_empty() {
                return Err(ToolError::InvalidArgs(
                    "old_string must not be empty; provide the text to replace".into(),
                ));
            }
            bound_pattern("old_string", old)?;
            bound_pattern("new_string", new)?;
            Ok(vec![PreparedOp::Literal {
                patterns: vec![old.clone()],
                replacements: vec![new.clone()],
                pick: Pick::Unique,
            }])
        }
        (None, Some(_), None) => Err(ToolError::InvalidArgs(
            "new_string is required when old_string is set".to_owned(),
        )),
        (None, None, Some(_)) => Err(ToolError::InvalidArgs(
            "old_string is required when new_string is set".to_owned(),
        )),
        (None, None, None) => Err(ToolError::InvalidArgs(
            "provide old_string/new_string or operations".to_owned(),
        )),
    }
}

fn bound_pattern(label: &str, value: &str) -> Result<(), ToolError> {
    if value.len() > MAX_PATTERN_BYTES {
        Err(ToolError::InvalidArgs(format!(
            "{label} exceeds {MAX_PATTERN_BYTES} bytes"
        )))
    } else {
        Ok(())
    }
}

fn prepare_op(op: &EditOp) -> Result<PreparedOp, ToolError> {
    match op {
        EditOp::Literal {
            pattern,
            replacement,
            patterns,
            replacements,
            occurrence,
            n,
        } => {
            let pick = pick_from(*occurrence, *n, true)?;
            let (patterns, replacements) = literal_needles(
                pattern.as_deref(),
                replacement.as_deref(),
                patterns.as_deref(),
                replacements.as_deref(),
            )?;
            Ok(PreparedOp::Literal {
                patterns,
                replacements,
                pick,
            })
        }
        EditOp::Regex {
            pattern,
            replacement,
            occurrence,
            n,
        } => {
            if matches!(occurrence, Occurrence::Nth) || n.is_some() {
                return Err(ToolError::InvalidArgs(
                    "regex operations support occurrence unique or all, not nth".to_owned(),
                ));
            }
            let pick = pick_from(*occurrence, None, false)?;
            bound_pattern("regex pattern", pattern)?;
            bound_pattern("regex replacement", replacement)?;
            if pattern.is_empty() {
                return Err(ToolError::InvalidArgs(
                    "regex pattern must not be empty".to_owned(),
                ));
            }
            let compiled = RegexBuilder::new(pattern)
                .size_limit(REGEX_SIZE_LIMIT)
                .dfa_size_limit(REGEX_DFA_SIZE_LIMIT)
                .nest_limit(REGEX_NEST_LIMIT)
                .build()
                .map_err(|error| {
                    ToolError::InvalidArgs(format!(
                        "invalid regex (lookaround and pattern backreferences are not supported): {error}"
                    ))
                })?;
            Ok(PreparedOp::Regex {
                compiled,
                replacement: replacement.clone(),
                pick,
            })
        }
        EditOp::LineRange {
            start_line,
            end_line,
            expected_text,
            expected_hash,
            replacement,
        } => {
            if *start_line < 1 || *end_line < *start_line {
                return Err(ToolError::InvalidArgs(
                    "line_range requires 1-based start_line <= end_line".to_owned(),
                ));
            }
            if expected_text.is_none() && expected_hash.is_none() {
                return Err(ToolError::InvalidArgs(
                    "line_range requires expected_text and/or expected_hash".to_owned(),
                ));
            }
            if let Some(text) = expected_text {
                bound_pattern("expected_text", text)?;
            }
            bound_pattern("line_range replacement", replacement)?;
            Ok(PreparedOp::LineRange {
                start_line: *start_line,
                end_line: *end_line,
                expected_text: expected_text.clone(),
                expected_hash: expected_hash.clone(),
                replacement: replacement.clone(),
            })
        }
    }
}

fn pick_from(occurrence: Occurrence, n: Option<u32>, nth_ok: bool) -> Result<Pick, ToolError> {
    match occurrence {
        Occurrence::Unique => {
            if n.is_some() {
                return Err(ToolError::InvalidArgs(
                    "n is only valid with occurrence=nth".to_owned(),
                ));
            }
            Ok(Pick::Unique)
        }
        Occurrence::All => {
            if n.is_some() {
                return Err(ToolError::InvalidArgs(
                    "n is only valid with occurrence=nth".to_owned(),
                ));
            }
            Ok(Pick::All)
        }
        Occurrence::Nth => {
            if !nth_ok {
                return Err(ToolError::InvalidArgs(
                    "nth is not supported for this operation".to_owned(),
                ));
            }
            let n = n.ok_or_else(|| {
                ToolError::InvalidArgs("occurrence=nth requires a 1-based n".to_owned())
            })?;
            if n < 1 {
                return Err(ToolError::InvalidArgs("n must be >= 1".to_owned()));
            }
            Ok(Pick::Nth(n as usize))
        }
    }
}

fn literal_needles(
    pattern: Option<&str>,
    replacement: Option<&str>,
    patterns: Option<&[String]>,
    replacements: Option<&[String]>,
) -> Result<(Vec<String>, Vec<String>), ToolError> {
    match (pattern, replacement, patterns, replacements) {
        (Some(pattern), Some(replacement), None, None) => {
            if pattern.is_empty() {
                return Err(ToolError::InvalidArgs(
                    "literal pattern must not be empty".to_owned(),
                ));
            }
            bound_pattern("literal pattern", pattern)?;
            bound_pattern("literal replacement", replacement)?;
            Ok((vec![pattern.to_owned()], vec![replacement.to_owned()]))
        }
        (None, None, Some(patterns), Some(replacements)) => {
            if patterns.is_empty() {
                return Err(ToolError::InvalidArgs(
                    "literal patterns must not be empty".to_owned(),
                ));
            }
            if patterns.len() > MAX_LITERAL_PATTERNS {
                return Err(ToolError::InvalidArgs(format!(
                    "at most {MAX_LITERAL_PATTERNS} literal patterns are allowed"
                )));
            }
            if patterns.len() != replacements.len() {
                return Err(ToolError::InvalidArgs(
                    "patterns and replacements must have the same length".to_owned(),
                ));
            }
            for (index, needle) in patterns.iter().enumerate() {
                if needle.is_empty() {
                    return Err(ToolError::InvalidArgs(
                        "literal pattern must not be empty".to_owned(),
                    ));
                }
                bound_pattern("literal pattern", needle)?;
                bound_pattern("literal replacement", &replacements[index])?;
            }
            Ok((patterns.to_vec(), replacements.to_vec()))
        }
        _ => Err(ToolError::InvalidArgs(
            "literal operations require pattern+replacement or patterns+replacements".to_owned(),
        )),
    }
}

struct Found {
    start: usize,
    end: usize,
    pattern_id: usize,
}

pub(super) fn plan_edits(
    snapshot: &str,
    ops: &[PreparedOp],
    cancel: &CancellationToken,
) -> Result<Vec<Planned>, ToolError> {
    let (had_bom, body) = strip_bom(snapshot);
    let bom_len = if had_bom { UTF8_BOM.len() } else { 0 };
    let mut planned = Vec::new();
    let mut replacement_bytes = 0usize;
    for op in ops {
        check_cancel(cancel)?;
        let found = match op {
            PreparedOp::Literal {
                patterns,
                replacements,
                pick,
            } => {
                let matches = literal_matches(body, patterns)?;
                let selected = select_matches(matches, *pick, "literal")?;
                selected
                    .into_iter()
                    .map(|found| Planned {
                        start: found.start + bom_len,
                        end: found.end + bom_len,
                        replacement: replacements[found.pattern_id].clone(),
                    })
                    .collect::<Vec<_>>()
            }
            PreparedOp::Regex {
                compiled,
                replacement,
                pick,
            } => regex_planned(body, compiled, replacement, *pick, bom_len)?,
            PreparedOp::LineRange {
                start_line,
                end_line,
                expected_text,
                expected_hash,
                replacement,
            } => {
                vec![line_range_planned(
                    body,
                    *start_line,
                    *end_line,
                    expected_text.as_deref(),
                    expected_hash.as_deref(),
                    replacement,
                    bom_len,
                )?]
            }
        };
        replacement_bytes =
            replacement_bytes.saturating_add(found.iter().map(|p| p.replacement.len()).sum());
        if replacement_bytes > MAX_WRITE_BYTES {
            return Err(ToolError::InvalidArgs(format!(
                "planned replacements exceed {MAX_WRITE_BYTES} bytes"
            )));
        }
        if planned.len() + found.len() > MAX_MATCHES {
            return Err(ToolError::InvalidArgs(format!(
                "edit produced more than {MAX_MATCHES} matches"
            )));
        }
        planned.extend(found);
    }
    Ok(planned)
}

fn strip_bom(text: &str) -> (bool, &str) {
    if let Some(rest) = text.strip_prefix(UTF8_BOM) {
        (true, rest)
    } else {
        (false, text)
    }
}

fn literal_matches(body: &str, patterns: &[String]) -> Result<Vec<Found>, ToolError> {
    if patterns.len() == 1 {
        memmem_matches(body, &patterns[0])
    } else {
        ac_matches(body, patterns)
    }
}

fn memmem_matches(body: &str, pattern: &str) -> Result<Vec<Found>, ToolError> {
    let needle = pattern.as_bytes();
    let haystack = body.as_bytes();
    let finder = Finder::new(needle);
    let mut found = Vec::new();
    for start in finder.find_iter(haystack) {
        if found.len() >= MAX_MATCHES {
            return Err(ToolError::InvalidArgs(format!(
                "edit produced more than {MAX_MATCHES} matches"
            )));
        }
        let end = start + needle.len();
        found.push(Found {
            start,
            end,
            pattern_id: 0,
        });
    }
    Ok(found)
}

fn ac_matches(body: &str, patterns: &[String]) -> Result<Vec<Found>, ToolError> {
    let ac = AhoCorasick::builder()
        .match_kind(MatchKind::Standard)
        .build(patterns)
        .map_err(|error| ToolError::InvalidArgs(format!("invalid literal patterns: {error}")))?;
    let mut found = Vec::new();
    for mat in ac.find_overlapping_iter(body) {
        if found.len() >= MAX_MATCHES {
            return Err(ToolError::InvalidArgs(format!(
                "edit produced more than {MAX_MATCHES} matches"
            )));
        }
        found.push(Found {
            start: mat.start(),
            end: mat.end(),
            pattern_id: mat.pattern().as_usize(),
        });
    }
    Ok(found)
}

fn select_matches(matches: Vec<Found>, pick: Pick, label: &str) -> Result<Vec<Found>, ToolError> {
    match pick {
        Pick::Unique => match matches.len() {
            0 => Err(ToolError::Execution(format!(
                "{label} pattern not found; re-read the file and provide the exact text"
            ))),
            1 => Ok(matches),
            n => Err(ToolError::Execution(format!(
                "{label} pattern occurs {n} times; include more surrounding lines to make it unique"
            ))),
        },
        Pick::All => {
            if matches.is_empty() {
                Err(ToolError::Execution(format!(
                    "{label} pattern not found; re-read the file and provide the exact text"
                )))
            } else {
                Ok(matches)
            }
        }
        Pick::Nth(n) => {
            if n > matches.len() {
                Err(ToolError::Execution(format!(
                    "{label} nth={n} requested but only {} match{}",
                    matches.len(),
                    if matches.len() == 1 { "" } else { "es" }
                )))
            } else {
                Ok(vec![matches.into_iter().nth(n - 1).expect("n is in range")])
            }
        }
    }
}

fn regex_planned(
    body: &str,
    compiled: &regex::Regex,
    replacement: &str,
    pick: Pick,
    bom_len: usize,
) -> Result<Vec<Planned>, ToolError> {
    if matches!(pick, Pick::Nth(_)) {
        return Err(ToolError::InvalidArgs(
            "regex operations support occurrence unique or all, not nth".to_owned(),
        ));
    }
    let mut planned = Vec::new();
    let mut replacement_bytes = 0usize;
    for caps in compiled.captures_iter(body) {
        let full = caps
            .get(0)
            .ok_or_else(|| ToolError::Execution("regex match is missing capture 0".to_owned()))?;
        if full.start() == full.end() {
            return Err(ToolError::InvalidArgs(
                "regex matches must not be zero-width".to_owned(),
            ));
        }
        if matches!(pick, Pick::Unique) && !planned.is_empty() {
            return Err(ToolError::Execution(
                "regex pattern occurs 2 times; include more surrounding context to make it unique"
                    .to_owned(),
            ));
        }
        if planned.len() >= MAX_MATCHES {
            return Err(ToolError::InvalidArgs(format!(
                "edit produced more than {MAX_MATCHES} matches"
            )));
        }
        let expanded = expand_captures(&caps, replacement)?;
        replacement_bytes = replacement_bytes.saturating_add(expanded.len());
        if replacement_bytes > MAX_WRITE_BYTES {
            return Err(ToolError::InvalidArgs(format!(
                "planned replacements exceed {MAX_WRITE_BYTES} bytes"
            )));
        }
        planned.push(Planned {
            start: full.start() + bom_len,
            end: full.end() + bom_len,
            replacement: expanded,
        });
    }
    if planned.is_empty() {
        return Err(ToolError::Execution(
            "regex pattern not found; re-read the file and provide the exact text".to_owned(),
        ));
    }
    if matches!(pick, Pick::Unique) && planned.len() != 1 {
        return Err(ToolError::Execution(format!(
            "regex pattern occurs {} times; include more surrounding context to make it unique",
            planned.len()
        )));
    }
    Ok(planned)
}

fn expand_captures(caps: &regex::Captures<'_>, template: &str) -> Result<String, ToolError> {
    let mut out = String::new();
    let bytes = template.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'$' {
            let start = index;
            while index < bytes.len() && bytes[index] != b'$' {
                index += 1;
            }
            push_bounded(&mut out, &template[start..index])?;
            continue;
        }
        if index + 1 < bytes.len() && bytes[index + 1] == b'$' {
            push_bounded(&mut out, "$")?;
            index += 2;
            continue;
        }
        let (consumed, capture) = parse_capture_ref(&bytes[index..], caps)?;
        push_bounded(&mut out, capture)?;
        index += consumed;
    }
    Ok(out)
}

fn parse_capture_ref<'a>(
    rest: &[u8],
    caps: &'a regex::Captures<'a>,
) -> Result<(usize, &'a str), ToolError> {
    if rest.len() >= 2 && rest[1] == b'{' {
        let close = rest
            .iter()
            .position(|byte| *byte == b'}')
            .ok_or_else(|| ToolError::InvalidArgs("unterminated capture replacement".to_owned()))?;
        let name = std::str::from_utf8(&rest[2..close])
            .map_err(|_| ToolError::InvalidArgs("capture name is not UTF-8".to_owned()))?;
        let text = capture_text(caps, name)?;
        return Ok((close + 1, text));
    }
    let mut end = 1usize;
    while end < rest.len() && rest[end].is_ascii_digit() {
        end += 1;
    }
    if end == 1 {
        while end < rest.len() && (rest[end].is_ascii_alphanumeric() || rest[end] == b'_') {
            end += 1;
        }
    }
    if end == 1 {
        return Ok((1, "$"));
    }
    let name = std::str::from_utf8(&rest[1..end])
        .map_err(|_| ToolError::InvalidArgs("capture name is not UTF-8".to_owned()))?;
    Ok((end, capture_text(caps, name)?))
}

fn capture_text<'a>(caps: &'a regex::Captures<'a>, name: &str) -> Result<&'a str, ToolError> {
    if let Ok(index) = name.parse::<usize>() {
        return Ok(caps.get(index).map(|m| m.as_str()).unwrap_or(""));
    }
    Ok(caps.name(name).map(|m| m.as_str()).unwrap_or(""))
}

fn push_bounded(out: &mut String, chunk: &str) -> Result<(), ToolError> {
    if out.len().saturating_add(chunk.len()) > MAX_CAPTURE_BYTES {
        return Err(ToolError::InvalidArgs(format!(
            "capture replacement exceeds {MAX_CAPTURE_BYTES} bytes"
        )));
    }
    out.push_str(chunk);
    Ok(())
}

fn line_range_planned(
    body: &str,
    start_line: usize,
    end_line: usize,
    expected_text: Option<&str>,
    expected_hash: Option<&str>,
    replacement: &str,
    bom_len: usize,
) -> Result<Planned, ToolError> {
    let lines = line_spans(body);
    if start_line > lines.len() || end_line > lines.len() {
        return Err(ToolError::Execution(format!(
            "line_range {start_line}-{end_line} is outside the file ({} lines)",
            lines.len()
        )));
    }
    let start = lines[start_line - 1].start;
    let end = lines[end_line - 1].end;
    let range = &body[start..end];
    if let Some(expected) = expected_text
        && range != expected
    {
        return Err(ToolError::Execution(
            "line range does not match expected_text; re-read the file".to_owned(),
        ));
    }
    if let Some(expected) = expected_hash {
        let actual = blake3::hash(range.as_bytes()).to_hex();
        if !hash_eq(expected, actual.as_str()) {
            return Err(ToolError::Execution(
                "line range does not match expected_hash; re-read the file".to_owned(),
            ));
        }
    }
    Ok(Planned {
        start: start + bom_len,
        end: end + bom_len,
        replacement: replacement.to_owned(),
    })
}

struct LineSpan {
    start: usize,
    end: usize,
}

fn line_spans(text: &str) -> Vec<LineSpan> {
    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'\r' && i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
            lines.push(LineSpan { start, end: i + 2 });
            i += 2;
            start = i;
        } else if bytes[i] == b'\n' || bytes[i] == b'\r' {
            lines.push(LineSpan { start, end: i + 1 });
            i += 1;
            start = i;
        } else {
            i += 1;
        }
    }
    if start < bytes.len() || lines.is_empty() {
        lines.push(LineSpan {
            start,
            end: bytes.len(),
        });
    }
    lines
}

fn hash_eq(expected: &str, actual_hex: &str) -> bool {
    let expected = expected.strip_prefix("blake3:").unwrap_or(expected).trim();
    expected.eq_ignore_ascii_case(actual_hex)
}

pub(super) fn apply_planned(
    snapshot: &str,
    planned: &[Planned],
    cancel: &CancellationToken,
) -> Result<Applied, ToolError> {
    check_cancel(cancel)?;
    let mut ordered: Vec<&Planned> = planned.iter().collect();
    ordered.sort_by_key(|item| (item.start, item.end));
    for item in &ordered {
        if item.start > item.end
            || item.end > snapshot.len()
            || !snapshot.is_char_boundary(item.start)
            || !snapshot.is_char_boundary(item.end)
        {
            return Err(ToolError::Execution(
                "edit match is not on a UTF-8 character boundary".to_owned(),
            ));
        }
    }
    for pair in ordered.windows(2) {
        if pair[1].start < pair[0].end {
            return Err(ToolError::Execution(
                "edit operations overlap on the snapshot; split them or make ranges unique"
                    .to_owned(),
            ));
        }
        if pair[1].start == pair[0].start {
            return Err(ToolError::Execution(
                "edit operations are order-dependent at the same byte range".to_owned(),
            ));
        }
    }
    let (had_bom, _body) = strip_bom(snapshot);
    let extra: isize = ordered.iter().fold(0isize, |acc, item| {
        acc.saturating_add(item.replacement.len() as isize)
            .saturating_sub((item.end - item.start) as isize)
    });
    let capacity = snapshot
        .len()
        .saturating_add_signed(extra)
        .saturating_add(UTF8_BOM.len());
    if capacity > MAX_WRITE_BYTES {
        return Err(ToolError::InvalidArgs(format!(
            "edited content exceeds {MAX_WRITE_BYTES} bytes"
        )));
    }
    let mut text = String::with_capacity(capacity);
    let mut cursor = 0usize;
    let mut diff = String::new();
    for item in &ordered {
        check_cancel(cancel)?;
        text.push_str(&snapshot[cursor..item.start]);
        let old = &snapshot[item.start..item.end];
        append_diff(&mut diff, old, &item.replacement);
        text.push_str(&item.replacement);
        cursor = item.end;
        if text.len() > MAX_WRITE_BYTES {
            return Err(ToolError::InvalidArgs(format!(
                "edited content exceeds {MAX_WRITE_BYTES} bytes"
            )));
        }
    }
    text.push_str(&snapshot[cursor..]);
    if had_bom && !text.starts_with(UTF8_BOM) {
        text.insert_str(0, UTF8_BOM);
    }
    Ok(Applied {
        replacements: ordered.len(),
        bytes_before: snapshot.len(),
        bytes_after: text.len(),
        diff,
        text,
    })
}

fn append_diff(diff: &mut String, old: &str, new: &str) {
    if old == new || diff.len() >= MAX_DIFF_SUMMARY_BYTES {
        return;
    }
    let old_s = snippet(old);
    let new_s = snippet(new);
    let hunk = format!("- {old_s}\n+ {new_s}\n");
    let remaining = MAX_DIFF_SUMMARY_BYTES.saturating_sub(diff.len());
    if hunk.len() > remaining {
        diff.push_str("[diff truncated]");
        return;
    }
    diff.push_str(&hunk);
}

fn snippet(text: &str) -> String {
    let (cut, truncated) = crate::builtin::truncate_bytes(text, MAX_DIFF_SNIPPET);
    let mut visible = cut.replace('\r', "\\r").replace('\n', "\\n");
    if truncated {
        visible.push('…');
    }
    visible
}

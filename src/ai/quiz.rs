//! Quiz data structures and robust parsing of model-generated quizzes.
//!
//! Models answer a quiz request in one of two shapes:
//!
//! 1. **JSON** — either bare, wrapped in a ```` ```json ```` fence, or embedded
//!    in prose ("Aquí tienes el cuestionario: `{...}`").
//! 2. **Plain text** — numbered questions, lettered options and a
//!    `Correcta: B` line.
//!
//! [`Quiz::parse_model_response`] accepts both and always returns a
//! *normalized* quiz: 4-6 questions, four options per question (A-D), exactly
//! one correct answer, no duplicated options and no duplicated questions.
//!
//! When fewer than [`MIN_QUESTIONS`] questions can be salvaged the parser
//! fails with an honest error instead of returning a broken quiz (an empty
//! quiz, or a question with an empty `Correcta:`).
//!
//! Numeric answers in the *text* format are read as 1-based ("Correcta: 2"
//! means the second option); numeric answers in the *JSON* format are 0-based
//! indexes, matching the documented `correct_answer` contract.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

/// Hard floor: below this many usable questions the parse fails.
pub const MIN_QUESTIONS: usize = 3;

/// Upper bound of questions kept in a normalized quiz.
pub const MAX_QUESTIONS: usize = 6;

/// Number of options per question (labels A-D).
pub const OPTION_COUNT: usize = 4;

/// Lower bound of the normalized question count (target band is 4..=6).
pub const MIN_TARGET_QUESTIONS: usize = 4;

/// Default number of questions requested from the model.
pub const DEFAULT_QUESTION_COUNT: usize = 5;

/// A generated quiz with title and questions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Quiz {
    pub title: String,
    pub questions: Vec<QuizQuestion>,
}

/// A single quiz question.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuizQuestion {
    pub question: String,
    pub options: Vec<String>,
    /// Index of the correct answer in `options`
    pub correct_answer: usize,
}

/// Why a model response could not be turned into a usable quiz.
#[derive(Debug, thiserror::Error)]
pub enum QuizParseError {
    #[error("model returned an empty response")]
    Empty,
    #[error("model response contained neither JSON nor a recognizable text quiz")]
    Unrecognized,
    #[error("model response produced {found} usable question(s); at least {MIN_QUESTIONS} are required")]
    TooFewQuestions { found: usize },
}

/// Bridge so callers can propagate a parse failure as an `AiError`.
impl From<QuizParseError> for crate::ai::openrouter::AiError {
    fn from(e: QuizParseError) -> Self {
        crate::ai::openrouter::AiError::Parse(e.to_string())
    }
}

impl Quiz {
    /// Parse a raw model response into a normalized quiz.
    ///
    /// Uses [`DEFAULT_QUESTION_COUNT`] as the target number of questions.
    pub fn parse_model_response(raw: &str) -> Result<Quiz, QuizParseError> {
        Self::parse_model_response_with_target(raw, DEFAULT_QUESTION_COUNT)
    }

    /// Parse a raw model response, keeping at most `target` questions.
    ///
    /// `target` is clamped to 4..=6. JSON is tried first (fenced, bare or
    /// embedded in prose), the plain-text format is the fallback.
    pub fn parse_model_response_with_target(
        raw: &str,
        target: usize,
    ) -> Result<Quiz, QuizParseError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(QuizParseError::Empty);
        }
        let max_questions = target.clamp(MIN_TARGET_QUESTIONS, MAX_QUESTIONS);

        if let Some(quiz) = parse_json_quiz(trimmed) {
            return quiz.normalized(max_questions);
        }
        match parse_text_quiz(trimmed) {
            Some(quiz) => quiz.normalized(max_questions),
            None => Err(QuizParseError::Unrecognized),
        }
    }

    /// Drop malformed/duplicated questions and enforce the quiz invariants.
    fn normalized(self, max_questions: usize) -> Result<Quiz, QuizParseError> {
        let mut questions: Vec<QuizQuestion> = Vec::new();
        let mut seen_questions: HashSet<String> = HashSet::new();

        for raw in self.questions {
            let question = raw.question.trim();
            if question.is_empty() {
                continue;
            }
            // Duplicated questions are dropped.
            if !seen_questions.insert(normalize_key(question)) {
                continue;
            }

            // Deduplicate options while tracking the new index of the answer.
            let mut options: Vec<String> = Vec::with_capacity(OPTION_COUNT);
            let mut seen_options: HashSet<String> = HashSet::new();
            let mut correct: Option<usize> = None;
            for (i, option) in raw.options.iter().enumerate() {
                let text = option.trim();
                if text.is_empty() {
                    continue;
                }
                if !seen_options.insert(normalize_key(text)) {
                    continue;
                }
                if raw.correct_answer == i {
                    correct = Some(options.len());
                }
                options.push(text.to_string());
            }

            // Four options (A-D) are required; a question with fewer cannot be
            // repaired without inventing content, so it is dropped.
            if options.len() < OPTION_COUNT {
                continue;
            }
            if options.len() > OPTION_COUNT {
                match correct {
                    // Keep the first three options plus the correct one.
                    Some(c) if c >= OPTION_COUNT => {
                        options.truncate(OPTION_COUNT - 1);
                        options.push(raw.options[raw.correct_answer].trim().to_string());
                        correct = Some(OPTION_COUNT - 1);
                    }
                    _ => options.truncate(OPTION_COUNT),
                }
            }

            let correct = match correct {
                Some(c) if c < options.len() => c,
                _ => continue, // no usable correct answer
            };

            questions.push(QuizQuestion {
                question: question.to_string(),
                options,
                correct_answer: correct,
            });
            if questions.len() >= max_questions {
                break;
            }
        }

        if questions.len() < MIN_QUESTIONS {
            return Err(QuizParseError::TooFewQuestions {
                found: questions.len(),
            });
        }

        Ok(Quiz {
            title: self.title,
            questions,
        })
    }
}

// ---------------------------------------------------------------------------
// JSON parsing
// ---------------------------------------------------------------------------

/// Parse the JSON shapes a model may use for a quiz.
fn parse_json_quiz(raw: &str) -> Option<Quiz> {
    for candidate in json_candidates(raw) {
        let value: Value = match serde_json::from_str(&candidate) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if let Some(quiz) = quiz_from_json(&value) {
            return Some(quiz);
        }
    }
    None
}

/// Build a quiz from a JSON value (object with a questions array, or a bare
/// array of questions).
fn quiz_from_json(value: &Value) -> Option<Quiz> {
    match value {
        Value::Array(items) => {
            let questions = items.iter().filter_map(question_from_json).collect();
            Some(Quiz {
                title: "Cuestionario".to_string(),
                questions,
            })
        }
        Value::Object(map) => {
            let title = first_string(map, &["title", "titulo", "name", "nombre"])
                .unwrap_or_else(|| "Cuestionario".to_string());
            let items = match first_value(map, &["questions", "preguntas", "items", "quiz"]) {
                Some(Value::Array(arr)) => arr.clone(),
                Some(Value::Object(inner)) => {
                    // Nested shape: {"quiz": {"questions": [...]}}
                    return quiz_from_json(&Value::Object(inner.clone()));
                }
                Some(_) => return Some(Quiz { title, questions: Vec::new() }),
                None => return None,
            };
            let questions = items.iter().filter_map(question_from_json).collect();
            Some(Quiz { title, questions })
        }
        _ => None,
    }
}

/// Build a question from a JSON object, tolerating the usual key aliases.
fn question_from_json(value: &Value) -> Option<QuizQuestion> {
    let map = value.as_object()?;
    let question = first_string(
        map,
        &[
            "question",
            "pregunta",
            "text",
            "enunciado",
            "prompt",
            "statement",
        ],
    )?;
    let options = json_options(first_value(
        map,
        &[
            "options",
            "choices",
            "opciones",
            "answers",
            "alternatives",
            "alternativas",
        ],
    )?)?;
    if options.len() < 2 {
        return None;
    }
    let correct_raw = first_value(
        map,
        &[
            "correct_answer",
            "correctAnswer",
            "answer",
            "correct",
            "correcta",
            "respuesta",
            "solucion",
            "solution",
            "correct_option",
            "correct_index",
        ],
    )?;
    let correct = resolve_json_answer(correct_raw, &options)?;
    Some(QuizQuestion {
        question,
        options,
        correct_answer: correct,
    })
}

/// Extract the option texts of a JSON value.
fn json_options(value: &Value) -> Option<Vec<String>> {
    let arr = value.as_array()?;
    let mut options = Vec::with_capacity(arr.len());
    for item in arr {
        let text = match item {
            Value::String(s) => s.clone(),
            Value::Object(m) => first_string(m, &["text", "label", "option", "opcion", "value"])?,
            _ => continue,
        };
        options.push(text);
    }
    Some(options)
}

/// Resolve the JSON answer into an index into `options`.
///
/// Numbers are 0-based indexes (the documented `correct_answer` contract);
/// strings may be a letter ("B"), a number or the option text itself.
fn resolve_json_answer(raw: &Value, options: &[String]) -> Option<usize> {
    match raw {
        Value::Number(n) => {
            let idx = n.as_i64()?;
            if idx < 0 {
                return None;
            }
            let idx = idx as usize;
            if idx < options.len() {
                Some(idx)
            } else {
                None
            }
        }
        Value::String(s) => resolve_answer(s, options, &vec![None; options.len()], 0),
        Value::Array(items) => items.first().and_then(|v| resolve_json_answer(v, options)),
        Value::Object(map) => {
            if let Some(v) = first_value(
                map,
                &["index", "correct_answer", "correctAnswer", "answer", "position"],
            ) {
                return resolve_json_answer(v, options);
            }
            if let Some(s) = first_string(map, &["text", "option", "opcion", "value", "letter"]) {
                return resolve_answer(&s, options, &vec![None; options.len()], 0);
            }
            None
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Plain-text parsing
// ---------------------------------------------------------------------------

/// A question while it is being read from the text format.
struct TextQuestion {
    question: String,
    options: Vec<(Option<char>, String)>,
    correct_raw: Option<String>,
}

/// Parse the plain-text quiz format (numbered questions, lettered options,
/// `Correcta: B`).
fn parse_text_quiz(raw: &str) -> Option<Quiz> {
    let lines: Vec<&str> = raw.lines().collect();
    let mut questions: Vec<QuizQuestion> = Vec::new();
    let mut title = String::new();
    let mut current: Option<TextQuestion> = None;
    let mut title_taken = false;
    let mut saw_question = false;
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i].trim();
        if line.is_empty() {
            i += 1;
            continue;
        }

        if let Some(value) = parse_correct_line(line) {
            let mut value = value;
            if value.is_empty() {
                // The answer may sit alone on the following line.
                if let Some(next) = lines.get(i + 1) {
                    let candidate = next.trim();
                    if is_bare_answer(candidate) {
                        value = candidate.to_string();
                        i += 1;
                    }
                }
            }
            if let Some(q) = current.as_mut() {
                q.correct_raw = Some(value);
            }
            i += 1;
            continue;
        }

        if let Some((label, text)) = parse_option_line(line) {
            if let Some(q) = current.as_mut() {
                q.options.push((label, text));
            }
            i += 1;
            continue;
        }

        if let Some(text) = parse_question_line(line) {
            if let Some(q) = current.take() {
                if let Some(finished) = finish_text_question(q) {
                    questions.push(finished);
                }
            }
            saw_question = true;
            current = Some(TextQuestion {
                question: text,
                options: Vec::new(),
                correct_raw: None,
            });
            i += 1;
            continue;
        }

        if !title_taken && current.is_none() && questions.is_empty() {
            let candidate = clean_title(line);
            if !candidate.is_empty() {
                title = candidate;
                title_taken = true;
                i += 1;
                continue;
            }
        }

        // Continuation of the question text (only before the first option).
        if let Some(q) = current.as_mut() {
            if q.options.is_empty() {
                let extra = strip_emphasis(line).trim().to_string();
                if !extra.is_empty() {
                    if !q.question.is_empty() {
                        q.question.push(' ');
                    }
                    q.question.push_str(&extra);
                }
            }
        }
        i += 1;
    }

    if let Some(q) = current.take() {
        if let Some(finished) = finish_text_question(q) {
            questions.push(finished);
        }
    }

    if questions.is_empty() && !saw_question {
        // No quiz structure at all.
        return None;
    }
    if title.is_empty() {
        title = "Cuestionario".to_string();
    }
    Some(Quiz { title, questions })
}

/// Turn a parsed text question into a `QuizQuestion` (None if unusable).
fn finish_text_question(q: TextQuestion) -> Option<QuizQuestion> {
    let question = q.question.trim().to_string();
    if question.is_empty() {
        return None;
    }
    let mut options: Vec<String> = Vec::new();
    let mut labels: Vec<Option<char>> = Vec::new();
    for (label, text) in &q.options {
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        options.push(text.to_string());
        labels.push(*label);
    }
    if options.len() < 2 {
        return None;
    }
    // A missing or empty "Correcta:" yields no question at all.
    let correct_raw = q.correct_raw.unwrap_or_default();
    let correct = resolve_answer(&correct_raw, &options, &labels, 1)?;
    Some(QuizQuestion {
        question,
        options,
        correct_answer: correct,
    })
}

/// Detect a "Correcta: B" style line and return the raw answer value.
fn parse_correct_line(line: &str) -> Option<String> {
    const KEYWORDS: [&str; 7] = [
        "respuesta correcta",
        "correcta",
        "respuesta",
        "correct answer",
        "correct",
        "answer",
        "solucion",
    ];
    let cleaned = strip_emphasis(line);
    let t = cleaned.trim();
    for keyword in KEYWORDS {
        if let Some(rest) = strip_keyword_prefix(t, keyword) {
            let rest = rest.trim_start();
            // The keyword must be followed by a separator (or nothing).
            if !rest.is_empty() && !rest.starts_with([' ', ':', '-', '=', ')', '.', ',']) {
                continue;
            }
            let value = rest
                .trim_start_matches([' ', ':', '-', '=', ')', '.', ',', '*'])
                .trim()
                .to_string();
            if value.len() <= 60 {
                return Some(value);
            }
        }
    }
    None
}

/// Detect an option line ("A) text", "B. text", "C: text", "(D) text").
fn parse_option_line(line: &str) -> Option<(Option<char>, String)> {
    let cleaned = strip_emphasis(line);
    let mut t = cleaned.trim();
    // Strip leading list bullets ("- ", "* ", "• ").
    t = t
        .trim_start_matches(['-', '*', '\u{2022}', '\u{00b7}'])
        .trim_start();

    // "(A) text"
    if let Some(rest) = t.strip_prefix('(') {
        let mut chars = rest.chars();
        if let Some(c) = chars.next() {
            if c.is_ascii_alphabetic() {
                let after = &rest[c.len_utf8()..];
                if after.starts_with(')') {
                    let letter = c.to_ascii_uppercase();
                    let text = after[1..].trim_start_matches(['.', ')', ':', '-']).trim().to_string();
                    return Some((Some(letter), text));
                }
            }
        }
    }

    // "A) text" / "A. text" / "A: text" / "A - text"
    let mut chars = t.chars();
    let c = chars.next()?;
    if !c.is_ascii_alphabetic() {
        return None;
    }
    let rest = &t[c.len_utf8()..];
    let rest = rest.trim_start();
    let sep = rest.chars().next()?;
    if !matches!(sep, '.' | ')' | ':' | '-') {
        return None;
    }
    let text = rest[1..].trim().to_string();
    Some((Some(c.to_ascii_uppercase()), text))
}

/// Detect a question line ("Pregunta 1: text", "1. text", "2) text").
fn parse_question_line(line: &str) -> Option<String> {
    let cleaned = strip_emphasis(line);
    let t = cleaned.trim();
    if t.is_empty() {
        return None;
    }

    // "Pregunta N: text" / "Pregunta: text" / "Preguntas N: text"
    if let Some(rest) = strip_keyword_prefix(t, "pregunta") {
        let mut rest = rest.trim_start();
        if let Some(tail) = rest.strip_prefix('s') {
            if !tail.chars().next().map(|c| c.is_alphabetic()).unwrap_or(false) {
                rest = tail.trim_start();
            }
        }
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        let text = if digits.is_empty() {
            rest.trim_start_matches([':', '.', ')', '-']).trim().to_string()
        } else {
            rest[digits.len()..]
                .trim_start()
                .trim_start_matches(['.', ')', ':', '-', '\u{2013}', '\u{2014}'])
                .trim()
                .to_string()
        };
        return Some(text);
    }

    // "N. text" / "N) text" / "N: text"
    let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() && digits.len() <= 2 {
        let tail = t[digits.len()..].trim_start();
        let sep = tail.chars().next()?;
        if matches!(sep, '.' | ')' | ':') {
            let text = tail[1..].trim_start_matches(['-', '\u{2013}']).trim().to_string();
            return Some(text);
        }
    }
    None
}

/// Is this line a bare answer token (a single letter or a number)?
fn is_bare_answer(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() || t.chars().count() > 2 {
        return false;
    }
    t.chars()
        .all(|c| c.is_ascii_alphanumeric())
        && t.chars().next().map(|c| c.is_ascii_alphabetic() || c.is_ascii_digit()) == Some(true)
}

/// Resolve an answer token into an index into `options`.
///
/// `numeric_base` is 1 for the text format ("Correcta: 2" = second option)
/// and 0 for JSON (0-based index).
fn resolve_answer(
    raw: &str,
    options: &[String],
    labels: &[Option<char>],
    numeric_base: usize,
) -> Option<usize> {
    let cleaned = strip_emphasis(raw);
    let mut t = cleaned.trim().to_string();
    if t.is_empty() {
        return None;
    }

    // Drop a leading "Opción"/"Respuesta" style prefix.
    loop {
        let lower = t.to_lowercase();
        let mut stripped = None;
        for prefix in [
            "opcion",
            "respuesta",
            "correcta",
            "solucion",
            "answer",
            "correct",
            "letra",
        ] {
            if lower.starts_with(prefix) {
                let rest = t[prefix.len()..].trim_start();
                if !rest.is_empty() {
                    let sep = rest.chars().next().unwrap_or(' ');
                    if matches!(sep, ' ' | ':' | '-' | '=' | ')' | '.') {
                        stripped = Some(rest);
                        break;
                    }
                }
            }
        }
        match stripped {
            Some(rest) => {
                t = rest
                    .trim_start_matches([' ', ':', '-', '=', ')', '.', '*'])
                    .trim()
                    .to_string();
            }
            None => break,
        }
    }
    if t.is_empty() {
        return None;
    }

    // 1) Letter answer ("B", "B)", "Opción B"). The explicit option label
    // wins over the position, so out-of-order options are still resolved.
    if let Some(idx) = answer_letter(&t) {
        let letter = (b'A' + idx as u8) as char;
        if let Some(pos) = labels.iter().position(|l| *l == Some(letter)) {
            return Some(pos);
        }
        if idx < options.len() {
            return Some(idx);
        }
    }

    // 2) Numeric answer.
    let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() {
        let tail = t[digits.len()..].trim();
        if tail.is_empty() || tail.starts_with([')', '.', ':', '-', ',']) {
            let n: usize = digits.parse().ok()?;
            let idx = if numeric_base == 1 { n.checked_sub(1)? } else { n };
            if idx < options.len() {
                return Some(idx);
            }
            return None;
        }
    }

    // 3) Match the option text.
    let key = normalize_key(&t);
    if !key.is_empty() {
        let exact: Vec<usize> = options
            .iter()
            .enumerate()
            .filter(|(_, o)| normalize_key(o) == key)
            .map(|(i, _)| i)
            .collect();
        if exact.len() == 1 {
            return Some(exact[0]);
        }
        if exact.len() > 1 {
            return None; // ambiguous answer
        }
        let contains: Vec<usize> = options
            .iter()
            .enumerate()
            .filter(|(_, o)| normalize_key(o).contains(&key))
            .map(|(i, _)| i)
            .collect();
        if contains.len() == 1 {
            return Some(contains[0]);
        }
    }
    None
}

/// Extract the option index from a letter answer ("B", "B)", "B:").
fn answer_letter(t: &str) -> Option<usize> {
    let mut chars = t.chars();
    let c = chars.next()?;
    if !c.is_ascii_alphabetic() {
        return None;
    }
    let rest = t[c.len_utf8()..].trim_start();
    if !(rest.is_empty() || rest.starts_with([')', '.', ':', '-', ',', ']', '*'])) {
        return None;
    }
    Some((c.to_ascii_uppercase() as u8 - b'A') as usize)
}

// ---------------------------------------------------------------------------
// JSON candidate extraction
// ---------------------------------------------------------------------------

/// Candidate JSON payloads found in a raw model response.
fn json_candidates(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for block in fenced_blocks(raw) {
        if !out.contains(&block) {
            out.push(block);
        }
    }
    for slice in balanced_slices(raw) {
        if !out.contains(&slice) {
            out.push(slice);
        }
    }
    out
}

/// Contents of the ```` ``` ```` fenced code blocks of a response.
fn fenced_blocks(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut search_from = 0usize;
    while let Some(pos) = raw[search_from..].find("```") {
        let content_start = search_from + pos + 3;
        let rest = &raw[content_start..];
        // Skip an optional language tag (e.g. "json").
        let tag_end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '+'))
            .unwrap_or(rest.len());
        let body_start = if rest[tag_end..].starts_with('\n') {
            content_start + tag_end + 1
        } else {
            content_start + tag_end
        };
        match rest.find("```") {
            Some(end) => {
                let block = raw[body_start..content_start + end].trim().to_string();
                if !block.is_empty() {
                    out.push(block);
                }
                search_from = content_start + end + 3;
            }
            None => break,
        }
    }
    out
}

/// Balanced `{...}` / `[...]` substrings of a response.
fn balanced_slices(raw: &str) -> Vec<String> {
    let chars: Vec<char> = raw.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '{' || chars[i] == '[' {
            if let Some(end) = match_balanced(&chars, i) {
                out.push(chars[i..=end].iter().collect());
                i = end + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// End index of the balanced block that starts at `start`.
fn match_balanced(chars: &[char], start: usize) -> Option<usize> {
    let mut stack: Vec<char> = Vec::new();
    let mut in_string = false;
    let mut escaped = false;
    for (i, &c) in chars.iter().enumerate().skip(start) {
        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            match c {
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' | '[' => stack.push(c),
            '}' | ']' => {
                let expected = if c == '}' { '{' } else { '[' };
                match stack.pop() {
                    Some(open) if open == expected => {
                        if stack.is_empty() {
                            return Some(i);
                        }
                    }
                    _ => return None,
                }
            }
            _ => {}
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

/// First present key of a JSON object.
fn first_value<'a>(
    map: &'a serde_json::Map<String, Value>,
    keys: &[&str],
) -> Option<&'a Value> {
    keys.iter().find_map(|k| map.get(*k))
}

/// First present key of a JSON object, as a non-empty string.
fn first_string(map: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    first_value(map, keys)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Case-insensitive prefix match that keeps byte offsets of the original.
fn strip_keyword_prefix<'a>(text: &'a str, keyword: &str) -> Option<&'a str> {
    if text.len() < keyword.len() {
        return None;
    }
    let (head, rest) = text.split_at(keyword.len());
    if head.eq_ignore_ascii_case(keyword) {
        Some(rest)
    } else {
        None
    }
}

/// Remove markdown emphasis markers.
fn strip_emphasis(s: &str) -> String {
    s.chars().filter(|c| !matches!(c, '*' | '`' | '~')).collect()
}

/// Clean a title line (strip leading hashes and emphasis).
fn clean_title(line: &str) -> String {
    strip_emphasis(line)
        .trim()
        .trim_start_matches('#')
        .trim()
        .to_string()
}

/// Lowercase, accent-tolerant key used for duplicate detection.
fn normalize_key(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pending_space = false;
    for c in s.chars() {
        if c.is_alphanumeric() {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            for lc in c.to_lowercase() {
                out.push(lc);
            }
        } else {
            pending_space = true;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quiz_creation() {
        let quiz = Quiz {
            title: "Math Quiz".to_string(),
            questions: vec![QuizQuestion {
                question: "2+2?".to_string(),
                options: vec!["3".into(), "4".into(), "5".into()],
                correct_answer: 1,
            }],
        };
        assert_eq!(quiz.title, "Math Quiz");
        assert_eq!(quiz.questions[0].correct_answer, 1);
    }

    #[test]
    fn test_quiz_json_roundtrip() {
        let quiz = Quiz {
            title: "Test".to_string(),
            questions: vec![QuizQuestion {
                question: "Q?".to_string(),
                options: vec!["a".into(), "b".into(), "c".into(), "d".into()],
                correct_answer: 2,
            }],
        };
        let json = serde_json::to_string(&quiz).unwrap();
        let parsed: Quiz = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.title, "Test");
    }

    /// JSON in a ```json fence (with prose around it) is normalized.
    #[test]
    fn test_parse_model_response_accepts_json_fence() {
        let raw = r#"Aquí tienes el cuestionario:

```json
{
  "title": "Cuestionario de Historia",
  "questions": [
    {"question": "¿Año de la Revolución Francesa?", "options": ["1789", "1776", "1815", "1914"], "correct_answer": 0},
    {"question": "¿Quién pintó La Gioconda?", "options": ["Leonardo da Vinci", "Miguel Ángel", "Rafael", "Goya"], "correct_answer": 0},
    {"question": "¿Capital de Perú?", "options": ["Lima", "Cusco", "Arequipa", "Trujillo"], "correct_answer": 0},
    {"question": "¿Cuántos lados tiene un hexágono?", "options": ["5", "6", "7", "8"], "correct_answer": 1},
    {"question": "¿Opciones duplicadas?", "options": ["igual", "igual", "otra", "cuarta"], "correct_answer": 2}
  ]
}
```

Espero que te sirva."#;

        let quiz = Quiz::parse_model_response(raw).expect("fenced JSON must parse");
        assert_eq!(quiz.title, "Cuestionario de Historia");
        // The duplicated-option question cannot reach four options: it is dropped.
        assert_eq!(quiz.questions.len(), 4);
        for q in &quiz.questions {
            assert_eq!(q.options.len(), OPTION_COUNT);
            assert!(q.correct_answer < q.options.len());
        }
        assert_eq!(quiz.questions[0].question, "¿Año de la Revolución Francesa?");
        assert_eq!(quiz.questions[3].correct_answer, 1);
    }

    /// The plain-text format is the fallback; garbage is rejected honestly.
    #[test]
    fn test_parse_model_response_text_fallback_and_rejects_garbage() {
        let raw = "Cuestionario de Matemáticas\n\n\
Pregunta 1: ¿Cuánto es 2 + 2?\n\
A) 3\n\
B) 4\n\
C) 5\n\
D) 6\n\
Correcta: B\n\n\
Pregunta 2: ¿Área de un cuadrado de lado 3?\n\
A) 6\n\
B) 8\n\
C) 9\n\
D) 12\n\
Correcta: C\n\n\
Pregunta 3: ¿Raíz cuadrada de 16?\n\
A) 2\n\
B) 4\n\
C) 8\n\
D) 16\n\
Correcta: D\n";

        let quiz = Quiz::parse_model_response(raw).expect("text quiz must parse");
        assert_eq!(quiz.title, "Cuestionario de Matemáticas");
        assert_eq!(quiz.questions.len(), 3);
        assert_eq!(quiz.questions[0].correct_answer, 1);
        assert_eq!(quiz.questions[1].correct_answer, 2);
        assert_eq!(quiz.questions[2].correct_answer, 3);
        assert!(quiz.questions.iter().all(|q| q.options.len() == OPTION_COUNT));

        // Garbage: neither JSON nor a recognizable text quiz.
        let err = Quiz::parse_model_response("Lo siento, no puedo generar un cuestionario ahora.")
            .expect_err("garbage must fail");
        assert!(matches!(err, QuizParseError::Unrecognized));

        // A question with an empty "Correcta:" must not produce a broken quiz.
        let err = Quiz::parse_model_response(
            "Pregunta 1: ¿X?\nA) 1\nB) 2\nC) 3\nD) 4\nCorrecta: ",
        )
        .expect_err("empty answer must fail");
        assert!(matches!(err, QuizParseError::TooFewQuestions { found: 0 }));

        // Empty response.
        let err = Quiz::parse_model_response("   ").expect_err("empty must fail");
        assert!(matches!(err, QuizParseError::Empty));
    }
}

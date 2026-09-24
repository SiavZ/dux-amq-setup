//! A status sentence built from its parts, so the web can draw every name in it
//! as a chip without ever parsing the finished words.
//!
//! One sentence has two spellings. The plain one ([`StatusText::message`]) is
//! what the terminal UI's status line prints, the logs record and every older
//! consumer reads, byte for byte what the sentence said before names were
//! structure: a name the sentence wraps in straight double quotes keeps them
//! there ([`q`] in [`status_text!`]), and a bare one stays bare ([`n`]). The
//! structured one ([`StatusText::segments`]) rides the wire beside it, and the
//! browser renders each name segment through its inline code chip with the
//! quotes dropped. Both come from the same parts, so they cannot disagree about
//! the words.
//!
//! A sentence built from a plain `String` has no segments at all, and the wire
//! leaves the field out: the browser renders it as the text it always was.
//!
//! [`q`]: crate::status_text!
//! [`n`]: crate::status_text!

use std::borrow::Cow;
use std::fmt;

use serde::{Deserialize, Serialize};

/// One part of a structured status sentence.
///
/// Serialized untagged so the wire shape is exactly the browser's `Prose`
/// (`lib/prose.tsx`): a bare JSON string for words, `{ "name", "quoted" }` for
/// a name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StatusSegment {
    /// Words of the sentence, drawn as they are.
    Text(String),
    /// A name (a branch, path, file, command, agent, project, terminal,
    /// provider, pull request, URL or device), drawn as a chip on the web.
    /// `quoted` records whether the plain spelling wraps it in straight double
    /// quotes.
    Name { name: String, quoted: bool },
}

/// A status sentence: its plain spelling, and the parts it was built from when
/// it was built from parts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatusText {
    message: String,
    /// `None` for a sentence that arrived as a finished string. `Some` once any
    /// part has been pushed through the builder, names or not, so a converted
    /// sentence is recognisably structured on the wire.
    segments: Option<Vec<StatusSegment>>,
}

impl StatusText {
    /// An empty structured sentence, ready for the builder methods.
    pub fn new() -> Self {
        Self {
            message: String::new(),
            segments: Some(Vec::new()),
        }
    }

    /// A finished string with no structure: what every unconverted site passes.
    pub fn plain(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            segments: None,
        }
    }

    /// Reassemble a sentence from the two halves a status carries. Used where a
    /// status is copied field by field (the status controller, the wire).
    pub fn from_parts(message: String, segments: Option<Vec<StatusSegment>>) -> Self {
        Self { message, segments }
    }

    /// The plain spelling: what the terminal UI prints, word for word.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The parts, or `None` when the sentence carries no structure.
    pub fn segments(&self) -> Option<&[StatusSegment]> {
        self.segments.as_deref()
    }

    /// Split into the plain spelling and the parts.
    pub fn into_parts(self) -> (String, Option<Vec<StatusSegment>>) {
        (self.message, self.segments)
    }

    pub fn is_empty(&self) -> bool {
        self.message.is_empty()
    }

    /// Append another sentence (words, or a whole sentence built elsewhere).
    ///
    /// Appending a plain string to a structured sentence adds it as words;
    /// appending a structured sentence splices its parts in, names included.
    pub fn push(&mut self, part: impl Into<StatusText>) {
        let part = part.into();
        if part.message.is_empty() {
            return;
        }
        let parts = match part.segments {
            Some(parts) => parts,
            None => vec![StatusSegment::Text(part.message.clone())],
        };
        self.message.push_str(&part.message);
        let own = self.segments.get_or_insert_with(|| {
            // A plain sentence becoming structured: its words so far are text.
            if self.message.len() == part.message.len() {
                Vec::new()
            } else {
                vec![StatusSegment::Text(
                    self.message[..self.message.len() - part.message.len()].to_string(),
                )]
            }
        });
        for segment in parts {
            match (own.last_mut(), segment) {
                (Some(StatusSegment::Text(last)), StatusSegment::Text(more)) => {
                    last.push_str(&more)
                }
                (_, segment) => own.push(segment),
            }
        }
    }

    /// Append a name the plain spelling wraps in straight double quotes.
    pub fn push_quoted(&mut self, name: impl fmt::Display) {
        self.push_name_segment(name.to_string(), true);
    }

    /// Append a name the plain spelling leaves bare (a path, a URL, a number).
    pub fn push_name(&mut self, name: impl fmt::Display) {
        self.push_name_segment(name.to_string(), false);
    }

    fn push_name_segment(&mut self, name: String, quoted: bool) {
        if quoted {
            self.message.push('"');
            self.message.push_str(&name);
            self.message.push('"');
        } else {
            self.message.push_str(&name);
        }
        let prefix_len = self.message.len() - name.len() - if quoted { 2 } else { 0 };
        let prefix = self.message[..prefix_len].to_string();
        self.segments
            .get_or_insert_with(|| {
                if prefix.is_empty() {
                    Vec::new()
                } else {
                    vec![StatusSegment::Text(prefix)]
                }
            })
            .push(StatusSegment::Name { name, quoted });
    }

    /// Builder form of [`Self::push`].
    pub fn text(mut self, part: impl Into<StatusText>) -> Self {
        self.push(part);
        self
    }

    /// Builder form of [`Self::push_quoted`].
    pub fn quoted(mut self, name: impl fmt::Display) -> Self {
        self.push_quoted(name);
        self
    }

    /// Builder form of [`Self::push_name`].
    pub fn name(mut self, name: impl fmt::Display) -> Self {
        self.push_name(name);
        self
    }
}

/// Build a [`StatusText`] from its parts, in reading order.
///
/// ```
/// use dux_core::status_text;
/// let text = status_text!["Checked out ", q("main"), " in ", n("/src/app"), "."];
/// assert_eq!(text, "Checked out \"main\" in /src/app.");
/// ```
///
/// - `q(expr)`: a name the plain spelling wraps in straight double quotes;
/// - `n(expr)`: a name the plain spelling leaves bare;
/// - anything else: words, or a whole [`StatusText`] spliced in.
#[macro_export]
macro_rules! status_text {
    ($($part:tt)*) => {{
        #[allow(unused_mut)]
        let mut __status_text = $crate::status_text::StatusText::new();
        $crate::__status_text_parts!(__status_text; $($part)*);
        __status_text
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __status_text_parts {
    ($t:ident;) => {};
    ($t:ident; q($e:expr) $(, $($rest:tt)*)?) => {
        $t.push_quoted($e);
        $crate::__status_text_parts!($t; $($($rest)*)?);
    };
    ($t:ident; n($e:expr) $(, $($rest:tt)*)?) => {
        $t.push_name($e);
        $crate::__status_text_parts!($t; $($($rest)*)?);
    };
    ($t:ident; $e:expr $(, $($rest:tt)*)?) => {
        $t.push($e);
        $crate::__status_text_parts!($t; $($($rest)*)?);
    };
}

impl From<String> for StatusText {
    fn from(message: String) -> Self {
        Self::plain(message)
    }
}

impl From<&str> for StatusText {
    fn from(message: &str) -> Self {
        Self::plain(message)
    }
}

impl From<&String> for StatusText {
    fn from(message: &String) -> Self {
        Self::plain(message.as_str())
    }
}

impl From<Cow<'_, str>> for StatusText {
    fn from(message: Cow<'_, str>) -> Self {
        Self::plain(message.into_owned())
    }
}

impl From<&StatusText> for StatusText {
    fn from(text: &StatusText) -> Self {
        text.clone()
    }
}

impl From<StatusText> for String {
    fn from(text: StatusText) -> Self {
        text.message
    }
}

/// A sentence reads as its plain spelling wherever it is only looked at
/// (`contains`, `starts_with`, a log line), so a producer that builds one from
/// parts changes nothing for a reader that only ever wanted the words.
impl std::ops::Deref for StatusText {
    type Target = str;

    fn deref(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for StatusText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl PartialEq<str> for StatusText {
    fn eq(&self, other: &str) -> bool {
        self.message == other
    }
}

impl PartialEq<&str> for StatusText {
    fn eq(&self, other: &&str) -> bool {
        self.message == *other
    }
}

impl PartialEq<String> for StatusText {
    fn eq(&self, other: &String) -> bool {
        &self.message == other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> StatusSegment {
        StatusSegment::Text(s.to_string())
    }

    fn name(s: &str, quoted: bool) -> StatusSegment {
        StatusSegment::Name {
            name: s.to_string(),
            quoted,
        }
    }

    #[test]
    fn a_quoted_name_keeps_its_quotes_in_the_plain_spelling_and_drops_them_in_the_segment() {
        let built = status_text![
            "Checked out ",
            q("main"),
            " for project ",
            q("p1-name"),
            "."
        ];
        assert_eq!(
            built.message(),
            "Checked out \"main\" for project \"p1-name\"."
        );
        assert_eq!(
            built.segments().unwrap(),
            &[
                text("Checked out "),
                name("main", true),
                text(" for project "),
                name("p1-name", true),
                text("."),
            ]
        );
    }

    #[test]
    fn a_bare_name_stays_bare_in_the_plain_spelling() {
        let built = status_text!["Saved to ", n("/tmp/a b/c.txt"), "."];
        assert_eq!(built.message(), "Saved to /tmp/a b/c.txt.");
        assert_eq!(
            built.segments().unwrap(),
            &[text("Saved to "), name("/tmp/a b/c.txt", false), text(".")]
        );
    }

    #[test]
    fn a_leading_name_has_no_empty_text_before_it() {
        let built = status_text![q("feature"), " is gone."];
        assert_eq!(
            built.segments().unwrap(),
            &[name("feature", true), text(" is gone.")]
        );
    }

    #[test]
    fn a_plain_string_carries_no_segments() {
        let plain: StatusText = format!("Nothing to name {}.", 3).into();
        assert_eq!(plain.message(), "Nothing to name 3.");
        assert_eq!(plain.segments(), None);
    }

    #[test]
    fn splicing_a_structured_sentence_keeps_its_names_and_merges_adjacent_words() {
        let suffix = status_text![" New worktrees branch from ", q("main"), " now."];
        let built = status_text!["Checked out ", q("main"), ".", suffix];
        assert_eq!(
            built.message(),
            "Checked out \"main\". New worktrees branch from \"main\" now."
        );
        assert_eq!(
            built.segments().unwrap(),
            &[
                text("Checked out "),
                name("main", true),
                text(". New worktrees branch from "),
                name("main", true),
                text(" now."),
            ]
        );
    }

    #[test]
    fn splicing_an_empty_sentence_changes_nothing() {
        let built = status_text!["Done", String::new(), "."];
        assert_eq!(built.segments().unwrap(), &[text("Done.")]);
    }

    #[test]
    fn a_plain_sentence_gains_structure_when_a_name_is_pushed_onto_it() {
        let mut built = StatusText::plain("Removed ");
        built.push_quoted("dev");
        assert_eq!(built.message(), "Removed \"dev\"");
        assert_eq!(
            built.segments().unwrap(),
            &[text("Removed "), name("dev", true)]
        );
    }

    #[test]
    fn names_with_multibyte_characters_are_carried_whole() {
        let built = status_text!["Añadido ", q("rama-ñ✓"), " ✓"];
        assert_eq!(built.message(), "Añadido \"rama-ñ✓\" ✓");
        assert_eq!(
            built.segments().unwrap(),
            &[text("Añadido "), name("rama-ñ✓", true), text(" ✓")]
        );
    }

    #[test]
    fn segments_serialize_as_the_browsers_prose_shape() {
        let built = status_text!["Pushed ", q("main"), " to ", n("origin"), "."];
        let json = serde_json::to_value(built.segments().unwrap()).unwrap();
        assert_eq!(
            json,
            serde_json::json!([
                "Pushed ",
                {"name": "main", "quoted": true},
                " to ",
                {"name": "origin", "quoted": false},
                "."
            ])
        );
        let back: Vec<StatusSegment> = serde_json::from_value(json).unwrap();
        assert_eq!(back, built.segments().unwrap());
    }

    #[test]
    fn it_compares_equal_to_its_plain_spelling() {
        let built = status_text!["Opened ", n(42), "."];
        assert_eq!(built, "Opened 42.");
        assert_eq!(built, "Opened 42.".to_string());
        assert_eq!(built.to_string(), "Opened 42.");
        assert_eq!(String::from(built), "Opened 42.");
    }
}

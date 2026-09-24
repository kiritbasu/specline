//! House style for everything Specline stores as prose.
//!
//! The rule is that a decision, a question, a note or a spec should read as
//! though a person wrote it — B-45 for milestones, B-46 for everything else.
//!
//! **What this module can and cannot do, stated up front**, because the gap is
//! the whole design. Structure is checkable: a field can be seen to be empty,
//! to restate its own title, or to cite `TQ-15` with nothing beside it. Voice
//! is not. Nothing here can tell a limp sentence from a sharp one, and a check
//! that claimed to would be wrong in both directions.
//!
//! That asymmetry decides the shape. A *false rejection is worse than a
//! mediocre sentence*: refused for a reason it does not accept, a model's
//! recovery is to satisfy the letter of the rule — swap the banned word for a
//! synonym, keep the shape — and the prose ends up both bad and compliant while
//! the check reports success. So the checks that block are the ones with an
//! objective referent, and the ones that are merely *signals* come back as
//! warnings attached to a write that landed.
//!
//! The layer that actually changes what gets written is neither of those: it is
//! the tool descriptions, which reach the model at the moment of writing. This
//! module is the backstop, not the mechanism.

use crate::{EntityType, Error, Result};

/// Phrases with essentially no legitimate use in a project tracker.
///
/// Deliberately short. Every entry earns its place by being a word that a
/// person describing their own work would not reach for, and each one is a
/// reliable tell rather than a matter of taste. The temptation is to grow this
/// list until it encodes a style guide; resist it, because every addition
/// widens the false-rejection surface and those are the expensive errors.
const REFUSED: &[(&str, &str)] = &[
    ("delve", "say what you looked at"),
    ("leverage", "use"),
    ("utilize", "use"),
    ("seamless", "say what does not break"),
    ("seamlessly", "say what does not break"),
    ("robust", "say what it withstands"),
    ("it's worth noting", "just say the thing"),
    ("it is worth noting", "just say the thing"),
    ("in order to", "to"),
    ("a testament to", "say what it shows"),
    ("navigating the complexities", "say what was hard"),
    ("tapestry", "say what it is"),
    ("underscores the importance", "say why it matters"),
    // KEEL-375. Each of these is filler with no technical referent: nothing
    // is ever important to note, fast-paced or a game-changer in a task row.
    // Only phrases, not single words that also have a plain meaning.
    ("it is important to note", "just say the thing"),
    ("it's important to note", "just say the thing"),
    ("in today's fast-paced", "cut it and start with the fact"),
    ("game-changer", "say what changes"),
    ("game changer", "say what changes"),
    ("cutting-edge", "name the version or the technique"),
];

/// Softer tells. Reported, never refused.
///
/// These are common in machine-written prose and also in perfectly good human
/// prose, which is exactly why they warn rather than block.
const WARNED: &[(&str, &str)] = &[
    ("crucial", "often padding — cut it or say why"),
    ("pivotal", "often padding — cut it or say why"),
    ("comprehensive", "say what it covers"),
    ("myriad", "say roughly how many"),
    ("plethora", "say roughly how many"),
    ("landscape", "name the thing itself"),
    ("holistic", "say what it includes"),
    // KEEL-375. Connectors and verbs that pad machine prose but have a plain
    // use too: "streamline the flags" and "moreover" in a quoted argument are
    // real. So they warn and the write lands.
    ("furthermore", "start a new sentence instead"),
    ("moreover", "start a new sentence instead"),
    ("empower", "say what it lets someone do"),
    ("streamline", "say what gets shorter or faster"),
    // Unhyphenated, it can be a literal edge. Hyphenated, it is refused above.
    ("cutting edge", "name the version or the technique"),
];

/// What a style check found that did not justify refusing the write.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Warning {
    /// The phrase that tripped it, as it appeared.
    pub found: String,
    /// What to write instead.
    pub instead: String,
}

/// Strip everything that is someone else's words.
///
/// Fenced code, inline code and block quotes come out before any check runs. A
/// note quoting an error message, a spec quoting a vendor's documentation, or a
/// decision quoting what a customer actually said is carrying text its author
/// did not choose — refusing those would stop the store recording the world as
/// it is, which is a far worse failure than a stray "utilize".
pub fn quotable_stripped(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_fence = false;

    for line in text.lines() {
        let trimmed = line.trim_start();

        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        // A block quote is attribution, not authorship.
        if trimmed.starts_with('>') {
            continue;
        }

        // Inline code spans, same reasoning at a smaller scale. Identifiers and
        // error strings live in these, and both are quotation.
        let mut in_code = false;
        for part in line.split('`') {
            if !in_code {
                out.push_str(part);
            }
            in_code = !in_code;
        }
        out.push('\n');
    }
    out
}

/// Whether `body` says nothing the title did not already say.
///
/// Containment rather than similarity, which is the rule KEEL-65 arrived at for
/// near-duplicate titles and the reason it is safe: one token set must be a
/// subset of the other, so the difference can only be *added* words, never
/// substituted ones. "Fix the board filter" against "Fix the board filter so it
/// survives a reload" is an addition and passes; two sentences that merely
/// share vocabulary do not trip it.
fn only_restates(title: &str, body: &str) -> bool {
    let words = |s: &str| -> Vec<String> {
        s.to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() > 2)
            .map(|w| w.to_owned())
            .collect()
    };
    let title_words = words(title);
    let body_words = words(body);

    if title_words.is_empty() || body_words.is_empty() {
        return false;
    }
    // The body adds nothing: every word in it already appears in the title.
    body_words.iter().all(|w| title_words.contains(w))
}

/// Check a prose field on its way into the store.
///
/// `field` names the MCP argument so the error points at what the caller sent.
/// `title` is the row's own title where there is one, so a body that only
/// restates it can be caught; pass `None` for prose that has no title above it.
///
/// Returns the warnings that did not justify refusing, so the caller can hand
/// them back with the successful write.
pub fn check(
    entity_type: EntityType,
    field: &str,
    text: &str,
    title: Option<&str>,
) -> Result<Vec<Warning>> {
    let prose = quotable_stripped(text);
    // A typographic apostrophe is the same word. Without this, "it’s worth
    // noting" passes where "it's worth noting" is refused.
    let lower = prose.to_lowercase().replace('\u{2019}', "'");

    for (phrase, instead) in REFUSED {
        if lower.contains(phrase) {
            return Err(Error::invalid(
                entity_type,
                field,
                format!("\"{phrase}\" is house-style banned — it reads as machine-written"),
                format!(
                    "{instead}. Write the way you would say it to a colleague: plain words, \
                     no padding, no sentence that exists to sound considered"
                ),
            ));
        }
    }

    if let Some(title) = title
        && only_restates(title, &prose)
    {
        return Err(Error::invalid(
            entity_type,
            field,
            "this only restates the title in different order — it adds nothing a reader \
             does not already have"
                .to_owned(),
            "say what a colleague could not work out from the title alone: what is wrong \
             or wanted, what it affects, and what done looks like"
                .to_owned(),
        ));
    }

    Ok(WARNED
        .iter()
        .filter(|(phrase, _)| lower.contains(phrase))
        .map(|(phrase, instead)| Warning {
            found: (*phrase).to_owned(),
            instead: (*instead).to_owned(),
        })
        .collect())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn check_body(text: &str) -> Result<Vec<Warning>> {
        check(EntityType::Decision, "body", text, None)
    }

    #[test]
    fn ordinary_prose_passes_clean() {
        let warnings = check_body(
            "The board shows a task's priority but never which phase it belongs to, so you \
             have to open each one to find out.",
        )
        .unwrap();
        assert!(warnings.is_empty());
    }

    // Failure case: the register list refuses, and says what to write instead.
    #[test]
    fn a_banned_phrase_is_refused_with_a_replacement() {
        let err = check_body("We should leverage the existing parser.").unwrap_err();
        let message = err.to_string();
        assert!(message.contains("leverage"), "{message}");
        // A model reading only the "Expected" half must be able to retry.
        assert!(message.contains("use"), "{message}");
    }

    // The exemption that keeps the store able to record reality. This is the
    // case that would make the whole rule unusable if it were missing.
    #[test]
    fn a_quoted_error_message_is_not_the_authors_voice() {
        check_body(
            "The daemon refused it:\n\n> Failed to utilize the temp store\n\nWhich is a \
             message from SQLite, not from us.",
        )
        .expect("a block quote is quotation, not authorship");
    }

    #[test]
    fn fenced_code_is_exempt_too() {
        check_body("Reproduced with:\n\n```\ncargo run -- --leverage-cache\n```\n\nIt panics.")
            .expect("a code fence is not prose");
    }

    #[test]
    fn inline_code_is_exempt() {
        check_body("The flag is called `--utilize-cache`, which we did not name.")
            .expect("an inline code span is quotation");
    }

    // Failure case: a body that says nothing the title did not.
    #[test]
    fn a_body_that_only_restates_its_title_is_refused() {
        let err = check(
            EntityType::Task,
            "body",
            "Fix the board filter",
            Some("Fix the board filter"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("restates the title"), "{err}");
    }

    #[test]
    fn a_body_that_adds_anything_is_accepted() {
        // Containment, not similarity: added words are fine, and this is the
        // case that a naive overlap score would wrongly refuse.
        check(
            EntityType::Task,
            "body",
            "Fix the board filter so it survives a reload, which it does not today.",
            Some("Fix the board filter"),
        )
        .expect("a body that adds detail is not a restatement");
    }

    #[test]
    fn a_soft_tell_warns_rather_than_refusing() {
        let warnings = check_body("This is a crucial part of the write path.").unwrap();
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].found, "crucial");
    }

    // Failure case for the KEEL-375 additions: filler phrases block the write.
    #[test]
    fn filler_phrases_are_refused() {
        for text in [
            "It is important to note that the cache is per project.",
            "This is a game-changer for the board.",
            "We use a cutting-edge parser.",
            "In today\u{2019}s fast-paced world, sessions need orientation.",
        ] {
            let err = check_body(text).expect_err(text);
            assert!(err.to_string().contains("house-style banned"), "{err}");
        }
    }

    #[test]
    fn a_typographic_apostrophe_does_not_get_round_the_list() {
        check_body("It\u{2019}s worth noting that the lock is advisory.")
            .expect_err("a curly apostrophe is the same phrase");
    }

    // The case that keeps the list honest: technical prose that uses a
    // borderline word in its plain meaning still lands, with a warning.
    #[test]
    fn legitimate_technical_use_warns_but_is_not_blocked() {
        let warnings = check_body(
            "Streamline the release flags: the build takes four of them and needs one. \
             Moreover, two of the four are ignored on Linux.",
        )
        .expect("a borderline word must not block a write");
        let found: Vec<&str> = warnings.iter().map(|w| w.found.as_str()).collect();
        assert_eq!(found, ["moreover", "streamline"], "{warnings:?}");
    }

    #[test]
    fn filler_inside_a_quote_or_code_is_exempt() {
        let warnings = check_body(
            "The vendor page says:\n\n> A game-changer for cutting-edge teams.\n\n\
             The flag is `--streamline`. Neither is our wording.",
        )
        .expect("quotation is not authorship");
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn stripping_leaves_the_authors_own_sentences_intact() {
        let out = quotable_stripped("Before.\n\n> quoted\n\n```\ncode\n```\n\nAfter.");
        assert!(out.contains("Before."), "{out}");
        assert!(out.contains("After."), "{out}");
        assert!(!out.contains("quoted"), "{out}");
        assert!(!out.contains("code"), "{out}");
    }
}

//! The user's feedback from an artifact's page as a session meets it (task
//! 1107): where a comment's words are in the plan now, found as the page's
//! script finds them, and where in the plan's Markdown its suggestion
//! applies; the artifact tool's read of the reviews and comments; and what
//! a listing of the artifact's notes says of each.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::ops::Range;

use crate::holder::Actor;
use crate::item::{Comment, Item, Quote, Review};
use crate::storage::ItemMap;

/// How many characters on either side of its words a comment keeps, as the
/// page's script takes them (`AROUND` there).
const AROUND: usize = 32;

/// How much of a comment's words the read quotes.
const QUOTED: usize = 200;

/// The plan's words as the page's script reads them (`words()` there): the
/// words of each section, without the heading that opens it, each run of
/// white space one space, and a space between blocks. For each character,
/// the section it is in, and the byte of the artifact's description it was
/// read from, when the Markdown there holds it as it shows.
pub struct Shown {
    chars: Vec<char>,
    from: Vec<Option<usize>>,
    part: Vec<usize>,
    parts: Vec<String>,
}

/// Where a comment's words are in a version of the plan.
#[derive(Debug, Clone, PartialEq)]
pub enum Found {
    /// The words themselves, at this span of `Shown`'s characters.
    Here(Range<usize>),
    /// Words close to them, between the same words on either side: the
    /// comment's words changed to these.
    Changed(Range<usize>),
    /// Neither: the comment is outdated.
    Gone,
}

impl Shown {
    /// Of an artifact's description: its title line, then the plan.
    pub fn of(description: &str) -> Shown {
        let start = description.find('\n').map_or(description.len(), |at| at + 1);
        let mut shown = Shown { chars: Vec::new(), from: Vec::new(), part: Vec::new(), parts: Vec::new() };
        for (heading, range) in crate::artifact::section_ranges(&description[start..]) {
            let part = shown.parts.len();
            shown.parts.push(heading);
            let mut read = Vec::new();
            read_section(&description[start + range.start..start + range.end], start + range.start, &mut read);
            let mut gap = true;
            for (c, at) in read {
                if c.is_whitespace() {
                    gap = true;
                    continue;
                }
                if gap && !shown.chars.is_empty() {
                    shown.push(' ', None, part);
                }
                shown.push(c, at, part);
                gap = false;
            }
        }
        shown
    }

    fn push(&mut self, c: char, from: Option<usize>, part: usize) {
        self.chars.push(c);
        self.from.push(from);
        self.part.push(part);
    }

    /// The words in `span`.
    pub fn words(&self, span: &Range<usize>) -> String {
        self.chars[span.clone()].iter().collect()
    }

    /// The bytes of `description` that `span` was read from: from its first
    /// character to its last, each read as it is written, with nothing
    /// between them there but the words and the white space the page shows
    /// as one space. `None` where Markdown runs through them.
    pub fn written(&self, description: &str, span: &Range<usize>) -> Option<Range<usize>> {
        let first = self.from.get(span.start).copied().flatten()?;
        let last = self.from.get(span.end.checked_sub(1)?).copied().flatten()?;
        let bytes = first..last + self.chars[span.end - 1].len_utf8();
        let there = description.get(bytes.clone())?;
        (crate::artifact::collapsed(there) == self.words(span)).then_some(bytes)
    }

    /// Where `quote` is in these words, as the page's script finds it
    /// (`find()` there): of the places that hold its words, the one whose
    /// surroundings and section match best. Else, as Hypothesis anchors, the
    /// words between the same words on either side, when they are close
    /// enough to the quote's.
    pub fn find(&self, quote: &Quote) -> Found {
        let exact: Vec<char> = flat(&quote.exact).trim().chars().collect();
        let prefix: Vec<char> = flat(&quote.prefix).chars().collect();
        let suffix: Vec<char> = flat(&quote.suffix).chars().collect();
        if exact.is_empty() {
            return Found::Gone;
        }
        let text = &self.chars;
        let mut best: Option<(usize, Range<usize>)> = None;
        for at in occurrences(text, &exact) {
            let section = usize::from(self.parts[self.part[at]] == quote.section);
            let here = same(&text[at.saturating_sub(AROUND)..at], &prefix, true) + same(&text[at + exact.len()..], &suffix, false) + section;
            if best.as_ref().is_none_or(|(score, _)| here > *score) {
                best = Some((here, at..at + exact.len()));
            }
        }
        if let Some((_, span)) = best {
            return Found::Here(span);
        }
        let before = &prefix[prefix.len().saturating_sub(20)..];
        let after = &suffix[..suffix.len().min(20)];
        let trimmed = |chars: &[char]| chars.iter().collect::<String>().trim().chars().count();
        if trimmed(before) < 8 || trimmed(after) < 8 {
            return Found::Gone;
        }
        let (mut close, mut near) = (0.0, None);
        for from in occurrences(text, before) {
            let mut start = from + before.len();
            let Some(mut end) = index_of(text, after, start) else { continue };
            if end - start > exact.len() * 2 + 40 {
                continue;
            }
            while start < end && text[start] == ' ' {
                start += 1;
            }
            while end > start && text[end - 1] == ' ' {
                end -= 1;
            }
            if end <= start {
                continue;
            }
            let alike = alike(&text[start..end], &exact);
            if alike > close {
                close = alike;
                near = Some(start..end);
            }
        }
        match near {
            Some(span) if close >= 0.5 => Found::Changed(span),
            _ => Found::Gone,
        }
    }
}

/// The characters of `text`, one section of the plan starting at byte
/// `base` of the description, as `artifact::shown_words` reads them before
/// it makes each run of white space one space: each with the byte it was
/// read from, where the Markdown holds it as it shows.
fn read_section(text: &str, base: usize, out: &mut Vec<(char, Option<usize>)>) {
    use pulldown_cmark::{Event, LinkType, Parser, Tag, TagEnd};
    let chars = |words: &str, at: Option<usize>| words.char_indices().map(move |(i, c)| (c, at.map(|at| at + i))).collect::<Vec<_>>();
    // Each link and image open, innermost last, as `artifact::events` keeps
    // them: whether it kept its tag, a kept image's source, and how many
    // events came before it, to tell an image with nothing in it, whose
    // source the page shows.
    let mut open: Vec<(bool, Option<String>, usize)> = Vec::new();
    for (seen, (event, at)) in Parser::new_ext(text, crate::artifact::markdown_options()).into_offset_iter().enumerate() {
        let written = &text[at.clone()];
        match event {
            Event::Text(words) | Event::Html(words) | Event::InlineHtml(words) => out.extend(chars(&words, (written == &*words).then_some(base + at.start))),
            Event::Code(words) => out.extend(chars(&words, inside(written, &words).map(|inner| base + at.start + inner))),
            Event::Start(Tag::Link { link_type, ref dest_url, .. }) => {
                let keep = !open.iter().any(|(kept, ..)| *kept) && (link_type == LinkType::Email || crate::artifact::linkable(dest_url));
                open.push((keep, None, seen));
            }
            Event::Start(Tag::Image { ref dest_url, .. }) => {
                let keep = !open.iter().any(|(kept, ..)| *kept) && crate::artifact::linkable(dest_url);
                open.push((keep, keep.then(|| dest_url.to_string()), seen));
            }
            Event::End(TagEnd::Link | TagEnd::Image) => {
                if let Some((true, Some(source), started)) = open.pop() {
                    if seen == started + 1 {
                        out.extend(chars(&source, None));
                    }
                }
            }
            Event::Start(Tag::Emphasis | Tag::Strong | Tag::Strikethrough) | Event::End(TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough) => {}
            Event::Start(_) | Event::End(_) | Event::SoftBreak | Event::HardBreak | Event::Rule => out.push((' ', None)),
            _ => {}
        }
    }
}

/// Where the words of a code span begin in what is written for it: past
/// its backticks, and past the one space CommonMark strips from either side.
fn inside(written: &str, words: &str) -> Option<usize> {
    let ticks = written.len() - written.trim_start_matches('`').len();
    let inner = written.get(ticks..written.len().checked_sub(ticks)?)?;
    if inner == words {
        return Some(ticks);
    }
    let padded = inner.len() >= 2 && inner.starts_with(' ') && inner.ends_with(' ') && inner[1..inner.len() - 1] == *words;
    padded.then_some(ticks + 1)
}

/// `text` with each run of white space one space, as the page's `flat`.
fn flat(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut white = false;
    for c in text.chars() {
        if !c.is_whitespace() {
            out.push(c);
        } else if !white {
            out.push(' ');
        }
        white = c.is_whitespace();
    }
    out
}

/// Each place `needle` starts in `text`, overlapping ones too, as repeated
/// `indexOf` finds them.
fn occurrences<'a>(text: &'a [char], needle: &'a [char]) -> impl Iterator<Item = usize> + 'a {
    let last = if needle.is_empty() { None } else { text.len().checked_sub(needle.len()) };
    last.into_iter().flat_map(|last| 0..=last).filter(move |&at| text[at..at + needle.len()] == *needle)
}

/// Where `needle` first starts in `text`, at `from` or after.
fn index_of(text: &[char], needle: &[char], from: usize) -> Option<usize> {
    (from..=text.len().checked_sub(needle.len())?).find(|&at| text[at..at + needle.len()] == *needle)
}

/// How many characters `a` and `b` share from their start, or from their
/// end.
fn same(a: &[char], b: &[char], from_end: bool) -> usize {
    if from_end {
        a.iter().rev().zip(b.iter().rev()).take_while(|(x, y)| x == y).count()
    } else {
        a.iter().zip(b).take_while(|(x, y)| x == y).count()
    }
}

/// How alike two runs of words are, from 0 to 1: by edit distance, or by
/// the words they share when they are too long to compare letter by letter.
fn alike(a: &[char], b: &[char]) -> f64 {
    let longest = a.len().max(b.len());
    if longest == 0 || a == b {
        return 1.0;
    }
    if a.len() * b.len() > 4_000_000 {
        let words = |chars: &[char]| chars.iter().collect::<String>().to_lowercase().split(' ').map(str::to_string).collect::<Vec<_>>();
        let (x, y) = (words(a), words(b));
        let seen: HashSet<&String> = x.iter().collect();
        let both = y.iter().filter(|word| seen.contains(word)).count();
        return both as f64 / x.len().max(y.len()) as f64;
    }
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut diagonal = row[0];
        row[0] = i;
        for j in 1..=b.len() {
            let above = row[j];
            row[j] = (above + 1).min(row[j - 1] + 1).min(diagonal + usize::from(a[i - 1] != b[j - 1]));
            diagonal = above;
        }
    }
    1.0 - row[b.len()] as f64 / longest as f64
}

/// What a suggestion says when the user wrote nothing else: the words it
/// replaces and what it puts there, or that it deletes them.
pub fn suggested(quote: &Quote, replacement: &str) -> String {
    if replacement.is_empty() {
        format!("Delete: {}", quote.exact)
    } else {
        format!("Replace {} with {replacement}", quote.exact)
    }
}

/// A review's verdict on its version, as the sessions are told it.
pub fn verdict(review: &Review) -> String {
    match review.verdict.as_str() {
        Review::APPROVE => format!("approved version {}", review.version),
        Review::CHANGES => format!("changes requested on version {}", review.version),
        _ => format!("commented on version {}", review.version),
    }
}

/// A review as the sessions are told it and read it (tasks 1108 and 1107):
/// its verdict, the question it answered with the answer it gave, and the
/// comments it sent. With it, whether the user wrote the review's text,
/// which then says more than this.
pub fn review_told<'a>(note: &Item, review: &Review, item: impl Fn(&str) -> Option<&'a Item>) -> (String, bool) {
    let ids: Vec<u32> = review.comments.iter().filter_map(|comment| item(comment)).map(|comment| comment.id).collect();
    let answering = review
        .answered
        .as_deref()
        .and_then(&item)
        .and_then(|question| Some((question.id, question.question.as_ref()?.answer.as_ref()?)))
        .map(|(id, answer)| format!(", answering question {id} with \"{}\"", crate::menu::picked(&answer.text)))
        .unwrap_or_default();
    let sending = match ids.as_slice() {
        [] => String::new(),
        [one] => format!(", sending comment {one}"),
        many => format!(", sending comments {}", many.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")),
    };
    let own = note.description != crate::artifact::review_said(&review.verdict, review.version, &ids);
    (format!("{}{answering}{sending}.", verdict(review)), own)
}

/// What a listing of an artifact's notes puts in the mark of a comment or
/// a review from its page (task 1107): "comment, sent", "suggestion,
/// applied in version 3", "reply to 12", "review, changes requested on
/// version 2, resolved".
pub fn mark(note: &Item, id: impl Fn(&str) -> Option<u32>) -> Option<String> {
    if let Some(review) = note.review.as_deref() {
        let resolved = if review.resolved.is_some() { ", resolved" } else { "" };
        return Some(format!("review, {}{resolved}", verdict(review)));
    }
    let comment = note.comment.as_deref()?;
    if let Some(to) = comment.reply_to.as_deref() {
        return Some(id(to).map_or_else(|| "reply".to_string(), |to| format!("reply to {to}")));
    }
    let what = if comment.quote.is_some() && comment.replacement.is_some() { "suggestion" } else { "comment" };
    Some(format!("{what}, {}", state(comment)))
}

/// Where a comment stands: pending until the user sends it, sent, resolved
/// once a session settles it, or applied in a version of the plan.
fn state(comment: &Comment) -> String {
    match (comment.applied, comment.resolved, comment.sent) {
        (Some(version), ..) => format!("applied in version {version}"),
        (None, Some(_), _) => "resolved".to_string(),
        (None, None, Some(_)) => "sent".to_string(),
        (None, None, None) => "pending".to_string(),
    }
}

/// The comment `note` holds, when it answers none: a thread's first.
fn thread(note: &Item) -> Option<&Comment> {
    note.comment.as_deref().filter(|comment| comment.reply_to.is_none())
}

/// Whether a person wrote `note`, not a session.
fn the_users(note: &Item) -> bool {
    note.created_by.as_ref().is_none_or(|by| by.pid.is_none())
}

/// The artifact tool's read of the user's feedback on artifact `item`, from
/// its page (task 1107): each review with the comments it sent, and each
/// comment sent alone, the open ones first and whole, with where each
/// comment's words are now, what it suggests and the replies to it; then
/// the settled ones by id, and how many comments wait unsent on the page.
/// Empty when the page has none. `me` names the replies this session wrote.
pub fn read(item: &Item, all: &ItemMap, me: Option<&Actor>) -> String {
    let Some(uid) = item.uid.as_deref() else { return String::new() };
    let index = crate::ekko::uid_index(all);
    let by_uid = |uid: &str| index.get(uid).and_then(|id| all.get(id));
    let mut notes: Vec<&Item> = all
        .values()
        .filter(|note| note.trashed.is_none() && note.attached_to.as_deref() == Some(uid) && (note.review.is_some() || note.comment.is_some()))
        .collect();
    notes.sort_by_key(|note| (note.timestamp, note.id));
    if notes.is_empty() {
        return String::new();
    }
    let replies = |to: &str| -> Vec<&Item> {
        notes.iter().copied().filter(|note| note.comment.as_deref().is_some_and(|comment| comment.reply_to.as_deref() == Some(to))).collect()
    };
    let open = |note: &Item| match (note.review.as_deref(), thread(note)) {
        (Some(_), _) => crate::artifact::feedback(note),
        (None, Some(comment)) => comment.sent.is_some() && comment.resolved.is_none(),
        (None, None) => false,
    };
    let in_review: HashSet<&str> = notes.iter().filter_map(|note| note.review.as_deref()).flat_map(|review| review.comments.iter().map(String::as_str)).collect();
    let sent_with = |review: &Review| review.comments.iter().filter_map(|uid| by_uid(uid)).filter(|note| note.trashed.is_none()).collect::<Vec<_>>();
    // Each review with the comments it sent, and each comment sent alone:
    // open when any of it waits on a session.
    let units: Vec<(&Item, Vec<&Item>)> = notes
        .iter()
        .filter_map(|note| match (note.review.as_deref(), thread(note)) {
            (Some(review), _) => Some((*note, sent_with(review))),
            (None, Some(comment)) if comment.sent.is_some() && !note.uid.as_deref().is_some_and(|uid| in_review.contains(uid)) => Some((*note, Vec::new())),
            _ => None,
        })
        .collect();
    let (open_units, settled): (Vec<_>, Vec<_>) = units.into_iter().partition(|(note, sent)| open(note) || sent.iter().any(|note| open(note)));

    let shown = Shown::of(&item.description);
    let steps: Vec<&str> = item.artifact.as_deref().map(|plan| plan.steps.iter().map(|step| step.key.as_str()).collect()).unwrap_or_default();
    // A step's task, by the step's key, while the board holds it.
    let task_of = |key: &str| item.artifact.as_deref()?.steps.iter().find(|step| step.key == key)?.task.as_deref().and_then(by_uid);
    let who = |note: &Item| match (&note.created_by, me) {
        (Some(by), Some(me)) if by.pid.is_some() && me.is(by) => "this session".to_string(),
        (Some(by), me) if by.pid.is_some() => format!("a session, {}", me.map_or_else(|| by.label(), |me| me.name(by))),
        _ => "the user".to_string(),
    };
    let comment_line = |out: &mut String, note: &Item, lead: &str| {
        let Some(comment) = thread(note) else { return };
        let _ = write!(out, "{lead}Comment {}", note.id);
        if lead.is_empty() {
            out.push_str(" from the user, sent alone,");
        }
        let settled = comment.resolved.is_some() || comment.applied.is_some();
        match (&comment.quote, &comment.step) {
            (Some(quote), _) => {
                let _ = write!(out, " on \"{}\"", crate::agent::clip(&quote.exact, QUOTED));
                if !quote.section.is_empty() {
                    let _ = write!(out, " in {}", quote.section);
                }
                if !settled {
                    match shown.find(quote) {
                        Found::Here(_) => out.push_str(", current"),
                        Found::Changed(span) => {
                            let _ = write!(out, ", its words changed to \"{}\"", crate::agent::clip(&shown.words(&span), QUOTED));
                        }
                        Found::Gone => {
                            let _ = write!(out, ", outdated: the plan no longer holds its words, made on version {}", comment.version);
                        }
                    }
                }
            }
            (None, Some(step)) => {
                let _ = write!(out, " on step {step}");
                if let Some(task) = task_of(step) {
                    let _ = write!(out, " (task {})", task.id);
                }
                if !settled && !steps.contains(&step.as_str()) {
                    out.push_str(", outdated: the plan has no such step now");
                }
            }
            (None, None) => out.push_str(" on the whole plan"),
        }
        match (comment.quote.as_ref(), comment.replacement.as_deref()) {
            (Some(_), Some("")) => out.push_str(", suggesting to delete them"),
            (Some(_), Some(replacement)) => {
                let _ = write!(out, ", suggesting \"{replacement}\" in their place");
            }
            _ => {}
        }
        if settled {
            let _ = write!(out, ", {}", state(comment));
        }
        let own = match (comment.quote.as_ref(), comment.replacement.as_deref()) {
            (Some(quote), Some(replacement)) => note.description != suggested(quote, replacement),
            _ => true,
        };
        if own {
            let _ = write!(out, ": {}", crate::artifact::collapsed(&note.description));
        }
        // Start on the page (task 1111) asks for the step's task to be
        // taken up, which resolves it.
        if !settled && crate::artifact::is_start(note) {
            out.push_str(" -- the user's Start: set its task in progress and take it up, which resolves it");
        }
        out.push('\n');
        if let Some(uid) = note.uid.as_deref() {
            let indent = " ".repeat(lead.len() + 2);
            for reply in replies(uid) {
                let _ = writeln!(out, "{indent}Reply {} from {}: {}", reply.id, who(reply), crate::artifact::collapsed(&reply.description));
            }
        }
    };

    let mut out = String::new();
    if open_units.is_empty() {
        out.push_str("Feedback from its page: none open.\n");
    } else {
        // A review asking for changes is answered by asking again.
        let changes = open_units.iter().any(|(note, _)| note.review.as_deref().is_some_and(|review| review.verdict == Review::CHANGES));
        let again = if changes { ", then ask with approve puts the plan to the user again" } else { "" };
        let _ = writeln!(out, "Feedback from its page, the open first: answer it with the artifact tool's apply, reply and resolve{again}.");
    }
    for (note, sent) in &open_units {
        match note.review.as_deref() {
            Some(review) => {
                let resolved = if review.resolved.is_some() { ", resolved" } else { "" };
                let (told, own) = review_told(note, review, by_uid);
                let says = if own { format!(" It says: {}", crate::artifact::collapsed(&note.description)) } else { String::new() };
                let _ = writeln!(out, "Review {} from the user{resolved}: {told}{says}", note.id);
                for comment in sent {
                    comment_line(&mut out, comment, "- ");
                }
            }
            None => comment_line(&mut out, note, ""),
        }
    }
    if !settled.is_empty() {
        let reviews: Vec<String> = settled.iter().filter(|(note, _)| note.review.is_some()).map(|(note, _)| note.id.to_string()).collect();
        let comments: Vec<String> = settled
            .iter()
            .flat_map(|(note, sent)| std::iter::once(*note).filter(|note| note.review.is_none()).chain(sent.iter().copied()))
            .map(|note| match note.comment.as_deref().and_then(|comment| comment.applied) {
                Some(version) => format!("{} (applied in version {version})", note.id),
                None => note.id.to_string(),
            })
            .collect();
        let named = |what: &str, ids: &[String]| match ids {
            [] => None,
            [one] => Some(format!("{what} {one}")),
            many => Some(format!("{what}s {}", many.join(", "))),
        };
        let parts: Vec<String> = [named("review", &reviews), named("comment", &comments)].into_iter().flatten().collect();
        let _ = writeln!(out, "Settled: {}.", parts.join("; "));
    }
    let pending = notes.iter().filter(|note| the_users(note) && thread(note).is_some_and(|comment| comment.sent.is_none())).count();
    match pending {
        0 => {}
        1 => out.push_str("The user has 1 comment pending on the page, not sent yet: a review or Send now sends it.\n"),
        n => {
            let _ = writeln!(out, "The user has {n} comments pending on the page, not sent yet: a review or Send now sends them.");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quote(exact: &str, prefix: &str, suffix: &str, section: &str) -> Quote {
        Quote { exact: exact.into(), prefix: prefix.into(), suffix: suffix.into(), section: section.into(), unknown: Default::default() }
    }

    /// The plan's words are read as the page reads them, section by
    /// section, which `artifact::shown_words` says on the server; and each
    /// character read as it is written comes from that very character of
    /// the description.
    #[test]
    fn the_words_are_read_as_the_page_reads_them() {
        let plans = [
            "Ship\n\n## Goal\nShip *the* page, with **bold** and `code` words, inter*nal*ly.\n## What is known\n- one [link](https://example.org/a) item\n- two ![alt](https://example.org/i.png) and ![](https://example.org/bare.png)\n- not ![](javascript:x) kept, nor [this](javascript:y)\n- an & entity &amp; and \\*escaped\\* stars\n\n| a | b |\n|---|---|\n| cell one | cell two |\n## Design\n```\nfenced\n## not a heading\n```\n<b>raw html</b> and `` a`b `` too\n## Risks and open questions\nNone.  \nHard break.\n",
            "Title only",
            "Lead\nbefore any heading.\n\n## Goal\n\n## Design\nCRLF lines\r\nhere.\r\n",
        ];
        for description in plans {
            let shown = Shown::of(description);
            let start = description.find('\n').map_or(description.len(), |at| at + 1);
            let plan = &description[start..];
            let expected: Vec<String> =
                crate::artifact::section_ranges(plan).into_iter().map(|(_, range)| crate::artifact::shown_words(&plan[range])).filter(|words| !words.is_empty()).collect();
            assert_eq!(shown.chars.iter().collect::<String>(), expected.join(" "), "{description:?}");
            for (c, from) in shown.chars.iter().zip(&shown.from) {
                if let Some(from) = from {
                    assert!(description[*from..].starts_with(*c), "{c:?} read from byte {from} of {description:?}");
                }
            }
        }
    }

    /// A comment's words are found as the page finds them: where they are,
    /// at the place whose surroundings and section match best; between the
    /// same words on either side when they changed a little; and nowhere
    /// when they are gone.
    #[test]
    fn a_comments_words_are_found_as_the_page_finds_them() {
        let description = "Ship\n\n## Goal\nThe session reads it.\n## Design\nThe session reads it, then writes.\n## Risks and open questions\nIt stops until an idle hour passes, then it ends.";
        let shown = Shown::of(description);
        let span = |found: Found| match found {
            Found::Here(span) | Found::Changed(span) => shown.words(&span) + &format!("@{}", span.start),
            Found::Gone => "gone".to_string(),
        };
        let first = "The session reads it.".len() + 1;
        assert_eq!(span(shown.find(&quote("The session reads it", "", ".", "Goal"))), "The session reads it@0");
        assert_eq!(span(shown.find(&quote("The session reads it", "", ", then", ""))), format!("The session reads it@{first}"), "by the words around it");
        assert_eq!(span(shown.find(&quote("The session reads it", "", "", "Design"))), format!("The session reads it@{first}"), "by its section");
        let idle = quote("until an idle hour", "then writes. It stops ", " passes, then it ends.", "Risks and open questions");
        assert!(matches!(shown.find(&idle), Found::Here(_)));

        let changed = Shown::of(&description.replace("until an idle hour", "until one idle hour"));
        assert!(matches!(changed.find(&idle), Found::Changed(ref span) if changed.words(span) == "until one idle hour"), "{:?}", changed.find(&idle));
        let gone = Shown::of(&description.replace("until an idle hour", "when the user says"));
        assert_eq!(gone.find(&idle), Found::Gone, "too far from the words it was on");
        let cut = Shown::of(&description.replace("It stops until an idle hour passes, then it ends.", "None."));
        assert_eq!(cut.find(&idle), Found::Gone);
        assert_eq!(shown.find(&quote("  ", "", "", "")), Found::Gone, "no words, no place");
    }

    /// The bytes a comment's words were read from are where a suggestion
    /// applies: inside a code span, across a line's end, but not where code,
    /// emphasis, an escape or an entity runs through them.
    #[test]
    fn a_suggestion_applies_where_its_words_are_written_as_they_show() {
        let description = "Ship\n\n## Goal\nThe `comment` field is *kept*\non the note, A &amp; B, a\\*b.";
        let shown = Shown::of(description);
        let written = |exact: &str| match shown.find(&quote(exact, "", "", "")) {
            Found::Here(span) => shown.written(description, &span).map(|bytes| description[bytes].to_string()),
            other => panic!("{exact:?} is {other:?}"),
        };
        assert_eq!(written("comment").as_deref(), Some("comment"), "inside the code span");
        assert_eq!(written("field is").as_deref(), Some("field is"));
        assert_eq!(written("on the note").as_deref(), Some("on the note"));
        assert_eq!(written("kept on the").as_deref(), None, "emphasis runs through them");
        assert_eq!(written("is kept").as_deref(), None, "emphasis opens inside them");
        assert_eq!(written("The comment field").as_deref(), None, "code runs through them");
        assert_eq!(written("A & B").as_deref(), None, "an entity is not written as it shows");
        assert_eq!(written("A &").as_deref(), None, "nor cut in half where it ends the words");
        assert_eq!(written("a*b").as_deref(), None, "nor an escape");
        let lines = "Ship\n\n## Goal\nIt stops until an\nidle hour.";
        let shown = Shown::of(lines);
        let Found::Here(span) = shown.find(&quote("an idle", "", "", "")) else { panic!("found") };
        assert_eq!(shown.written(lines, &span).map(|bytes| &lines[bytes]), Some("an\nidle"), "across the line's end");
    }

    /// The artifact tool's read of the user's feedback (task 1107): a
    /// comment sent alone and a review with the comments it sent, the open
    /// first and whole -- where each comment's words are now, what it
    /// suggests, the replies to it -- then the settled by id, and the
    /// comments still pending on the page by count. A listing of the
    /// artifact's notes marks each comment, reply and review.
    #[test]
    fn the_read_gives_the_open_feedback_first_and_whole() {
        use crate::ops::{ArtifactSpec, Draft, Ref};
        let (me, _, _) = crate::holder::test_sessions();
        let dir = crate::paths::test_dir("ekko-feedback-read");
        std::fs::create_dir_all(&dir).unwrap();
        let open = |actor: Actor| crate::ekko::Ekko::new(crate::storage::Storage::new(&dir).unwrap()).acting_as(actor);
        let (person, session) = (open(Actor::person()), open(me.clone()));
        let plan = "Ship\n\n## Goal\nTell the session.\n## What is known\nFacts, measured here.\n## Design\nIt stops until an idle hour passes, then it ends.\n## Risks and open questions\nNone.";
        let spec: ArtifactSpec = serde_json::from_value(serde_json::json!({"text": plan, "steps": [{"key": "one", "text": "First"}]})).unwrap();
        let mut draft = Draft::open(&session).unwrap();
        let artifact = draft.artifact(&spec).unwrap();
        draft.commit(false).unwrap();
        let on = Ref::Id(artifact);
        let comment = |exact: Option<(&str, &str, &str, &str)>, step: Option<&str>, replacement: Option<&str>| Comment {
            version: 1,
            quote: exact.map(|(exact, prefix, suffix, section)| quote(exact, prefix, suffix, section)),
            replacement: replacement.map(str::to_string),
            step: step.map(str::to_string),
            reply_to: None,
            sent: None,
            resolved: None,
            applied: None,
            theme: None,
            color: None,
            unknown: Default::default(),
        };
        let mut draft = Draft::open(&person).unwrap();
        let start = draft.comment(&on, "Start here.", comment(None, Some("one"), None)).unwrap();
        let typo = draft.comment(&on, "Typo?", comment(Some(("Tell", "", " the session. Facts, measured he", "Goal")), None, None)).unwrap();
        draft.commit(false).unwrap();
        let mut draft = Draft::open(&person).unwrap();
        for note in [start, typo] {
            draft.send_comment(&on, session.storage.get().unwrap()[&note].uid.as_deref().unwrap()).unwrap();
        }
        let idle = draft.comment(&on, "", comment(Some(("until an idle hour", "e. It stops ", " passes, then it ends. None.", "Design")), None, Some("after an idle hour"))).unwrap();
        let facts = draft.comment(&on, "Which facts?", comment(Some(("Facts", "Tell the session. ", ", measured here. It stops until", "What is known")), None, None)).unwrap();
        let whole = draft.comment(&on, "Overall fine.", comment(None, None, None)).unwrap();
        let review = draft.review(&on, Review::CHANGES, "Tighten it.", 1).unwrap();
        draft.commit(false).unwrap();
        let replied: ArtifactSpec = serde_json::from_value(serde_json::json!({"artifact": artifact, "reply": [{"to": idle, "text": "Will do."}], "resolve": [typo]})).unwrap();
        let mut draft = Draft::open(&session).unwrap();
        let edit = crate::ops::Edit { item: on.clone(), text: Some(plan.replace("Facts, measured here.", "Numbers, taken on this machine.")), replace: None, append: None, if_updated_at: None };
        draft.edit(&edit).unwrap();
        let reply = draft.answer_feedback(artifact, &replied).unwrap().replies[0];
        draft.commit(false).unwrap();
        let mut draft = Draft::open(&person).unwrap();
        draft.comment(&on, "Later.", Comment { version: 2, ..comment(None, None, None) }).unwrap();
        draft.commit(false).unwrap();

        let all = session.storage.get().unwrap();
        let read = read(&all[&artifact], &all, Some(&me));
        let expected = format!(
            "Feedback from its page, the open first: answer it with the artifact tool's apply, reply and resolve, then ask with approve puts the plan to the user again.\n\
             Comment {start} from the user, sent alone, on step one: Start here.\n\
             Review {review} from the user: changes requested on version 1, sending comments {idle}, {facts}, {whole}. It says: Tighten it.\n\
             - Comment {idle} on \"until an idle hour\" in Design, current, suggesting \"after an idle hour\" in their place\n\
             \x20   Reply {reply} from this session: Will do.\n\
             - Comment {facts} on \"Facts\" in What is known, outdated: the plan no longer holds its words, made on version 1: Which facts?\n\
             - Comment {whole} on the whole plan: Overall fine.\n\
             Settled: comment {typo}.\n\
             The user has 1 comment pending on the page, not sent yet: a review or Send now sends it.\n"
        );
        assert_eq!(read, expected);
        assert_eq!(super::read(&all[&review], &all, Some(&me)), "", "a note is no artifact with feedback");

        let context = crate::agent::contexts_text(&crate::agent::contexts(&session, &[artifact.to_string()]).unwrap(), crate::agent::Detail::Concise);
        for mark in [
            format!("{idle}. [suggestion, sent] "),
            format!("feedback open: the artifact tool reads it whole with artifact {artifact}, and its apply, reply and resolve answer it\n"),
            format!("{typo}. [comment, resolved] "),
            format!("{reply}. [reply to {idle}] "),
            format!("{review}. [review, changes requested on version 1] "),
        ] {
            assert!(context.contains(&mark), "{mark:?} in {context}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// How alike two runs of words are, as the page's script measures it.
    #[test]
    fn words_alike_by_edit_distance() {
        let chars = |text: &str| text.chars().collect::<Vec<_>>();
        assert_eq!(alike(&chars("idle hour"), &chars("idle hour")), 1.0);
        assert_eq!(alike(&chars(""), &chars("")), 1.0);
        assert!((alike(&chars("until one idle hour"), &chars("until an idle hour")) - (1.0 - 2.0 / 19.0)).abs() < 1e-9);
        assert_eq!(alike(&chars("abc"), &chars("xyz")), 0.0);
    }
}

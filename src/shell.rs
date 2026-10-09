//! The calls a Bash tool command runs, in order, and what reaches each one.
//!
//! A port of ctx's `src/lib/shell_calls.py` (ekko task 805), whose lexer was
//! replayed on the 30,334 tool calls of this machine's transcripts (note 797):
//! ekko cannot depend on ctx, and a guard keyed on a different reading of the
//! same command would disagree with the one already proven. Quotes, comments,
//! `$(...)` and backticks, `bash -c` strings and here-documents are read the
//! way that lexer reads them, down to the tokenizer it borrows from Python's
//! `shlex` (posix, `punctuation_chars`, `whitespace_split`). A word that only
//! names a command, in an echo, a commit message or a here-document, is not a
//! call.
//!
//! One addition: each call carries what is fed to it beyond its arguments, as
//! far as the command's own text holds it -- its here-documents and
//! here-strings, the text of each `$(...)`, `<(...)` or backticks in its
//! arguments, and what the calls piped into it are given. A cue's words match
//! there too, so a GraphQL mutation passed through `-f query="$(cat <<'EOF'`
//! is seen, while a here-document written to a file still names nothing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Words after which the next word is a command: shell syntax and the
/// wrappers that run their arguments (`timeout 5 git`, `xargs -r rm`).
const SEPARATORS: &[&str] =
    &[";", ";;", "&", "&&", "|", "||", "|&", "(", ")", "`", "!", "{", "}", "if", "then", "elif", "else", "do", "while", "until", "time"];
const WRAPPERS: &[&str] =
    &["timeout", "nohup", "nice", "stdbuf", "xargs", "sudo", "doas", "env", "exec", "command", "builtin", "watch", "setsid"];
/// Shells whose `-c` string is a command of its own.
const SHELLS: &[&str] = &["bash", "sh", "zsh", "dash"];
/// What a `$(...)` or a backtick span leaves in the text around it: a word no
/// expansion can resolve. A `<(...)` or `>(...)` leaves the path bash passes.
pub const CAPTURED: &str = "$__ctx_substitution";
const PROCESS: &str = "/dev/fd/63";
/// Where a redirection to one of these still ends up in the tool's output.
const SHOWN: &[&str] = &["/dev/stdout", "/dev/stderr", "/dev/tty", "/dev/fd/1", "/dev/fd/2"];
/// The characters shlex gives tokens of their own, in runs.
const PUNCTUATION: &str = "();<>|&";
/// Around what the text left where a here-document's body was cut, after
/// its delimiter, or where a substitution was taken out: a kind and a number,
/// between private-use characters no command types. `unmarked` takes them
/// back out of every word, so the words are the Python's.
const MARK_OPEN: char = '\u{E000}';
const MARK_CLOSE: char = '\u{E001}';

/// Where what a call prints ends up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sink {
    /// In the tool's result.
    Shown,
    /// In a file.
    File,
    /// Captured by a `$(...)` or backticks.
    Captured,
}

impl Sink {
    #[cfg(test)]
    pub fn word(self) -> &'static str {
        match self {
            Sink::Shown => "shown",
            Sink::File => "file",
            Sink::Captured => "captured",
        }
    }
}

/// A call, as a command name with its directory cut off and its arguments.
/// An assignment such as `S=/tmp/x` is a call named `=`, with the name and the
/// value as its arguments, so a caller can follow the variables a later call
/// names. A redirection and its target are left out of the arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub name: String,
    pub args: Vec<String>,
    /// Where its own output goes; see `output` for where it ends up.
    pub sink: Sink,
    /// The call its output feeds, by index in the same list.
    pub piped_to: Option<usize>,
    /// The `bash -c` that runs it, by index in the same list.
    pub parent: Option<usize>,
    /// The file a `<` feeds it.
    pub stdin: Option<String>,
    /// Its here-documents and here-strings, and the text of each
    /// substitution in its arguments: see `fed` for all that reaches it.
    pub input: Vec<String>,
    /// The substitutions its arguments hold, by the index of their first
    /// call and the index after their last.
    substitutions: Vec<(usize, usize)>,
}

impl Call {
    fn new(name: &str) -> Call {
        Call {
            name: name.to_string(),
            args: Vec::new(),
            sink: Sink::Shown,
            piped_to: None,
            parent: None,
            stdin: None,
            input: Vec::new(),
            substitutions: Vec::new(),
        }
    }

    fn redirect(&mut self, operator: &str, fd: Option<&str>, target: &str) {
        if operator == "<" && matches!(fd, None | Some("0")) {
            self.stdin = Some(target.to_string());
        } else if operator.starts_with("&>") {
            self.sink = if SHOWN.contains(&target) { Sink::Shown } else { Sink::File };
        } else if operator.contains('>') && !operator.starts_with('<') && matches!(fd, None | Some("1")) {
            if operator == ">&" && !target.is_empty() && target.chars().all(py_isdigit) {
                return; // >&2 and >&1 leave it in the tool's output
            }
            self.sink = if SHOWN.contains(&target) { Sink::Shown } else { Sink::File };
        } else if operator == "<<<" {
            self.input.push(target.to_string());
        }
    }
}

/// Where what `calls[at]` prints ends up, through its pipes and the `bash -c`
/// that runs it: the end of a pipeline decides.
/// ekko's guard needs no sink; the port keeps it, and the corpus test
/// compares it with the Python's.
#[cfg(test)]
pub fn output(calls: &[Call], at: usize) -> Sink {
    let mut call = &calls[at];
    while let Some(next) = call.piped_to {
        call = &calls[next];
    }
    match (call.sink, call.parent) {
        (Sink::Shown, Some(parent)) => output(calls, parent),
        (sink, _) => sink,
    }
}

/// All the text that reaches `calls[at]` beyond its own arguments: its
/// here-documents and here-strings, and the arguments and input of every
/// call in its substitutions and of every call piped into it.
pub fn fed(calls: &[Call], at: usize) -> Vec<String> {
    let mut texts = Vec::new();
    let mut seen = vec![false; calls.len()];
    gather(calls, at, &mut texts, &mut seen, false);
    texts
}

fn gather(calls: &[Call], at: usize, texts: &mut Vec<String>, seen: &mut [bool], with_args: bool) {
    if std::mem::replace(&mut seen[at], true) {
        return;
    }
    let call = &calls[at];
    if with_args {
        texts.extend(call.args.iter().cloned());
    }
    texts.extend(call.input.iter().cloned());
    for &(start, end) in &call.substitutions {
        for inner in start..end.min(calls.len()) {
            gather(calls, inner, texts, seen, true);
        }
    }
    for (from, other) in calls.iter().enumerate() {
        if other.piped_to == Some(at) {
            gather(calls, from, texts, seen, true);
        }
    }
}
/// Each call `command` runs, in the order written; the calls inside
/// `$(...)`, `<(...)` and backticks come after the rest. `piped_to` and
/// `parent` index into the same list.
pub fn calls(command: &str) -> Vec<Call> {
    let mut parse = Parse::default();
    parse.calls_into(command, Sink::Shown);
    let Parse { mut found, owners, spans, .. } = parse;
    for (owner, span) in owners.into_iter().zip(spans) {
        if let (Some(owner), Some(span)) = (owner, span) {
            found[owner].substitutions.push(span);
        }
    }
    found
}

/// A command being read: the calls found so far, and what was taken out of
/// its text on the way, numbered across every level of it -- a `bash -c`
/// string and a substitution are read as commands of their own.
#[derive(Default)]
struct Parse {
    found: Vec<Call>,
    /// Each here-document's body, and whether a call has it yet.
    bodies: Vec<(String, bool)>,
    /// For each substitution, the call whose words held it, and the calls it
    /// runs: the index of the first, and the one after the last.
    owners: Vec<Option<usize>>,
    spans: Vec<Option<(usize, usize)>>,
}

/// What a mark in a word stands for, by its number.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Mark {
    Heredoc(usize),
    Substitution(usize),
}

fn mark(kind: char, number: usize) -> String {
    format!("{MARK_OPEN}{kind}{number}{MARK_CLOSE}")
}

/// `word` without the marks it holds, and what they stand for.
fn unmarked(word: &str) -> (String, Vec<Mark>) {
    if !word.contains(MARK_OPEN) {
        return (word.to_string(), Vec::new());
    }
    let (mut clean, mut marks, mut held, mut inside) = (String::new(), Vec::new(), String::new(), false);
    for c in word.chars() {
        match c {
            MARK_OPEN => {
                inside = true;
                held.clear();
            }
            MARK_CLOSE if inside => {
                inside = false;
                let (kind, number) = held.split_at(held.chars().next().map_or(0, char::len_utf8));
                match (kind, number.parse()) {
                    ("h", Ok(n)) => marks.push(Mark::Heredoc(n)),
                    ("s", Ok(n)) => marks.push(Mark::Substitution(n)),
                    _ => {}
                }
            }
            c if inside => held.push(c),
            c => clean.push(c),
        }
    }
    (clean, marks)
}

impl Parse {
    /// Appends the calls of `command`, as shell_calls.calls does: the calls
    /// of its text, then those of each substitution it holds. A `$(...)` or
    /// backticks capture what their calls print; a `<(...)` hands it to the
    /// call around it, so it counts as shown.
    fn calls_into(&mut self, command: &str, sink: Sink) {
        let cut = self.cut_heredocs(command);
        let (top, inner) = self.blanked(&cut);
        let start = self.found.len();
        match words(&top) {
            Ok(tokens) => self.simple_calls(tokens),
            Err(Unpaired) => self.loose_calls(&top), // quotes shlex cannot pair: split on blanks instead
        }
        for call in &mut self.found[start..] {
            if call.sink == Sink::Shown {
                call.sink = sink;
            }
        }
        for (number, text, captured) in inner {
            let first = self.found.len();
            self.calls_into(&text, if captured { Sink::Captured } else { sink });
            self.spans[number] = Some((first, self.found.len()));
        }
    }

    /// Hands what `marks` stand for to the call at `owner`: a here-document's
    /// body to read, or a substitution among its words.
    fn claim(&mut self, marks: &[Mark], owner: usize) {
        for mark in marks {
            match *mark {
                Mark::Heredoc(number) => {
                    if let Some((body, claimed)) = self.bodies.get_mut(number).filter(|(_, claimed)| !claimed) {
                        *claimed = true;
                        self.found[owner].input.push(body.clone());
                    }
                }
                Mark::Substitution(number) => {
                    if let Some(slot) = self.owners.get_mut(number) {
                        slot.get_or_insert(owner);
                    }
                }
            }
        }
    }

    /// The text with each here-document's body cut, as shell_calls'
    /// HEREDOC.sub does: `(<<-?\s*(['"]?)(\w+)\2[^\n]*)\n.*?\n[ \t]*\3[ \t]*(?=\n|$)`,
    /// dot matching newlines, replaced by its first group, the header line.
    /// The body goes to `bodies`, and a mark after the delimiter, where the
    /// call redirecting from it finds it.
    fn cut_heredocs(&mut self, command: &str) -> String {
        let chars: Vec<char> = command.chars().collect();
        let mut out = String::with_capacity(command.len());
        let mut i = 0;
        while i < chars.len() {
            match heredoc_at(&chars, i) {
                Some(found) => {
                    out.extend(&chars[i..found.delimiter_end]);
                    out.push_str(&mark('h', self.bodies.len()));
                    out.extend(&chars[found.delimiter_end..found.header_end]);
                    self.bodies.push((found.body, false));
                    i = found.end;
                }
                None => {
                    out.push(chars[i]);
                    i += 1;
                }
            }
        }
        out
    }

    /// The text with each outermost `$(...)`, `<(...)`, `>(...)` and
    /// `` `...` `` put back as one word, marked with its number, and what
    /// each held: `(number, inside, captured)`. Quoted or not: a
    /// double-quoted "$(git stash drop)" is one word to shlex, and still runs.
    fn blanked(&mut self, text: &str) -> (String, Vec<(usize, String, bool)>) {
        let chars: Vec<char> = text.chars().collect();
        let (mut out, mut inner, mut i) = (String::with_capacity(text.len()), Vec::new(), 0);
        let escaped = |i: usize| i > 0 && chars[i - 1] == '\\';
        while i < chars.len() {
            let taken = if matches!((chars[i], chars.get(i + 1)), ('$' | '<' | '>', Some('('))) && !escaped(i) {
                let (mut depth, mut end) = (0i64, i + 2);
                while end < chars.len() && depth >= 0 {
                    depth += match chars[end] {
                        '(' => 1,
                        ')' => -1,
                        _ => 0,
                    };
                    end += 1;
                }
                let held: String = chars[i + 2..end.saturating_sub(1).max(i + 2)].iter().collect();
                Some((held, chars[i] == '$', end))
            } else if chars[i] == '`' && !escaped(i) {
                let mut end = find(&chars, '`', i + 1);
                while let Some(at) = end.filter(|at| chars[at - 1] == '\\') {
                    end = find(&chars, '`', at + 1);
                }
                let Some(end) = end else {
                    out.extend(&chars[i..]);
                    break;
                };
                Some((chars[i + 1..end].iter().collect(), true, end + 1))
            } else {
                None
            };
            match taken {
                Some((held, captured, end)) => {
                    let number = self.owners.len();
                    self.owners.push(None);
                    self.spans.push(None);
                    inner.push((number, held, captured));
                    out.push_str(if captured { CAPTURED } else { PROCESS });
                    out.push_str(&mark('s', number));
                    i = end;
                }
                None => {
                    out.push(chars[i]);
                    i += 1;
                }
            }
        }
        (out, inner)
    }

    /// The calls of the words, as shell_calls.simple_calls finds them.
    fn simple_calls(&mut self, tokens: Vec<String>) {
        let (mut current, mut prev): (Option<usize>, Option<String>) = (None, None);
        let mut fd: Option<String> = None;
        let mut redirection: Option<(String, Option<String>)> = None;
        let mut piping: Option<usize> = None;
        // Marks read where no call could own them yet, for the next one.
        let mut pending: Vec<Mark> = Vec::new();
        for raw in tokens {
            let (word, marks) = unmarked(&raw);
            if word.is_empty() && !raw.is_empty() {
                pending.extend(marks); // a mark alone, a word shlex never saw
                continue;
            }
            // A redirection, its descriptor and its target are no arguments,
            // and leave prev as it was: 2>/dev/null git ... still starts a call.
            if let Some((operator, descriptor)) = redirection.take() {
                match current {
                    Some(at) => {
                        self.found[at].redirect(&operator, descriptor.as_deref(), &word);
                        self.claim(&marks, at);
                    }
                    None => pending.extend(marks),
                }
                continue;
            }
            if let Some(digit) = fd_mark(&word) {
                fd = Some(digit.to_string());
                continue;
            }
            if is_redirection(&word) {
                redirection = Some((word, fd.take()));
                continue;
            }
            let mut owner = None;
            if let Some(at) = current {
                if SEPARATORS.contains(&word.as_str()) {
                    piping = if word == "|" || word == "|&" { Some(at) } else { None };
                    current = None;
                } else {
                    if SHELLS.contains(&self.found[at].name.as_str()) && prev.as_deref().is_some_and(is_command_flag) {
                        // Its own command, which keeps the marks for its calls.
                        let start = self.found.len();
                        self.calls_into(&raw, Sink::Shown);
                        for call in &mut self.found[start..] {
                            call.parent = call.parent.or(Some(at));
                        }
                    } else {
                        owner = Some(at);
                    }
                    self.found[at].args.push(word.clone());
                }
            } else if SEPARATORS.contains(&word.as_str()) {
            } else if at_command_position(prev.as_deref()) {
                if is_assignment(&word) {
                    let (name, value) = word.split_once('=').unwrap_or((&word, ""));
                    let mut call = Call::new("=");
                    call.args = vec![name.to_string(), value.to_string()];
                    self.found.push(call);
                } else if !(word.starts_with('-') || is_duration(&word)) {
                    let name = word.rsplit('/').next().unwrap_or(&word);
                    self.found.push(Call::new(name));
                    // A wrapper's next word is still a command.
                    if !WRAPPERS.contains(&name) {
                        let at = self.found.len() - 1;
                        if let Some(from) = piping.take() {
                            self.found[from].piped_to = Some(at);
                        }
                        current = Some(at);
                        owner = Some(at);
                        let waiting = std::mem::take(&mut pending);
                        self.claim(&waiting, at);
                    }
                }
            }
            match owner {
                Some(at) => self.claim(&marks, at),
                None => pending.extend(marks),
            }
            prev = Some(word);
        }
        if let Some(last) = self.found.len().checked_sub(1) {
            self.claim(&pending, last);
        }
    }

    /// The fallback for quotes shlex cannot pair, as shell_calls.loose_calls:
    /// the command split on its separators and blanks.
    fn loose_calls(&mut self, text: &str) {
        let mut pending = Vec::new();
        for part in split_loose(&uncommented(text)) {
            let mut pieces: Vec<String> = Vec::new();
            for piece in part.split_whitespace() {
                let (clean, marks) = unmarked(piece);
                pending.extend(marks);
                if !clean.is_empty() && fd_mark(&clean).is_none() {
                    pieces.push(clean);
                }
            }
            let mut pieces = pieces.as_slice();
            while let Some((first, rest)) = pieces.split_first().filter(|(first, _)| is_assignment(first)) {
                let (name, value) = first.split_once('=').unwrap_or((first, ""));
                let mut call = Call::new("=");
                call.args = vec![name.to_string(), value.to_string()];
                self.found.push(call);
                pieces = rest;
            }
            while let Some((_, rest)) = pieces.split_first().filter(|(first, _)| WRAPPERS.contains(&base_name(first))) {
                pieces = rest;
            }
            if let Some((first, rest)) = pieces.split_first() {
                let mut call = Call::new(base_name(first));
                call.args = rest.to_vec();
                self.found.push(call);
                let waiting = std::mem::take(&mut pending);
                self.claim(&waiting, self.found.len() - 1);
            }
        }
        if let Some(last) = self.found.len().checked_sub(1) {
            self.claim(&pending, last);
        }
    }
}

/// Whether a call named `name` runs its arguments as a command, as `timeout`
/// and `env` do: `calls` lists it, and the command after it.
pub(crate) fn is_wrapper(name: &str) -> bool {
    WRAPPERS.contains(&name)
}

/// A command's name without its directory, as `word.rsplit("/", 1)[-1]`.
fn base_name(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

struct HeredocMatch {
    /// Where the delimiter, and its closing quote, end.
    delimiter_end: usize,
    /// Where the header line ends, before its newline.
    header_end: usize,
    /// Where the whole match ends: after the terminator line, before the
    /// newline that follows it.
    end: usize,
    body: String,
}

/// HEREDOC's match at `start`, with the regex's backtracking: the word is
/// tried from its longest to its shortest, since the terminator must repeat
/// it.
fn heredoc_at(chars: &[char], start: usize) -> Option<HeredocMatch> {
    if chars.get(start) != Some(&'<') || chars.get(start + 1) != Some(&'<') {
        return None;
    }
    let mut at = start + 2;
    if chars.get(at) == Some(&'-') {
        at += 1;
    }
    while chars.get(at).is_some_and(|c| py_isspace(*c)) {
        at += 1;
    }
    let quote = chars.get(at).copied().filter(|c| *c == '\'' || *c == '"');
    if quote.is_some() {
        at += 1;
    }
    let word_start = at;
    while chars.get(at).is_some_and(|c| py_word(*c)) {
        at += 1;
    }
    for word_end in (word_start + 1..=at).rev() {
        let mut next = word_end;
        if let Some(quote) = quote {
            if chars.get(next) != Some(&quote) {
                continue;
            }
            next += 1;
        }
        let delimiter_end = next;
        while chars.get(next).is_some_and(|c| *c != '\n') {
            next += 1;
        }
        if next >= chars.len() {
            continue;
        }
        let (header_end, body_start) = (next, next + 1);
        let word = &chars[word_start..word_end];
        for body_end in body_start..chars.len() {
            if chars[body_end] != '\n' {
                continue;
            }
            let mut t = body_end + 1;
            while chars.get(t).is_some_and(|c| *c == ' ' || *c == '\t') {
                t += 1;
            }
            if chars.len() < t + word.len() || &chars[t..t + word.len()] != word {
                continue;
            }
            t += word.len();
            while chars.get(t).is_some_and(|c| *c == ' ' || *c == '\t') {
                t += 1;
            }
            if t == chars.len() || chars[t] == '\n' {
                return Some(HeredocMatch { delimiter_end, header_end, end: t, body: chars[body_start..body_end].iter().collect() });
            }
        }
    }
    None
}

fn find(chars: &[char], wanted: char, from: usize) -> Option<usize> {
    chars.iter().skip(from).position(|c| *c == wanted).map(|at| at + from)
}

/// `re.split(r"&&|\|\||[;&|\n]", text)`.
fn split_loose(text: &str) -> Vec<&str> {
    let (mut parts, mut start, mut i) = (Vec::new(), 0, 0);
    let bytes = text.as_bytes();
    while i < bytes.len() {
        let two = &bytes[i..(i + 2).min(bytes.len())];
        let width = if two == b"&&" || two == b"||" {
            2
        } else if matches!(bytes[i], b';' | b'&' | b'|' | b'\n') {
            1
        } else {
            0
        };
        if width > 0 {
            parts.push(&text[start..i]);
            i += width;
            start = i;
        } else {
            i += 1;
        }
    }
    parts.push(&text[start..]);
    parts
}

// --- words ---------------------------------------------------------------------

/// The command with its comments cut, each unquoted newline made a `;`, and
/// the file descriptor before a redirection marked, as `__fdN__`. shlex alone
/// would end a comment only at a newline, and a newline between two commands
/// would be mere whitespace to it.
fn uncommented(command: &str) -> String {
    let chars: Vec<char> = command.chars().collect();
    let mut out = String::with_capacity(command.len());
    let (mut quote, mut i) = (None, 0);
    // The last character written, which the Python kept as out[-1][-1].
    let after_blank = |out: &String| out.chars().next_back().is_none_or(|c| " \t\n;&|(".contains(c));
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && quote != Some('\'') {
            out.extend(chars.get(i..i + 2).unwrap_or(&chars[i..]));
            i += 2;
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            out.push(c);
        } else if c == '\'' || c == '"' {
            quote = Some(c);
            out.push(c);
        } else if c == '#' && after_blank(&out) {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        } else if c == '\n' {
            out.push_str(" ; ");
        } else if py_isdigit(c) && matches!(chars.get(i + 1), Some('<' | '>')) && after_blank(&out) {
            out.push_str(&format!(" __fd{c}__"));
        } else {
            out.push(c);
        }
        i += 1;
    }
    out
}

/// Quotes shlex could not pair, or an escape at the very end.
#[derive(Debug)]
struct Unpaired;

/// The words of `command`, as shlex.shlex(uncommented(command), posix=True,
/// punctuation_chars=True) with whitespace_split and no commenters gives
/// them: runs of `();<>|&` are words of their own, quotes are removed, and
/// within double quotes a backslash escapes only `"` and itself.
fn words(command: &str) -> Result<Vec<String>, Unpaired> {
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        Blank,
        Word,
        Punctuation,
        Quote(char),
        Escape,
        End,
    }
    let text = uncommented(command);
    let mut chars = text.chars();
    let (mut found, mut pushback) = (Vec::new(), None);
    let mut state = State::Blank;
    loop {
        let (mut token, mut quoted, mut escaped_from) = (String::new(), false, State::Word);
        loop {
            let next = pushback.take().or_else(|| chars.next());
            match state {
                State::End => break,
                State::Blank => match next {
                    None => {
                        state = State::End;
                        break;
                    }
                    Some(c) if py_whitespace(c) => {
                        if !token.is_empty() || quoted {
                            break;
                        }
                    }
                    Some('\\') => {
                        escaped_from = State::Word;
                        state = State::Escape;
                    }
                    Some(c) if PUNCTUATION.contains(c) => {
                        token.push(c);
                        state = State::Punctuation;
                    }
                    Some(c @ ('\'' | '"')) => state = State::Quote(c),
                    Some(c) => {
                        token.push(c);
                        state = State::Word;
                    }
                },
                State::Quote(q) => {
                    quoted = true;
                    match next {
                        None => return Err(Unpaired),
                        Some(c) if c == q => state = State::Word,
                        Some('\\') if q == '"' => {
                            escaped_from = state;
                            state = State::Escape;
                        }
                        Some(c) => token.push(c),
                    }
                }
                State::Escape => {
                    let Some(c) = next else { return Err(Unpaired) };
                    // In posix shells, only the quote itself or the escape
                    // character may be escaped within quotes.
                    if let State::Quote(q) = escaped_from {
                        if c != '\\' && c != q {
                            token.push('\\');
                        }
                    }
                    token.push(c);
                    state = escaped_from;
                }
                State::Word | State::Punctuation => match next {
                    None => {
                        state = State::End;
                        break;
                    }
                    Some(c) if py_whitespace(c) => {
                        state = State::Blank;
                        if !token.is_empty() || quoted {
                            break;
                        }
                    }
                    Some(c) if state == State::Punctuation => {
                        if PUNCTUATION.contains(c) {
                            token.push(c);
                        } else {
                            pushback = Some(c);
                            state = State::Blank;
                            break;
                        }
                    }
                    Some(c @ ('\'' | '"')) => state = State::Quote(c),
                    Some('\\') => {
                        escaped_from = State::Word;
                        state = State::Escape;
                    }
                    Some(c) if !PUNCTUATION.contains(c) => token.push(c),
                    Some(c) => {
                        pushback = Some(c);
                        state = State::Blank;
                        if !token.is_empty() || quoted {
                            break;
                        }
                    }
                },
            }
        }
        if !quoted && token.is_empty() {
            return Ok(found);
        }
        found.push(token);
    }
}

// --- calls ---------------------------------------------------------------------

fn at_command_position(prev: Option<&str>) -> bool {
    match prev {
        None => true,
        Some(prev) => {
            SEPARATORS.contains(&prev)
                || WRAPPERS.contains(&prev)
                || prev.starts_with('-')
                || is_duration(prev)
                || is_assignment(prev)
        }
    }
}

/// `[A-Za-z_]\w*=.*`, whole.
fn is_assignment(word: &str) -> bool {
    let mut chars = word.chars();
    if !chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') {
        return false;
    }
    for c in chars {
        if c == '=' {
            return true;
        }
        if !py_word(c) {
            return false;
        }
    }
    false
}

/// `[\d.]+[smhd]?`, whole.
fn is_duration(word: &str) -> bool {
    let body = word.strip_suffix(['s', 'm', 'h', 'd']).unwrap_or(word);
    !body.is_empty() && body.chars().all(|c| c.is_ascii_digit() || c == '.')
}

/// `[<>&|]*[<>][<>&|]*`, whole.
fn is_redirection(word: &str) -> bool {
    !word.is_empty() && word.chars().all(|c| "<>&|".contains(c)) && word.chars().any(|c| c == '<' || c == '>')
}

/// The descriptor `uncommented` marked, as in `__fd2__`.
fn fd_mark(word: &str) -> Option<&str> {
    let digit = word.strip_prefix("__fd")?.strip_suffix("__")?;
    (digit.len() == 1 && digit.chars().all(|c| c.is_ascii_digit())).then_some(digit)
}

/// `-\w*c`, whole: the flag before a shell's command string.
fn is_command_flag(word: &str) -> bool {
    word.strip_prefix('-').and_then(|rest| rest.strip_suffix('c')).is_some_and(|middle| middle.chars().all(py_word))
}

// --- the words of a call -------------------------------------------------------

/// The arguments that are not options, skipping the values `valued` take.
pub fn operands<'a>(args: &'a [String], valued: &[&str]) -> Vec<&'a str> {
    let (mut found, mut skip, mut dashes) = (Vec::new(), false, false);
    for arg in args {
        if skip {
            skip = false;
        } else if dashes || !arg.starts_with('-') || arg == "-" {
            found.push(arg.as_str());
        } else if arg == "--" {
            dashes = true;
        } else if valued.contains(&arg.as_str()) {
            skip = true;
        }
    }
    found
}

/// The paths a word names, or none when it names one the hook cannot know.
pub fn expand(word: &str, variables: &HashMap<String, String>, at: &Path) -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let word = match word.strip_prefix('~') {
        Some(rest) => format!("{home}{rest}"),
        None => word.to_string(),
    };
    let Some(word) = substituted(&word, variables) else { return Vec::new() };
    if word.contains('$') || word.contains('`') {
        return Vec::new();
    }
    let path = if word.starts_with('/') { word.clone() } else { format!("{}/{word}", at.to_string_lossy().trim_end_matches('/')) };
    let path = if at.as_os_str().is_empty() && !word.starts_with('/') { word.clone() } else { path };
    if word.contains(['*', '?', '[']) {
        return glob(&path);
    }
    vec![PathBuf::from(path)]
}

/// `re.sub(r"\$\{(\w+)\}|\$(\w+)", value, word)`, where a name neither the
/// command nor the environment sets gives `None`.
fn substituted(word: &str, variables: &HashMap<String, String>) -> Option<String> {
    let chars: Vec<char> = word.chars().collect();
    let (mut out, mut i) = (String::new(), 0);
    let value = |name: &str| variables.get(name).cloned().or_else(|| std::env::var(name).ok());
    while i < chars.len() {
        if chars[i] == '$' {
            if chars.get(i + 1) == Some(&'{') {
                let start = i + 2;
                let mut end = start;
                while chars.get(end).is_some_and(|c| py_word(*c)) {
                    end += 1;
                }
                if end > start && chars.get(end) == Some(&'}') {
                    out.push_str(&value(&chars[start..end].iter().collect::<String>())?);
                    i = end + 1;
                    continue;
                }
            }
            let start = i + 1;
            let mut end = start;
            while chars.get(end).is_some_and(|c| py_word(*c)) {
                end += 1;
            }
            if end > start {
                out.push_str(&value(&chars[start..end].iter().collect::<String>())?);
                i = end;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    Some(out)
}

/// Python's glob.glob: the paths that exist and match, a component at a
/// time, where `*` and `?` never match a leading dot unless the pattern has
/// one.
fn glob(pattern: &str) -> Vec<PathBuf> {
    let absolute = pattern.starts_with('/');
    let mut found = vec![PathBuf::from(if absolute { "/" } else { "" })];
    for part in pattern.split('/').filter(|part| !part.is_empty()) {
        let mut next = Vec::new();
        for base in &found {
            if !part.contains(['*', '?', '[']) {
                next.push(base.join(part));
                continue;
            }
            let listed = if base.as_os_str().is_empty() { Path::new(".") } else { base.as_path() };
            let Ok(entries) = std::fs::read_dir(listed) else { continue };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') && !part.starts_with('.') {
                    continue;
                }
                if fnmatch(&name.chars().collect::<Vec<_>>(), &part.chars().collect::<Vec<_>>()) {
                    next.push(base.join(&name));
                }
            }
        }
        found = next;
    }
    found.into_iter().filter(|path| std::fs::symlink_metadata(path).is_ok()).collect()
}

/// fnmatch.fnmatchcase: `*`, `?`, `[seq]` and `[!seq]`.
fn fnmatch(name: &[char], pattern: &[char]) -> bool {
    match pattern.split_first() {
        None => name.is_empty(),
        Some(('*', rest)) => (0..=name.len()).any(|skip| fnmatch(&name[skip..], rest)),
        Some(('?', rest)) => !name.is_empty() && fnmatch(&name[1..], rest),
        Some(('[', rest)) => {
            let Some((&c, name_rest)) = name.split_first() else { return false };
            let negated = rest.first() == Some(&'!');
            let set = if negated { &rest[1..] } else { rest };
            // A ']' first in the set is one of its characters.
            let Some(close) = set.iter().skip(1).position(|c| *c == ']').map(|at| at + 1) else {
                return c == '[' && fnmatch(name_rest, rest);
            };
            let members = &set[..close];
            let mut matched = false;
            let mut i = 0;
            while i < members.len() {
                if i + 2 < members.len() && members[i + 1] == '-' {
                    matched |= members[i] <= c && c <= members[i + 2];
                    i += 3;
                } else {
                    matched |= members[i] == c;
                    i += 1;
                }
            }
            matched != negated && fnmatch(name_rest, &set[close + 1..])
        }
        Some((&p, rest)) => name.first() == Some(&p) && fnmatch(&name[1..], rest),
    }
}

/// A call other than `cd`, `pushd` and the assignments, with the folder it
/// runs in, as far as the command's own text says. The Python yields the
/// variables set before it too, which ekko's guard has no use for.
#[derive(Debug, Clone)]
pub struct Located {
    /// Its index in the calls.
    pub at: usize,
    pub folder: PathBuf,
}

/// The calls of `command`, and each one but `cd`, `pushd` and the
/// assignments with where it runs: `cwd`, then wherever a `cd` or `pushd`
/// before it went.
pub fn located(command: &str, cwd: &Path) -> (Vec<Call>, Vec<Located>) {
    let found = calls(command);
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let (mut folder, mut variables) = (cwd.to_path_buf(), HashMap::new());
    let mut placed = Vec::new();
    for (at, call) in found.iter().enumerate() {
        match call.name.as_str() {
            "=" => {
                if let [name, value] = call.args.as_slice() {
                    variables.insert(name.clone(), value.clone());
                }
            }
            "export" | "local" | "declare" | "readonly" => {
                for arg in &call.args {
                    if let Some((key, value)) = arg.split_once('=').filter(|_| !arg.starts_with('-')) {
                        variables.insert(key.to_string(), value.to_string());
                    }
                }
            }
            "cd" | "pushd" => {
                let target = operands(&call.args, &[]);
                let paths = match target.first() {
                    Some(word) => expand(word, &variables, &folder),
                    None => vec![home.clone()],
                };
                if let [path] = paths.as_slice() {
                    if path.is_dir() {
                        folder = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
                    }
                }
            }
            _ => placed.push(Located { at, folder: folder.clone() }),
        }
    }
    (found, placed)
}

// --- Python's character classes ------------------------------------------------

/// str.isdigit for one character: the ASCII digits and the superscripts
/// commands can hold.
fn py_isdigit(c: char) -> bool {
    c.is_ascii_digit() || matches!(c, '¹' | '²' | '³' | '⁰' | '⁴'..='⁹' | '₀'..='₉')
}

/// `\w` in a str pattern: letters, digits and the underscore, of any script.
fn py_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `\s` in a str pattern.
fn py_isspace(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// shlex's whitespace: " \t\r\n".
fn py_whitespace(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r' | '\n')
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (name, args) of each call, as the Python gives its tuples.
    fn named(command: &str) -> Vec<(String, Vec<String>)> {
        calls(command).into_iter().map(|call| (call.name, call.args)).collect()
    }

    fn pair(name: &str, args: &[&str]) -> (String, Vec<String>) {
        (name.to_string(), args.iter().map(|arg| arg.to_string()).collect())
    }

    #[test]
    fn a_word_that_only_names_a_command_is_no_call() {
        assert_eq!(named("echo \"git reset --hard\""), vec![pair("echo", &["git reset --hard"])]);
        assert_eq!(named("git commit -m 'pkill -f x'"), vec![pair("git", &["commit", "-m", "pkill -f x"])]);
        assert_eq!(named("cat <<EOF\ngit reset --hard\nEOF"), vec![pair("cat", &[])]);
        assert_eq!(named("cat > notes.md <<'EOF'\nrun cargo fmt --all\nEOF\nls"), vec![pair("cat", &[]), pair("ls", &[])]);
        assert_eq!(named("ls # then git reset --hard"), vec![pair("ls", &[])]);
    }

    #[test]
    fn separators_pipes_and_wrappers_each_start_a_call() {
        assert_eq!(
            named("cd /tmp && timeout 5 git status | head -n 3; sudo rm -rf x || true"),
            vec![
                pair("cd", &["/tmp"]),
                pair("timeout", &[]),
                pair("git", &["status"]),
                pair("head", &["-n", "3"]),
                pair("sudo", &[]),
                pair("rm", &["-rf", "x"]),
                pair("true", &[]),
            ]
        );
        assert_eq!(named("S=/tmp/x; rm -r \"$S\""), vec![pair("=", &["S", "/tmp/x"]), pair("rm", &["-r", "$S"])]);
        // A stray = is a call of that name, which located() does not take
        // for an assignment.
        assert_eq!(named("= y; git reset --hard"), vec![pair("=", &["y"]), pair("git", &["reset", "--hard"])]);
        assert_eq!(named("/usr/bin/git status\nls"), vec![pair("git", &["status"]), pair("ls", &[])]);
    }

    #[test]
    fn a_redirection_and_its_target_are_no_arguments() {
        let found = calls("git status 2>&1 >/dev/null | tail -1");
        assert_eq!(found[0].args, vec!["status"]);
        assert_eq!(found[0].sink, Sink::File);
        assert_eq!(output(&found, 0), Sink::Shown, "the pipe's end decides");
        let found = calls("head -n 5 > out.txt < in.txt");
        assert_eq!(found[0].args, vec!["-n", "5"]);
        assert_eq!((found[0].sink, found[0].stdin.as_deref()), (Sink::File, Some("in.txt")));
        assert_eq!(calls("cat x >&2")[0].sink, Sink::Shown);
    }

    #[test]
    fn substitutions_and_shell_strings_run_their_own_calls() {
        assert_eq!(
            named("echo \"$(git stash drop)\" `whoami`"),
            vec![pair("echo", &[CAPTURED, CAPTURED]), pair("git", &["stash", "drop"]), pair("whoami", &[])]
        );
        let found = calls("timeout 60 bash -c 'git reset --hard && ls' > log");
        assert_eq!(found[2].name, "git");
        assert_eq!(found[2].parent, Some(1));
        assert_eq!(output(&found, 2), Sink::File, "the bash -c's own redirection");
        assert_eq!(output(&found, 3), Sink::File);
        assert_eq!(calls("x=$(cat f)")[1].sink, Sink::Captured);
        assert_eq!(named("diff <(ls a) <(ls b)")[0].1, vec![PROCESS, PROCESS]);
    }

    #[test]
    fn quotes_shlex_cannot_pair_fall_back_to_blanks() {
        assert_eq!(named("echo 'unterminated; git reset --hard"), vec![pair("echo", &["'unterminated"]), pair("git", &["reset", "--hard"])]);
    }

    #[test]
    fn shlex_keeps_what_posix_quoting_keeps() {
        assert_eq!(words(r#"a "b\$c" "d\"e" 'f\g' h\ i '' x"#).unwrap(), vec!["a", "b\\$c", "d\"e", "f\\g", "h i", "", "x"]);
        assert_eq!(words("a;b&&c 2>&1").unwrap(), vec!["a", ";", "b", "&&", "c", "__fd2__", ">&", "1"]);
        assert!(words("echo \"open").is_err());
        assert!(words("echo \\").is_err());
    }

    #[test]
    fn what_reaches_a_call_holds_its_heredoc_herestring_substitution_and_pipe() {
        let text = |command: &str, name: &str| {
            let found = calls(command);
            let at = found.iter().position(|call| call.name == name).unwrap();
            fed(&found, at).join("\n")
        };
        let inline = "gh api graphql -f query=\"$(cat <<'EOF'\nmutation { updateProjectV2Field(input: {}) { x } }\nEOF\n)\"";
        assert!(text(inline, "gh").contains("updateProjectV2Field"));
        let piped = "cat <<'EOF' | gh api graphql -F query=@-\nmutation { updateProjectV2Field }\nEOF";
        assert!(text(piped, "gh").contains("updateProjectV2Field"));
        assert!(text("gh api graphql -F query=@- <<< 'updateProjectV2Field'", "gh").contains("updateProjectV2Field"));
        let direct = "gh api graphql -F query=@- <<EOF\nupdateProjectV2Field\nEOF";
        assert!(text(direct, "gh").contains("updateProjectV2Field"));
        let written = "cat > memory.md <<'EOF'\nnever run gh api graphql with updateProjectV2Field\nEOF\ngh issue list";
        assert!(!text(written, "gh").contains("updateProjectV2Field"), "a here-document written to a file feeds cat, not gh");
    }

    #[test]
    fn located_follows_cd_pushd_and_the_command_s_variables() {
        let root = crate::paths::test_dir("shell-located");
        std::fs::create_dir_all(root.join("repo/sub")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        let command = format!("git status; cd {0}/repo && ls; D={0}/repo/sub; cd \"$D\" && git log; cd nowhere; pwd", root.display());
        let (found, placed) = located(&command, Path::new("/"));
        let at = |name: &str| placed.iter().filter(|p| found[p.at].name == name).map(|p| p.folder.clone()).collect::<Vec<_>>();
        assert_eq!(at("git"), vec![PathBuf::from("/"), root.join("repo/sub")]);
        assert_eq!(at("ls"), vec![root.join("repo")]);
        assert_eq!(at("pwd"), vec![root.join("repo/sub")], "a cd to a folder that is not there leaves it");
        let globbed = format!("cd {}/re* && ls", root.display());
        let (found, placed) = located(&globbed, Path::new("/"));
        assert_eq!((found[placed[0].at].name.as_str(), placed[0].folder.clone()), ("ls", root.join("repo")));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// This port's reading of a corpus, for comparing with the Python's: the
    /// file `EKKO_SHELL_CORPUS` names holds a {"command", "cwd"} per line,
    /// and each call, pipe and folder goes to the file `EKKO_SHELL_OUT`
    /// names. Run by hand over the transcripts' commands, which no public
    /// repository may hold: `cargo test -- --ignored reads_a_corpus`.
    #[test]
    #[ignore]
    fn reads_a_corpus() {
        let (Ok(corpus), Ok(out)) = (std::env::var("EKKO_SHELL_CORPUS"), std::env::var("EKKO_SHELL_OUT")) else { return };
        let mut lines = String::new();
        for line in std::fs::read_to_string(corpus).unwrap().lines() {
            let item: serde_json::Value = serde_json::from_str(line).unwrap();
            let cwd = item["cwd"].as_str().filter(|cwd| !cwd.is_empty()).unwrap_or("/");
            let (found, placed) = located(item["command"].as_str().unwrap(), Path::new(cwd));
            let calls: Vec<serde_json::Value> = (0..found.len())
                .map(|i| {
                    let c = &found[i];
                    serde_json::json!([c.name, c.args, c.sink.word(), output(&found, i).word(), c.stdin, c.piped_to, c.parent])
                })
                .collect();
            let placed: Vec<serde_json::Value> = placed.iter().map(|p| serde_json::json!([p.at, p.folder.to_string_lossy()])).collect();
            lines.push_str(&serde_json::json!({"calls": calls, "located": placed}).to_string());
            lines.push('\n');
        }
        std::fs::write(out, lines).unwrap();
    }

    #[test]
    fn a_here_document_cut_keeps_its_header_and_what_follows() {
        let mut parse = Parse::default();
        let cut = parse.cut_heredocs("cat <<-'EOF' > f\n\tbody\n\tEOF\necho done");
        assert_eq!(cut, format!("cat <<-'EOF'{MARK_OPEN}h0{MARK_CLOSE} > f\necho done"));
        assert_eq!(parse.bodies, vec![("\tbody".to_string(), false)]);
        let mut none = Parse::default();
        assert_eq!(none.cut_heredocs("cat <<EOF\nEOF"), "cat <<EOF\nEOF", "the Python's regex needs a line between");
        assert!(none.bodies.is_empty());
    }
}

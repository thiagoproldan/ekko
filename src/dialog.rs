//! ask's dialog: the question put to the user through MCP elicitation.
//!
//! A question asked in chat, or through Claude Code's own AskUserQuestion,
//! never reached the board, and was lost with the session at a /clear or a
//! restart (note 357). So ask records the question first, then asks the
//! client to show it: an `elicitation/create` request in form mode, which
//! Claude Code shows as a dialog, and whose answer ask records in turn.
//! Under 2026-07-28, whose clients take no request from a server, the same
//! form goes in an `input_required` result instead, and the answer comes
//! back on the call's retry (task 708).
//!
//! The form has one field. With options it is a choice among them plus
//! "Other answer…", which opens a second dialog holding a text field; with
//! none, it is the text field alone. Two fields in one form read as a form
//! for building another form, the user found on 2026-09-23 (note 471), since
//! Claude Code lists a form's fields before it takes a value.
//!
//! What comes back unanswered -- declined, dismissed, failed, or a client with
//! no dialog, such as Claude Code under `-p`, which dismisses it at once --
//! leaves the question open on the board for the chat to take up.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::ops::{Choice, Inquiry};

/// The value of "Other answer…" in the choice: no label can take it, since
/// options are checked against it.
const OTHER: &str = "ekko:other";

/// The line over the options of a question where several may be chosen.
pub const MULTIPLE: &str = "\nOptions, any number of them:";

/// The most options one question offers.
const MOST: usize = 6;

/// Where a question's dialog stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// The choice among the options, or the text field of a question without.
    Asked,
    /// The text field "Other answer…" opened.
    Writing,
}

/// A question the client is showing, waiting for the user.
pub struct Pending {
    /// The id of the tools/call the answer is for.
    pub call: Value,
    /// The questions' uids: a display id could be renumbered before the
    /// answers come. The client's dialog puts only the first.
    pub questions: Vec<String>,
    /// The reply to the write that recorded the question.
    pub recorded: Value,
    pub message: String,
    pub options: Vec<Choice>,
    pub stage: Stage,
    /// The token the call came with, for the progress that keeps it alive.
    pub progress: Option<Value>,
    pub beats: u64,
    /// ekko's own menu, when the questions are there rather than in the
    /// client's dialog.
    pub window: Option<crate::menu::Window>,
    /// Whether the call is a 2026-07-28 request: its replies are complete
    /// results, and its dialog is a result asking for one, which the client
    /// answers by retrying the call, since it takes no request from a server.
    pub modern: bool,
}

/// The dialogs of one server: whether its client shows them, and the ones
/// open, by the id of the request that opened each.
#[derive(Default)]
pub struct Dialogs {
    pub form: bool,
    pub pending: HashMap<String, Pending>,
    sent: u64,
}

impl Dialogs {
    /// Opens `pending`'s dialog at its stage: the request to send.
    pub fn open(&mut self, pending: Pending) -> Value {
        self.sent += 1;
        let id = format!("ekko-ask-{}", self.sent);
        let request = json!({"jsonrpc": "2.0", "id": id, "method": "elicitation/create", "params": params(&pending)});
        self.pending.insert(id, pending);
        request
    }

    /// `pending`'s dialog at its stage for a 2026-07-28 client, which takes
    /// no request from a server: the result asking for it, and the call that
    /// result answers. The client retries the call with the answer and the
    /// `requestState` naming the dialog (see `resume`); the call itself is
    /// over, so nothing keeps it alive any more.
    pub fn input_required(&mut self, mut pending: Pending) -> (Value, Value) {
        self.sent += 1;
        let state = format!("ekko-input-{}", self.sent);
        let result = json!({
            "resultType": "input_required",
            "inputRequests": {"answer": {"method": "elicitation/create", "params": params(&pending)}},
            "requestState": state,
        });
        pending.progress = None;
        let call = pending.call.clone();
        self.pending.insert(state, pending);
        (call, result)
    }

    /// The dialog a retry's `requestState` names, taken out: None for a
    /// state this server never gave, or gave to a retry already.
    pub fn resume(&mut self, state: &str) -> Option<Pending> {
        let given = self.pending.get(state).is_some_and(|pending| pending.modern && pending.window.is_none());
        given.then(|| self.pending.remove(state)).flatten()
    }

    /// Keeps `pending`, whose question is in ekko's menu, until the answer
    /// lands on the board or the window closes.
    pub fn watch(&mut self, pending: Pending) {
        self.sent += 1;
        self.pending.insert(format!("ekko-menu-{}", self.sent), pending);
    }

    /// The pending dialog a client's response is for, taken out: only one
    /// this server sent a request for, which a 2026-07-28 call never has.
    pub fn take(&mut self, id: &Value) -> Option<Pending> {
        let id = id.as_str()?;
        let sent = self.pending.get(id).is_some_and(|pending| !pending.modern && pending.window.is_none());
        sent.then(|| self.pending.remove(id)).flatten()
    }

    /// The dialog opened for tools/call `call`, taken out, with the id of the
    /// request that opened it.
    pub fn take_call(&mut self, call: &Value) -> Option<(String, Pending)> {
        let id = self.pending.iter().find(|(_, pending)| pending.call == *call)?.0.clone();
        self.pending.remove(&id).map(|pending| (id, pending))
    }

    /// A progress notification for each open dialog whose call carried a
    /// token: a stdio call silent for 30 minutes is aborted (mcp.md), and
    /// the user may take longer than that to answer.
    pub fn beats(&mut self) -> Vec<Value> {
        self.pending
            .values_mut()
            .filter_map(|pending| {
                let token = pending.progress.clone()?;
                pending.beats += 1;
                Some(json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/progress",
                    "params": {"progressToken": token, "progress": pending.beats, "message": "Waiting for the user's answer"},
                }))
            })
            .collect()
    }
}

/// Whether a client's capabilities include form-mode elicitation: an empty
/// `elicitation` object means form mode, for backwards compatibility.
pub fn shows_forms(capabilities: &Value) -> bool {
    capabilities.get("elicitation").and_then(Value::as_object).is_some_and(|modes| modes.contains_key("form") || !modes.contains_key("url"))
}

/// Why a question cannot be asked as it is, if it cannot.
pub fn check(inquiry: &Inquiry) -> Option<String> {
    if inquiry.text.trim().is_empty() {
        return Some("each question needs its text".to_string());
    }
    if inquiry.multiple && inquiry.options.is_empty() {
        return Some("multiple needs options to pick among".to_string());
    }
    refuse(&inquiry.options)
}

/// Why options cannot be offered, if they cannot.
pub fn refuse(options: &[Choice]) -> Option<String> {
    if options.len() == 1 || options.len() > MOST {
        return Some(format!("options offers 2 to {MOST} answers, or none for a free-text question"));
    }
    for (n, option) in options.iter().enumerate() {
        let label = option.label.trim();
        if label.is_empty() || label == OTHER {
            return Some("each option needs a label".to_string());
        }
        if label.contains(": ") || label.contains('\n') {
            return Some(format!("a label holds no \": \" and no line break, since the note lists each option as \"label: description\": {label}"));
        }
        if options[..n].iter().any(|earlier| earlier.label.trim() == label) {
            return Some(format!("two options are labelled {label}"));
        }
    }
    None
}

/// The question as the board records it: the text, and the options offered
/// under it, so a session reading it later knows what the user chose among,
/// and whether several could be chosen.
pub fn noted(text: &str, options: &[Choice], multiple: bool) -> String {
    let mut noted = text.trim_end().to_string();
    if !options.is_empty() {
        noted.push_str(if multiple { MULTIPLE } else { "\nOptions:" });
        for option in options {
            noted.push_str(&format!("\n- {}", title(option)));
        }
    }
    noted
}

fn title(option: &Choice) -> String {
    match option.description.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
        Some(description) => format!("{}: {}", option.label.trim(), description.split_whitespace().collect::<Vec<_>>().join(" ")),
        None => option.label.trim().to_string(),
    }
}

/// The form request's params, the same under either revision.
fn params(pending: &Pending) -> Value {
    json!({"mode": "form", "message": pending.message, "requestedSchema": schema(&pending.options, pending.stage)})
}

/// The form: one field, a choice or a text.
fn schema(options: &[Choice], stage: Stage) -> Value {
    let field = if options.is_empty() || stage == Stage::Writing {
        json!({"type": "string", "title": "Answer"})
    } else {
        let mut choices: Vec<Value> = options.iter().map(|option| json!({"const": option.label.trim(), "title": title(option)})).collect();
        choices.push(json!({"const": OTHER, "title": "Other answer…"}));
        json!({"type": "string", "title": "Answer", "oneOf": choices})
    };
    json!({"type": "object", "properties": {"answer": field}, "required": ["answer"]})
}

/// What a dialog came back with.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The user's answer: the label chosen, or the text written.
    Answer(String),
    /// "Other answer…" chosen: the text field comes next.
    Other,
    /// No answer, and why.
    Unanswered(String),
}

pub fn outcome(response: &Value, stage: Stage) -> Outcome {
    if let Some(error) = response.get("error") {
        let message = error.get("message").and_then(Value::as_str).unwrap_or("no message");
        return Outcome::Unanswered(format!("the dialog failed: {message}"));
    }
    let result = response.get("result");
    let answer = result.and_then(|r| r.get("content")).and_then(|c| c.get("answer")).and_then(Value::as_str).map(str::trim);
    match result.and_then(|r| r.get("action")).and_then(Value::as_str) {
        Some("accept") => match answer {
            Some(OTHER) if stage == Stage::Asked => Outcome::Other,
            Some(answer) if !answer.is_empty() && answer != OTHER => Outcome::Answer(answer.to_string()),
            _ => Outcome::Unanswered("the dialog came back without an answer".to_string()),
        },
        Some("decline") => Outcome::Unanswered("the user declined the dialog".to_string()),
        Some("cancel") => Outcome::Unanswered("the user dismissed the dialog".to_string()),
        _ => Outcome::Unanswered("the dialog came back without an action".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice(label: &str, description: Option<&str>) -> Choice {
        Choice { label: label.to_string(), description: description.map(str::to_string), preview: None }
    }

    #[test]
    fn an_empty_elicitation_capability_is_form_mode_and_url_alone_is_not() {
        assert!(shows_forms(&json!({"elicitation": {}})));
        assert!(shows_forms(&json!({"elicitation": {"form": {}, "url": {}}})));
        assert!(!shows_forms(&json!({"elicitation": {"url": {}}})));
        assert!(!shows_forms(&json!({"roots": {}})));
    }

    #[test]
    fn options_are_a_choice_with_other_last_and_other_opens_a_text_field() {
        let options = [choice("Yes", Some("mark all four")), choice("No", None)];
        let asked = schema(&options, Stage::Asked);
        let field = &asked["properties"]["answer"];
        assert_eq!(field["oneOf"][0], json!({"const": "Yes", "title": "Yes: mark all four"}));
        assert_eq!(field["oneOf"][1], json!({"const": "No", "title": "No"}));
        assert_eq!(field["oneOf"][2]["const"], OTHER);
        assert_eq!(asked["required"], json!(["answer"]));
        assert_eq!(schema(&options, Stage::Writing)["properties"]["answer"], json!({"type": "string", "title": "Answer"}));
        assert_eq!(schema(&[], Stage::Asked)["properties"]["answer"], json!({"type": "string", "title": "Answer"}));
    }

    #[test]
    fn what_comes_back_is_an_answer_other_or_why_there_is_none() {
        let accept = |answer: &str| json!({"result": {"action": "accept", "content": {"answer": answer}}});
        assert_eq!(outcome(&accept(" Yes "), Stage::Asked), Outcome::Answer("Yes".to_string()));
        assert_eq!(outcome(&accept(OTHER), Stage::Asked), Outcome::Other);
        assert!(matches!(outcome(&accept(OTHER), Stage::Writing), Outcome::Unanswered(_)));
        assert!(matches!(outcome(&accept("  "), Stage::Writing), Outcome::Unanswered(_)));
        assert_eq!(outcome(&json!({"result": {"action": "decline"}}), Stage::Asked), Outcome::Unanswered("the user declined the dialog".to_string()));
        assert_eq!(outcome(&json!({"result": {"action": "cancel"}}), Stage::Asked), Outcome::Unanswered("the user dismissed the dialog".to_string()));
        assert_eq!(
            outcome(&json!({"error": {"code": -32602, "message": "bad schema"}}), Stage::Asked),
            Outcome::Unanswered("the dialog failed: bad schema".to_string())
        );
    }

    #[test]
    fn options_are_refused_alone_empty_repeated_or_too_many() {
        assert!(refuse(&[]).is_none());
        assert!(refuse(&[choice("A", None), choice("B", None)]).is_none());
        assert!(refuse(&[choice("A", None)]).is_some());
        assert!(refuse(&[choice("A", None), choice(" ", None)]).is_some());
        assert!(refuse(&[choice("A", None), choice(OTHER, None)]).is_some());
        assert!(refuse(&[choice("A", None), choice(" A", None)]).is_some());
        assert!(refuse(&(0..7).map(|n| choice(&n.to_string(), None)).collect::<Vec<_>>()).is_some());
    }

    #[test]
    fn the_board_records_the_options_under_the_question() {
        assert_eq!(noted("Mark them?", &[], false), "Mark them?");
        let options = [choice("Yes", Some("all\nfour")), choice("No", None)];
        assert_eq!(noted("Mark them?\n", &options, false), "Mark them?\nOptions:\n- Yes: all four\n- No");
        assert_eq!(noted("Which?", &options, true), "Which?\nOptions, any number of them:\n- Yes: all four\n- No");
    }
}

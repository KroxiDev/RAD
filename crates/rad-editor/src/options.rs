//! Dynamic option lists of a plugin form: what to ask the runtime, what each answer means and
//! when it no longer applies. Pure state; sockets and drawing live elsewhere.
use crate::{Request, plugins::FormDraft, session::Error};
use rad_plugin_contract::{
    ChoiceList, ChoiceOption, Definition, FieldKind, MAX_CHOICES, Parameters,
};
use serde_json::{Map, Value};

/// What a form configures, as the runtime's `options` operation needs to know it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target<'a> {
    pub type_key: &'a str,
    /// The saved action or visual source being edited; `None` while one is being added.
    pub entity: Option<&'a str>,
}

/// Where one dynamic field's list stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListState {
    /// Not asked yet, or the context changed since: a question is due.
    Due,
    /// The question with this ticket is in flight.
    Asking(u64),
    Answered(ChoiceList),
    Failed(Failure),
}

/// Why a question got no list. None of them touches the saved configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// `options_timeout`: the plugin did not answer in time.
    NoAnswer,
    /// `resource_exhausted`: the plugin already has its questions pending, or its list does
    /// not fit in an answer (that one is not fixed by asking again).
    Busy,
    /// `executor_unavailable`: the plugin is absent or not ready.
    PluginUnavailable,
    /// Any other error code, shown as the editor shows errors.
    Other(String),
}

impl Failure {
    fn of(code: &str) -> Self {
        match code {
            "options_timeout" => Self::NoAnswer,
            "resource_exhausted" => Self::Busy,
            "executor_unavailable" => Self::PluginUnavailable,
            other => Self::Other(other.into()),
        }
    }
}

/// A question to put to the runtime, tagged so only its own answer is accepted.
#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub ticket: u64,
    pub field: usize,
    pub request: Request,
}

/// Per field: the state of its list, `None` for any field that is not a `dynamic_choice`.
#[derive(Debug, Clone)]
pub(crate) struct OptionLists {
    states: Vec<Option<ListState>>,
    /// The field values the form was loaded with: what the runtime would merge into a
    /// question asked by entity.
    saved: Vec<Option<String>>,
}

impl OptionLists {
    pub(crate) fn new(definition: &Definition, saved: Vec<Option<String>>) -> Self {
        let states = definition
            .schema
            .fields
            .iter()
            .map(|f| matches!(f.kind, FieldKind::DynamicChoice { .. }).then_some(ListState::Due))
            .collect();
        Self { states, saved }
    }
    /// The value of field `index` changed: every list that depends on it, directly or through
    /// another list, no longer applies. Its question in flight (if any) is orphaned.
    pub(crate) fn invalidate_dependents(&mut self, definition: &Definition, index: usize) {
        let fields = &definition.schema.fields;
        let mut stale = vec![fields[index].id.as_str()];
        for (at, field) in fields.iter().enumerate().skip(index + 1) {
            if let FieldKind::DynamicChoice { depends_on } = &field.kind
                && depends_on.iter().any(|d| stale.contains(&d.as_str()))
            {
                self.states[at] = Some(ListState::Due);
                stale.push(&field.id);
            }
        }
    }
}

/// The list in an `ok` answer for `field`, or `None` when the answer breaks the contract:
/// the editor draws no list it cannot trust.
fn parse_list(data: &Value, field: &str) -> Option<ChoiceList> {
    let text = |v: &Value| {
        v.as_str()
            .filter(|s| !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control))
            .map(str::to_owned)
    };
    if data["field"] != field {
        return None;
    }
    let items = data["items"]
        .as_array()
        .filter(|a| a.len() <= MAX_CHOICES)?;
    match data["availability"].as_str()? {
        "available" => items
            .iter()
            .map(|item| {
                Some(ChoiceOption {
                    value: text(&item["value"])?,
                    label: text(&item["label"])?,
                })
            })
            .collect::<Option<Vec<_>>>()
            .map(|options| ChoiceList::Available { options }),
        "unavailable" => items.is_empty().then_some(ChoiceList::Unavailable),
        _ => None,
    }
}

/// A value that takes part in a question: set, and not the empty "nothing chosen yet".
pub(crate) fn fixed(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|v| !v.is_empty())
}

impl FormDraft {
    /// State of the list of a dynamic field; `None` for any other field.
    pub fn list(&self, index: usize) -> Option<&ListState> {
        self.options.states.get(index)?.as_ref()
    }
    /// The draft holds a value the current list does not offer. Keep it and show it as
    /// unavailable; never replace it silently.
    pub fn is_unavailable(&self, index: usize) -> bool {
        let Some(value) = self.fields.get(index).and_then(fixed) else {
            return false;
        };
        match self.list(index) {
            Some(ListState::Answered(ChoiceList::Available { options })) => {
                options.iter().all(|option| option.value != value)
            }
            Some(ListState::Answered(ChoiceList::Unavailable)) => true,
            _ => false,
        }
    }
    /// Label of the first required dependency of the dynamic field `index` that has no value:
    /// the list cannot be known before it, so it is not asked.
    pub fn waiting_for(&self, index: usize) -> Option<&str> {
        let fields = &self.definition.schema.fields;
        let FieldKind::DynamicChoice { depends_on } = &fields.get(index)?.kind else {
            return None;
        };
        depends_on
            .iter()
            .filter_map(|dependency| fields.iter().position(|f| &f.id == dependency))
            .find(|&at| fields[at].required && fixed(&self.fields[at]).is_none())
            .map(|at| fields[at].label.as_str())
    }
    /// The user asks again for one list; a list whose question is in flight is left alone.
    pub fn refresh_list(&mut self, index: usize) {
        if let Some(state @ Some(ListState::Answered(_) | ListState::Failed(_))) =
            self.options.states.get_mut(index)
        {
            *state = Some(ListState::Due);
        }
    }
    /// The next question due, if none is in flight: at most one per form.
    pub fn next_query(&mut self, target: Target<'_>, ticket: u64) -> Option<Query> {
        if self
            .options
            .states
            .iter()
            .flatten()
            .any(|state| matches!(state, ListState::Asking(_)))
        {
            return None;
        }
        for index in 0..self.options.states.len() {
            // A field the user has not set offers no control, so there is no list to show; one
            // that waits for a required dependency has no list to ask for yet.
            if self.list(index) != Some(&ListState::Due)
                || self.fields[index].is_none()
                || self.waiting_for(index).is_some()
            {
                continue;
            }
            match self.request_for(index, target) {
                Some(request) => {
                    self.options.states[index] = Some(ListState::Asking(ticket));
                    return Some(Query {
                        ticket,
                        field: index,
                        request,
                    });
                }
                None => {
                    let failure = Failure::Other("invalid_request".into());
                    self.options.states[index] = Some(ListState::Failed(failure));
                }
            }
        }
        None
    }
    /// Takes the runtime's answer to the question with `ticket`; an answer to any other
    /// question (a context that has since changed, a closed form) is ignored.
    pub fn accept_answer(&mut self, ticket: u64, answer: Result<Value, Error>) -> bool {
        let asked = Some(ListState::Asking(ticket));
        let Some(index) = self.options.states.iter().position(|state| *state == asked) else {
            return false;
        };
        let state = match answer {
            Ok(data) => parse_list(&data, &self.definition.schema.fields[index].id)
                .map(ListState::Answered)
                .unwrap_or_else(|| ListState::Failed(Failure::Other("invalid_request".into()))),
            Err(error) => ListState::Failed(Failure::of(&error.code)),
        };
        self.options.states[index] = Some(state);
        true
    }

    /// The question for a dynamic field as the draft stands, or `None` when the draft holds a
    /// context the contract would reject: that is not worth a round trip.
    fn request_for(&self, index: usize, target: Target<'_>) -> Option<Request> {
        let fields = &self.definition.schema.fields;
        let field = &fields[index];
        let FieldKind::DynamicChoice { depends_on } = &field.kind else {
            return None;
        };
        let mut context = Map::new();
        let mut cleared = false;
        for dependency in depends_on {
            let at = fields.iter().position(|f| &f.id == dependency)?;
            match fixed(&self.fields[at]) {
                Some(value) => {
                    context.insert(dependency.clone(), Value::String(value.into()));
                }
                None => cleared |= fixed(&self.options.saved[at]).is_some(),
            }
        }
        let parameters = Parameters {
            schema_version: self.definition.schema.version,
            values: context.clone(),
        };
        self.definition
            .validate_options_context(&field.id, &parameters)
            .ok()?;
        // A question by entity gets the saved values of the dependencies unless the context
        // overrides them, and a context cannot say "unset": asked by type, nothing comes back.
        let id = target.entity.filter(|_| !cleared);
        Some(crate::Request::Options {
            id: id.map(str::to_owned),
            type_key: id.is_none().then(|| target.type_key.to_owned()),
            field: field.id.clone(),
            cursor: None,
            context: if context.is_empty() {
                None
            } else {
                Some(
                    context
                        .into_iter()
                        .map(|(k, v)| Some((k, v.as_str()?.to_owned())))
                        .collect::<Option<_>>()?,
                )
            },
        })
    }
}

//! One captured action edit and its derived visual, composed off the GUI thread.
use crate::{
    Request,
    plugins::PluginTexts,
    session::{Error, Port, Session},
};
use rad_api::{ActionParameters, Change, EntityFields, FieldEdit, InspectorView, VisualSource};
use rad_client::response::Response;
use serde_json::Value;

#[derive(Debug, Clone)]
pub enum Mutation {
    Add { parameters: Value },
    Edit { id: String, parameters: Value },
    Remove { id: String },
}
#[derive(Debug, Clone)]
pub struct LinkedAction {
    pub button: String,
    pub type_key: String,
    pub texts: PluginTexts,
    pub mutation: Mutation,
}
fn changed() -> Error {
    Error {
        code: "cursor_expired".into(),
        cause: None,
        source_change: None,
        unknown: false,
    }
}
fn inspect<P: Port>(
    session: &mut Session<P>,
    id: &str,
    revision: Option<&str>,
) -> Result<InspectorView, Error> {
    let rad_api::ResponseData::Inspector(view) =
        session.request(Request::Inspector { id: id.into() })?.data
    else {
        return Err(Error::protocol());
    };
    if view.entity.id != id
        || view.entity.view_revision != view.view_revision
        || revision.is_some_and(|r| r != view.view_revision)
    {
        return Err(changed());
    }
    Ok(view)
}
fn source(plugin: rad_api::PluginConfiguration) -> VisualSource {
    VisualSource {
        type_key: plugin.type_key,
        parameters: Some(plugin.parameters),
    }
}
impl LinkedAction {
    fn binding<P: Port>(
        &self,
        session: &mut Session<P>,
        id: &str,
        key: &str,
        revision: &str,
    ) -> Result<Option<VisualSource>, Error> {
        let action = inspect(session, id, Some(revision))?;
        if !matches!(action.entity.entity, EntityFields::Action(ref f) if f.parent_id.as_deref() == Some(&self.button) && f.type_key == key)
        {
            return Err(changed());
        }
        let plugin = action
            .plugin
            .filter(|p| p.type_key == key)
            .ok_or_else(changed)?;
        Ok(self.texts.bound_visual(
            key,
            &serde_json::to_value(plugin.parameters).map_err(|_| Error::protocol())?,
        ))
    }
    pub fn publish<P: Port>(&self, session: &mut Session<P>) -> Result<Response, Error> {
        let view = inspect(session, &self.button, None)?;
        if view.appearance.is_none() || view.actions.len() > 20 {
            return Err(Error::protocol());
        }
        let current = view.plugin.map(source);
        let id = match &self.mutation {
            Mutation::Add { .. } => None,
            Mutation::Edit { id, .. } | Mutation::Remove { id } => Some(id),
        };
        if let Some(id) = id
            && !view.actions.iter().any(|a| matches!(a, rad_api::ActionView::Visible { id: found, entity: EntityFields::Action(fields), view_revision, .. } if found == id && fields.type_key == self.type_key && view_revision == &view.view_revision)) {
            return Err(changed());
        }
        let original = id
            .map(|id| self.binding(session, id, &self.type_key, &view.view_revision))
            .transpose()?
            .flatten();
        let (change, desired) = match &self.mutation {
            Mutation::Add { parameters } => (
                Change::AddAction {
                    button_id: self.button.clone(),
                    type_key: self.type_key.clone(),
                    parameters: serde_json::from_value::<ActionParameters>(parameters.clone())
                        .map_err(|_| Error::protocol())?,
                },
                self.texts.bound_visual(&self.type_key, parameters),
            ),
            Mutation::Edit { id, parameters } => (
                Change::Set {
                    id: id.clone(),
                    edit: FieldEdit::Parameters(
                        serde_json::from_value(parameters.clone())
                            .map_err(|_| Error::protocol())?,
                    ),
                },
                self.texts.bound_visual(&self.type_key, parameters),
            ),
            Mutation::Remove { id } => (Change::RemoveAction { id: id.clone() }, None),
        };
        let mut changes = vec![change];
        let owns_current = current.is_some() && current == original;
        let mut shared = false;
        if owns_current {
            for action in &view.actions {
                let rad_api::ActionView::Visible {
                    id: other,
                    entity: EntityFields::Action(fields),
                    view_revision,
                    ..
                } = action
                else {
                    return Err(Error::protocol());
                };
                if view_revision != &view.view_revision {
                    return Err(changed());
                }
                if Some(other) != id
                    && self.texts.binds_visual(&fields.type_key)
                    && self.binding(session, other, &fields.type_key, &view.view_revision)?
                        == current
                {
                    shared = true;
                    break;
                }
            }
        }
        // No owner is persisted: only an exact, unshared derived binding may be changed.
        let replace = if id.is_none() {
            current.is_none() && desired.is_some()
        } else {
            (current.is_none() && desired.is_some()) || (owns_current && !shared)
        };
        if replace && current != desired {
            changes.push(Change::Set {
                id: self.button.clone(),
                edit: FieldEdit::Visual(desired.unwrap_or(VisualSource {
                    type_key: "manual".into(),
                    parameters: None,
                })),
            });
        }
        session.request(Request::Edit {
            changes,
            expected_revision: Some(view.view_revision),
        })
    }
}

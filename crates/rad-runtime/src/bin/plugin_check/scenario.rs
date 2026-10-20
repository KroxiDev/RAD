use rad_plugin_contract::{FieldKind, Gesture, Manifest, OptionsScope, Parameters};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub version: u32,
    /// Explicit consent to execute native code; not a sandbox declaration.
    pub trusted_native_code: bool,
    pub timeout_ms: u64,
    pub freshness_ms: u64,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub settings: Option<Parameters>,
    pub actions: Vec<Action>,
    pub options: Vec<Options>,
    pub visuals: Vec<Visual>,
    pub pressure: Pressure,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Action {
    #[serde(default = "press")]
    pub gesture: Gesture,
    pub id: String,
    pub parameters: Parameters,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub scope: OptionsScope,
    pub definition: String,
    pub field: String,
    pub context: Parameters,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Visual {
    pub id: String,
    pub parameters: Parameters,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Pressure {
    /// Index in options: only this explicitly repeatable query is replayed.
    Options {
        index: usize,
        safe_to_repeat: bool,
    },
    /// Repeated subscribe/unsubscribe to this explicitly safe source.
    Visual {
        index: usize,
        safe_to_repeat: bool,
    },
    Action {
        index: usize,
        safe_to_repeat: bool,
    },
}
fn press() -> Gesture {
    Gesture::Press
}
impl Scenario {
    pub fn validate(&self, manifest: &Manifest) -> Result<(), &'static str> {
        if self.version != 1
            || !self.trusted_native_code
            || !(100..=10_000).contains(&self.timeout_ms)
            || !(1100..=65_000).contains(&self.freshness_ms)
            || self.args.len() > 32
            || self.args.iter().any(|a| a.len() > 4096)
            || self.actions.len() > 128
            || self.options.len() > 256
            || self.visuals.len() > 32
        {
            return Err("scenario_bounds_or_consent");
        }
        // Contract v1 has no Settings options scope; never certify an unexercisable field.
        if manifest.settings.as_ref().is_some_and(|schema| {
            schema
                .fields
                .iter()
                .any(|field| matches!(field.kind, FieldKind::DynamicChoice { .. }))
        }) {
            return Err("settings_dynamic_options_unobservable");
        }
        match (&manifest.settings, &self.settings) {
            (Some(schema), Some(parameters)) => parameters
                .validate(&rad_plugin_contract::Definition {
                    id: "settings".into(),
                    label: "Settings".into(),
                    schema: schema.clone(),
                    credential_slots: vec![],
                    response_timeout: None,
                    gestures: rad_plugin_contract::default_gestures(),
                })
                .map_err(|_| "settings_parameters")?,
            (None, None) => {}
            _ => return Err("incomplete_settings"),
        }
        for definition in &manifest.actions {
            if !definition.gestures.iter().all(|g| {
                self.actions
                    .iter()
                    .any(|a| a.id == definition.id && a.gesture == *g)
            }) {
                return Err("incomplete_actions");
            }
        }
        for definition in &manifest.visuals {
            if !self.visuals.iter().any(|v| v.id == definition.id) {
                return Err("incomplete_visuals");
            }
        }
        for (scope, definitions) in [
            (OptionsScope::Action, &manifest.actions),
            (OptionsScope::Visual, &manifest.visuals),
        ] {
            for definition in definitions {
                for field in &definition.schema.fields {
                    if matches!(field.kind, FieldKind::DynamicChoice { .. })
                        && !self.options.iter().any(|o| {
                            o.scope == scope && o.definition == definition.id && o.field == field.id
                        })
                    {
                        return Err("incomplete_dynamic_options");
                    }
                }
            }
        }
        for action in &self.actions {
            let d = manifest
                .actions
                .iter()
                .find(|d| d.id == action.id)
                .ok_or("unknown_action")?;
            if !d.gestures.contains(&action.gesture) {
                return Err("unsupported_gesture");
            }
            action
                .parameters
                .validate(d)
                .map_err(|_| "action_parameters")?;
        }
        for visual in &self.visuals {
            let d = manifest
                .visuals
                .iter()
                .find(|d| d.id == visual.id)
                .ok_or("unknown_visual")?;
            visual
                .parameters
                .validate(d)
                .map_err(|_| "visual_parameters")?;
        }
        for options in &self.options {
            let defs = match options.scope {
                OptionsScope::Action => &manifest.actions,
                OptionsScope::Visual => &manifest.visuals,
            };
            let d = defs
                .iter()
                .find(|d| d.id == options.definition)
                .ok_or("unknown_options")?;
            d.validate_options_context(&options.field, &options.context)
                .map_err(|_| "options_context")?;
        }
        match self.pressure {
            Pressure::Options {
                index,
                safe_to_repeat: true,
            } if index < self.options.len() => {}
            Pressure::Visual {
                index,
                safe_to_repeat: true,
            } if index < self.visuals.len() => {}
            Pressure::Action {
                index,
                safe_to_repeat: true,
            } if index < self.actions.len() => {}
            _ => return Err("no_safe_pressure_vector"),
        }
        Ok(())
    }
}

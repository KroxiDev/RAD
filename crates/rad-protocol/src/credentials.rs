//! Editor-only secret mutation DTO; no credential value is returned by queries.
use super::*;
#[derive(Debug, PartialEq)]
pub enum CredentialOperation {
    Status {
        plugin_id: String,
        slot: String,
    },
    Replace {
        plugin_id: String,
        secret: rad_plugin_contract::CredentialValue,
    },
    Remove {
        plugin_id: String,
        slot: String,
    },
}
impl CredentialOperation {
    pub fn plugin_id(&self) -> &str {
        match self {
            Self::Status { plugin_id, .. }
            | Self::Replace { plugin_id, .. }
            | Self::Remove { plugin_id, .. } => plugin_id,
        }
    }
    pub fn slot(&self) -> &str {
        match self {
            Self::Status { slot, .. } | Self::Remove { slot, .. } => slot,
            Self::Replace { secret, .. } => &secret.slot,
        }
    }
    pub fn mutation(&self) -> bool {
        !matches!(self, Self::Status { .. })
    }
}
pub(super) fn convert(
    c: IncomingContext,
    request: &rad_api::Request,
) -> Checked<CredentialOperation> {
    use rad_api::Request as R;
    if c.origin != Origin::Editor {
        return Err("permission_denied");
    }
    Ok(match request {
        R::CredentialStatus { plugin_id, slot } => CredentialOperation::Status {
            plugin_id: identifier(plugin_id)?,
            slot: identifier(slot)?,
        },
        R::CredentialRemove { plugin_id, slot } => CredentialOperation::Remove {
            plugin_id: identifier(plugin_id)?,
            slot: identifier(slot)?,
        },
        R::CredentialReplace {
            plugin_id,
            slot,
            secret,
        } => {
            if secret.len() > 4096 {
                return Err("invalid_request");
            }
            CredentialOperation::Replace {
                plugin_id: identifier(plugin_id)?,
                secret: rad_plugin_contract::CredentialValue {
                    slot: identifier(slot)?,
                    value: secret.clone(),
                },
            }
        }
        _ => return Err("invalid_request"),
    })
}
fn identifier(s: &str) -> Checked<String> {
    if s.is_empty()
        || s.len() > 96
        || !s.as_bytes()[0].is_ascii_alphanumeric()
        || !s
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
    {
        return Err("invalid_request");
    }
    Ok(s.into())
}

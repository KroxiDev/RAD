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
pub(super) fn parse(
    c: IncomingContext,
    op: &str,
    args: &Map<String, Value>,
) -> Checked<CredentialOperation> {
    if c.origin != Origin::Editor {
        return Err("permission_denied");
    }
    closed(
        args,
        if op == "credential_replace" {
            &["plugin_id", "slot", "secret"]
        } else {
            &["plugin_id", "slot"]
        },
    )?;
    let plugin_id = identifier(string(args, "plugin_id")?)?;
    let slot = identifier(string(args, "slot")?)?;
    Ok(match op {
        "credential_status" => CredentialOperation::Status { plugin_id, slot },
        "credential_remove" => CredentialOperation::Remove { plugin_id, slot },
        "credential_replace" => {
            let value = string(args, "secret")?;
            if value.len() > 4096 {
                return Err("invalid_request");
            }
            CredentialOperation::Replace {
                plugin_id,
                secret: rad_plugin_contract::CredentialValue {
                    slot,
                    value: value.into(),
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

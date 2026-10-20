//! Presentation of runtime facts; never infers hardware state from domain revision.
use serde_json::Value;
pub fn deck_label(
    status: &Value,
    es: impl Into<crate::preferences::Language> + Copy,
) -> &'static str {
    crate::texts::tr(
        es,
        match status["deck_state"].as_str() {
            Some("disabled") => "Deck disabled — editor available",
            Some("disconnected") => "Deck disconnected — editor available",
            Some("connected") => "Deck connected",
            Some("inaccessible") => "Deck inaccessible — editor available",
            _ => "Deck state unknown",
        },
    )
}
pub fn recovery_notice(
    status: &Value,
    es: impl Into<crate::preferences::Language> + Copy,
) -> Option<&'static str> {
    (status["recovered_previous"] == true).then(|| {
        crate::texts::tr(
            es,
            "Recovered the last valid backup. Recent changes may be missing.",
        )
    })
}
/// A sentence for a known error code. Unknown codes have a plain fallback; the caller retains technical details.
pub fn describe_error(code: &str, es: impl Into<crate::preferences::Language> + Copy) -> &str {
    let key = match code {
        "unsupported_version" => "Update RAD to open this backup",
        "plugin_failed" => "Plugin failed",
        "plugin_limit" => "Maximum of 32 plugins reached",
        "protocol" => "Plugin sent an invalid protocol message",
        "pipe" => "Plugin communication failed",
        "exited" => "Plugin process exited",
        "backpressure" => "Plugin communication queue is full",
        "plugin_in_use" => "Plugin in use — retry manually later",
        "plugin_residual" => "Plugin has residual resources",
        "publication_pending" => "Another publication is pending",
        "incompatible_plugin" => "Incompatible plugin — configuration preserved",
        "invalid_source" => "Use a public GitHub repository or tagged release",
        "invalid_package" => "Invalid plugin package",
        "plugin_already_installed" => "Plugin already installed — use Update",
        "plugin_mismatch" => "That package belongs to a different plugin",
        "plugin_same_version" => "That version is already selected — nothing to update",
        "network" => "Plugin download failed",
        "deadline" => "Plugin operation timed out",
        "io" => "Plugin storage operation failed",
        "recovery_required" => "Plugin selection uncertain — restart RAD before continuing",
        "invalid_configuration" => "That change is not valid for this key",
        "invalid_request" => "That request is not valid",
        "operation_not_allowed" => "That is not allowed here",
        "permission_denied" => "Permission denied",
        "not_found" => "It no longer exists",
        "busy" => "Busy: that key is still running",
        "resource_exhausted" => "Limit reached",
        "storage_error" => "Could not save to disk",
        "storage_uncertain" => "Unsure whether it was saved; check before continuing",
        "deadline_exceeded" => "RAD took too long to answer",
        "runtime_unavailable" => "RAD is not running",
        "executor_unavailable" | "service_unavailable" => "That action is not available right now",
        "options_timeout" => "The plugin did not answer in time; try again",
        "cursor_expired" => "The list changed; search again",
        "file_error" => "Could not read or write that file",
        "result_expired" => "That result is no longer available",
        _ => "Something went wrong",
    };
    crate::texts::tr(es, key)
}

/// Keeps the wire code for details while explaining capacity failures with a way out.
pub fn describe_failure(
    error: &crate::session::Error,
    es: impl Into<crate::preferences::Language> + Copy,
) -> &str {
    if error.code != "resource_exhausted" {
        return describe_error(&error.code, es);
    }
    let key = match error.cause.as_deref() {
        Some("entity_limit") => {
            "No more buttons can be created: maximum reached — remove unused items"
        }
        Some("identity_counter_exhausted") => {
            "No new identities are available — export a backup and contact support"
        }
        Some("document_limit") => {
            "Configuration is too large — shorten texts or remove unused items"
        }
        Some("storage_capacity") => "Not enough storage to save — free disk space and retry",
        _ => return describe_error(&error.code, es),
    };
    crate::texts::tr(es, key)
}

/// Overview attention, deduplicated into the same four states as the Plugins panel.
/// IDs stay in Details; the bar offers only navigation, never a speculative retry.
pub fn plugin_attention_states(status: &Value) -> Vec<PluginState> {
    let mut states = Vec::new();
    if let Some(plugins) = status["plugins"].as_array() {
        for plugin in plugins {
            let state = match serde_json::from_value::<rad_api::PluginAttentionCode>(
                plugin["code"].clone(),
            ) {
                Ok(rad_api::PluginAttentionCode::NeedsCredentials) => PluginState::NeedsPassword,
                Ok(rad_api::PluginAttentionCode::NeedsUpdate) => PluginState::NeedsUpdate,
                Ok(
                    rad_api::PluginAttentionCode::Unavailable
                    | rad_api::PluginAttentionCode::Failed
                    | rad_api::PluginAttentionCode::Busy,
                )
                | Err(_) => PluginState::NotWorking,
            };
            if !states.contains(&state) {
                states.push(state);
            }
        }
    }
    states
}

/// Exactly the four user-facing plugin states. Internal host/manager states stay in details.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginState {
    Ready,
    NeedsPassword,
    NeedsUpdate,
    NotWorking,
}
impl PluginState {
    pub fn of(item: &Value) -> Self {
        if item["needs_credentials"] == true {
            Self::NeedsPassword
        } else if matches!(
            item["diagnostic"].as_str(),
            Some("incompatible_plugin" | "unsupported_version")
        ) || item["needs_update"] == true
        {
            Self::NeedsUpdate
        } else if matches!(item["host"].as_str(), Some("ready" | "absent"))
            && item["diagnostic"].is_null()
        {
            Self::Ready
        } else {
            Self::NotWorking
        }
    }
    pub fn label(self, es: impl Into<crate::preferences::Language> + Copy) -> &'static str {
        crate::texts::tr(
            es,
            match self {
                Self::Ready => "Ready",
                Self::NeedsPassword => "Needs password",
                Self::NeedsUpdate => "Needs update",
                Self::NotWorking => "Not working",
            },
        )
    }
    pub fn action(
        self,
        es: impl Into<crate::preferences::Language> + Copy,
    ) -> Option<&'static str> {
        match self {
            Self::Ready => None,
            Self::NeedsPassword => Some(crate::texts::tr(es, "Enter password")),
            Self::NeedsUpdate => Some(crate::texts::tr(es, "Update")),
            Self::NotWorking => Some(crate::texts::tr(es, "Retry")),
        }
    }
}

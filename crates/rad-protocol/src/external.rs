//! Typed I/O instructions only. Runtime owns bounded workers, leases and revalidation.
use super::*;
#[derive(Debug, Clone, Copy, Default)]
pub struct Capabilities {
    pub transfers: bool,
    pub media: bool,
    pub launcher: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferKind {
    ImageImport,
    Restore,
    Export,
}
#[derive(Debug, PartialEq)]
pub enum External {
    Credential(CredentialOperation),
    PluginAdmin(PluginAdmin),
    PluginOptions(Box<PluginOptions>),
    PluginStatus,
    PluginVisual {
        id: ShortId,
    },
    PluginVisualRead {
        id: ShortId,
        revision: u64,
        offset: usize,
    },
    TransferBegin {
        kind: TransferKind,
        size: Option<u64>,
    },
    TransferWrite {
        transfer_id: String,
        offset: u64,
        bytes: Vec<u8>,
    },
    TransferFinish {
        transfer_id: String,
    },
    TransferRead {
        transfer_id: String,
        offset: u64,
    },
    TransferDrop {
        transfer_id: String,
    },
    RestoreCommit {
        transfer_id: String,
        confirmed: bool,
    },
    MediaRead {
        id: ShortId,
        media_ref: String,
        offset: u64,
    },
    LauncherCatalog {
        cursor: Option<String>,
        query: String,
    },
    LauncherOptions {
        id: ShortId,
        cursor: Option<String>,
    },
}
pub(super) fn parse(
    a: &Authority,
    c: IncomingContext,
    op: &str,
    m: &Map<String, Value>,
    caps: Capabilities,
) -> Checked<External> {
    if op.starts_with("transfer_") || op == "restore_commit" {
        if c.origin != Origin::Editor {
            return Err("permission_denied");
        };
        if !caps.transfers {
            return Err("executor_unavailable");
        }
    }
    match op {
        "transfer_begin" => {
            closed(m, &["kind", "size"])?;
            let kind = match string(m, "kind")? {
                "image_import" => TransferKind::ImageImport,
                "restore" => TransferKind::Restore,
                "export" => TransferKind::Export,
                _ => return Err("invalid_request"),
            };
            let size = match m.get("size") {
                Some(Value::Null) => None,
                Some(Value::String(s)) => Some(decimal(s)?),
                _ => return Err("invalid_request"),
            };
            let max = if kind == TransferKind::ImageImport {
                8 * 1024 * 1024
            } else {
                256 * 1024 * 1024
            };
            if size.is_some_and(|n| n > max) || (kind == TransferKind::Export && size.is_some()) {
                return Err("invalid_request");
            };
            Ok(External::TransferBegin { kind, size })
        }
        "transfer_write" => {
            closed(m, &["transfer_id", "offset", "base64"])?;
            Ok(External::TransferWrite {
                transfer_id: token(m, "transfer_id")?,
                offset: decimal(string(m, "offset")?)?,
                bytes: decode_base64(string(m, "base64")?)?,
            })
        }
        "transfer_read" => {
            closed(m, &["transfer_id", "offset"])?;
            Ok(External::TransferRead {
                transfer_id: token(m, "transfer_id")?,
                offset: decimal(string(m, "offset")?)?,
            })
        }
        "transfer_finish" | "transfer_drop" => {
            closed(m, &["transfer_id"])?;
            let transfer_id = token(m, "transfer_id")?;
            Ok(if op == "transfer_finish" {
                External::TransferFinish { transfer_id }
            } else {
                External::TransferDrop { transfer_id }
            })
        }
        "restore_commit" => {
            closed(m, &["transfer_id", "confirm_replace"])?;
            if m.get("confirm_replace") != Some(&Value::Bool(true)) {
                return Err("invalid_request");
            };
            Ok(External::RestoreCommit {
                transfer_id: token(m, "transfer_id")?,
                confirmed: true,
            })
        }
        "launcher_catalog" => {
            if c.origin != Origin::Editor {
                return Err("permission_denied");
            };
            if !caps.launcher {
                return Err("executor_unavailable");
            };
            closed(m, &["cursor", "query"])?;
            let query = string(m, "query")?;
            if serde_json::to_vec(query).unwrap().len() > 8192 {
                return Err("invalid_request");
            };
            Ok(External::LauncherCatalog {
                cursor: opaque_cursor(m)?,
                query: query.into(),
            })
        }
        "options" => {
            if !caps.launcher {
                return Err("executor_unavailable");
            };
            closed(m, &["id", "field", "cursor"])?;
            let id = id(m, "id")?;
            if string(m, "field")? != "desktop_entry" {
                return Err("invalid_request");
            };
            match a.get(c.origin, id) {
                Some(rad_authority::EntityView::Action(v))
                    if v.kind == rad_authority::ActionType::Launcher => {}
                _ => return Err("not_found"),
            };
            Ok(External::LauncherOptions {
                id,
                cursor: opaque_cursor(m)?,
            })
        }
        "media_read" => {
            if !caps.media {
                return Err("executor_unavailable");
            };
            closed(m, &["id", "offset"])?;
            let id = id(m, "id")?;
            let offset = decimal(string(m, "offset")?)?;
            if offset > 512 * 1024 {
                return Err("invalid_request");
            };
            let media_ref = a
                .appearance(c.origin, id)
                .and_then(|v| v.appearance.image)
                .ok_or("not_found")?;
            Ok(External::MediaRead {
                id,
                media_ref,
                offset,
            })
        }
        _ => Err("invalid_request"),
    }
}
fn token(m: &Map<String, Value>, key: &str) -> Checked<String> {
    let s = string(m, key)?;
    if s.is_empty() || s.len() > 128 || s.chars().any(char::is_control) {
        Err("invalid_request")
    } else {
        Ok(s.into())
    }
}
fn opaque_cursor(m: &Map<String, Value>) -> Checked<Option<String>> {
    match m.get("cursor") {
        Some(Value::Null) => Ok(None),
        Some(Value::String(_)) => token(m, "cursor").map(Some),
        _ => Err("invalid_request"),
    }
}
fn decode_base64(s: &str) -> Checked<Vec<u8>> {
    if s.len() > 10924 || !s.len().is_multiple_of(4) {
        return Err("invalid_request");
    };
    let mut out = Vec::with_capacity((s.len() / 4 * 3).min(8192));
    let digit = |b: u8| -> Checked<u8> {
        match b {
            b'A'..=b'Z' => Ok(b - b'A'),
            b'a'..=b'z' => Ok(b - b'a' + 26),
            b'0'..=b'9' => Ok(b - b'0' + 52),
            b'+' => Ok(62),
            b'/' => Ok(63),
            _ => Err("invalid_request"),
        }
    };
    for (i, c) in s.as_bytes().as_chunks::<4>().0.iter().enumerate() {
        let a = digit(c[0])?;
        let b = digit(c[1])?;
        out.push(a << 2 | b >> 4);
        let last = (i + 1) * 4 == s.len();
        if c[2] == b'=' {
            if !last || c[3] != b'=' || b & 15 != 0 {
                return Err("invalid_request");
            }
        } else {
            let d = digit(c[2])?;
            out.push(b << 4 | d >> 2);
            if c[3] == b'=' {
                if !last || d & 3 != 0 {
                    return Err("invalid_request");
                }
            } else {
                out.push(d << 6 | digit(c[3])?)
            }
        }
    }
    if out.len() > 8192 {
        return Err("invalid_request");
    };
    Ok(out)
}

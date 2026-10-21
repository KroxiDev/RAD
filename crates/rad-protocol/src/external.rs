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
    PluginRetry {
        plugin_id: String,
    },
    PluginLog {
        plugin_id: String,
    },
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
/// The transfer and restore operations, which need no Authority: the Editor's alone, and
/// the same wherever they are served (normal mode, or restoring in recovery mode).
pub fn parse_transfer(origin: Origin, request: &rad_api::Request) -> Checked<External> {
    use rad_api::Request as R;
    if origin != Origin::Editor {
        return Err("permission_denied");
    };
    match request {
        R::TransferBegin { kind, size } => {
            let kind = match kind {
                rad_api::TransferKind::ImageImport => TransferKind::ImageImport,
                rad_api::TransferKind::Restore => TransferKind::Restore,
                rad_api::TransferKind::Export => TransferKind::Export,
            };
            let size = size.as_deref().map(decimal).transpose()?;
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
        R::TransferWrite {
            transfer_id,
            offset,
            base64,
        } => Ok(External::TransferWrite {
            transfer_id: token(transfer_id)?,
            offset: decimal(offset)?,
            bytes: decode_base64(base64)?,
        }),
        R::TransferRead {
            transfer_id,
            offset,
        } => Ok(External::TransferRead {
            transfer_id: token(transfer_id)?,
            offset: decimal(offset)?,
        }),
        R::TransferFinish { transfer_id } | R::TransferDrop { transfer_id } => {
            let transfer_id = token(transfer_id)?;
            Ok(if matches!(request, R::TransferFinish { .. }) {
                External::TransferFinish { transfer_id }
            } else {
                External::TransferDrop { transfer_id }
            })
        }
        R::RestoreCommit {
            transfer_id,
            confirm_replace,
        } => {
            if !confirm_replace {
                return Err("invalid_request");
            };
            Ok(External::RestoreCommit {
                transfer_id: token(transfer_id)?,
                confirmed: true,
            })
        }
        _ => Err("invalid_request"),
    }
}
fn is_transfer(request: &rad_api::Request) -> bool {
    use rad_api::Request as R;
    matches!(
        request,
        R::TransferBegin { .. }
            | R::TransferWrite { .. }
            | R::TransferRead { .. }
            | R::TransferFinish { .. }
            | R::TransferDrop { .. }
            | R::RestoreCommit { .. }
    )
}
pub(super) fn parse(
    a: &Authority,
    c: IncomingContext,
    request: &rad_api::Request,
    caps: Capabilities,
) -> Checked<External> {
    use rad_api::Request as R;
    if is_transfer(request) {
        if c.origin != Origin::Editor {
            return Err("permission_denied");
        };
        if !caps.transfers {
            return Err("executor_unavailable");
        }
        return parse_transfer(c.origin, request);
    }
    match request {
        R::LauncherCatalog { query, cursor } => {
            if c.origin != Origin::Editor {
                return Err("permission_denied");
            };
            if !caps.launcher {
                return Err("executor_unavailable");
            };
            if serde_json::to_vec(query).unwrap().len() > 8192 {
                return Err("invalid_request");
            };
            Ok(External::LauncherCatalog {
                cursor: cursor.as_deref().map(token).transpose()?,
                query: query.clone(),
            })
        }
        R::Options {
            id: Some(id),
            type_key,
            field,
            cursor,
            context,
        } => {
            if !caps.launcher {
                return Err("executor_unavailable");
            };
            let id = parse_id(id)?;
            if type_key.is_some() || context.is_some() || field != "desktop_entry" {
                return Err("invalid_request");
            };
            match a.get(c.origin, id) {
                Some(rad_authority::EntityView::Action(v))
                    if v.kind == rad_authority::ActionType::Launcher => {}
                _ => return Err("not_found"),
            };
            Ok(External::LauncherOptions {
                id,
                cursor: cursor.as_deref().map(token).transpose()?,
            })
        }
        R::MediaRead { id, offset } => {
            if !caps.media {
                return Err("executor_unavailable");
            };
            let id = parse_id(id)?;
            let offset = decimal(offset)?;
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
fn token(s: &str) -> Checked<String> {
    if s.is_empty() || s.len() > 128 || s.chars().any(char::is_control) {
        Err("invalid_request")
    } else {
        Ok(s.into())
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

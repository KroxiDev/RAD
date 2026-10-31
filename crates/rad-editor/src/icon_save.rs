//! Publish a generated image and a captured key edit on the same client session.
use crate::{
    Request,
    session::{Error, Port, Session},
    transfer::{Progress, Upload},
};
use rad_client::response::Response;

/// What a save produced: the runtime's answer and the image it now refers to.
#[derive(Debug)]
pub struct Saved {
    pub response: Response,
    /// Digest of the image as the runtime stored it.
    pub image: String,
}

/// Only cleanup is retryable; the captured edit is never replayed.
#[derive(Debug, Clone, Default)]
pub struct Cleanup {
    pub released: Vec<String>,
    pub retry: Vec<String>,
    pub disconnect: bool,
}
#[derive(Debug)]
pub struct Failed {
    pub error: Box<Error>,
    pub cleanup: Cleanup,
}
impl From<Error> for Failed {
    fn from(error: Error) -> Self {
        Self {
            error: Box::new(error),
            cleanup: Cleanup::default(),
        }
    }
}
/// The runtime holds at most two transfers at once. With more tokens to drop, the
/// editor cannot tell which are still live, so cleanup closes its own connection.
pub(crate) const MAX_PREPARED_IMAGES: usize = 2;
pub(crate) fn drop_images<P: Port>(session: &mut Session<P>, tokens: Vec<String>) -> Cleanup {
    let tokens: std::collections::BTreeSet<_> = tokens.into_iter().collect();
    let mut result = Cleanup::default();
    if tokens.len() > MAX_PREPARED_IMAGES {
        result.disconnect = true;
        return result;
    }
    for token in tokens {
        match session.request(Request::TransferDrop {
            transfer_id: token.clone(),
        }) {
            Ok(_) => result.released.push(token),
            Err(error) if !error.unknown && error.code == "not_found" => {
                result.released.push(token)
            }
            Err(error)
                if !error.unknown
                    && matches!(error.code.as_str(), "resource_exhausted" | "busy") =>
            {
                result.retry.push(token)
            }
            Err(_) => {
                result.disconnect = true;
                break;
            }
        }
    }
    result
}

/// No preparation is held in the GUI: its 60-second lease starts only after Save.
/// The captured edit owns its destination; a later UI selection cannot redirect it.
pub fn save<P: Port>(
    session: &mut Session<P>,
    button: &str,
    png: &[u8],
    edit: Request,
) -> Result<Saved, Failed> {
    save_to_state(session, button, None, png, edit)
}
pub fn save_to_state<P: Port>(
    session: &mut Session<P>,
    button: &str,
    state: Option<&str>,
    png: &[u8],
    mut edit: Request,
) -> Result<Saved, Failed> {
    let Request::Edit { changes, .. } = &mut edit else {
        return Err(Error::protocol().into());
    };
    let appearances: Vec<_> = changes
        .iter()
        .enumerate()
        .filter_map(|(index, change)| match change {
            rad_api::Change::Set {
                edit: rad_api::FieldEdit::Appearance(_),
                ..
            } => Some(index),
            _ => None,
        })
        .collect();
    if appearances.len() != 1
        || !matches!(&changes[appearances[0]], rad_api::Change::Set { id, .. } if id == button)
        || png.len() > 512 * 1024
        || !png.starts_with(b"\x89PNG\r\n\x1a\n")
    {
        return Err(Error::protocol().into());
    }
    let response = session.request(Request::TransferBegin {
        kind: rad_api::TransferKind::ImageImport,
        size: Some(png.len().to_string()),
    })?;
    let rad_api::ResponseData::Transfer(data) = response.data else {
        return Err(Error::protocol().into());
    };
    let transfer_id = data.transfer_id;
    let result: Result<Saved, Error> = (|| {
        let mut upload = Upload::new(
            std::io::Cursor::new(png),
            transfer_id.clone(),
            png.len() as u64,
        );
        let reference = loop {
            if let Progress::Finished(data) = upload.step(session)? {
                break data["media_ref"]
                    .as_str()
                    .filter(|r| !r.is_empty())
                    .ok_or_else(Error::protocol)?
                    .to_owned();
            }
        };
        let Request::Edit { changes, .. } = &mut edit else {
            unreachable!()
        };
        let rad_api::Change::Set {
            edit: rad_api::FieldEdit::Appearance(appearance),
            ..
        } = &mut changes[appearances[0]]
        else {
            unreachable!()
        };
        if let Some(state) = state {
            appearance
                .states
                .get_or_insert_with(Default::default)
                .entry(state.into())
                .or_default()
                .image = Some(reference.clone());
        } else {
            appearance.image = Some(reference.clone());
        }
        session.request(edit).map(|response| Saved {
            response,
            image: reference,
        })
    })();
    result.map_err(|error| {
        let cleanup = if session.is_open() {
            drop_images(session, vec![transfer_id])
        } else {
            Cleanup::default()
        };
        Failed {
            error: Box::new(error),
            cleanup,
        }
    })
}

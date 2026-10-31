//! Publish a generated image and a captured key edit on the same client session.
use crate::{
    Request,
    session::{Error, Port, Session},
    transfer::{Progress, Upload},
};
use rad_client::response::Response;

/// No preparation is held in the GUI: its 60-second lease starts only after Save.
/// The captured edit owns its destination; a later UI selection cannot redirect it.
pub fn save<P: Port>(
    session: &mut Session<P>,
    button: &str,
    png: &[u8],
    mut edit: Request,
) -> Result<Response, Error> {
    let Request::Edit { changes } = &mut edit else {
        return Err(Error::protocol());
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
        return Err(Error::protocol());
    }
    let response = session.request(Request::TransferBegin {
        kind: rad_api::TransferKind::ImageImport,
        size: Some(png.len().to_string()),
    })?;
    let rad_api::ResponseData::Transfer(data) = response.data else {
        return Err(Error::protocol());
    };
    let transfer_id = data.transfer_id;
    let result = (|| {
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
        let Request::Edit { changes } = &mut edit else {
            unreachable!()
        };
        let rad_api::Change::Set {
            edit: rad_api::FieldEdit::Appearance(appearance),
            ..
        } = &mut changes[appearances[0]]
        else {
            unreachable!()
        };
        appearance.image = Some(reference);
        session.request(edit)
    })();
    if result.is_err() && session.is_open() {
        // Keep the original error, including Unknown. Cancellation never repeats the edit.
        let _ = session.request(Request::TransferDrop { transfer_id });
    }
    result
}

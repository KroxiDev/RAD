use rad_client::{
    connection::Failure,
    response::{Response, Status},
};
use rad_editor::{
    session::{Port, Session},
    transfer::{Progress, Upload},
};
use serde_json::json;
struct UploadServer {
    offset: usize,
    finished: bool,
}
impl Port for UploadServer {
    fn call(&mut self, request: rad_api::Request) -> Result<Response, Failure> {
        let op = request.operation().as_str();
        let args = request.arguments();
        let data = match op {
            "transfer_write" => {
                assert_eq!(args["offset"], self.offset.to_string());
                let encoded = args["base64"].as_str().unwrap();
                assert!(encoded.len() <= 10924);
                if self.offset == 0 {
                    assert!(encoded.starts_with("AQEBAQEB"));
                    self.offset += 8192;
                } else {
                    assert_eq!(encoded, "AQ==");
                    self.offset += 1;
                }
                json!({})
            }
            "transfer_finish" => {
                assert_eq!(self.offset, 8193);
                self.finished = true;
                json!({"media_ref":"owned-media"})
            }
            _ => panic!("unexpected {op}"),
        };
        Ok(Response {
            v: 1,
            id: Some("1".into()),
            session_id: None,
            status: Status::Ok,
            data: rad_api::ResponseData::decode(
                rad_api::Operation::parse(op).unwrap(),
                rad_api::Status::Ok,
                data,
            )
            .map_err(|_| Failure {
                code: "invalid_request",
                id: None,
                session_id: None,
                outcome: None,
            })?,
            error: None,
        })
    }
}
#[test]
fn image_upload_yields_between_bounded_chunks_then_prepares_not_publishes() {
    let mut s = Session::new(UploadServer {
        offset: 0,
        finished: false,
    });
    let mut upload = Upload::new(std::io::Cursor::new(vec![1; 8193]), "t1".into(), 8193);
    assert!(matches!(
        upload.step(&mut s).unwrap(),
        Progress::Chunk(8192)
    ));
    assert!(matches!(
        upload.step(&mut s).unwrap(),
        Progress::Chunk(8193)
    ));
    let Progress::Finished(data) = upload.step(&mut s).unwrap() else {
        panic!()
    };
    assert_eq!(data, json!({"media_ref":"owned-media"}));
}
struct ExportServer;
impl Port for ExportServer {
    fn call(&mut self, request: rad_api::Request) -> Result<Response, Failure> {
        let op = request.operation().as_str();
        let args = request.arguments();
        assert_eq!(op, "transfer_read");
        assert_eq!(args["offset"], "0");
        Ok(Response {
            v: 1,
            id: Some("1".into()),
            session_id: None,
            status: Status::Ok,
            data: rad_api::ResponseData::TransferChunk(rad_api::TransferChunk {
                base64: "AAEC/w==".into(),
                eof: true,
            }),
            error: None,
        })
    }
}
#[test]
fn export_writes_exact_bytes_without_loading_whole_package() {
    let mut s = Session::new(ExportServer);
    let mut d = rad_editor::transfer::Download::new(Vec::new(), "t2".into());
    assert!(matches!(d.step(&mut s).unwrap(), Progress::Finished(_)));
    assert_eq!(d.into_inner(), vec![0, 1, 2, 255]);
}

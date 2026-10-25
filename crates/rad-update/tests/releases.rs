use rad_update::release::{Source, check_with};
use rad_update::{Cancellation, Candidate, Error};
use serde_json::{Value, json};
use std::collections::HashMap;
struct Fixtures {
    responses: HashMap<String, Vec<u8>>,
    requests: Vec<String>,
}
impl Fixtures {
    fn new(items: Vec<Value>) -> Self {
        Self {
            responses: HashMap::from([(url(1), serde_json::to_vec(&items).unwrap())]),
            requests: vec![],
        }
    }
    fn metadata(&mut self, version: &str, rel: u32) {
        let tag = format!("v{version}-{rel}");
        let m = json!({"schema":1,"product":"RAD","package":"rust-action-deck","version":version,"pkgrel":rel,"architecture":"x86_64","file":format!("rust-action-deck-{version}-{rel}-x86_64.pkg.tar.zst"),"size":10,"sha256":"a".repeat(64),"base":"omarchy-4.0.6","commit":"b".repeat(40)});
        let bytes = serde_json::to_vec(&m).unwrap();
        let key = url(1);
        if let Some(page) = self.responses.get_mut(&key) {
            let mut releases: Vec<Value> = serde_json::from_slice(page).unwrap();
            for release in &mut releases {
                if release["tag_name"] == tag {
                    release["assets"][0]["size"] = json!(bytes.len());
                }
            }
            *page = serde_json::to_vec(&releases).unwrap();
        }
        self.responses.insert(
            format!("https://github.com/KroxiDev/RAD/releases/download/{tag}/rad-update.json"),
            bytes,
        );
    }
}
impl Source for Fixtures {
    fn get(&mut self, url: &str, _: usize, cancel: &Cancellation) -> Result<Vec<u8>, Error> {
        cancel.check()?;
        self.requests.push(url.into());
        self.responses
            .remove(url)
            .ok_or(Error(format!("unexpected source {url}")))
    }
}
fn url(page: u32) -> String {
    format!("https://api.github.com/repos/KroxiDev/RAD/releases?per_page=100&page={page}")
}
fn release(v: &str, r: u32) -> Value {
    let tag = format!("v{v}-{r}");
    json!({"draft":false,"prerelease":false,"tag_name":tag,"assets":[{"name":"rad-update.json","size":500,"browser_download_url":format!("https://github.com/KroxiDev/RAD/releases/download/{tag}/rad-update.json")},{"name":format!("rust-action-deck-{v}-{r}-x86_64.pkg.tar.zst"),"size":10,"browser_download_url":format!("https://github.com/KroxiDev/RAD/releases/download/{tag}/rust-action-deck-{v}-{r}-x86_64.pkg.tar.zst")}]})
}
fn candidate(value: Option<Candidate>) -> Candidate {
    value.expect("upgrade available")
}
#[test]
fn stable_selection_compares_numeric_versions_and_package_revisions_without_downgrades() {
    let mut fake = Fixtures::new(vec![
        release("0.9.0", 1),
        release("0.10.0", 1),
        release("0.10.0", 2),
    ]);
    for (v, r) in [("0.9.0", 1), ("0.10.0", 1), ("0.10.0", 2)] {
        fake.metadata(v, r);
    }
    let found = candidate(check_with(&mut fake, "0.10.0-1", &Cancellation::default()).unwrap());
    assert_eq!(found.metadata.package_version(), "0.10.0-2");
    let mut fake = Fixtures::new(vec![release("0.9.0", 1)]);
    fake.metadata("0.9.0", 1);
    assert!(
        check_with(&mut fake, "1.0.0-1", &Cancellation::default())
            .unwrap()
            .is_none()
    );
}
#[test]
fn empty_is_no_update_but_malformed_missing_ambiguous_and_limits_are_errors() {
    assert!(
        check_with(
            &mut Fixtures::new(vec![]),
            "0.1.0-1",
            &Cancellation::default()
        )
        .unwrap()
        .is_none()
    );
    let mut malformed = Fixtures {
        responses: HashMap::from([(url(1), b"{}".to_vec())]),
        requests: vec![],
    };
    assert!(check_with(&mut malformed, "0.1.0-1", &Cancellation::default()).is_err());
    let mut missing = Fixtures::new(vec![release("0.2.0", 1)]);
    assert!(check_with(&mut missing, "0.1.0-1", &Cancellation::default()).is_err());
    let mut ambiguous = release("0.2.0", 1);
    let repeated = ambiguous["assets"][0].clone();
    ambiguous["assets"].as_array_mut().unwrap().push(repeated);
    assert!(
        check_with(
            &mut Fixtures::new(vec![ambiguous]),
            "0.1.0-1",
            &Cancellation::default()
        )
        .is_err()
    );
    let old = release("0.1.0", 1);
    let mut limit = Fixtures::new(vec![old.clone(); 100]);
    for page in 2..=5 {
        limit.responses.insert(
            url(page),
            serde_json::to_vec(&vec![old.clone(); 100]).unwrap(),
        );
    }
    assert!(check_with(&mut limit, "1.0.0-1", &Cancellation::default()).is_err());
    assert_eq!(limit.requests.len(), 5);
}
#[test]
fn draft_prerelease_and_mistagged_prerelease_never_fetch_assets_and_cancellation_is_silent() {
    let mut draft = release("9.0.0", 1);
    draft["draft"] = json!(true);
    let mut pre = release("8.0.0", 1);
    pre["prerelease"] = json!(true);
    let mut fake = Fixtures::new(vec![draft, pre, release("10.0.0-beta.1", 1)]);
    assert!(
        check_with(&mut fake, "0.1.0-1", &Cancellation::default())
            .unwrap()
            .is_none()
    );
    assert_eq!(fake.requests.len(), 1);
    let cancellation = Cancellation::default();
    cancellation.cancel();
    let mut fake = Fixtures::new(vec![]);
    assert!(check_with(&mut fake, "0.1.0-1", &cancellation).is_err());
    assert!(fake.requests.is_empty());
}

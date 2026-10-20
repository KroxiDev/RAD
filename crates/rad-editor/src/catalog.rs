use crate::{Request, session::Error};
use serde_json::{Value, json};
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub desktop_entry: String,
    pub name: String,
}
#[derive(Debug, Default)]
pub struct Catalog {
    pub query: String,
    pub entries: Vec<Entry>,
    pub next: Option<String>,
}
impl Catalog {
    pub fn request(&self, next: bool) -> Request {
        Request::new(
            "launcher_catalog",
            json!({"query":self.query,"cursor":if next{self.next.clone()}else{None}}),
        )
    }
    pub fn accept(&mut self, data: &Value) -> Result<(), Error> {
        let items = data["items"]
            .as_array()
            .filter(|v| v.len() <= 32)
            .ok_or_else(Error::protocol)?;
        let entries = items
            .iter()
            .map(|item| {
                Ok(Entry {
                    desktop_entry: item["desktop_entry"]
                        .as_str()
                        .ok_or_else(Error::protocol)?
                        .into(),
                    name: item["name"].as_str().ok_or_else(Error::protocol)?.into(),
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let next = if data["next_cursor"].is_null() {
            None
        } else {
            Some(
                data["next_cursor"]
                    .as_str()
                    .ok_or_else(Error::protocol)?
                    .into(),
            )
        };
        self.entries = entries;
        self.next = next;
        Ok(())
    }
}

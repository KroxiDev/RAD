use crate::{Request, session::Error};

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
        Request::LauncherCatalog {
            query: self.query.clone(),
            cursor: if next { self.next.clone() } else { None },
        }
    }
    pub fn accept(&mut self, data: &rad_api::ResponseData) -> Result<(), Error> {
        let rad_api::ResponseData::Launcher(data) = data else {
            return Err(Error::protocol());
        };
        if data.items.len() > 32 {
            return Err(Error::protocol());
        }
        self.entries = data
            .items
            .iter()
            .map(|e| Entry {
                desktop_entry: e.desktop_entry.clone(),
                name: e.name.clone(),
            })
            .collect();
        self.next = data.next_cursor.clone();
        Ok(())
    }
}

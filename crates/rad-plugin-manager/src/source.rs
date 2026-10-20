use crate::Error;

/// Parsed public source. Raw URLs are deliberately not included in Debug/logs.
#[derive(Clone)]
pub struct GithubSource {
    owner: String,
    repository: String,
    tag: Option<String>,
}
impl GithubSource {
    pub fn parse(input: &str) -> Result<Self, Error> {
        if input.len() > 1024 {
            return Err(Error::InvalidSource);
        }
        let path = input
            .strip_prefix("https://github.com/")
            .ok_or(Error::InvalidSource)?;
        let parts: Vec<_> = path.trim_end_matches('/').split('/').collect();
        if !matches!(parts.len(), 2..=5)
            || !parts.iter().all(|v| component(v))
            || parts.len() >= 3 && parts[2] != "releases"
            || parts.len() == 4 && parts[3] != "latest"
            || parts.len() == 5 && parts[3] != "tag"
        {
            return Err(Error::InvalidSource);
        }
        let repository = parts[1].strip_suffix(".git").unwrap_or(parts[1]);
        if !component(repository) {
            return Err(Error::InvalidSource);
        }
        Ok(Self {
            owner: parts[0].into(),
            repository: repository.into(),
            tag: parts.get(4).map(|s| (*s).into()),
        })
    }
    pub fn canonical_url(&self) -> String {
        let base = format!("https://github.com/{}/{}", self.owner, self.repository);
        self.tag
            .as_ref()
            .map(|tag| format!("{base}/releases/tag/{tag}"))
            .unwrap_or(base)
    }
    pub(crate) fn metadata_url(&self) -> String {
        let release = self
            .tag
            .as_ref()
            .map(|tag| format!("tags/{tag}"))
            .unwrap_or_else(|| "latest".into());
        format!(
            "https://api.github.com/repos/{}/{}/releases/{release}",
            self.owner, self.repository
        )
    }
}
fn component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

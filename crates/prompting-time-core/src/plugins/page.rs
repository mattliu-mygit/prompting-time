//! The files of a plugin's page, which PT serves to the page's sandboxed frame. A page reads only
//! the files in the folder that holds its manifest's `page.entry`, such as `page/dist/` for
//! `page/dist/index.html`.

use std::fs;
use std::path::{Component, Path};

use super::PluginHost;

/// Why PT does not serve a page file.
#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum PageFileError {
    #[error("no plugin named {0} is installed")]
    UnknownPlugin(String),
    /// The path leaves the page's folder: through `..`, or a symlink that points outside it.
    #[error("{0} is outside the plugin's page folder")]
    Outside(String),
    #[error("the plugin's page has no file {0}")]
    NotFound(String),
}

impl PluginHost {
    /// The bytes of the file at `path` in the plugin's page folder. `path` is a decoded URL path,
    /// such as `/assets/index.js` for `<page folder>/assets/index.js`.
    pub fn page_file(&self, plugin: &str, path: &str) -> Result<Vec<u8>, PageFileError> {
        let folder = {
            let plugins = self.inner.plugins();
            let installed = &plugins
                .get(plugin)
                .ok_or_else(|| PageFileError::UnknownPlugin(plugin.to_owned()))?
                .installed;
            installed.folder.join(installed.manifest.page.folder())
        };
        let relative = Path::new(path.trim_start_matches('/'));
        if relative.as_os_str().is_empty() {
            return Err(PageFileError::NotFound(path.to_owned()));
        }
        if !relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
        {
            return Err(PageFileError::Outside(path.to_owned()));
        }
        let not_found = |_| PageFileError::NotFound(path.to_owned());
        // A symlink can point anywhere, so the file it resolves to must be in the folder as resolved.
        let folder = folder.canonicalize().map_err(not_found)?;
        let file = folder.join(relative).canonicalize().map_err(not_found)?;
        if !file.starts_with(&folder) {
            return Err(PageFileError::Outside(path.to_owned()));
        }
        // A folder fails to read, so it counts as not found.
        fs::read(&file).map_err(not_found)
    }
}

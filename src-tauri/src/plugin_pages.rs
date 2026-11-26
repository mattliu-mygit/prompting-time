//! The pt-plugin:// scheme, which serves each plugin's page to the sandboxed frame the window shows
//! it in: `pt-plugin://<plugin>/<path in the page's folder>`, such as
//! `pt-plugin://whooper/index.html` for Whooper's `page/dist/index.html`.

use percent_encoding::{AsciiSet, CONTROLS, percent_decode_str, utf8_percent_encode};
use prompting_time_core::plugins::{PageFileError, PluginHost};
use tauri::http::{Method, Request, Response, StatusCode, header};

pub const SCHEME: &str = "pt-plugin";

/// What a plugin's page may load: its own files, styles it sets inline, and images and fonts as
/// data URLs. It may fetch nothing, so the page reaches no network, and submits no forms. The frame's
/// sandbox (scripts only) already keeps it from Prompting Time's window and from Tauri.
pub const PAGE_CSP: &str = "default-src 'none'; script-src pt-plugin:; style-src pt-plugin: 'unsafe-inline'; img-src pt-plugin: data:; font-src pt-plugin: data:; connect-src 'none'; form-action 'none'; base-uri 'none'";

/// Characters a file name cannot hold as they are in a URL path segment.
const SEGMENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'/')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}');

/// The URL of a plugin's page entry, such as `pt-plugin://whooper/index.html`.
pub fn page_url(plugin: &str, entry_file_name: &str) -> String {
    format!(
        "{SCHEME}://{plugin}/{}",
        utf8_percent_encode(entry_file_name, SEGMENT)
    )
}

/// Answers one request from a plugin's page. Each answer allows the origin "null", the origin of a
/// sandboxed frame, since the page's module scripts and fonts load as cross-origin requests.
pub fn respond(plugins: Option<&PluginHost>, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    if request.method() != Method::GET {
        return failure(
            StatusCode::METHOD_NOT_ALLOWED,
            "Plugin pages are read with GET.",
        );
    }
    let (Some(plugins), Some(plugin)) = (plugins, request.uri().host()) else {
        return failure(StatusCode::NOT_FOUND, "No such plugin page.");
    };
    let Ok(path) = percent_decode_str(request.uri().path()).decode_utf8() else {
        return failure(StatusCode::NOT_FOUND, "No such plugin page file.");
    };
    match plugins.page_file(plugin, &path) {
        Ok(bytes) => with_common_headers(Response::builder())
            .header(header::CONTENT_TYPE, content_type(&path))
            .header(header::CONTENT_SECURITY_POLICY, PAGE_CSP)
            .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
            // An update replaces the files while the window runs, so nothing is reused.
            .header(header::CACHE_CONTROL, "no-store")
            .body(bytes)
            .expect("static headers are valid"),
        Err(error @ PageFileError::Outside(_)) => {
            failure(StatusCode::FORBIDDEN, &error.to_string())
        }
        Err(error) => failure(StatusCode::NOT_FOUND, &error.to_string()),
    }
}

fn with_common_headers(builder: tauri::http::response::Builder) -> tauri::http::response::Builder {
    builder.header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "null")
}

fn failure(status: StatusCode, message: &str) -> Response<Vec<u8>> {
    with_common_headers(Response::builder())
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(message.as_bytes().to_vec())
        .expect("static headers are valid")
}

/// The type of a page file by its extension. Module scripts run only with a JavaScript type.
fn content_type(path: &str) -> &'static str {
    let extension = path
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::path::Path;

    use prompting_time_core::plugins::{PluginHostConfig, PluginSource};

    use super::*;

    /// An invented plugin whose page lives in page/dist, beside a file the page must not read.
    fn write_plugin(folder: &Path) {
        let manifest = serde_json::json!({
            "manifest_version": 1,
            "name": "page-agent",
            "title": "Page agent",
            "version": "0.1.0",
            "description": "An invented plugin with a page.",
            "start": ["bin/run"],
            "prerequisites": [],
            "page": { "entry": "page/dist/index.html", "protocol": 1 },
            "settings": "schemas/settings.json",
            "actions": []
        });
        fs::create_dir_all(folder.join("bin")).unwrap();
        fs::create_dir_all(folder.join("schemas")).unwrap();
        fs::create_dir_all(folder.join("page/dist/assets")).unwrap();
        fs::write(
            folder.join("prompting-time-plugin.json"),
            manifest.to_string(),
        )
        .unwrap();
        fs::write(folder.join("bin/run"), "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(folder.join("bin/run"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(
            folder.join("schemas/settings.json"),
            r#"{"type": "object"}"#,
        )
        .unwrap();
        fs::write(
            folder.join("page/dist/index.html"),
            "<!doctype html><title>Page</title>",
        )
        .unwrap();
        fs::write(folder.join("page/dist/assets/index.js"), "export {};").unwrap();
        fs::write(folder.join("page/dist/assets/two words.css"), "body {}").unwrap();
        fs::write(folder.join("secret.txt"), "not for the page").unwrap();
        symlink("../../secret.txt", folder.join("page/dist/escape.txt")).unwrap();
    }

    async fn host_with_plugin(root: &Path) -> PluginHost {
        let host = PluginHost::new(PluginHostConfig {
            plugins_dir: root.join("plugins"),
            state_home: root.join("state"),
            downloader: root.join("no-curl"),
        })
        .unwrap();
        let folder = root.join("source/page-agent");
        write_plugin(&folder);
        host.install(PluginSource::Folder { path: folder })
            .await
            .unwrap();
        host
    }

    fn get(host: &PluginHost, uri: &str) -> Response<Vec<u8>> {
        respond(
            Some(host),
            &Request::builder().uri(uri).body(Vec::new()).unwrap(),
        )
    }

    fn header_value(response: &Response<Vec<u8>>, name: header::HeaderName) -> &str {
        response.headers()[name].to_str().unwrap()
    }

    #[tokio::test]
    async fn page_files_are_served_with_their_type_the_null_origin_and_the_page_policy() {
        let root = tempfile::tempdir().unwrap();
        let host = host_with_plugin(root.path()).await;

        let page = get(&host, "pt-plugin://page-agent/index.html");
        assert_eq!(page.status(), StatusCode::OK);
        assert_eq!(page.body(), b"<!doctype html><title>Page</title>");
        assert_eq!(
            header_value(&page, header::CONTENT_TYPE),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            header_value(&page, header::ACCESS_CONTROL_ALLOW_ORIGIN),
            "null"
        );
        assert_eq!(
            header_value(&page, header::CONTENT_SECURITY_POLICY),
            PAGE_CSP
        );
        assert_eq!(
            header_value(&page, header::X_CONTENT_TYPE_OPTIONS),
            "nosniff"
        );

        let script = get(&host, "pt-plugin://page-agent/assets/index.js");
        assert_eq!(script.status(), StatusCode::OK);
        assert_eq!(
            header_value(&script, header::CONTENT_TYPE),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(
            header_value(&script, header::ACCESS_CONTROL_ALLOW_ORIGIN),
            "null"
        );
        let encoded = get(&host, "pt-plugin://page-agent/assets/two%20words.css");
        assert_eq!(encoded.status(), StatusCode::OK);
        assert_eq!(
            header_value(&encoded, header::CONTENT_TYPE),
            "text/css; charset=utf-8"
        );
        host.shutdown().await;
    }

    #[tokio::test]
    async fn paths_outside_the_page_folder_are_refused() {
        let root = tempfile::tempdir().unwrap();
        let host = host_with_plugin(root.path()).await;

        for uri in [
            "pt-plugin://page-agent/../../secret.txt",
            "pt-plugin://page-agent/%2e%2e/%2e%2e/secret.txt",
            "pt-plugin://page-agent/assets/..%2f..%2f..%2fsecret.txt",
            "pt-plugin://page-agent/escape.txt",
        ] {
            let response = get(&host, uri);
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{uri}");
            assert_ne!(response.body(), b"not for the page", "{uri}");
            assert_eq!(
                header_value(&response, header::ACCESS_CONTROL_ALLOW_ORIGIN),
                "null"
            );
        }
        host.shutdown().await;
    }

    #[tokio::test]
    async fn missing_files_unknown_plugins_and_other_methods_are_not_served() {
        let root = tempfile::tempdir().unwrap();
        let host = host_with_plugin(root.path()).await;

        for uri in [
            "pt-plugin://page-agent/missing.js",
            "pt-plugin://page-agent/assets",
            "pt-plugin://page-agent/",
            "pt-plugin://other-agent/index.html",
        ] {
            assert_eq!(get(&host, uri).status(), StatusCode::NOT_FOUND, "{uri}");
        }
        let post = respond(
            Some(&host),
            &Request::builder()
                .method(Method::POST)
                .uri("pt-plugin://page-agent/index.html")
                .body(Vec::new())
                .unwrap(),
        );
        assert_eq!(post.status(), StatusCode::METHOD_NOT_ALLOWED);
        let unavailable = respond(
            None,
            &Request::builder()
                .uri("pt-plugin://page-agent/index.html")
                .body(Vec::new())
                .unwrap(),
        );
        assert_eq!(unavailable.status(), StatusCode::NOT_FOUND);
        host.shutdown().await;
    }

    #[test]
    fn a_page_url_names_the_plugin_and_encodes_the_entry() {
        assert_eq!(
            page_url("whooper", "index.html"),
            "pt-plugin://whooper/index.html"
        );
        assert_eq!(
            page_url("fixture", "my page#1.html"),
            "pt-plugin://fixture/my%20page%231.html"
        );
    }
}

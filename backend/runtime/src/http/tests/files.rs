// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::support::*;
use axum::body::{Body, Bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};
use std::io::{Read, Write};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nimage bytes";

fn fixture() -> (Fixture, Value, std::path::PathBuf) {
    let f = Fixture::with_options(false, |options| {
        let root = std::path::Path::new(&options.project_root);
        for dir in ["public/assets", "web-fonts", "data"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        for (name, body) in [
            ("public/index.html", "<html>Installed UI</html>"),
            ("public/assets/app.css", "body { margin: 0; }"),
            ("public/assets/app.js", "console.log(\"bundle\")"),
            ("public/assets/%2e%2e.css", "literal percent"),
            ("web-fonts/ui.woff2", "font bytes"),
            ("web-fonts/inter.css", "@font-face {}"),
            ("data/example.json", "{\"example\":true}"),
            ("private.txt", "must not be served"),
        ] {
            std::fs::write(root.join(name), body).unwrap();
        }
        options.public_dir = Some(root.join("public").to_string_lossy().into_owned());
        options.web_font_folder = Some(root.join("web-fonts").to_string_lossy().into_owned());
        options.data_dir = Some(root.join("data").to_string_lossy().into_owned());
    });
    let page = f.page("Upload owner", "");
    let assets = f.item_path(&page["id"]).parent().unwrap().join("assets");
    (f, page, assets)
}

#[test]
fn reopening_preserves_configured_static_and_font_resources() {
    let (mut f, _, _) = fixture();
    f.reopen(false);
    for (path, expected) in [
        ("/", "<html>Installed UI</html>"),
        ("/fonts/ui.woff2", "font bytes"),
        ("/data/example.json", "{\"example\":true}"),
    ] {
        let (status, _, body) = f.request("GET", path, Body::empty());
        assert_eq!(status, StatusCode::OK, "{path}");
        assert_eq!(body.as_ref(), expected.as_bytes(), "{path}");
    }
}

fn multipart(id: Option<&str>, file: bool, duplicate: bool, mime: &str) -> Vec<u8> {
    let mut body = Vec::new();
    if let Some(id) = id {
        for _ in 0..if duplicate { 2 } else { 1 } {
            write!(
                body,
                "--test\r\nContent-Disposition: form-data; name=\"itemId\"\r\n\r\n{id}\r\n"
            )
            .unwrap();
        }
    }
    if file {
        write!(body,"--test\r\nContent-Disposition: form-data; name=\"file\"; filename=\"pretend.png\"\r\nContent-Type: {mime}\r\n\r\n").unwrap();
        body.extend_from_slice(PNG);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(b"--test--\r\n");
    body
}

fn upload(f: &Fixture, body: Body) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("POST")
        .uri("/api/upload_asset/?test=1")
        .header(header::CONTENT_TYPE, "multipart/form-data; boundary=test")
        .body(body)
        .unwrap();
    let (status, _, bytes) = f.send(request);
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[test]
fn static_resources_and_note_assets_keep_mime_security_headers_and_confined_paths() {
    let (f, page, assets) = fixture();
    std::fs::create_dir_all(&assets).unwrap();
    std::fs::write(assets.join("diagram.svg"), "<svg/>").unwrap();
    for (route, mime, body) in [
        (
            "/?note=example",
            "text/html; charset=utf-8",
            "<html>Installed UI</html>",
        ),
        (
            "/assets/app.css?build=1",
            "text/css; charset=utf-8",
            "body { margin: 0; }",
        ),
        (
            "/assets/app.js",
            "text/javascript; charset=utf-8",
            "console.log(\"bundle\")",
        ),
        (
            "/assets/%252e%252e.css",
            "text/css; charset=utf-8",
            "literal percent",
        ),
        ("/fonts/ui.woff2", "font/woff2", "font bytes"),
        (
            "/fonts/inter.css",
            "text/css; charset=utf-8",
            "@font-face {}",
        ),
        (
            "/data/example.json",
            "application/json; charset=utf-8",
            "{\"example\":true}",
        ),
    ] {
        let (status, headers, bytes) = f.request("GET", route, "");
        assert_eq!(status, StatusCode::OK, "{route}");
        assert_eq!(headers[header::CONTENT_TYPE], mime);
        assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(headers["referrer-policy"], "no-referrer");
        let policy = headers["content-security-policy"].to_str().unwrap();
        assert!(policy.contains("script-src 'self' 'wasm-unsafe-eval';"));
        assert!(!policy.contains("'unsafe-eval'"));
        assert!(policy.contains("object-src 'none'"));
        assert_eq!(bytes.as_ref(), body.as_bytes());
    }
    let asset = format!("/assets/{}/diagram.svg", page["id"].as_str().unwrap());
    let (status, headers, body) = f.request("GET", &asset, "");
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "image/svg+xml");
    assert_eq!(body.as_ref(), b"<svg/>");
    for route in [
        "/assets/missing.css",
        "/assets/missing.js",
        "/unknown",
        "/fonts/../private.txt",
        "/assets/%2e%2e%2fprivate.txt",
        "/%5cprivate.txt",
        "/%zz",
        "/%ff",
        "/%00",
        "/C:%2fprivate.txt",
    ] {
        let (status, _, body) = f.request("GET", route, "");
        assert_ne!(status, StatusCode::OK, "{route}");
        assert!(!String::from_utf8_lossy(&body).contains("must not be served"));
    }
    assert_eq!(
        f.request("POST", "/assets/app.js", "").0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.request(
            "GET",
            &format!("/assets/{}/missing.svg", page["id"].as_str().unwrap()),
            ""
        )
        .0,
        StatusCode::NOT_FOUND
    );
}

#[test]
fn multipart_uploads_use_random_names_preserve_bytes_and_leave_no_partial_files_on_rejection() {
    let (f, page, assets) = fixture();
    let id = page["id"].as_str().unwrap();
    let (status, uploaded) = upload(
        &f,
        Body::from(multipart(Some(id), true, false, "image/png")),
    );
    assert_eq!(status, StatusCode::OK);
    let filename = uploaded["filename"].as_str().unwrap();
    let uuid = uuid::Uuid::parse_str(filename.strip_suffix(".png").unwrap()).unwrap();
    assert_eq!(uuid.get_version_num(), 4);
    assert_eq!(uploaded["src"], format!("/assets/{id}/{filename}"));
    assert_eq!(uploaded["contentType"], "image/png");
    assert_eq!(uploaded["size"], PNG.len());
    assert_eq!(std::fs::read(assets.join(filename)).unwrap(), PNG);
    let (status, headers, bytes) = f.request("GET", uploaded["src"].as_str().unwrap(), "");
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "image/png");
    assert_eq!(bytes.as_ref(), PNG);
    for (body, error) in [
        (
            multipart(None, true, false, "image/png"),
            "Missing item ID.",
        ),
        (
            multipart(Some(id), false, false, "image/png"),
            "Missing image file.",
        ),
        (
            multipart(Some(id), true, true, "image/png"),
            "ERR_INVALID_UPLOAD",
        ),
        (
            multipart(Some(id), true, false, "image/svg+xml"),
            "Unsupported image type.",
        ),
        (
            b"--test\r\nnot a complete field".to_vec(),
            "ERR_INVALID_UPLOAD",
        ),
    ] {
        let (status, value) = upload(&f, Body::from(body));
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(value, json!({"error":error,"diagnostics":[]}));
    }
    assert_eq!(
        f.request("GET", "/api/upload_asset", "").0,
        StatusCode::METHOD_NOT_ALLOWED
    );
    assert_eq!(
        f.error("upload_asset", json!({}), StatusCode::BAD_REQUEST),
        "Expected multipart image upload."
    );
    assert_eq!(std::fs::read_dir(assets).unwrap().count(), 1);
}

#[test]
fn whole_body_limits_apply_to_fixed_streamed_and_interrupted_uploads() {
    let (f, page, assets) = fixture();
    let id = page["id"].as_str().unwrap();
    let large = Bytes::from(vec![b'x'; 10 * 1024 * 1024 + 1]);
    let stream = futures_util::stream::iter([
        Ok::<_, std::io::Error>(large.slice(..5_000_000)),
        Ok(large.slice(5_000_000..)),
    ]);
    for body in [Body::from(large), Body::from_stream(stream)] {
        let (status, value) = upload(&f, body);
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            value,
            json!({"error":"Image upload is larger than 10 MB.","diagnostics":[]})
        );
    }
    let partial = Bytes::from(multipart(Some(id), true, false, "image/png"));
    let interrupted = futures_util::stream::iter([
        Ok(partial),
        Err(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "client left",
        )),
    ]);
    let (status, value) = upload(&f, Body::from_stream(interrupted));
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        value,
        json!({"error":"ERR_INVALID_UPLOAD","diagnostics":[]})
    );
    assert!(!assets.exists());
    assert_eq!(
        upload(
            &f,
            Body::from(multipart(Some(id), true, false, "image/png"))
        )
        .0,
        StatusCode::OK
    );
}

#[test]
fn disconnected_multipart_socket_leaves_no_asset_and_the_real_listener_stays_usable() {
    let (f, page, assets) = fixture();
    let id = page["id"].as_str().unwrap();
    let address = f.runtime.block_on(f.app.start_http(Some(0))).unwrap();
    let partial = format!(
        "--test\r\nContent-Disposition: form-data; name=\"itemId\"\r\n\r\n{id}\r\n--test\r\nContent-Disposition: form-data; name=\"file\"; filename=\"partial.png\"\r\nContent-Type: image/png\r\n\r\npartial image"
    );
    let mut socket =
        std::net::TcpStream::connect(("127.0.0.1", u16::try_from(address.port).unwrap())).unwrap();
    socket
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    write!(socket,"POST /api/upload_asset HTTP/1.1\r\nHost: localhost\r\nContent-Type: multipart/form-data; boundary=test\r\nContent-Length: {}\r\nExpect: 100-continue\r\n\r\n",partial.len()+4096).unwrap();
    let mut interim = Vec::new();
    let mut byte = [0];
    while !interim.ends_with(b"\r\n\r\n") {
        socket.read_exact(&mut byte).unwrap();
        interim.push(byte[0]);
    }
    assert!(String::from_utf8_lossy(&interim).contains("100 Continue"));
    socket.write_all(partial.as_bytes()).unwrap();
    drop(socket);
    let mut socket =
        std::net::TcpStream::connect(("127.0.0.1", u16::try_from(address.port).unwrap())).unwrap();
    socket
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    socket
        .write_all(
            b"POST /api/read_metadata_fields HTTP/1.1\r\nHost: localhost\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
        )
        .unwrap();
    let mut response = String::new();
    socket.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"));
    assert!(!assets.exists());
    assert_eq!(
        upload(
            &f,
            Body::from(multipart(Some(id), true, false, "image/png"))
        )
        .0,
        StatusCode::OK
    );
    assert_eq!(std::fs::read_dir(assets).unwrap().count(), 1);
}

#[cfg(unix)]
#[test]
fn static_and_asset_upload_paths_reject_symlink_escapes() {
    let (f, page, assets) = fixture();
    let id = page["id"].as_str().unwrap();
    let outside = f.root.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("secret.txt"), "private outside content").unwrap();
    std::os::unix::fs::symlink(&outside, f.root.path().join("public/assets/linked")).unwrap();
    if assets.exists() {
        std::fs::remove_dir(&assets).unwrap();
    }
    std::os::unix::fs::symlink(&outside, &assets).unwrap();
    for path in [
        "/assets/linked/secret.txt".to_owned(),
        format!("/assets/{id}/secret.txt"),
    ] {
        assert_eq!(f.request("GET", &path, "").0, StatusCode::NOT_FOUND);
    }
    let (status, value) = upload(
        &f,
        Body::from(multipart(Some(id), true, false, "image/png")),
    );
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        value["error"]
            .as_str()
            .unwrap()
            .contains("within the owning item")
    );
    assert_eq!(std::fs::read_dir(outside).unwrap().count(), 1);
}

#[test]
fn image_assets_come_at_a_listed_width_while_the_original_stays() {
    use image::GenericImageView;
    let f = Fixture::new(true);
    let page = f.page("Cover owner", "");
    let assets = f.item_path(&page["id"]).parent().unwrap().join("assets");
    std::fs::create_dir_all(&assets).unwrap();
    image::RgbImage::from_pixel(900, 450, image::Rgb([40, 80, 120]))
        .save(assets.join("cover.jpg"))
        .unwrap();
    let original = std::fs::read(assets.join("cover.jpg")).unwrap();
    let url = format!("/assets/{}/cover.jpg", page["id"].as_str().unwrap());
    let (status, headers, body) = f.request("GET", &format!("{url}?w=320"), "");
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "image/jpeg");
    assert_eq!(
        image::load_from_memory(&body).unwrap().dimensions(),
        (320, 160)
    );
    // Unlisted widths and plain requests get the original, which is unchanged.
    for path in [format!("{url}?w=300"), url] {
        let (status, _, body) = f.request("GET", &path, "");
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.as_ref(), original.as_slice());
    }
    // The copy lives in the rebuildable cache, not beside the original.
    assert_eq!(std::fs::read_dir(&assets).unwrap().count(), 1);
}

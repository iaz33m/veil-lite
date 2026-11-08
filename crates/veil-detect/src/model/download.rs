//! Model download: resumable (`.part` + HTTP `Range`), cancellable, SHA-256 pinned, atomic rename.

use super::{spec, FileSpec, ModelSpec};
use anyhow::{bail, Context};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// HTTP client honouring `HTTPS_PROXY`/`NO_PROXY` and the OS certificate store (corporate proxies).
pub fn http_agent() -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build();
    ureq::Agent::config_builder()
        .proxy(ureq::Proxy::try_from_env())
        .tls_config(tls)
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .user_agent(concat!("veil/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// Download the published model into `models_dir/<id>/`. Progress is `(bytes done, bytes total)`.
pub fn download(models_dir: &Path, cancel: &AtomicBool, progress: &mut dyn FnMut(u64, u64)) -> anyhow::Result<()> {
    download_spec(spec(), models_dir, cancel, progress, &http_agent())
}

pub fn download_spec(
    spec: &ModelSpec,
    models_dir: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64, u64),
    agent: &ureq::Agent,
) -> anyhow::Result<()> {
    if !spec.published {
        bail!(
            "The Veil AI model ({}) has not been published yet. Keyword and personal-data checks still protect you.",
            spec.id
        );
    }
    for f in &spec.files {
        if f.sha256.len() != 64 || f.size == 0 {
            bail!("model file {} has no pinned SHA-256/size; refusing to download", f.name);
        }
    }
    let dir = spec.dir(models_dir);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let total = spec.total_size();
    let mut done = 0u64;
    progress(0, total);
    for f in &spec.files {
        let final_path = dir.join(&f.name);
        if std::fs::metadata(&final_path).map(|m| m.len() == f.size).unwrap_or(false)
            && sha256_file(&final_path)? == f.sha256
        {
            done += f.size;
            progress(done, total);
            continue;
        }
        fetch_file(f, &dir, cancel, &mut |n| progress(done + n, total), agent)?;
        done += f.size;
        progress(done, total);
    }
    Ok(())
}

fn fetch_file(
    f: &FileSpec,
    dir: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64),
    agent: &ureq::Agent,
) -> anyhow::Result<()> {
    let part = dir.join(format!("{}.part", f.name));
    let final_path = dir.join(&f.name);
    let mut have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if have > f.size {
        std::fs::remove_file(&part)?;
        have = 0;
    }

    if have < f.size {
        let mut req = agent.get(&f.url);
        if have > 0 {
            req = req.header("Range", &format!("bytes={have}-"));
        }
        let resp = req.call().with_context(|| format!("downloading {}", f.name))?;
        let status = resp.status().as_u16();
        let append = match status {
            206 if have > 0 => true,
            200 => false,
            416 if have == f.size => true,
            416 => {
                std::fs::remove_file(&part)?;
                bail!("server rejected resume for {}; try again", f.name);
            }
            s => bail!("downloading {}: HTTP {s}", f.name),
        };
        if !append {
            have = 0;
        }
        let mut out = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(append)
            .truncate(!append)
            .open(&part)
            .with_context(|| format!("opening {}", part.display()))?;
        let mut reader = resp.into_body().into_reader();
        let mut buf = vec![0u8; 64 * 1024];
        progress(have);
        loop {
            if cancel.load(Ordering::Relaxed) {
                out.flush()?;
                bail!("download cancelled");
            }
            let n = reader.read(&mut buf).with_context(|| format!("downloading {}", f.name))?;
            if n == 0 {
                break;
            }
            have += n as u64;
            if have > f.size {
                drop(out);
                std::fs::remove_file(&part)?;
                bail!("{} is larger than expected; download discarded", f.name);
            }
            out.write_all(&buf[..n])?;
            progress(have);
        }
        out.flush()?;
        out.sync_all().ok();
    }

    if have != f.size {
        bail!("download of {} was interrupted ({have} of {} bytes); it will resume next time", f.name, f.size);
    }
    let digest = sha256_file(&part)?;
    if digest != f.sha256 {
        std::fs::remove_file(&part).ok();
        bail!("{} failed verification (SHA-256 mismatch); the download was discarded", f.name);
    }
    std::fs::rename(&part, &final_path).with_context(|| format!("installing {}", f.name))?;
    Ok(())
}

pub(crate) fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::is_installed_spec;
    use std::io::BufRead;
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    /// A tiny HTTP/1.1 server serving `files` (path → bytes) with Range support. Records Range headers.
    fn serve(files: Vec<(String, Vec<u8>)>, requests: usize) -> (String, Arc<Mutex<Vec<Option<String>>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(requests) {
                let mut stream = stream.unwrap();
                let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
                let mut range = None;
                loop {
                    let mut h = String::new();
                    reader.read_line(&mut h).unwrap();
                    if h.trim().is_empty() {
                        break;
                    }
                    if let Some(v) = h.to_ascii_lowercase().strip_prefix("range:") {
                        range = Some(v.trim().to_string());
                    }
                }
                seen2.lock().unwrap().push(range.clone());
                let Some((_, body)) = files.iter().find(|(p, _)| *p == path) else {
                    write!(stream, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                    continue;
                };
                let start: usize = range
                    .as_deref()
                    .and_then(|r| r.strip_prefix("bytes="))
                    .and_then(|r| r.trim_end_matches('-').parse().ok())
                    .unwrap_or(0);
                let slice = &body[start.min(body.len())..];
                let status = if start > 0 { "206 Partial Content" } else { "200 OK" };
                write!(stream, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", slice.len())
                    .unwrap();
                stream.write_all(slice).unwrap();
            }
        });
        (base, seen)
    }

    fn no_proxy_agent() -> ureq::Agent {
        ureq::Agent::config_builder().proxy(None).http_status_as_error(false).build().into()
    }

    fn sha(b: &[u8]) -> String {
        hex::encode(Sha256::digest(b))
    }

    fn spec_for(base: &str, files: &[(&str, &[u8])]) -> ModelSpec {
        ModelSpec {
            id: "test-model".into(),
            version: "1".into(),
            published: true,
            license: "Apache-2.0".into(),
            source: String::new(),
            files: files
                .iter()
                .map(|(n, b)| FileSpec {
                    name: n.to_string(),
                    url: format!("{base}/{n}"),
                    sha256: sha(b),
                    size: b.len() as u64,
                })
                .collect(),
        }
    }

    #[test]
    fn downloads_verifies_and_installs() {
        let a = b"{\"format\":1}".to_vec();
        let b = vec![7u8; 200_000];
        let (base, _) = serve(vec![("/a.json".into(), a.clone()), ("/b.onnx".into(), b.clone())], 2);
        let spec = spec_for(&base, &[("a.json", &a), ("b.onnx", &b)]);
        let dir = tempfile::tempdir().unwrap();
        let mut last = (0, 0);
        download_spec(&spec, dir.path(), &AtomicBool::new(false), &mut |d, t| last = (d, t), &no_proxy_agent())
            .unwrap();
        assert_eq!(last, (200_012, 200_012));
        assert!(is_installed_spec(&spec, dir.path()));
        assert_eq!(std::fs::read(spec.dir(dir.path()).join("b.onnx")).unwrap(), b);
        assert!(!spec.dir(dir.path()).join("b.onnx.part").exists());
    }

    #[test]
    fn resumes_a_partial_download_with_range() {
        let body: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        let (base, seen) = serve(vec![("/m.onnx".into(), body.clone())], 1);
        let spec = spec_for(&base, &[("m.onnx", &body)]);
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(spec.dir(dir.path())).unwrap();
        std::fs::write(spec.dir(dir.path()).join("m.onnx.part"), &body[..40_000]).unwrap();
        download_spec(&spec, dir.path(), &AtomicBool::new(false), &mut |_, _| {}, &no_proxy_agent()).unwrap();
        assert_eq!(seen.lock().unwrap()[0].as_deref(), Some("bytes=40000-"));
        assert_eq!(std::fs::read(spec.dir(dir.path()).join("m.onnx")).unwrap(), body);
    }

    #[test]
    fn hash_mismatch_is_rejected_and_discarded() {
        let body = b"tampered".to_vec();
        let (base, _) = serve(vec![("/m.onnx".into(), body.clone())], 1);
        let mut spec = spec_for(&base, &[("m.onnx", &body)]);
        spec.files[0].sha256 = sha(b"original");
        let dir = tempfile::tempdir().unwrap();
        let err =
            download_spec(&spec, dir.path(), &AtomicBool::new(false), &mut |_, _| {}, &no_proxy_agent()).unwrap_err();
        assert!(err.to_string().contains("SHA-256"), "{err}");
        assert!(!spec.dir(dir.path()).join("m.onnx").exists());
        assert!(!spec.dir(dir.path()).join("m.onnx.part").exists());
    }

    #[test]
    fn cancel_keeps_the_partial_file() {
        let body = vec![1u8; 300_000];
        let (base, _) = serve(vec![("/m.onnx".into(), body.clone())], 1);
        let spec = spec_for(&base, &[("m.onnx", &body)]);
        let dir = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        let err = download_spec(
            &spec,
            dir.path(),
            &cancel,
            &mut |d, _| {
                if d > 0 {
                    cancel.store(true, Ordering::Relaxed)
                }
            },
            &no_proxy_agent(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("cancelled"));
        assert!(spec.dir(dir.path()).join("m.onnx.part").exists());
    }

    #[test]
    fn unpublished_or_unpinned_specs_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = spec_for("http://127.0.0.1:9", &[("m", b"x")]);
        s.published = false;
        let e = download_spec(&s, dir.path(), &AtomicBool::new(false), &mut |_, _| {}, &no_proxy_agent()).unwrap_err();
        assert!(e.to_string().contains("not been published"));
        s.published = true;
        s.files[0].sha256.clear();
        let e = download_spec(&s, dir.path(), &AtomicBool::new(false), &mut |_, _| {}, &no_proxy_agent()).unwrap_err();
        assert!(e.to_string().contains("SHA-256"));
        if !spec().published {
            assert!(download(dir.path(), &AtomicBool::new(false), &mut |_, _| {}).is_err());
        }
    }
}

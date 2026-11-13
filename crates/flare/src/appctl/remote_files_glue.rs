//! Remote sessions PR 6 on the viewer's UI thread: the files of a remote tab.
//!
//! - A click on a file link in a remote tab (`TermMsg::RemoteLink`) is never resolved here: the tab's link fetches the
//!   host's file (`remote.fetch`, rule 303), a copy is kept in this run's temp folder (`remote_files::Copies`), and the
//!   copy is shown in the side panel when it is a picture or a text file, or opened like any clicked file otherwise
//!   (the click allowlist: a program is revealed, never run, rule 14).
//! - Files dropped on a remote tab, and copied files or an image pasted there (`TermMsg::RemoteUpload`), are read on a
//!   worker and handed to the tab's link, which copies them to the host (`remote.put`) and pastes their host paths as
//!   the host quoted them (rule 304). A path of this machine is never typed into the host's shell.
//!
//! Nothing here waits: the reads and the copies run on workers and the link's own thread (rule 34); an answer comes
//! back as a `UiMsg`.

use std::path::Path;
use std::sync::Arc;

use flare_ctl::remote::FILE_MAX;
use flare_term::path_open::{OpenMode, open_term_path};
use flare_term::remote_pty::{Errand, ErrandFail, Upload};

use super::{AppCtl, Post, TermMsgBox, UiMsg};
use crate::term_ui::TermMsg;

const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

/// Reads the dropped or copied files to copy them to the host: only files, each at most `FILE_MAX`. A worker.
pub(crate) fn read_uploads(paths: &[String]) -> Result<Vec<Upload>, String> {
    let mb = FILE_MAX / (1024 * 1024);
    let mut out = Vec::with_capacity(paths.len());
    for p in paths {
        let path = Path::new(p);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| p.clone());
        // sync-ok: a worker reads the dropped file, never the UI thread
        let meta = std::fs::metadata(path).map_err(|_| format!("{name} could not be read here"))?;
        if meta.is_dir() {
            return Err(format!(
                "{name} is a folder: only files are copied to the other machine"
            ));
        }
        if meta.len() > FILE_MAX {
            return Err(format!(
                "{name} is over {mb} MB: copy it to the other machine another way"
            ));
        }
        // sync-ok: a worker reads the dropped file, never the UI thread
        let data = std::fs::read(path).map_err(|_| format!("{name} could not be read here"))?;
        out.push(Upload { name, data });
    }
    Ok(out)
}

/// The clipboard's image as a PNG: a PNG as it is, a bitmap (or any picture the image reader knows) encoded as one.
pub(crate) fn png_of(bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    if bytes.starts_with(PNG_MAGIC) {
        return Ok(bytes);
    }
    let img = image::load_from_memory(&bytes)
        .map_err(|e| format!("the image on the clipboard could not be read: {e}"))?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| format!("the image on the clipboard could not be made a PNG: {e}"))?;
    Ok(out.into_inner())
}

/// The name a pasted image is copied to the host under (the host makes it unique in the tab's folder).
pub(crate) fn pasted_image_name(unix_ms: u128) -> String {
    format!("pasted-image-{unix_ms}.png")
}

fn refused(post: &Post, tab: &str, text: String) {
    post(UiMsg::Term(TermMsgBox(TermMsg::RemoteFileRefused {
        tab: tab.to_string(),
        text,
    })));
}

impl AppCtl {
    /// A click on a file link in a remote tab (rule 303): the tab's link fetches the host's file by each spelling in
    /// turn; a copy of it is kept here and shown or opened. Never a path of this machine.
    pub(crate) fn remote_fetch_link(&mut self, tab: &str, texts: Vec<String>, line: Option<u32>) {
        let Some(live) = self.model.remote_live(tab) else {
            return;
        };
        let Some(first) = texts.first().cloned() else {
            return;
        };
        let post = Arc::clone(&self.deps.post);
        let copies = Arc::clone(&self.remote_copies);
        let openers = Arc::clone(&self.deps.term.openers);
        let home = self.deps.home.clone();
        let previews = flare_settings::term_prefs::open_previews(&self.prefs);
        let id = tab.to_string();
        live.send_errand(Errand::Fetch {
            texts,
            done: Box::new(move |r| match r {
                Err(ErrandFail::NoSuchFile) => {
                    post(UiMsg::Term(TermMsgBox(TermMsg::NoSuchFile {
                        tab: id,
                        text: first,
                    })));
                }
                Err(ErrandFail::Failed(why)) => refused(
                    &post,
                    &id,
                    format!("Could not fetch {first} from the other machine: {why}"),
                ),
                Ok(f) => {
                    // The copy is written and opened on a thread of its own: the link goes on with its polls.
                    let spawned = std::thread::Builder::new()
                        .name("flare-remote-copy".into())
                        .spawn(move || {
                            let (dir, name) = match copies.save(&id, &f.name, &f.data) {
                                Ok(c) => c,
                                Err(e) => {
                                    refused(
                                        &post,
                                        &id,
                                        format!("Could not keep a copy of {}: {e}", f.name),
                                    );
                                    return;
                                }
                            };
                            let base = dir.to_string_lossy().into_owned();
                            if previews
                                && let Some((kind, abs)) =
                                    crate::term_ui::previewable(&base, &name, &home)
                            {
                                post(UiMsg::Term(TermMsgBox(TermMsg::Preview {
                                    tab: id,
                                    kind,
                                    abs,
                                    line,
                                    asked: false,
                                })));
                                return;
                            }
                            // The copy goes the way a click on a file of this machine goes: a program is revealed,
                            // never run (rule 14).
                            let _ = open_term_path(&base, &name, OpenMode::Open, &*openers, &home);
                        });
                    if let Err(e) = spawned {
                        log::warn!("could not start the remote copy's writer: {e}");
                    }
                }
            }),
        });
    }

    /// Files dropped on a remote tab, or copied files or an image pasted there (rule 304): read on a worker, then the
    /// tab's link copies them to the host and pastes their host paths. A refusal is said in the tab's panel.
    pub(crate) fn remote_upload(&mut self, tab: &str, files: Vec<String>, image: bool) {
        let Some(live) = self.model.remote_live(tab).cloned() else {
            return;
        };
        let post = Arc::clone(&self.deps.post);
        let clip = Arc::clone(&self.deps.term.clip_port);
        let id = tab.to_string();
        let spawned = std::thread::Builder::new()
            .name("flare-remote-upload".into())
            .spawn(move || {
                let read = if files.is_empty() && image {
                    clip.read_image()
                        .ok_or_else(|| "the image on the clipboard could not be read".to_string())
                        .and_then(png_of)
                        .map(|data| {
                            let ms = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map_or(0, |d| d.as_millis());
                            vec![Upload {
                                name: pasted_image_name(ms),
                                data,
                            }]
                        })
                } else {
                    read_uploads(&files)
                };
                let uploads = match read {
                    Ok(u) if !u.is_empty() => u,
                    Ok(_) => return,
                    Err(why) => {
                        refused(
                            &post,
                            &id,
                            format!("Nothing was copied to the other machine: {why}"),
                        );
                        return;
                    }
                };
                live.send_errand(Errand::Put {
                    files: uploads,
                    done: Box::new(move |r| {
                        if let Err(why) = r {
                            let why = match why {
                                ErrandFail::NoSuchFile => {
                                    "the host has no folder for it".to_string()
                                }
                                ErrandFail::Failed(w) => w,
                            };
                            refused(
                                &post,
                                &id,
                                format!("Nothing was copied to the other machine: {why}"),
                            );
                        }
                    }),
                });
            });
        if let Err(e) = spawned {
            log::warn!("could not start the remote upload's reader: {e}");
        }
    }

    /// A remote tab closed: this run's copies of its host files go, on a thread of their own.
    pub(crate) fn forget_remote_copies(&self, tab: &str) {
        if let Some(dir) = self.remote_copies.forget(tab) {
            crate::remote_files::remove_later(dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropped_files_are_read_whole_and_a_folder_or_a_missing_file_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a b.txt");
        std::fs::write(&a, b"hello").unwrap();
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let got = read_uploads(&[s(&a)]).unwrap();
        assert_eq!(
            got,
            vec![Upload {
                name: "a b.txt".into(),
                data: b"hello".to_vec()
            }]
        );
        let e = read_uploads(&[s(&a), s(d.path())]).unwrap_err();
        assert!(e.contains("is a folder"), "{e}");
        let e = read_uploads(&[s(&d.path().join("gone.txt"))]).unwrap_err();
        assert!(e.contains("could not be read"), "{e}");
    }

    /// A PNG goes as it is; a clipboard bitmap is made a PNG; anything else is refused with a line.
    #[test]
    fn a_pasted_image_goes_as_a_png() {
        let png = PNG_MAGIC.to_vec();
        assert_eq!(png_of(png.clone()).unwrap(), png);
        // A 1x1 bitmap: the BMP the clipboard port makes of a DIB.
        let mut dib = vec![0u8; 40];
        dib[0..4].copy_from_slice(&40u32.to_le_bytes());
        dib[4..8].copy_from_slice(&1i32.to_le_bytes());
        dib[8..12].copy_from_slice(&1i32.to_le_bytes());
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        dib[14..16].copy_from_slice(&24u16.to_le_bytes());
        dib.extend([0, 0, 255, 0]);
        let bmp = flare_win::clipboard_port::bmp_of_dib(&dib).unwrap();
        let out = png_of(bmp).unwrap();
        assert!(out.starts_with(PNG_MAGIC));
        let back = image::load_from_memory(&out).unwrap().to_rgb8();
        assert_eq!(back.get_pixel(0, 0).0, [255, 0, 0]);
        assert!(png_of(b"not a picture".to_vec()).is_err());
        assert_eq!(pasted_image_name(17), "pasted-image-17.png");
    }
}

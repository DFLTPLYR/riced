//! Native dialogs are managed jobs; no VM references cross the async boundary.
use iced::futures::future::{Either, select};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{future::Future, path::PathBuf};
use tokio::sync::{Semaphore, oneshot};

static PICKERS: Semaphore = Semaphore::const_new(1);

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct FileOptions {
    pub title: Option<String>,
    pub extensions: Vec<String>,
}

impl FileOptions {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self
            .title
            .as_ref()
            .is_some_and(|title| title.len() > 1024 || title.contains('\0'))
        {
            return Err("pick_file title must be NUL-free and at most 1024 bytes");
        }
        if self.extensions.len() > 32
            || self.extensions.iter().any(|extension| {
                extension.is_empty()
                    || extension.len() > 32
                    || !extension.bytes().all(|c| c.is_ascii_alphanumeric())
            })
        {
            return Err(
                "pick_file extensions must be at most 32 alphanumeric suffixes without dots or wildcards, each 1..32 bytes",
            );
        }
        Ok(())
    }
}

pub(crate) async fn run(options: FileOptions, receiver: oneshot::Receiver<()>) -> Value {
    let Ok(_permit) = PICKERS.try_acquire() else {
        return json!({"ok":false,"type":"file_dialog","error":{"code":"busy","message":"another file picker is already open"}});
    };
    let pick = async move {
        let mut dialog = rfd::AsyncFileDialog::new();
        if let Some(title) = options.title {
            dialog = dialog.set_title(title);
        }
        if !options.extensions.is_empty() {
            dialog = dialog.add_filter("Files", &options.extensions);
        }
        dialog
            .pick_file()
            .await
            .map(|handle| handle.path().to_path_buf())
    };
    complete(pick, receiver).await
}

async fn complete(
    pick: impl Future<Output = Option<PathBuf>>,
    receiver: oneshot::Receiver<()>,
) -> Value {
    match select(Box::pin(pick), receiver).await {
        Either::Left((Some(path), _)) => match path.into_os_string().into_string() {
            Ok(path) => json!({"ok":true,"type":"file_dialog","cancelled":false,"path":path}),
            Err(_) => {
                json!({"ok":false,"type":"file_dialog","error":{"code":"path","message":"selected path is not UTF-8"}})
            }
        },
        Either::Left((None, _)) => json!({"ok":true,"type":"file_dialog","cancelled":true}),
        Either::Right(_) => {
            json!({"ok":false,"type":"file_dialog","error":{"code":"cancelled","message":"file picker cancelled"}})
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_bounded_titles_and_plain_extensions() {
        assert!(
            FileOptions {
                title: Some("Choose wallpaper".into()),
                extensions: vec!["png".into(), "jpeg".into()]
            }
            .validate()
            .is_ok()
        );
        for extension in ["", "*.png", ".jpg", "../png"] {
            assert!(
                FileOptions {
                    extensions: vec![extension.into()],
                    ..Default::default()
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            FileOptions {
                title: Some("bad\0title".into()),
                ..Default::default()
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn delivers_selected_paths_or_user_dismissal() {
        iced::futures::executor::block_on(async {
            let (_sender, receiver) = oneshot::channel();
            let result = complete(
                async { Some(PathBuf::from("/pictures/my wallpaper.png")) },
                receiver,
            )
            .await;
            assert_eq!(result["path"], "/pictures/my wallpaper.png");
            assert_eq!(result["cancelled"], false);
            let (_sender, receiver) = oneshot::channel();
            assert_eq!(
                complete(async { None }, receiver).await,
                json!({"ok":true,"type":"file_dialog","cancelled":true})
            );
        });
    }

    #[test]
    fn owner_retirement_stops_waiting_for_a_dialog() {
        iced::futures::executor::block_on(async {
            let (sender, receiver) = oneshot::channel();
            drop(sender);
            let result = complete(std::future::pending(), receiver).await;
            assert_eq!(result["error"]["code"], "cancelled");
        });
    }

    #[test]
    fn concurrent_picker_requests_fail_without_opening_another_dialog() {
        let _permit = PICKERS.try_acquire().unwrap();
        let (_sender, receiver) = oneshot::channel();
        let result = iced::futures::executor::block_on(run(FileOptions::default(), receiver));
        assert_eq!(result["error"]["code"], "busy");
    }

    #[test]
    fn non_utf8_selections_return_an_error_instead_of_a_lossy_path() {
        use std::os::unix::ffi::OsStringExt;
        let path = PathBuf::from(std::ffi::OsString::from_vec(vec![b'/', 0xff]));
        let (_sender, receiver) = oneshot::channel();
        let result = iced::futures::executor::block_on(complete(async { Some(path) }, receiver));
        assert_eq!(result["error"]["code"], "path");
    }
}

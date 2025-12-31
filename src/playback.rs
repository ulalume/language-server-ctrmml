use std::os::raw::c_void;
use std::thread;

use tokio::sync::mpsc;
use tower_lsp::lsp_types::Diagnostic;

use crate::backend::Backend;
use crate::ctrmml_cmd_ffi;
use crate::diagnostics::{diagnostics_for_positions, HighlightPosition};
use crate::utils::{read_file_text, uri_to_path};

struct HighlightCallbackData {
    sender: mpsc::UnboundedSender<Vec<HighlightPosition>>,
}

unsafe extern "C" fn highlight_callback(
    _ticks: u32,
    positions: *const ctrmml_cmd_ffi::ctrmml_cmd_highlight_position,
    count: usize,
    user_data: *mut c_void,
) {
    if user_data.is_null() {
        return;
    }
    let data = &*(user_data as *const HighlightCallbackData);
    if positions.is_null() || count == 0 {
        let _ = data.sender.send(Vec::new());
        return;
    }
    let slice = std::slice::from_raw_parts(positions, count);
    let mut out = Vec::with_capacity(slice.len());
    for pos in slice {
        out.push(HighlightPosition {
            line: pos.line,
            col: pos.col,
        });
    }
    let _ = data.sender.send(out);
}

pub(crate) struct Playback {
    pub(crate) uri: String,
    pub(crate) stop_flag: ctrmml_cmd_ffi::StopFlag,
    pub(crate) thread: thread::JoinHandle<()>,
}

impl Backend {
    pub(crate) async fn start_playback(
        &self,
        uri: String,
        start: Option<(u32, u32)>,
    ) -> std::result::Result<(), String> {
        self.stop_playback().await;

        let text = self
            .docs
            .read()
            .await
            .get(&uri)
            .cloned()
            .or_else(|| read_file_text(&uri));
        let path = uri_to_path(&uri).ok_or_else(|| "invalid file uri".to_string())?;
        let path_str = path.to_string_lossy().to_string();
        let base_dir = path
            .parent()
            .ok_or_else(|| "invalid file uri".to_string())?
            .to_string_lossy()
            .to_string();
        let display_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(".mml")
            .to_string();

        let stop_flag = ctrmml_cmd_ffi::StopFlag::new()?;
        let stop_flag_ptr = stop_flag.as_ptr();
        let stop_flag_addr = stop_flag_ptr as usize;

        let (tx, mut rx) = mpsc::unbounded_channel::<Vec<HighlightPosition>>();

        let token = {
            let mut seq = self.playback_seq.lock().await;
            *seq += 1;
            *seq
        };

        let client = self.client.clone();
        let docs = self.docs.clone();
        let seq = self.playback_seq.clone();
        let uri_clone = uri.clone();
        tokio::spawn(async move {
            while let Some(positions) = rx.recv().await {
                if *seq.lock().await != token {
                    break;
                }
                let text = docs
                    .read()
                    .await
                    .get(&uri_clone)
                    .cloned()
                    .or_else(|| read_file_text(&uri_clone))
                    .unwrap_or_default();
                let diags: Vec<Diagnostic> = diagnostics_for_positions(&text, &positions);
                if let Ok(uri) = uri_clone.parse() {
                    let _ = client.publish_diagnostics(uri, diags, None).await;
                }
            }

            if *seq.lock().await == token {
                if let Ok(uri) = uri_clone.parse() {
                    let _ = client.publish_diagnostics(uri, Vec::new(), None).await;
                }
            }
        });

        let text_for_thread = text.clone();
        let thread_handle = thread::spawn(move || {
            let callback_data = Box::new(HighlightCallbackData { sender: tx });
            let callback_ptr = Box::into_raw(callback_data) as *mut c_void;
            let (start_line, start_col, has_start) = match start {
                Some((line, col)) => (line, col, 1),
                None => (0, 0, 0),
            };

            let stop_flag_ptr = stop_flag_addr as *mut ctrmml_cmd_ffi::ctrmml_cmd_stop_flag;
            let options = ctrmml_cmd_ffi::ctrmml_cmd_play_options {
                follow: 1,
                log_messages: 0,
                has_start,
                start_line,
                start_col,
                stop_flag: stop_flag_ptr,
                on_highlight: Some(highlight_callback),
                user_data: callback_ptr,
            };

            let result = if let Some(text) = text_for_thread {
                ctrmml_cmd_ffi::play_text(&text, &base_dir, &display_name, &options)
            } else {
                ctrmml_cmd_ffi::play_file(&path_str, &options)
            };

            unsafe {
                let _ = Box::from_raw(callback_ptr as *mut HighlightCallbackData);
            }

            if let Err(error) = result {
                eprintln!("ctrmml-cmd play failed: {error}");
            }
        });

        let mut slot = self.playback.lock().await;
        *slot = Some(Playback {
            uri: uri.clone(),
            stop_flag,
            thread: thread_handle,
        });

        Ok(())
    }

    pub(crate) async fn stop_playback(&self) {
        {
            let mut seq = self.playback_seq.lock().await;
            *seq += 1;
        }
        let playback = self.playback.lock().await.take();
        if let Some(playback) = playback {
            let Playback {
                uri,
                stop_flag,
                thread,
            } = playback;
            stop_flag.set();
            let _ = tokio::task::spawn_blocking(move || {
                let _ = thread.join();
                drop(stop_flag);
            })
            .await;
            if let Ok(uri) = uri.parse() {
                let _ = self.client.publish_diagnostics(uri, Vec::new(), None).await;
            }
        }
    }
}

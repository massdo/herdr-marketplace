//! Images Herdr lays over a pane. `pane.graphics.stream` takes PNG frames on
//! a socket of their own and keeps them compressed up to the terminal, where
//! an image the pane redraws with the kitty protocol travels as raw pixels
//! each time it comes back. The README animations play through it.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

use crate::adapters::herdr_socket::HerdrSocket;
use crate::domain::PLUGIN_ID;

const TIMEOUT: Duration = Duration::from_secs(5);
const MAX_ANSWER_BYTES: u64 = 64 * 1024;
/// Above the images the pane draws itself, at z-index 0.
const Z_INDEX: i32 = 1;

/// Cells an image covers, counted from the top left cell of the pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cells {
    pub column: u16,
    pub row: u16,
    pub columns: u16,
    pub rows: u16,
}

/// A layer of images over the pane, for as long as it stays open: Herdr
/// removes it when it closes.
pub struct Layer {
    stream: UnixStream,
}

impl Layer {
    /// Layer `layer` of pane `pane`, which Herdr accepted.
    pub fn open(socket: &Path, pane: &str, layer: &str) -> Result<Self, String> {
        let stream = UnixStream::connect(socket).map_err(|error| error.to_string())?;
        stream
            .set_read_timeout(Some(TIMEOUT))
            .and_then(|()| stream.set_write_timeout(Some(TIMEOUT)))
            .map_err(|error| error.to_string())?;
        let method = "pane.graphics.stream";
        let request = json!({
            "id": format!("{PLUGIN_ID}:{method}"),
            "method": method,
            "params": { "pane_id": pane, "layer_id": layer, "z_index": Z_INDEX },
        });
        (&stream)
            .write_all(format!("{request}\n").as_bytes())
            .map_err(|error| error.to_string())?;
        // Herdr says nothing more after its answer, unless a frame fails.
        let mut line = String::new();
        BufReader::new((&stream).take(MAX_ANSWER_BYTES))
            .read_line(&mut line)
            .map_err(|error| error.to_string())?;
        let answer: Value = serde_json::from_str(line.trim())
            .map_err(|error| format!("{method} answer is not JSON: {error}"))?;
        if let Some(error) = answer.get("error") {
            let text = |key| error.get(key).and_then(Value::as_str).unwrap_or("unknown");
            return Err(format!("{method}: {} ({})", text("message"), text("code")));
        }
        if answer.pointer("/result/type").and_then(Value::as_str) != Some("ok") {
            return Err(format!("{method}: unexpected answer {line:?}"));
        }
        Ok(Self { stream })
    }

    /// Shows a PNG of `width` × `height` pixels over `cells`, in place of the
    /// image before it.
    pub fn show(
        &mut self,
        png: &[u8],
        width: u32,
        height: u32,
        cells: Cells,
    ) -> Result<(), String> {
        let header = json!({
            "format": "png",
            "image_width": width,
            "image_height": height,
            "data_length": png.len(),
            "placement": {
                "viewport_col": cells.column,
                "viewport_row": cells.row,
                "grid_cols": cells.columns,
                "grid_rows": cells.rows,
            },
        });
        let mut frame = format!("{header}\n").into_bytes();
        frame.extend_from_slice(png);
        self.stream
            .write_all(&frame)
            .and_then(|()| self.stream.flush())
            .map_err(|error| error.to_string())
    }
}

/// Whether Herdr shows `pane` now: in the active workspace and tab, and not
/// hidden by a zoomed pane.
pub fn pane_visible(socket: &Path, pane: &str) -> Result<bool, String> {
    let result = HerdrSocket::new(socket.to_path_buf())
        .call("pane.graphics.info", json!({ "pane_id": pane }))
        .map_err(|error| error.to_string())?;
    result
        .get("pane_visible")
        .and_then(Value::as_bool)
        .ok_or_else(|| "pane.graphics.info has no pane_visible".into())
}

//! A preview rendered by the [`crate::preview_worker::PreviewWorker`], as the thumbnail cache
//! tracks it.

use egui::TextureHandle;

/// Where a worker-rendered preview is.
pub enum WorkerPreview {
    /// Requested; the worker has not answered yet.
    Rendering,
    Ready(TextureHandle),
    /// The asset is unknown or the worker could not render it (the tile keeps its placeholder).
    Failed,
}

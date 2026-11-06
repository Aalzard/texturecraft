//! The smart mask preview shape, baked on first use.

use texturecraft_engine::mask_preview::MaskPreview;

/// Whether the smart mask preview shape is baked.
#[derive(Default)]
pub enum MaskShape {
    #[default]
    NotBaked,
    /// Boxed: the baked session is large and the enum sits in the worker for its whole life.
    Baked(Box<MaskPreview>),
    /// The bake failed; smart mask previews stay empty.
    Failed,
}

impl MaskShape {
    /// The baked shape, baking it on the first call (about two seconds; None when it fails).
    pub fn get(&mut self) -> Option<&mut MaskPreview> {
        if matches!(self, MaskShape::NotBaked) {
            *self = match MaskPreview::new() {
                Ok(preview) => MaskShape::Baked(Box::new(preview)),
                Err(e) => {
                    log::warn!("smart mask previews: {e}");
                    MaskShape::Failed
                }
            };
        }
        match self {
            MaskShape::Baked(preview) => Some(preview),
            MaskShape::NotBaked | MaskShape::Failed => None,
        }
    }
}

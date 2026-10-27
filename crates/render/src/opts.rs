use super::accel::Backend;
use super::*;

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RenderOpts {
    /// Output scale relative to comp pixels (1 = Full, 0.5 = Half…).
    pub scale: f64,
    pub motion_blur: bool,
    /// Render guide layers (viewer only). Only the top-level comp's own guide layers: nested
    /// comps (precomps, collapsed precomps, comps read by effects) never draw theirs, as in
    /// After Effects (see `Renderer::nested_opts`).
    pub guides: bool,
    /// Draft quality: fewer motion-blur samples, bilinear only.
    pub draft: bool,
    /// 3D view camera override (viewer Front/Top/Custom views); None = the comp's active camera.
    pub view: Option<three_d::CameraState>,
    /// CPU or GPU compositor (the GPU needs [`Renderer::accel`]).
    pub backend: Backend,
    /// Region of interest `[x, y, w, h]` in comp pixels (viewer only): the top-level frame covers
    /// just this rectangle, and only its pixels are composited.
    pub roi: Option<[f64; 4]>,
    /// Which proxies stand in for footage and compositions (Render Settings ▸ Proxy Use).
    pub proxy: effectcraft_project::render_queue::ProxyUse,
    /// Switches Affect Nested Comps (Settings ▸ General): a precomp layer's Quality and Motion
    /// Blur switches also limit the layers of the nested comp (Draft/Wireframe or motion blur off
    /// propagate down; they never raise a nested layer's own setting).
    pub nested_switches: bool,
    /// Realtime Shadows in Draft (Settings ▸ 3D): draft renders (`draft`) still cast shadows.
    pub draft_shadows: bool,
}

impl RenderOpts {
    /// Whether 3D layers cast shadows in this render.
    pub fn shadows(&self) -> bool {
        !self.draft || self.draft_shadows
    }
}

impl Default for RenderOpts {
    fn default() -> Self {
        RenderOpts {
            scale: 1.0,
            motion_blur: true,
            guides: false,
            draft: false,
            view: None,
            roi: None,
            backend: Backend::Cpu,
            nested_switches: true,
            draft_shadows: true,
            proxy: effectcraft_project::render_queue::ProxyUse::CurrentSettings,
        }
    }
}

/// One collapsed precomp layer a nested comp is drawn through.
#[derive(Clone, Copy)]
pub(crate) struct Through<'a> {
    pub(crate) comp_id: ItemId,
    pub(crate) comp: &'a Comp,
    /// The containing comp's time at this frame.
    pub(crate) time: Tick,
    pub(crate) layer: &'a Layer,
    /// The precomp layer is motion blurred: its own motion blurs the nested layers.
    pub(crate) blur: bool,
}

/// Collapsed precomps within collapsed precomps that motion blur follows (more than
/// [`Renderer::collapsed`]'s nesting limit).
pub(crate) const MAX_THROUGH: usize = 16;

/// How a collapsed precomp's 3D layers are placed in the parent (see `Renderer::collapse_into`).
#[derive(Clone, Copy)]
pub(crate) struct Collapse3d<'a> {
    /// Nested comp world → parent world (the precomp layer's world transform).
    pub world: Mat4,
    /// The parent's camera.
    pub cam: three_d::CameraState,
    /// The outermost comp's context (its lights light the nested layers).
    pub parent: EvalCtx<'a>,
}

/// Time spent on one layer of one frame (milliseconds).
#[derive(Clone, Debug, Default)]
pub struct LayerTiming {
    pub depth: usize,
    pub layer: String,
    /// Source + masks + effects (cache lookup only, on a hit).
    pub process_ms: f64,
    /// Per effect: (effect id, ms). Empty on a cache hit.
    pub effects: Vec<(String, f64)>,
    /// Transform + matte + blend.
    pub composite_ms: f64,
    /// The processed buffer came from the layer cache.
    pub cached: bool,
}

//! The ferros mark: logo.vsf's `roe` ring segments and pie sectors (tools/logo-vsf), drawn at the size the window needs by vsf's own analytic rasterizer, then held as a fluor layer (α + darkness, G2t16) that composes under whatever is in front of it.
//! The shapes stay analytic: a resize draws them again from the geometry, never scales pixels.

use fluor::pixel::{G2t16, Pixel};
use vsf::VsfType;
use vsf::colour::convert::delinearize_gamma2tanh_bits_f32;

/// The mark drawn at one size.
pub struct Mark {
    scene: VsfType,
    pub side: usize,
    /// `side × side` pixels, α + straight darkness at 16 bits, ready for `under`.
    pub pixels: Vec<G2t16>,
}

impl Mark {
    /// The scene read from the release's logo document; nothing drawn yet.
    pub fn read(doc: &[u8]) -> Result<Mark, String> {
        let scene = vsf::types::renderable::read_scene(doc)?;
        Ok(Mark { scene, side: 0, pixels: Vec::new() })
    }

    /// Draw the mark `side` pixels square, if it is not that size already.
    pub fn draw(&mut self, side: usize) {
        if side == self.side && !self.pixels.is_empty() || side == 0 {
            return;
        }
        let r = vsf::raster::render_fitted(&self.scene, side, side, ([0., 0.], [1., 1.]));
        let one = G2t16::ONE as f32;
        self.pixels = r
            .image
            .rgba
            .iter()
            .map(|p| {
                // Premultiplied linear VSF RGB and coverage: the straight light is the colour over the coverage, encoded as a 16-bit g2t code; darkness is its complement, and the coverage is the α (fluor's `under` premultiplies as it composes).
                let a = p[3];
                if a <= 0. {
                    return G2t16::EMPTY;
                }
                let code = |c: f32| delinearize_gamma2tanh_bits_f32(c / a, 16);
                let dark = [p[0], p[1], p[2]].map(|c| G2t16::ONE - code(c));
                G2t16::from_channels((a * one) as u32, dark)
            })
            .collect();
        self.side = side;
    }
}

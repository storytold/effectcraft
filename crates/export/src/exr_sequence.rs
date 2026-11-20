//! Selected composition channels in an EXR sequence.

use std::io::Cursor;

use effectcraft_project::Comp;
use effectcraft_project::render_queue::{Channels, ExrBitDepth, ExrSelection, FrameSample, sequence_path};
use effectcraft_raster::Image;
use effectcraft_raster::channels3d::BACKGROUND_DEPTH;
use effectcraft_render::color::Pipe;
use effectcraft_time::Tick;

use crate::{Cx, ExportError, Result, State, exr_out, publish_sequence_frame};

pub(crate) async fn write(cx: &Cx<'_>, comp: &Comp, selection: &ExrSelection, size: (u32, u32), f0: i64, st: &mut State<'_>) -> Result<u64> {
    selection.validate(comp).map_err(|e| ExportError::Unsupported(e.into()))?;
    let om = cx.output;
    if selection.depth && om.exr_compression.is_lossy(ExrBitDepth::Float) {
        return Err(ExportError::Unsupported("Z depth needs compression that is lossless for FLOAT channels".into()));
    }
    let layers = selection.layer_ids(comp);
    let count = 4 * (layers.len() as u64 + u64::from(selection.composite)) + u64::from(selection.depth);
    exr_out::Channels::new(size).check_channel_count(count).map_err(encode_error)?;
    if selection.depth {
        exr_out::Channels::new(cx.settings.output_size(comp)).check_channel_count(1).map_err(encode_error)?;
    }
    let names = exr_out::layer_names(comp.layers.iter().filter(|l| ExrSelection::can_export_layer(l)), selection.multipart);
    let pipe = Pipe::of(&cx.project.settings);
    let chroma = Some(pipe.output_linear_space().exr_chromaticities());
    let mut bytes = 0u64;
    // Keep one frame's source buffers live at a time. Ordinary stills use the parallel path.
    for i in 0..st.total {
        let path = sequence_path(cx.path, f0 + i as i64);
        if !(cx.settings.skip_existing && cx.frame_exists(&path)) {
            let mut channels = if selection.multipart { exr_out::Channels::new_multipart(size) } else { exr_out::Channels::new(size) };
            if selection.composite {
                let img = cx.frame_async(comp, i).await;
                channels.push_rgba("", into_linear_pixels(img, comp, size, om.channels, &pipe), om.exr_bit_depth).map_err(encode_error)?;
            }
            for layer in &layers {
                let name = names.get(layer).ok_or_else(|| ExportError::Encode(format!("No EXR channel name for layer {}", layer.0)))?;
                let img = cx.exr_layer_frame(comp, i, *layer).await?;
                channels.push_rgba(name, into_linear_pixels(img, comp, size, Channels::Rgba, &pipe), om.exr_bit_depth).map_err(encode_error)?;
            }
            if selection.depth {
                channels.push_depth(depth_frame(cx, comp, i, size)?).map_err(encode_error)?;
            }
            let mut buf = Cursor::new(Vec::new());
            channels.write(&mut buf, om.exr_compression, chroma).map_err(encode_error)?;
            bytes = bytes.saturating_add(publish_sequence_frame(cx, &path, &buf.into_inner())?);
        }
        st.advance(1)?;
    }
    Ok(bytes)
}

fn encode_error(e: craft_exr::decode::Error) -> ExportError {
    ExportError::Encode(e.to_string())
}

/// Decode the output curve in the output primaries, preserving premultiplication and HDR values.
pub(crate) fn linear_pixels(img: &Image, comp: &Comp, (w, h): (u32, u32), channels: Channels, pipe: &Pipe) -> Vec<[f32; 4]> {
    let convert = pixel_conversion(comp, channels, pipe);
    let mut data = Vec::with_capacity((w as usize).saturating_mul(h as usize));
    for y in 0..h {
        for x in 0..w {
            data.push(convert(img.get(i64::from(x), i64::from(y))));
        }
    }
    data
}

fn into_linear_pixels(mut img: Image, comp: &Comp, size: (u32, u32), channels: Channels, pipe: &Pipe) -> Vec<[f32; 4]> {
    if (img.width, img.height) != size {
        return linear_pixels(&img, comp, size, channels, pipe);
    }
    let convert = pixel_conversion(comp, channels, pipe);
    for pixel in &mut img.data {
        *pixel = convert(*pixel);
    }
    img.data
}

fn pixel_conversion(comp: &Comp, channels: Channels, pipe: &Pipe) -> impl Fn([f32; 4]) -> [f32; 4] {
    let dec = pipe.output_linear();
    let background = comp.background;
    move |p| {
        let lin3 = |c: [f32; 3]| dec.apply(c.map(|v| v.max(0.0)));
        let a = p[3].clamp(0.0, 1.0);
        let (c, a) = match channels {
            Channels::Alpha => ([a; 3], 1.0),
            Channels::Rgba => {
                let c = if a > 0.0 { lin3([p[0] / a, p[1] / a, p[2] / a]).map(|v| v * a) } else { [0.0; 3] };
                (c, a)
            }
            // Composite over the background in the encoded space before decoding, as for PNG.
            Channels::Rgb => (lin3([0, 1, 2].map(|i| p[i] + background[i] * (1.0 - a))), 1.0),
        };
        [c[0], c[1], c[2], a]
    }
}

fn depth_frame(cx: &Cx<'_>, comp: &Comp, i: u64, size: (u32, u32)) -> Result<Vec<f32>> {
    let (first, second, upper_first) = match cx.settings.sample(comp, i) {
        FrameSample::Progressive(t) => (t, t, true),
        FrameSample::Fields { first, second, upper_first } => (first, second, upper_first),
    };
    let (source, mut depth) = depth_at(cx, first)?;
    if second != first {
        let (second_size, second_depth) = depth_at(cx, second)?;
        if second_size != source {
            return Err(ExportError::Encode("EXR depth fields have different dimensions".into()));
        }
        let w = source.0 as usize;
        for (y, (a, b)) in depth.chunks_exact_mut(w).zip(second_depth.chunks_exact(w)).enumerate() {
            if (y % 2 == 0) != upper_first {
                a.copy_from_slice(b);
            }
        }
    }
    let output = cx.output;
    let scale = cx.settings.effective_scale(comp);
    let (t, l, b, r) = output.crop.edges(source.0, source.1, scale);
    if (t, l, b, r) == (0, 0, 0, 0) && source == size {
        return Ok(depth);
    }
    // Nearest-neighbour resize keeps the measured depth of a surface at its edges.
    let cw = (i64::from(source.0) - i64::from(l) - i64::from(r)).max(1) as u64;
    let ch = (i64::from(source.1) - i64::from(t) - i64::from(b)).max(1) as u64;
    let mut result = Vec::with_capacity((size.0 as usize).saturating_mul(size.1 as usize));
    for y in 0..size.1 {
        let sy = i64::from(t) + ((u64::from(y) * ch + ch / 2) / u64::from(size.1)) as i64;
        for x in 0..size.0 {
            let sx = i64::from(l) + ((u64::from(x) * cw + cw / 2) / u64::from(size.0)) as i64;
            let z = if (0..i64::from(source.0)).contains(&sx) && (0..i64::from(source.1)).contains(&sy) {
                let offset = sy as usize * source.0 as usize + sx as usize;
                depth.get(offset).copied().unwrap_or(BACKGROUND_DEPTH)
            } else {
                BACKGROUND_DEPTH
            };
            result.push(z);
        }
    }
    Ok(result)
}

fn depth_at(cx: &Cx<'_>, t: Tick) -> Result<((u32, u32), Vec<f32>)> {
    let aux = cx.exr_depth_at(t).ok_or(ExportError::NoComp)?;
    let size = (aux.width, aux.height);
    let plane = aux.channels.into_iter().find_map(|(name, plane)| (name == "Z").then_some(plane));
    let pixels = usize::try_from(u64::from(size.0) * u64::from(size.1)).ok();
    let plane = plane.filter(|z| size.0 > 0 && size.1 > 0 && Some(z.len()) == pixels);
    plane.map(|z| (size, z)).ok_or_else(|| ExportError::Encode("The renderer did not supply a valid EXR depth plane".into()))
}

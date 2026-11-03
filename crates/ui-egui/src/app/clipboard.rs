#[cfg(test)]
use super::*;

/// Put `width` × `height` straight-alpha RGBA8 pixels on the system clipboard (Composition ▸ Copy
/// Frame to Clipboard). The pixel count is checked first: egui's image constructor panics on a
/// size that doesn't match its buffer.
pub(super) fn copy_image(ctx: &egui::Context, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || w.checked_mul(h).and_then(|n| n.checked_mul(4)) != Some(rgba.len()) {
        return Err(format!("cannot copy a malformed {width}×{height} image to the clipboard"));
    }
    ctx.copy_image(egui::ColorImage::from_rgba_unmultiplied([w, h], rgba));
    Ok(())
}

#[cfg(test)]
mod copy_frame_tests {
    use super::*;

    fn copied(output: &egui::FullOutput) -> Vec<&egui::ColorImage> {
        output
            .platform_output
            .commands
            .iter()
            .filter_map(|c| match c {
                egui::OutputCommand::CopyImage(image) => Some(image),
                _ => None,
            })
            .collect()
    }

    /// Composition ▸ Copy Frame to Clipboard puts the rendered frame on the system clipboard.
    #[test]
    fn copy_frame_puts_the_frame_on_the_clipboard() {
        let mut session = Session::default();
        session.execute("comp.new", json!({"name": "Copy", "width": 8, "height": 6, "duration": 1})).unwrap();
        session.execute("layer.newSolid", json!({"color": "#ff0000"})).unwrap();
        let mut app = EffectcraftApp::new(session);
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            crate::menus::invoke(&mut app, ui.ctx(), "comp.copyFrame", json!({})).unwrap();
            app.handle_events(ui.ctx());
        });
        output.textures_delta.clear();
        let images = copied(&output);
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].size, [8, 6]);
        assert!(images[0].pixels.iter().all(|p| *p == egui::Color32::from_rgb(255, 0, 0)));
    }

    /// A malformed image never reaches egui's panicking constructor.
    #[test]
    fn a_malformed_image_is_an_error_not_a_crash() {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            assert!(copy_image(ui.ctx(), 4, 4, &[0; 10]).is_err());
            assert!(copy_image(ui.ctx(), 0, 4, &[]).is_err());
            assert!(copy_image(ui.ctx(), u32::MAX, u32::MAX, &[0; 4]).is_err());
        });
        output.textures_delta.clear();
        assert!(copied(&output).is_empty());
    }
}

use super::setup;
use crate::eval::EvalCtx;
use crate::{FootageSource, Image, LayerCache, RenderOpts, Renderer};
use effectcraft_color::Label;
use effectcraft_project::{Footage, FootageKind, ItemId, ItemKind, LayerSource, Proxy, build, render_queue::ProxyUse};
use effectcraft_time::Tick;
use std::sync::Arc;

/// A source and its proxy deliberately have different pixels.
struct Pictures;
impl FootageSource for Pictures {
    fn frame(&self, _: ItemId, footage: &Footage, _: Tick) -> Option<Arc<Image>> {
        let color = if footage.path == "proxy" { [0.0, 1.0, 0.0, 1.0] } else { [1.0, 0.0, 0.0, 1.0] };
        Some(Arc::new(Image::filled(16, 16, color)))
    }
}

/// A queue run reuses its layer cache across jobs with different Proxy Use settings.
/// Time effects read layer_input, which must never serve another job's source pixels.
#[test]
fn shared_layer_cache_separates_proxy_use() {
    let (mut p, cid, comp) = setup();
    let footage = Footage { path: "original".into(), kind: FootageKind::Still, width: 16, height: 16, has_video: true, ..Default::default() };
    let item = p.add_item("Footage", Label::Red, None, ItemKind::Footage(footage.clone()));
    p.item_mut(item).unwrap().proxy = Some(Box::new(Proxy { footage: Footage { path: "proxy".into(), ..footage }, enabled: false }));
    let layer = build::layer(&mut p, &comp, "Footage", LayerSource::Footage { item }, (16, 16), None);
    p.comp_mut(cid).unwrap().layers.push(layer);
    for enabled in [false, true] {
        p.item_mut(item).unwrap().proxy.as_mut().unwrap().enabled = enabled;
        let cache = LayerCache::default();
        let ctx = EvalCtx::new(&p, cid, p.comp(cid).unwrap(), Tick::ZERO);
        let layer = &ctx.comp.layers[0];
        for proxy in [ProxyUse::UseAll, ProxyUse::UseNone, ProxyUse::CurrentSettings, ProxyUse::UseCompOnly, ProxyUse::UseAll] {
            let mut r = Renderer::new(&p, &Pictures, RenderOpts { proxy, ..Default::default() });
            let input = r.layer_input(&ctx, layer, 0).unwrap();
            let content = r.content_buf(&ctx, layer).unwrap();
            r.cache = Some(&cache);
            assert_eq!(r.layer_input(&ctx, layer, 0).unwrap().img.data, input.img.data, "{proxy:?}, enabled={enabled}");
            assert_eq!(r.content_buf(&ctx, layer).unwrap().img.data, content.img.data, "{proxy:?}, enabled={enabled}");
            let hits = cache.stats().hits;
            r.layer_input(&ctx, layer, 0).unwrap();
            assert!(cache.stats().hits > hits, "unchanged proxy settings still reuse inputs");
        }
    }
}

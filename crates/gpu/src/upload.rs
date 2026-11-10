//! Footage uploads (#3333): each decoded footage picture uploads once and its texture is shared
//! by every layer and effect input that shows it ([`Pictures`]), and large uploads copy through
//! staging buffers that are kept mapped and reused ([`Staging`]) instead of one that
//! `Queue::write_texture` allocates and frees per upload.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use effectcraft_raster::Image;

use crate::context::GpuImage;

/// Bytes of picture textures kept after their frame (a sequence's next frame takes the oldest
/// one's texture back from the pool), at most a quarter of the context's upload budget. The
/// newest picture is always kept, whatever its size.
pub(crate) const PICTURE_BUDGET: usize = 256 << 20;

/// Uploads from this size go through [`Staging`]; smaller ones through `Queue::write_texture`.
const STAGING_MIN: u64 = 1 << 20;

/// Bytes of staging buffers that may exist at once (mapped and ready, or in flight).
const STAGING_BUDGET: u64 = 192 << 20;

struct Picture {
    img: Weak<Image>,
    tex: GpuImage,
    bytes: usize,
    last_use: u64,
}

/// Uploaded footage pictures by the identity of the decoded frame (its `Arc`): the footage
/// source serves one `Arc` per (file, frame, size, colour curve) while it keeps the frame, and
/// a new one when the file changed, so the same `Arc` means the same pixels. The `Weak` keeps
/// the allocation, so a key cannot be reused by another picture while its entry exists.
#[derive(Default)]
pub(crate) struct Pictures {
    map: HashMap<usize, Picture>,
    bytes: usize,
    clock: u64,
}

impl Pictures {
    /// The texture of `img` if it was uploaded and is still kept.
    pub(crate) fn get(&mut self, img: &Arc<Image>) -> Option<GpuImage> {
        self.clock += 1;
        let now = self.clock;
        let e = self.map.get_mut(&(Arc::as_ptr(img) as usize))?;
        if !e.img.upgrade().is_some_and(|i| Arc::ptr_eq(&i, img)) {
            return None;
        }
        e.last_use = now;
        Some(e.tex.clone())
    }

    /// Keep `tex`, the upload of `img`. Pictures the footage source no longer holds go at once
    /// (nothing can ask for them again), then the least recently used over [`PICTURE_BUDGET`],
    /// never `img` itself. A dropped texture goes back to the pool once no open encoder can
    /// still read it.
    pub(crate) fn insert(&mut self, img: &Arc<Image>, tex: GpuImage, budget: usize) {
        let key = Arc::as_ptr(img) as usize;
        let bytes = img.data.len().saturating_mul(16);
        self.clock += 1;
        let entry = Picture { img: Arc::downgrade(img), tex, bytes, last_use: self.clock };
        if let Some(old) = self.map.insert(key, entry) {
            self.bytes = self.bytes.saturating_sub(old.bytes);
        }
        self.bytes = self.bytes.saturating_add(bytes);
        let dead: Vec<usize> = self.map.iter().filter(|(_, e)| e.img.strong_count() == 0).map(|(k, _)| *k).collect();
        for k in dead {
            if let Some(e) = self.map.remove(&k) {
                self.bytes = self.bytes.saturating_sub(e.bytes);
            }
        }
        while self.bytes > budget {
            let Some(k) = self.map.iter().filter(|(k, _)| **k != key).min_by_key(|(_, e)| e.last_use).map(|(k, _)| *k) else { break };
            if let Some(e) = self.map.remove(&k) {
                self.bytes = self.bytes.saturating_sub(e.bytes);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.map.len()
    }

    pub(crate) fn clear(&mut self) {
        self.map.clear();
        self.bytes = 0;
    }
}

#[derive(Default)]
struct StagingState {
    /// Buffers mapped for writing and not in use, each mapped whole.
    ready: Vec<wgpu::Buffer>,
    /// Bytes of every staging buffer that exists (ready, being mapped or in flight).
    total: u64,
    /// Raised by [`Staging::clear`]: a buffer of an earlier generation is dropped when its
    /// mapping completes.
    generation: u64,
    /// Uploads copied through a staging buffer (diagnostics, tests).
    copies: u64,
}

/// Reused upload staging buffers (`MAP_WRITE | COPY_SRC`). A buffer is written while mapped,
/// unmapped, copied into the texture by a command buffer submitted at once, and mapped again
/// when that submission is done (`CommandEncoder::map_buffer_on_submit`; the callback runs when
/// the device is polled, as every frame does). A new buffer is mapped asynchronously too (never
/// `mapped_at_creation`, which fails hard on drivers that report a failed allocation as an
/// internal error, #591), so the upload that made it, and any upload while no ready buffer fits
/// or the budget is spent, goes through `Queue::write_texture` instead.
#[derive(Default)]
pub(crate) struct Staging {
    state: Arc<Mutex<StagingState>>,
}

/// `bytes_per_row` of a staging copy: rows padded to `COPY_BYTES_PER_ROW_ALIGNMENT`.
fn padded_row(width: u32) -> Option<u64> {
    let row = u64::from(width).checked_mul(16)?;
    let a = u64::from(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    row.div_ceil(a).checked_mul(a)
}

impl Staging {
    fn lock(&self) -> std::sync::MutexGuard<'_, StagingState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Bytes of staging buffers that exist (see [`STAGING_BUDGET`]).
    pub(crate) fn bytes(&self) -> u64 {
        self.lock().total
    }

    /// Uploads copied through a staging buffer so far.
    #[cfg(test)]
    pub(crate) fn copies(&self) -> u64 {
        self.lock().copies
    }

    /// Forget every staging buffer (after a failed frame, #1319): those in flight are dropped
    /// when their mapping completes.
    pub(crate) fn clear(&self) {
        let mut s = self.lock();
        s.ready.clear();
        // (buffers still in flight are no longer counted: they drop when their mapping ends)
        s.total = 0;
        s.generation = s.generation.wrapping_add(1);
    }

    /// The callback of a write mapping of `buffer`: once mapped it is ready for the next
    /// upload; it is dropped (and its bytes uncounted) when mapping failed, and dropped when
    /// the staging was cleared meanwhile (whose [`Staging::clear`] stopped counting it).
    fn on_mapped(&self, buffer: &wgpu::Buffer, generation: u64) -> impl FnOnce(Result<(), wgpu::BufferAsyncError>) + wgpu::WasmNotSend + 'static {
        let state = Arc::downgrade(&self.state);
        let buffer = buffer.clone();
        move |res: Result<(), wgpu::BufferAsyncError>| {
            let Some(state) = state.upgrade() else { return };
            let mut s = state.lock().unwrap_or_else(PoisonError::into_inner);
            if s.generation != generation {
                return;
            }
            if res.is_ok() {
                s.ready.push(buffer);
            } else {
                s.total = s.total.saturating_sub(buffer.size());
            }
        }
    }

    /// Copy `img` into `texture` (its size) through a ready staging buffer, recorded and
    /// submitted at once: before anything an open encoder records later, as
    /// `Queue::write_texture` would. `false`: not done (small, no ready buffer fits, or the
    /// mapping failed); the caller writes it through the queue. Starts mapping a new buffer for
    /// later uploads when none fits and the budget allows.
    pub(crate) fn write(&self, device: &wgpu::Device, queue: &wgpu::Queue, img: &Image, texture: &wgpu::Texture) -> bool {
        let (w, h) = (img.width, img.height);
        let row = w as usize * 16;
        let Some(stride) = padded_row(w) else { return false };
        let (Some(need), Ok(stride32)) = (stride.checked_mul(u64::from(h)), u32::try_from(stride)) else { return false };
        if need < STAGING_MIN || img.data.len() != w as usize * h as usize {
            return false;
        }
        let (buffer, generation) = {
            let mut s = self.lock();
            let generation = s.generation;
            // The smallest ready buffer that holds it.
            let best = s.ready.iter().enumerate().filter(|(_, b)| b.size() >= need).min_by_key(|(_, b)| b.size()).map(|(i, _)| i);
            match best {
                Some(i) => (Some(s.ready.swap_remove(i)), generation),
                None => {
                    // Ready buffers too small for it make room (a size that no longer occurs).
                    while s.total.saturating_add(need) > STAGING_BUDGET
                        && let Some(b) = s.ready.pop()
                    {
                        s.total = s.total.saturating_sub(b.size());
                    }
                    if s.total.saturating_add(need) <= STAGING_BUDGET {
                        s.total += need;
                        drop(s);
                        let b = device.create_buffer(&wgpu::BufferDescriptor {
                            label: Some("upload staging"),
                            size: need,
                            usage: wgpu::BufferUsages::MAP_WRITE | wgpu::BufferUsages::COPY_SRC,
                            mapped_at_creation: false,
                        });
                        let done = self.on_mapped(&b, generation);
                        b.map_async(wgpu::MapMode::Write, .., done);
                    }
                    (None, generation)
                }
            }
        };
        let Some(buffer) = buffer else { return false };
        self.lock().copies += 1;
        {
            let Ok(mut view) = buffer.get_mapped_range_mut(..need) else {
                // Not mapped after all: give it up (its bytes too).
                let mut s = self.lock();
                if s.generation == generation {
                    s.total = s.total.saturating_sub(buffer.size());
                }
                return false;
            };
            let src: &[u8] = bytemuck::cast_slice(&img.data);
            let stride = stride as usize;
            if stride == row {
                view.slice(..src.len()).copy_from_slice(src);
            } else {
                for (y, line) in src.chunks_exact(row).enumerate() {
                    let at = y * stride;
                    view.slice(at..at + row).copy_from_slice(line);
                }
            }
        }
        buffer.unmap();
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("upload") });
        enc.copy_buffer_to_texture(
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(stride32), rows_per_image: Some(h) },
            },
            wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        let done = self.on_mapped(&buffer, generation);
        enc.map_buffer_on_submit(&buffer, wgpu::MapMode::Write, .., done);
        queue.submit([enc.finish()]);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::padded_row;

    #[test]
    fn staging_rows_are_padded_to_the_copy_alignment() {
        assert_eq!(padded_row(1920), Some(30720));
        assert_eq!(padded_row(97), Some(1792));
        assert_eq!(padded_row(16), Some(256));
        assert_eq!(padded_row(1), Some(256));
    }
}

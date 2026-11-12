//! The crate's integration tests, one binary (each module is a former `tests/<name>.rs`).

mod audio_sample_positions;
mod av1_encoder;
mod banding;
mod exr_scene_linear;
mod h264_gpu_yuv;
mod hevc_av1_roundtrip;
mod hevc_encoder;
#[cfg(not(target_arch = "wasm32"))]
mod mov_audio_sample_positions;
mod movie_bytes;
mod movie_color;
mod ms_rounded_footage;
#[cfg(not(target_arch = "wasm32"))]
mod native_audio_sample_positions;
mod opus_roundtrip;
mod prores_color;
mod render_settings;
mod roundtrip;
mod targa_sequence;
mod uncompressed_mov;
mod vp9_roundtrip;

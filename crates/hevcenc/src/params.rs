//! Parameter sets (7.3.2.1 VPS, 7.3.2.2 SPS with E.2.1 VUI, 7.3.2.3 PPS), profile_tier_level (7.3.3),
//! level selection (Annex A, Table A.8) and the `hvcC` record (ISO/IEC 14496-15 §8.3.3.1).

use crate::bitstream::{BitWriter, NAL_PPS, NAL_SPS, NAL_VPS, nal_unit};

/// Fixed coding structure of the encoder.
pub const LOG2_CTB: u32 = 6;
pub const LOG2_MIN_CB: u32 = 3;
pub const LOG2_MIN_TB: u32 = 2;
pub const LOG2_MAX_TB: u32 = 5;
pub const LOG2_MAX_POC_LSB: u32 = 8;
/// MaxNumMergeCand.
pub const MAX_MERGE_CAND: usize = 5;

/// Sequence-level settings shared by the parameter-set writers.
#[derive(Clone, Debug)]
pub struct SeqParams {
    /// Coded (padded) size, multiples of the minimum CB size.
    pub coded_width: u32,
    pub coded_height: u32,
    /// Output (cropped) size.
    pub width: u32,
    pub height: u32,
    pub bit_depth: u32,
    pub profile_idc: u8,
    pub level_idc: u8,
    pub full_range: bool,
    pub fps_num: u32,
    pub fps_den: u32,
}

/// (level_idc, MaxLumaPs, MaxLumaSr, MaxBR main tier in kbit/s) of Table A.8 (and A.9 for luma rates).
const LEVELS: [(u8, u64, u64, u64); 13] = [
    (30, 36_864, 552_960, 128),
    (60, 122_880, 3_686_400, 1_500),
    (63, 245_760, 7_372_800, 3_000),
    (90, 552_960, 16_588_800, 6_000),
    (93, 983_040, 33_177_600, 10_000),
    (120, 2_228_224, 66_846_720, 12_000),
    (123, 2_228_224, 133_693_440, 20_000),
    (150, 8_912_896, 267_386_880, 25_000),
    (153, 8_912_896, 534_773_760, 40_000),
    (156, 8_912_896, 1_069_547_520, 60_000),
    (180, 35_651_584, 1_069_547_520, 60_000),
    (183, 35_651_584, 2_139_095_040, 120_000),
    (186, 35_651_584, 4_278_190_080, 240_000),
];

/// Lowest level whose picture size, sample rate and (when known) bitrate limits fit.
pub fn choose_level(width: u32, height: u32, fps_num: u32, fps_den: u32, kbps: Option<u32>) -> u8 {
    let ps = width as u64 * height as u64;
    let sr = (ps as f64 * fps_num as f64 / fps_den.max(1) as f64).ceil() as u64;
    for &(idc, max_ps, max_sr, max_br) in &LEVELS {
        let max_dim = ((max_ps * 8) as f64).sqrt() as u64;
        if ps <= max_ps && (width as u64) <= max_dim && (height as u64) <= max_dim && sr <= max_sr && kbps.is_none_or(|k| k as u64 <= max_br) {
            return idc;
        }
    }
    186
}

fn profile_tier_level(w: &mut BitWriter, p: &SeqParams) {
    w.put(0, 2); // general_profile_space
    w.put(0, 1); // general_tier_flag (Main tier)
    w.put(p.profile_idc as u32, 5);
    w.put(compat_flags(p.profile_idc), 32);
    w.put(1, 1); // general_progressive_source_flag
    w.put(0, 1); // general_interlaced_source_flag
    w.put(0, 1); // general_non_packed_constraint_flag
    w.put(1, 1); // general_frame_only_constraint_flag
    w.put(0, 32); // general_reserved_zero_43bits ...
    w.put(0, 11);
    w.put(0, 1); // general_inbld_flag / reserved
    w.put(p.level_idc as u32, 8);
    // sps_max_sub_layers_minus1 == 0: no sub-layer flags
}

/// general_profile_compatibility_flag[j] as a 32-bit word (flag 0 is the MSB). A Main stream is also
/// Main 10 compatible.
fn compat_flags(profile_idc: u8) -> u32 {
    let mut f = 1u32 << (31 - profile_idc as u32);
    if profile_idc == 1 {
        f |= 1 << (31 - 2);
    }
    f
}

pub fn vps(p: &SeqParams) -> Vec<u8> {
    let mut w = BitWriter::new();
    w.put(0, 4); // vps_video_parameter_set_id
    w.put(1, 1); // vps_base_layer_internal_flag
    w.put(1, 1); // vps_base_layer_available_flag
    w.put(0, 6); // vps_max_layers_minus1
    w.put(0, 3); // vps_max_sub_layers_minus1
    w.put(1, 1); // vps_temporal_id_nesting_flag
    w.put(0xffff, 16);
    profile_tier_level(&mut w, p);
    w.put(1, 1); // vps_sub_layer_ordering_info_present_flag
    w.ue(1); // vps_max_dec_pic_buffering_minus1
    w.ue(0); // vps_max_num_reorder_pics
    w.ue(0); // vps_max_latency_increase_plus1
    w.put(0, 6); // vps_max_layer_id
    w.ue(0); // vps_num_layer_sets_minus1
    w.put(0, 1); // vps_timing_info_present_flag
    w.put(0, 1); // vps_extension_flag
    w.trailing();
    nal_unit(NAL_VPS, &w.into_bytes())
}

pub fn sps(p: &SeqParams) -> Vec<u8> {
    let mut w = BitWriter::new();
    w.put(0, 4); // sps_video_parameter_set_id
    w.put(0, 3); // sps_max_sub_layers_minus1
    w.put(1, 1); // sps_temporal_id_nesting_flag
    profile_tier_level(&mut w, p);
    w.ue(0); // sps_seq_parameter_set_id
    w.ue(1); // chroma_format_idc 4:2:0
    w.ue(p.coded_width);
    w.ue(p.coded_height);
    let crop = p.coded_width != p.width || p.coded_height != p.height;
    w.flag(crop); // conformance_window_flag
    if crop {
        // offsets in chroma sample units (SubWidthC = SubHeightC = 2)
        w.ue(0);
        w.ue((p.coded_width - p.width) / 2);
        w.ue(0);
        w.ue((p.coded_height - p.height) / 2);
    }
    w.ue(p.bit_depth - 8); // bit_depth_luma_minus8
    w.ue(p.bit_depth - 8); // bit_depth_chroma_minus8
    w.ue(LOG2_MAX_POC_LSB - 4);
    w.put(1, 1); // sps_sub_layer_ordering_info_present_flag
    w.ue(1); // sps_max_dec_pic_buffering_minus1
    w.ue(0); // sps_max_num_reorder_pics
    w.ue(0); // sps_max_latency_increase_plus1
    w.ue(LOG2_MIN_CB - 3);
    w.ue(LOG2_CTB - LOG2_MIN_CB);
    w.ue(LOG2_MIN_TB - 2);
    w.ue(LOG2_MAX_TB - LOG2_MIN_TB);
    w.ue(0); // max_transform_hierarchy_depth_inter
    w.ue(0); // max_transform_hierarchy_depth_intra
    w.put(0, 1); // scaling_list_enabled_flag
    w.put(0, 1); // amp_enabled_flag
    w.put(0, 1); // sample_adaptive_offset_enabled_flag
    w.put(0, 1); // pcm_enabled_flag
    w.ue(1); // num_short_term_ref_pic_sets
    // st_ref_pic_set(0): one negative picture at delta POC -1, used by the current picture
    w.ue(1); // num_negative_pics
    w.ue(0); // num_positive_pics
    w.ue(0); // delta_poc_s0_minus1
    w.put(1, 1); // used_by_curr_pic_s0_flag
    w.put(0, 1); // long_term_ref_pics_present_flag
    w.put(0, 1); // sps_temporal_mvp_enabled_flag
    w.put(1, 1); // strong_intra_smoothing_enabled_flag
    w.put(1, 1); // vui_parameters_present_flag
    vui(&mut w, p);
    w.put(0, 1); // sps_extension_present_flag
    w.trailing();
    nal_unit(NAL_SPS, &w.into_bytes())
}

fn vui(w: &mut BitWriter, p: &SeqParams) {
    w.put(0, 1); // aspect_ratio_info_present_flag
    w.put(0, 1); // overscan_info_present_flag
    w.put(1, 1); // video_signal_type_present_flag
    w.put(5, 3); // video_format: unspecified
    w.flag(p.full_range);
    w.put(1, 1); // colour_description_present_flag
    w.put(1, 8); // colour_primaries BT.709
    w.put(1, 8); // transfer_characteristics BT.709
    w.put(1, 8); // matrix_coeffs BT.709
    w.put(0, 1); // chroma_loc_info_present_flag
    w.put(0, 1); // neutral_chroma_indication_flag
    w.put(0, 1); // field_seq_flag
    w.put(0, 1); // frame_field_info_present_flag
    w.put(0, 1); // default_display_window_flag
    w.put(1, 1); // vui_timing_info_present_flag
    w.put(p.fps_den, 32); // vui_num_units_in_tick
    w.put(p.fps_num, 32); // vui_time_scale
    w.put(0, 1); // vui_poc_proportional_to_timing_flag
    w.put(0, 1); // vui_hrd_parameters_present_flag
    w.put(0, 1); // bitstream_restriction_flag
}

pub fn pps() -> Vec<u8> {
    let mut w = BitWriter::new();
    w.ue(0); // pps_pic_parameter_set_id
    w.ue(0); // pps_seq_parameter_set_id
    w.put(0, 1); // dependent_slice_segments_enabled_flag
    w.put(0, 1); // output_flag_present_flag
    w.put(0, 3); // num_extra_slice_header_bits
    w.put(1, 1); // sign_data_hiding_enabled_flag
    w.put(0, 1); // cabac_init_present_flag
    w.ue(0); // num_ref_idx_l0_default_active_minus1
    w.ue(0); // num_ref_idx_l1_default_active_minus1
    w.se(0); // init_qp_minus26
    w.put(0, 1); // constrained_intra_pred_flag
    w.put(0, 1); // transform_skip_enabled_flag
    w.put(0, 1); // cu_qp_delta_enabled_flag
    w.se(0); // pps_cb_qp_offset
    w.se(0); // pps_cr_qp_offset
    w.put(0, 1); // pps_slice_chroma_qp_offsets_present_flag
    w.put(0, 1); // weighted_pred_flag
    w.put(0, 1); // weighted_bipred_flag
    w.put(0, 1); // transquant_bypass_enabled_flag
    w.put(0, 1); // tiles_enabled_flag
    w.put(1, 1); // entropy_coding_sync_enabled_flag (WPP: CTU rows code in parallel)
    w.put(0, 1); // pps_loop_filter_across_slices_enabled_flag
    w.put(1, 1); // deblocking_filter_control_present_flag
    w.put(0, 1); // deblocking_filter_override_enabled_flag
    w.put(0, 1); // pps_deblocking_filter_disabled_flag
    w.se(0); // pps_beta_offset_div2
    w.se(0); // pps_tc_offset_div2
    w.put(0, 1); // pps_scaling_list_data_present_flag
    w.put(0, 1); // lists_modification_present_flag
    w.ue(0); // log2_parallel_merge_level_minus2
    w.put(0, 1); // slice_segment_header_extension_present_flag
    w.put(0, 1); // pps_extension_present_flag
    w.trailing();
    nal_unit(NAL_PPS, &w.into_bytes())
}

/// HEVCDecoderConfigurationRecord with one VPS, SPS and PPS (lengthSizeMinusOne = 3).
pub fn hvcc(p: &SeqParams, sets: &[Vec<u8>; 3]) -> Vec<u8> {
    let mut o = vec![1u8]; // configurationVersion
    o.push(p.profile_idc); // profile_space 0, tier 0, profile_idc
    o.extend_from_slice(&compat_flags(p.profile_idc).to_be_bytes());
    // general_constraint_indicator_flags (48 bits): progressive, interlaced, non_packed, frame_only
    o.extend_from_slice(&[0x90, 0, 0, 0, 0, 0]);
    o.push(p.level_idc);
    o.extend_from_slice(&[0xf0, 0x00]); // reserved + min_spatial_segmentation_idc 0
    o.push(0xfc); // reserved + parallelismType 0
    o.push(0xfc | 1); // reserved + chroma_format_idc 1
    o.push(0xf8 | (p.bit_depth - 8) as u8);
    o.push(0xf8 | (p.bit_depth - 8) as u8);
    o.extend_from_slice(&[0, 0]); // avgFrameRate unspecified
    // constantFrameRate 0, numTemporalLayers 1, temporalIdNested 1, lengthSizeMinusOne 3
    o.push((1 << 3) | (1 << 2) | 3);
    o.push(3); // numOfArrays
    for (set, ty) in sets.iter().zip([NAL_VPS, NAL_SPS, NAL_PPS]) {
        o.push(0x80 | ty); // array_completeness 1
        o.extend_from_slice(&1u16.to_be_bytes());
        o.extend_from_slice(&(set.len() as u16).to_be_bytes());
        o.extend_from_slice(set);
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels() {
        assert_eq!(choose_level(1920, 1080, 30, 1, None), 120);
        assert_eq!(choose_level(1920, 1080, 60, 1, None), 123);
        assert_eq!(choose_level(3840, 2160, 30, 1, None), 150);
        assert_eq!(choose_level(1280, 720, 30, 1, None), 93);
        assert_eq!(choose_level(64, 64, 30, 1, None), 30);
        assert_eq!(choose_level(1920, 1080, 30, 1, Some(15_000)), 123);
    }
}

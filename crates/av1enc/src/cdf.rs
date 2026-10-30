//! The adaptive CDFs this encoder codes with (the subset of spec 6.8.2 init_non_coeff_cdfs /
//! init_coeff_cdfs that its syntax uses). Layout as in filmcraft-av1: each innermost array
//! holds N cumulative values (the last is 32768) plus the adaptation counter.

use crate::tables::{
    DEFAULT_ANGLE_DELTA_CDF, DEFAULT_COEFF_BASE_CDF, DEFAULT_COEFF_BASE_EOB_CDF, DEFAULT_COEFF_BR_CDF, DEFAULT_DC_SIGN_CDF, DEFAULT_DRL_MODE_CDF,
    DEFAULT_EOB_EXTRA_CDF, DEFAULT_EOB_PT_16_CDF, DEFAULT_EOB_PT_32_CDF, DEFAULT_EOB_PT_64_CDF, DEFAULT_EOB_PT_128_CDF, DEFAULT_EOB_PT_256_CDF,
    DEFAULT_EOB_PT_512_CDF, DEFAULT_EOB_PT_1024_CDF, DEFAULT_INTER_TX_TYPE_SET3_CDF, DEFAULT_INTRA_TX_TYPE_SET2_CDF, DEFAULT_IS_INTER_CDF, DEFAULT_MV_BIT_CDF,
    DEFAULT_MV_CLASS_CDF, DEFAULT_MV_CLASS0_BIT_CDF, DEFAULT_MV_CLASS0_FR_CDF, DEFAULT_MV_FR_CDF, DEFAULT_MV_JOINT_CDF, DEFAULT_MV_SIGN_CDF,
    DEFAULT_NEW_MV_CDF, DEFAULT_PARTITION_W8_CDF, DEFAULT_PARTITION_W16_CDF, DEFAULT_PARTITION_W32_CDF, DEFAULT_PARTITION_W64_CDF, DEFAULT_REF_MV_CDF,
    DEFAULT_SINGLE_REF_CDF, DEFAULT_SKIP_CDF, DEFAULT_TXB_SKIP_CDF, DEFAULT_UV_MODE_CFL_ALLOWED_CDF, DEFAULT_UV_MODE_CFL_NOT_ALLOWED_CDF, DEFAULT_Y_MODE_CDF,
    DEFAULT_ZERO_MV_CDF,
};

#[derive(Clone)]
pub(crate) struct Cdfs {
    pub y_mode: [[u16; 14]; 4],
    pub uv_mode_cfl_not_allowed: [[u16; 14]; 13],
    pub uv_mode_cfl_allowed: [[u16; 15]; 13],
    pub angle_delta: [[u16; 8]; 8],
    pub partition_w8: [[u16; 5]; 4],
    pub partition_w16: [[u16; 11]; 4],
    pub partition_w32: [[u16; 11]; 4],
    pub partition_w64: [[u16; 11]; 4],
    pub skip: [[u16; 3]; 3],
    pub is_inter: [[u16; 3]; 4],
    pub single_ref: [[[u16; 3]; 6]; 3],
    pub new_mv: [[u16; 3]; 6],
    pub zero_mv: [[u16; 3]; 2],
    pub ref_mv: [[u16; 3]; 6],
    pub drl_mode: [[u16; 3]; 3],
    pub mv_joint: [u16; 5],
    pub mv_class: [[u16; 12]; 2],
    pub mv_class0_bit: [[u16; 3]; 2],
    pub mv_fr: [[u16; 5]; 2],
    pub mv_class0_fr: [[[u16; 5]; 2]; 2],
    pub mv_sign: [[u16; 3]; 2],
    pub mv_bit: [[[u16; 3]; 10]; 2],
    pub intra_tx_type_set2: [[[u16; 6]; 13]; 3],
    pub inter_tx_type_set3: [[u16; 3]; 4],
    pub txb_skip: [[[u16; 3]; 13]; 5],
    pub eob_pt_16: [[[u16; 6]; 2]; 2],
    pub eob_pt_32: [[[u16; 7]; 2]; 2],
    pub eob_pt_64: [[[u16; 8]; 2]; 2],
    pub eob_pt_128: [[[u16; 9]; 2]; 2],
    pub eob_pt_256: [[[u16; 10]; 2]; 2],
    pub eob_pt_512: [[u16; 11]; 2],
    pub eob_pt_1024: [[u16; 12]; 2],
    pub eob_extra: [[[[u16; 3]; 9]; 2]; 5],
    pub dc_sign: [[[u16; 3]; 3]; 2],
    pub coeff_base_eob: [[[[u16; 4]; 4]; 2]; 5],
    pub coeff_base: [[[[u16; 5]; 42]; 2]; 5],
    pub coeff_br: [[[[u16; 5]; 21]; 2]; 5],
}

impl Cdfs {
    /// init_non_coeff_cdfs( ) followed by init_coeff_cdfs( ) for `base_q_idx`.
    pub fn new(base_q_idx: u32) -> Box<Cdfs> {
        let idx = match base_q_idx {
            0..=20 => 0,
            21..=60 => 1,
            61..=120 => 2,
            _ => 3,
        };
        Box::new(Cdfs {
            y_mode: DEFAULT_Y_MODE_CDF,
            uv_mode_cfl_not_allowed: DEFAULT_UV_MODE_CFL_NOT_ALLOWED_CDF,
            uv_mode_cfl_allowed: DEFAULT_UV_MODE_CFL_ALLOWED_CDF,
            angle_delta: DEFAULT_ANGLE_DELTA_CDF,
            partition_w8: DEFAULT_PARTITION_W8_CDF,
            partition_w16: DEFAULT_PARTITION_W16_CDF,
            partition_w32: DEFAULT_PARTITION_W32_CDF,
            partition_w64: DEFAULT_PARTITION_W64_CDF,
            skip: DEFAULT_SKIP_CDF,
            is_inter: DEFAULT_IS_INTER_CDF,
            single_ref: DEFAULT_SINGLE_REF_CDF,
            new_mv: DEFAULT_NEW_MV_CDF,
            zero_mv: DEFAULT_ZERO_MV_CDF,
            ref_mv: DEFAULT_REF_MV_CDF,
            drl_mode: DEFAULT_DRL_MODE_CDF,
            mv_joint: DEFAULT_MV_JOINT_CDF,
            mv_class: DEFAULT_MV_CLASS_CDF,
            mv_class0_bit: [DEFAULT_MV_CLASS0_BIT_CDF; 2],
            mv_fr: DEFAULT_MV_FR_CDF,
            mv_class0_fr: DEFAULT_MV_CLASS0_FR_CDF,
            mv_sign: [DEFAULT_MV_SIGN_CDF; 2],
            mv_bit: [DEFAULT_MV_BIT_CDF; 2],
            intra_tx_type_set2: DEFAULT_INTRA_TX_TYPE_SET2_CDF,
            inter_tx_type_set3: DEFAULT_INTER_TX_TYPE_SET3_CDF,
            txb_skip: DEFAULT_TXB_SKIP_CDF[idx],
            eob_pt_16: DEFAULT_EOB_PT_16_CDF[idx],
            eob_pt_32: DEFAULT_EOB_PT_32_CDF[idx],
            eob_pt_64: DEFAULT_EOB_PT_64_CDF[idx],
            eob_pt_128: DEFAULT_EOB_PT_128_CDF[idx],
            eob_pt_256: DEFAULT_EOB_PT_256_CDF[idx],
            eob_pt_512: DEFAULT_EOB_PT_512_CDF[idx],
            eob_pt_1024: DEFAULT_EOB_PT_1024_CDF[idx],
            eob_extra: DEFAULT_EOB_EXTRA_CDF[idx],
            dc_sign: DEFAULT_DC_SIGN_CDF[idx],
            coeff_base_eob: DEFAULT_COEFF_BASE_EOB_CDF[idx],
            coeff_base: DEFAULT_COEFF_BASE_CDF[idx],
            coeff_br: DEFAULT_COEFF_BR_CDF[idx],
        })
    }
}

impl Cdfs {
    /// The counter reset of load_cdfs( ): the last entry of every CDF array is set to 0.
    pub fn clear_counts(&mut self) {
        macro_rules! clr {
            ($($f:ident),*) => { $( self.$f.clear_count(); )* };
        }
        clr!(
            y_mode,
            uv_mode_cfl_not_allowed,
            uv_mode_cfl_allowed,
            angle_delta,
            partition_w8,
            partition_w16,
            partition_w32,
            partition_w64,
            skip,
            is_inter,
            single_ref,
            new_mv,
            zero_mv,
            ref_mv,
            drl_mode,
            mv_joint,
            mv_class,
            mv_class0_bit,
            mv_fr,
            mv_class0_fr,
            mv_sign,
            mv_bit,
            intra_tx_type_set2,
            inter_tx_type_set3,
            txb_skip,
            eob_pt_16,
            eob_pt_32,
            eob_pt_64,
            eob_pt_128,
            eob_pt_256,
            eob_pt_512,
            eob_pt_1024,
            eob_extra,
            dc_sign,
            coeff_base_eob,
            coeff_base,
            coeff_br
        );
    }
}

/// Zeroes the adaptation counter (last element) of every innermost CDF array.
trait ClearCount {
    fn clear_count(&mut self);
}

impl<const N: usize> ClearCount for [u16; N] {
    fn clear_count(&mut self) {
        self[N - 1] = 0;
    }
}

impl<T: ClearCount, const M: usize> ClearCount for [T; M] {
    fn clear_count(&mut self) {
        for e in self.iter_mut() {
            e.clear_count();
        }
    }
}

//! Effect names in the interface language, keyed by effect id (#821).
//!
//! The generated Effect menu, Effect Controls, the Timeline and Effects & Presets draw an
//! effect's name. The registry (`effectcraft_effects`) keeps the English name: scripts,
//! expressions, MCP and the control channel match on it, and the property tree stores it as the
//! effect group's name. Only the *drawing* goes through this catalog, so a project file stays
//! identical whichever interface language opens it.
//!
//! Rows are keyed by id rather than by the English source string, for two reasons:
//!
//! - a second instance of an effect is stored as `Gaussian Blur 2` (the engine appends the
//!   index to the group name), which an English-string key would miss;
//! - upstream renames a display name now and then (26.5 turned `Twirl` into `Twirl (Legacy)`);
//!   the id is stable, so a translation never becomes an orphan.
//!
//! Each row repeats the registry's English name so that a rename upstream fails
//! [`tests::every_row_names_a_registry_effect`] instead of silently pointing a translation at
//! another effect. An effect with no row draws the registry's English name; the
//! `missing_translations_report` test (run by `cargo xtask ci`) lists those per language.
//!
//! A language decides for itself which names stay English: After Effects' Japanese interface
//! leaves the `CC` series, the `VR` set and the `OCIO` set in English, while its Simplified
//! Chinese one translates them (the `CC` names come from Adobe's Chinese *documentation*, which
//! names 68 of the 69). Repeating the English name is therefore a language-level choice, not a
//! missing row.
//!
//! The Simplified Chinese names come from the installed After Effects 2026 (26.5.0.89)
//! Simplified Chinese interface, its 21 category folder names included; `Cryptomatte`,
//! `EXtractoR`, `IDentifier`, `Key Light` and `Unmult` stay in English as brand names, and
//! `Face Measurements` / `Face Track Points` are EffectCraft's own effects, so their wording is
//! ours.

use std::collections::HashMap;
use std::sync::OnceLock;

use super::ui::current;

/// One row: effect id, the registry's English name, the translation.
type Row = (&'static str, &'static str, &'static str);

/// One language's catalog: its effect rows and its category labels, the latter keyed by the
/// registry's English category name.
struct Catalog {
    rows: &'static [Row],
    categories: &'static [(&'static str, &'static str)],
    /// `rows` indexed by effect id, built on first use: the panels look a name up once per drawn
    /// row and per frame.
    index: OnceLock<HashMap<&'static str, (&'static str, &'static str)>>,
}

impl Catalog {
    fn row(&self, id: &str) -> Option<(&'static str, &'static str)> {
        self.index.get_or_init(|| self.rows.iter().map(|(id, english, translated)| (*id, (*english, *translated))).collect()).get(id).copied()
    }

    fn category(&self, source: &str) -> Option<&'static str> {
        self.categories.iter().find(|(english, _)| *english == source).map(|(_, translated)| *translated)
    }
}

/// The catalog for `language`; `None` for English (the source of every name, needing no lookup)
/// and for a language with no table.
///
/// **Adding a language** is two tables and one arm: write its rows (one per effect, ids from
/// [`SIMPLIFIED_CHINESE`]) and its category labels, name them in the `static` below and in this
/// `match`. Nothing else changes — not the lookups, not one panel. `zh-hant` and `ja` are listed
/// with empty tables, so a translator fills rows in without touching this function, and until
/// then those languages draw the registry's English name.
fn catalog(language: &str) -> Option<&'static Catalog> {
    static SIMPLIFIED: Catalog = Catalog { rows: SIMPLIFIED_CHINESE, categories: SIMPLIFIED_CHINESE_CATEGORIES, index: OnceLock::new() };
    static TRADITIONAL: Catalog = Catalog { rows: TRADITIONAL_CHINESE, categories: TRADITIONAL_CHINESE_CATEGORIES, index: OnceLock::new() };
    static JAPANESE: Catalog = Catalog { rows: JAPANESE_EFFECTS, categories: JAPANESE_CATEGORIES, index: OnceLock::new() };
    static GEORGIAN_CATALOG: Catalog = Catalog { rows: GEORGIAN, categories: GEORGIAN_CATEGORIES, index: OnceLock::new() };
    match language {
        "zh-hans" => Some(&SIMPLIFIED),
        "zh-hant" => Some(&TRADITIONAL),
        // Japanese: the slot the Japanese data point on #821 asked for, so both languages share
        // this one catalog instead of inventing a second mechanism.
        "ja" => Some(&JAPANESE),
        "ka" => Some(&GEORGIAN_CATALOG),
        _ => None,
    }
}

/// The effect's name to draw: the catalog row for `id`, else the registry's `english`.
pub(crate) fn name(id: &str, english: &str) -> String {
    catalog(current()).and_then(|c| c.row(id)).map_or_else(|| english.to_string(), |(_, translated)| translated.to_string())
}

/// The name to draw for an effect group the property tree stores as `stored`: the catalog name,
/// plus the instance index the engine appends to a repeat (`Gaussian Blur 2` → `高斯模糊 2`).
///
/// A name that is neither the registry's nor the registry's plus an index was typed by the user
/// (Effect Controls ▸ Rename), so it stays verbatim — as does the whole string in a language
/// with no row.
pub(crate) fn group(id: &str, stored: &str) -> String {
    let Some((english, translated)) = catalog(current()).and_then(|c| c.row(id)) else { return stored.to_string() };
    if stored == english {
        return translated.to_string();
    }
    if let Some(index) = stored.strip_prefix(english).and_then(|rest| rest.strip_prefix(' '))
        && !index.is_empty()
        && index.bytes().all(|b| b.is_ascii_digit())
    {
        return format!("{translated} {index}");
    }
    stored.to_string()
}

/// The label to draw for the effect category `source` (the generated Effect menu's submenus and
/// Effects & Presets' folder rows), else `source`.
pub(crate) fn category(source: &str) -> String {
    catalog(current()).and_then(|c| c.category(source)).map_or_else(|| source.to_string(), str::to_string)
}

/// Does effect `id` (`english` when the language has no row) match the Effects & Presets search
/// `query` (lowercase)? Both names match, so a translated interface can be searched either way.
pub(crate) fn matches(id: &str, english: &str, query: &str) -> bool {
    english.to_lowercase().contains(query)
        || catalog(current()).and_then(|c| c.row(id)).is_some_and(|(_, translated)| translated.to_lowercase().contains(query))
}

/// The registry's effect names and categories `language` has no row for, for the missing-strings
/// report.
#[cfg(test)]
pub(super) fn missing(language: &str) -> Vec<String> {
    let table = catalog(language);
    let effects = effectcraft_engine::effects::registry().iter().filter(|spec| table.and_then(|c| c.row(spec.id)).is_none()).map(|spec| spec.name.to_string());
    let categories = effectcraft_engine::effects::categories()
        .into_iter()
        .filter(|source| table.and_then(|c| c.category(source)).is_none())
        .map(|source| format!("{source} (category)"));
    effects.chain(categories).collect()
}

/// Simplified Chinese (`zh-hans`): one row per effect in the registry.
const SIMPLIFIED_CHINESE: &[Row] = &[
    ("ec.3d.channelextract", "3D Channel Extract", "3D 通道提取"),
    ("ec.3d.cryptomatte", "Cryptomatte", "Cryptomatte"),
    ("ec.3d.depthmatte", "Depth Matte", "深度遮罩"),
    ("ec.3d.depthoffield", "Depth of Field", "场深度"),
    ("ec.3d.extractor", "EXtractoR", "EXtractoR"),
    ("ec.3d.fog3d", "Fog 3D", "雾 3D"),
    ("ec.3d.identifier", "IDentifier", "IDentifier"),
    ("ec.3d.idmatte", "ID Matte", "ID 遮罩"),
    ("ec.audio.backwards", "Backwards", "倒放"),
    ("ec.audio.basstreble", "Bass & Treble", "低音和高音"),
    ("ec.audio.compressor", "Compressor", "压缩程序"),
    ("ec.audio.delay", "Delay", "延迟"),
    ("ec.audio.distortion", "Distortion", "扭曲"),
    ("ec.audio.flangechorus", "Flange & Chorus", "变调与合声"),
    ("ec.audio.gate", "Gate", "门"),
    ("ec.audio.highlowpass", "High-Low Pass", "高通/低通"),
    ("ec.audio.modulator", "Modulator", "调制器"),
    ("ec.audio.parametriceq", "Parametric EQ", "参数均衡"),
    ("ec.audio.reverb", "Reverb", "混响"),
    ("ec.audio.stereomixer", "Stereo Mixer", "立体声混合器"),
    ("ec.audio.tone", "Tone", "音调"),
    ("ec.blur.bilateral", "Bilateral Blur", "双向模糊"),
    ("ec.blur.cameralens", "Camera Lens Blur", "摄像机镜头模糊"),
    ("ec.blur.camerashakedeblur", "Camera-Shake Deblur", "摄像机抖动去模糊"),
    ("ec.blur.cccross", "CC Cross Blur", "CC 交叉模糊"),
    ("ec.blur.ccradial", "CC Radial Blur", "CC 径向模糊"),
    ("ec.blur.ccradialfast", "CC Radial Fast Blur", "CC 径向快速模糊"),
    ("ec.blur.ccvector", "CC Vector Blur", "CC 矢量模糊"),
    ("ec.blur.channel", "Channel Blur", "通道模糊"),
    ("ec.blur.compound", "Compound Blur", "复合模糊"),
    ("ec.blur.directional", "Directional Blur", "定向模糊"),
    ("ec.blur.fastbox", "Fast Box Blur", "快速方框模糊"),
    ("ec.blur.gaussian", "Gaussian Blur", "高斯模糊"),
    ("ec.blur.radial", "Radial Blur", "径向模糊"),
    ("ec.blur.reduceflicker", "Reduce Interlace Flicker", "减少交错闪烁"),
    ("ec.blur.sharpen", "Sharpen", "锐化"),
    ("ec.blur.smart", "Smart Blur", "智能模糊"),
    ("ec.blur.unsharp", "Unsharp Mask", "钝化蒙版"),
    ("ec.channel.arithmetic", "Arithmetic", "算术"),
    ("ec.channel.blend", "Blend", "混合"),
    ("ec.channel.calculations", "Calculations", "计算"),
    ("ec.channel.cccomposite", "CC Composite", "CC 合成"),
    ("ec.channel.combiner", "Channel Combiner", "通道合成器"),
    ("ec.channel.compoundarithmetic", "Compound Arithmetic", "复合运算"),
    ("ec.channel.invert", "Invert", "反转"),
    ("ec.channel.minimax", "Minimax", "最小/最大"),
    ("ec.channel.removecolormatting", "Remove Color Matting", "移除颜色遮罩"),
    ("ec.channel.setchannels", "Set Channels", "设置通道"),
    ("ec.channel.setmatte", "Set Matte", "设置遮罩"),
    ("ec.channel.shiftchannels", "Shift Channels", "转换通道"),
    ("ec.channel.solidcomposite", "Solid Composite", "固态层合成"),
    ("ec.color.autocolor", "Auto Color", "自动颜色"),
    ("ec.color.autocontrast", "Auto Contrast", "自动对比度"),
    ("ec.color.autolevels", "Auto Levels", "自动色阶"),
    ("ec.color.blackwhite", "Black & White", "黑色和白色"),
    ("ec.color.brightnesscontrast", "Brightness & Contrast", "亮度和对比度"),
    ("ec.color.broadcast", "Broadcast Colors", "广播颜色"),
    ("ec.color.cccolorneutralizer", "CC Color Neutralizer", "CC 颜色中和器"),
    ("ec.color.cccoloroffset", "CC Color Offset", "CC 颜色偏移"),
    ("ec.color.cckernel", "CC Kernel", "CC 内核"),
    ("ec.color.cctoner", "CC Toner", "CC 调色器"),
    ("ec.color.changecolor", "Change Color", "更改颜色"),
    ("ec.color.changetocolor", "Change to Color", "更改为颜色"),
    ("ec.color.channelmixer", "Channel Mixer", "通道混合器"),
    ("ec.color.colorama", "Colorama", "色光"),
    ("ec.color.colorbalance", "Color Balance", "颜色平衡"),
    ("ec.color.colorbalancehls", "Color Balance (HLS)", "颜色平衡 (HLS)"),
    ("ec.color.colorlink", "Color Link", "颜色链接"),
    ("ec.color.colorstabilizer", "Color Stabilizer", "颜色稳定器"),
    ("ec.color.curves", "Curves", "曲线"),
    ("ec.color.equalize", "Equalize", "色调均化"),
    ("ec.color.exposure", "Exposure", "曝光度"),
    ("ec.color.gammapedestalgain", "Gamma/Pedestal/Gain", "灰度系数/基值/增益"),
    ("ec.color.huesaturation", "Hue/Saturation", "色相/饱和度"),
    ("ec.color.leavecolor", "Leave Color", "保留颜色"),
    ("ec.color.levels", "Levels", "色阶"),
    ("ec.color.levelsic", "Levels (Individual Controls)", "色阶 (单独控件)"),
    ("ec.color.lumetri", "Lumetri Color", "Lumetri 颜色"),
    ("ec.color.ociocdl", "OCIO CDL Transform", "OCIO CDL 变换"),
    ("ec.color.ociocolorspace", "OCIO Color Space Transform", "OCIO 颜色空间变换"),
    ("ec.color.ociodisplay", "OCIO Display Transform", "OCIO 显示变换"),
    ("ec.color.ociofile", "OCIO File Transform", "OCIO 文件转换"),
    ("ec.color.ociolook", "OCIO Look Transform", "OCIO 外观变换"),
    ("ec.color.photofilter", "Photo Filter", "照片滤镜"),
    ("ec.color.psarbitrarymap", "PS Arbitrary Map", "PS 任意映射"),
    ("ec.color.selectivecolor", "Selective Color", "可选颜色"),
    ("ec.color.shadowhighlight", "Shadow/Highlight", "阴影/高光"),
    ("ec.color.tint", "Tint", "色调"),
    ("ec.color.tritone", "Tritone", "三色调"),
    ("ec.color.vibrance", "Vibrance", "自然饱和度"),
    ("ec.color.videolimiter", "Video Limiter", "视频限幅器"),
    ("ec.control.angle", "Angle Control", "角度控制"),
    ("ec.control.checkbox", "Checkbox Control", "复选框控件"),
    ("ec.control.color", "Color Control", "颜色控制"),
    ("ec.control.dropdown", "Dropdown Menu Control", "下拉菜单控件"),
    ("ec.control.layer", "Layer Control", "图层控制"),
    ("ec.control.point", "Point Control", "点控制"),
    ("ec.control.point3d", "3D Point Control", "3D 点控制"),
    ("ec.control.slider", "Slider Control", "滑块控制"),
    ("ec.control.stereo3d", "Stereo 3D Controls", "立体 3D 控制"),
    ("ec.distort.bezierwarp", "Bezier Warp", "贝塞尔曲线变形"),
    ("ec.distort.bulge", "Bulge", "凸出"),
    ("ec.distort.ccbender", "CC Bender", "CC 弯曲器"),
    ("ec.distort.ccbendit", "CC Bend It", "CC 弯曲"),
    ("ec.distort.ccblobbylize", "CC Blobbylize", "CC 融化"),
    ("ec.distort.ccflomotion", "CC Flo Motion", "CC 流动运动"),
    ("ec.distort.ccgriddler", "CC Griddler", "CC 网格生成器"),
    ("ec.distort.cclens", "CC Lens", "CC 镜头"),
    ("ec.distort.ccpageturn", "CC Page Turn", "CC 翻页"),
    ("ec.distort.ccpowerpin", "CC Power Pin", "CC 强力边角定位"),
    ("ec.distort.ccripplepulse", "CC Ripple Pulse", "CC 波纹脉冲"),
    ("ec.distort.ccslant", "CC Slant", "CC 倾斜"),
    ("ec.distort.ccsmear", "CC Smear", "CC 漩涡条纹"),
    ("ec.distort.ccsplit", "CC Split", "CC 拆分"),
    ("ec.distort.ccsplit2", "CC Split 2", "CC 拆分 2"),
    ("ec.distort.cctiler", "CC Tiler", "CC 拼贴器"),
    ("ec.distort.cornerpin", "Corner Pin", "边角定位"),
    ("ec.distort.displacementmap", "Displacement Map", "置换图"),
    ("ec.distort.liquify", "Liquify", "液化"),
    ("ec.distort.magnify", "Magnify", "放大"),
    ("ec.distort.meshwarp", "Mesh Warp", "网格变形"),
    ("ec.distort.mirror", "Mirror", "镜像"),
    ("ec.distort.offset", "Offset", "偏移"),
    ("ec.distort.opticscompensation", "Optics Compensation", "光学补偿"),
    ("ec.distort.polar", "Polar Coordinates", "极坐标"),
    ("ec.distort.puppet", "Puppet", "操控"),
    ("ec.distort.reshape", "Reshape", "改变形状"),
    ("ec.distort.ripple", "Ripple", "波纹"),
    ("ec.distort.rollingshutterrepair", "Rolling Shutter Repair", "果冻效应修复"),
    ("ec.distort.smear", "Smear", "涂抹"),
    ("ec.distort.spherize", "Spherize", "球面化"),
    ("ec.distort.transform", "Transform", "变换"),
    ("ec.distort.turbulentdisplace", "Turbulent Displace", "湍流置换"),
    ("ec.distort.twirl", "Twirl", "旋转扭曲"),
    ("ec.distort.twirllegacy", "Twirl (Legacy)", "旋转扭曲 (旧版)"),
    ("ec.distort.upscale", "Detail-preserving Upscale", "保留细节放大"),
    ("ec.distort.warp", "Warp", "变形"),
    ("ec.distort.warpstabilizer", "Warp Stabilizer", "变形稳定器"),
    ("ec.distort.wavewarp", "Wave Warp", "波形变形"),
    ("ec.generate.advancedlightning", "Advanced Lightning", "高级闪电"),
    ("ec.generate.audiospectrum", "Audio Spectrum", "音频频谱"),
    ("ec.generate.audiowaveform", "Audio Waveform", "音频波形"),
    ("ec.generate.beam", "Beam", "光束"),
    ("ec.generate.ccgluegun", "CC Glue Gun", "CC 胶水枪"),
    ("ec.generate.cclightburst", "CC Light Burst 2.5", "CC 光束放射 2.5"),
    ("ec.generate.cclightrays", "CC Light Rays", "CC 光线"),
    ("ec.generate.cclightsweep", "CC Light Sweep", "CC 光线扫描"),
    ("ec.generate.ccthreads", "CC Threads", "CC 编织条"),
    ("ec.generate.cellpattern", "Cell Pattern", "单元格图案"),
    ("ec.generate.checkerboard", "Checkerboard", "棋盘"),
    ("ec.generate.circle", "Circle", "圆形"),
    ("ec.generate.ellipse", "Ellipse", "椭圆"),
    ("ec.generate.eyedropperfill", "Eyedropper Fill", "吸管填充"),
    ("ec.generate.fill", "Fill", "填充"),
    ("ec.generate.fourcolor", "4-Color Gradient", "四色渐变"),
    ("ec.generate.fractal", "Fractal", "分形"),
    ("ec.generate.gradientramp", "Gradient Ramp", "梯度渐变"),
    ("ec.generate.grid", "Grid", "网格"),
    ("ec.generate.lensflare", "Lens Flare", "镜头光晕"),
    ("ec.generate.paintbucket", "Paint Bucket", "油漆桶"),
    ("ec.generate.radiowaves", "Radio Waves", "无线电波"),
    ("ec.generate.scribble", "Scribble", "涂写"),
    ("ec.generate.stroke", "Stroke", "描边"),
    ("ec.generate.vegas", "Vegas", "勾画"),
    ("ec.generate.writeon", "Write-on", "写入"),
    ("ec.key.advancedspill", "Advanced Spill Suppressor", "高级溢出抑制器"),
    ("ec.key.ccsimplewireremoval", "CC Simple Wire Removal", "CC 简单丝线移除"),
    ("ec.key.colordifference", "Color Difference Key", "颜色差值键"),
    ("ec.key.colorkey", "Color Key", "颜色键"),
    ("ec.key.colorrange", "Color Range", "颜色范围"),
    ("ec.key.differencematte", "Difference Matte", "差值遮罩"),
    ("ec.key.extract", "Extract", "提取"),
    ("ec.key.innerouter", "Inner/Outer Key", "内部/外部键"),
    ("ec.key.keycleaner", "Key Cleaner", "抠像清除键"),
    ("ec.key.linearcolor", "Linear Color Key", "线性颜色键"),
    ("ec.key.luma", "Luma Key", "亮度键"),
    ("ec.key.screen", "Screen Key", "屏幕键"),
    ("ec.key.spill", "Spill Suppressor", "溢出抑制"),
    ("ec.key.unmult", "Unmult", "Unmult"),
    ("ec.keying.keylight", "Key Light", "Key Light"),
    ("ec.matte.mattechoker", "Matte Choker", "遮罩阻塞工具"),
    ("ec.matte.objectmatte", "Object Matte", "对象遮罩"),
    ("ec.matte.refinehard", "Refine Hard Matte", "调整实边遮罩"),
    ("ec.matte.refinesoft", "Refine Soft Matte", "调整柔和遮罩"),
    ("ec.matte.rotobrush", "Roto Brush & Refine Edge", "Roto 笔刷和细化边缘"),
    ("ec.matte.simplechoker", "Simple Choker", "简单阻塞工具"),
    ("ec.noise.addgrain", "Add Grain", "添加颗粒"),
    ("ec.noise.curlnoise", "Curl Noise", "旋度噪声"),
    ("ec.noise.dustscratches", "Dust & Scratches", "蒙尘与划痕"),
    ("ec.noise.fractal", "Fractal Noise", "分形杂色"),
    ("ec.noise.matchgrain", "Match Grain", "匹配颗粒"),
    ("ec.noise.median", "Median", "中间值"),
    ("ec.noise.medianlegacy", "Median (Legacy)", "中间值 (旧版)"),
    ("ec.noise.noise", "Noise", "杂色"),
    ("ec.noise.noisealpha", "Noise Alpha", "杂色 Alpha"),
    ("ec.noise.noisehls", "Noise HLS", "杂色 HLS"),
    ("ec.noise.noisehlsauto", "Noise HLS Auto", "杂色 HLS 自动"),
    ("ec.noise.removegrain", "Remove Grain", "移除颗粒"),
    ("ec.noise.turbulent", "Turbulent Noise", "湍流杂色"),
    ("ec.obsolete.basic3d", "Basic 3D", "基本 3D"),
    ("ec.obsolete.basictext", "Basic Text", "基本文字"),
    ("ec.obsolete.gaussianlegacy", "Gaussian Blur (Legacy)", "高斯模糊 (旧版)"),
    ("ec.obsolete.lightning", "Lightning", "闪电"),
    ("ec.obsolete.mochashape", "Mocha shape", "Mocha 形状"),
    ("ec.obsolete.pathtext", "Path Text", "路径文本"),
    ("ec.paint.paint", "Paint", "绘画"),
    ("ec.perspective.3dglasses", "3D Glasses", "3D 眼镜"),
    ("ec.perspective.bevelalpha", "Bevel Alpha", "斜面 Alpha"),
    ("ec.perspective.beveledges", "Bevel Edges", "边缘斜面"),
    ("ec.perspective.cameratracker", "3D Camera Tracker", "3D 摄像机跟踪器"),
    ("ec.perspective.cccylinder", "CC Cylinder", "CC 圆柱体"),
    ("ec.perspective.ccenvironment", "CC Environment", "CC 环境"),
    ("ec.perspective.ccsphere", "CC Sphere", "CC 球体"),
    ("ec.perspective.ccspotlight", "CC Spotlight", "CC 聚光"),
    ("ec.perspective.dropshadow", "Drop Shadow", "投影"),
    ("ec.perspective.radialshadow", "Radial Shadow", "径向阴影"),
    ("ec.sim.carddance", "Card Dance", "卡片动画"),
    ("ec.sim.caustics", "Caustics", "焦散"),
    ("ec.sim.ccballaction", "CC Ball Action", "CC 球动作"),
    ("ec.sim.ccbubbles", "CC Bubbles", "CC 气泡"),
    ("ec.sim.ccdrizzle", "CC Drizzle", "CC 细雨波纹"),
    ("ec.sim.cchair", "CC Hair", "CC 毛发"),
    ("ec.sim.ccmrmercury", "CC Mr. Mercury", "CC 水银流体"),
    ("ec.sim.ccparticlesystems2", "CC Particle Systems II", "CC 粒子系统 II"),
    ("ec.sim.ccparticleworld", "CC Particle World", "CC 粒子世界"),
    ("ec.sim.ccpixelpolly", "CC Pixel Polly", "CC 像素多边形"),
    ("ec.sim.ccrainfall", "CC Rainfall", "CC 降雨"),
    ("ec.sim.ccscatterize", "CC Scatterize", "CC 散射"),
    ("ec.sim.ccsnowfall", "CC Snowfall", "CC 降雪"),
    ("ec.sim.ccstarburst", "CC Star Burst", "CC 星爆"),
    ("ec.sim.foam", "Foam", "泡沫"),
    ("ec.sim.particleplayground", "Particle Playground", "粒子运动场"),
    ("ec.sim.shatter", "Shatter", "碎片"),
    ("ec.sim.waveworld", "Wave World", "波形环境"),
    ("ec.stylize.brushstrokes", "Brush Strokes", "画笔描边"),
    ("ec.stylize.cartoon", "Cartoon", "卡通"),
    ("ec.stylize.ccblockload", "CC Block Load", "CC 块加载"),
    ("ec.stylize.ccburnfilm", "CC Burn Film", "CC 烧毁胶片"),
    ("ec.stylize.ccglass", "CC Glass", "CC 玻璃"),
    ("ec.stylize.cchextile", "CC HexTile", "CC 六边形拼贴"),
    ("ec.stylize.cckaleida", "CC Kaleida", "CC 万花筒"),
    ("ec.stylize.ccmrsmoothie", "CC Mr. Smoothie", "CC 像素溶解"),
    ("ec.stylize.ccplastic", "CC Plastic", "CC 塑料"),
    ("ec.stylize.ccrepetile", "CC RepeTile", "CC 重复拼贴"),
    ("ec.stylize.ccthreshold", "CC Threshold", "CC 阈值"),
    ("ec.stylize.ccthresholdrgb", "CC Threshold RGB", "CC 阈值 RGB"),
    ("ec.stylize.ccvignette", "CC Vignette", "CC 暗角"),
    ("ec.stylize.coloremboss", "Color Emboss", "彩色浮雕"),
    ("ec.stylize.emboss", "Emboss", "浮雕"),
    ("ec.stylize.findedges", "Find Edges", "查找边缘"),
    ("ec.stylize.glow", "Glow", "发光"),
    ("ec.stylize.mosaic", "Mosaic", "马赛克"),
    ("ec.stylize.motiontile", "Motion Tile", "动态拼贴"),
    ("ec.stylize.posterize", "Posterize", "色调分离"),
    ("ec.stylize.roughenedges", "Roughen Edges", "毛边"),
    ("ec.stylize.scatter", "Scatter", "散布"),
    ("ec.stylize.strobe", "Strobe Light", "闪光灯"),
    ("ec.stylize.texturize", "Texturize", "纹理化"),
    ("ec.stylize.threshold", "Threshold", "阈值"),
    ("ec.text.numbers", "Numbers", "编号"),
    ("ec.text.timecode", "Timecode", "时间码"),
    ("ec.time.ccforcemotionblur", "CC Force Motion Blur", "CC 强制运动模糊"),
    ("ec.time.ccwidetime", "CC Wide Time", "CC 宽泛时间"),
    ("ec.time.echo", "Echo", "残影"),
    ("ec.time.pixelmotionblur", "Pixel Motion Blur", "像素运动模糊"),
    ("ec.time.posterizetime", "Posterize Time", "抽帧"),
    ("ec.time.timedifference", "Time Difference", "时差"),
    ("ec.time.timedisplacement", "Time Displacement", "时间置换"),
    ("ec.time.timewarp", "Timewarp", "时间扭曲"),
    ("ec.transition.blockdissolve", "Block Dissolve", "块溶解"),
    ("ec.transition.cardwipe", "Card Wipe", "卡片擦除"),
    ("ec.transition.ccglasswipe", "CC Glass Wipe", "CC 玻璃擦除"),
    ("ec.transition.ccgridwipe", "CC Grid Wipe", "CC 网格擦除"),
    ("ec.transition.ccimagewipe", "CC Image Wipe", "CC 图像擦除"),
    ("ec.transition.ccjaws", "CC Jaws", "CC 锯齿"),
    ("ec.transition.cclightwipe", "CC Light Wipe", "CC 光线擦除"),
    ("ec.transition.cclinesweep", "CC Line Sweep", "CC 线条扫掠"),
    ("ec.transition.ccradialscalewipe", "CC Radial ScaleWipe", "CC 径向缩放擦除"),
    ("ec.transition.ccscalewipe", "CC Scale Wipe", "CC 缩放擦除"),
    ("ec.transition.cctwister", "CC Twister", "CC 扭曲器"),
    ("ec.transition.ccwarpomatic", "CC WarpoMatic", "CC 自动扭曲"),
    ("ec.transition.gradientwipe", "Gradient Wipe", "渐变擦除"),
    ("ec.transition.iriswipe", "Iris Wipe", "光圈擦除"),
    ("ec.transition.linearwipe", "Linear Wipe", "线性擦除"),
    ("ec.transition.radialwipe", "Radial Wipe", "径向擦除"),
    ("ec.transition.venetian", "Venetian Blinds", "百叶窗"),
    ("ec.utility.applylut", "Apply Color LUT", "应用颜色 LUT"),
    ("ec.utility.ccoverbrights", "CC Overbrights", "CC 高光溢出"),
    ("ec.utility.cineon", "Cineon Converter", "Cineon 转换器"),
    ("ec.utility.colorprofileconverter", "Color Profile Converter", "颜色配置文件转换器"),
    ("ec.utility.facemeasurements", "Face Measurements", "面部测量"),
    ("ec.utility.facetrackpoints", "Face Track Points", "面部追踪点"),
    ("ec.utility.growbounds", "Grow Bounds", "范围扩散"),
    ("ec.utility.hdrcompander", "HDR Compander", "HDR 压缩扩展器"),
    ("ec.utility.hdrcompression", "HDR Highlight Compression", "HDR 高光压缩"),
    ("ec.vr.blur", "VR Blur", "VR 模糊"),
    ("ec.vr.chromaticaberrations", "VR Chromatic Aberrations", "VR 色差"),
    ("ec.vr.colorgradients", "VR Color Gradients", "VR 颜色渐变"),
    ("ec.vr.converter", "VR Converter", "VR 转换器"),
    ("ec.vr.denoise", "VR De-Noise", "VR 降噪"),
    ("ec.vr.digitalglitch", "VR Digital Glitch", "VR 数字故障"),
    ("ec.vr.fractalnoise", "VR Fractal Noise", "VR 分形杂色"),
    ("ec.vr.glow", "VR Glow", "VR 发光"),
    ("ec.vr.planetosphere", "VR Plane to Sphere", "VR 平面到球面"),
    ("ec.vr.rotatesphere", "VR Rotate Sphere", "VR 旋转球面"),
    ("ec.vr.sharpen", "VR Sharpen", "VR 锐化"),
    ("ec.vr.spheretoplane", "VR Sphere to Plane", "VR 球面到平面"),
];
/// Traditional Chinese (`zh-hant`): no rows yet. A translator fills one row per effect, ids from
/// [`SIMPLIFIED_CHINESE`]; a row may repeat the English name, which is what the language draws
/// for that effect.
const TRADITIONAL_CHINESE: &[Row] = &[];

/// Japanese (`ja`): reserved for the Japanese data point on #821, so both languages share this
/// one catalog instead of inventing a second mechanism. A translator fills one row per effect,
/// ids from [`SIMPLIFIED_CHINESE`]; the names Adobe's Japanese interface leaves in English (the
/// `CC` series, the `VR` set, the `OCIO` set, `Cryptomatte`, `EXtractoR`, `IDentifier`,
/// `Key Light`, `Unmult`, `Twirl (Legacy)` and the two EffectCraft-only effects) repeat that
/// English name here rather than being left out.
///
/// The rows below were read from the installed After Effects 2026 (26.5) Japanese interface
/// (its generated Effect menu and its Effects & Presets panel).
const JAPANESE_EFFECTS: &[Row] = &[
    ("ec.blur.bilateral", "Bilateral Blur", "ブラー（バイラテラル）"),
    ("ec.blur.cameralens", "Camera Lens Blur", "ブラー（カメラレンズ）"),
    ("ec.blur.channel", "Channel Blur", "ブラー（チャンネル）"),
    ("ec.blur.compound", "Compound Blur", "ブラー（合成）"),
    ("ec.blur.directional", "Directional Blur", "ブラー（方向）"),
    ("ec.blur.fastbox", "Fast Box Blur", "高速ボックスブラー"),
    ("ec.blur.gaussian", "Gaussian Blur", "ブラー（ガウス）"),
    ("ec.blur.radial", "Radial Blur", "ブラー（放射状）"),
    ("ec.color.lumetri", "Lumetri Color", "Lumetri カラー"),
    ("ec.key.colorkey", "Color Key", "カラーキー"),
    ("ec.key.luma", "Luma Key", "ルミナンスキー"),
    ("ec.key.spill", "Spill Suppressor", "スピルサプレッション"),
    ("ec.obsolete.basic3d", "Basic 3D", "基本 3D"),
    ("ec.obsolete.basictext", "Basic Text", "基本テキスト"),
    ("ec.obsolete.gaussianlegacy", "Gaussian Blur (Legacy)", "ブラー（ガウス）（レガシー）"),
    ("ec.obsolete.lightning", "Lightning", "稲妻"),
    ("ec.obsolete.pathtext", "Path Text", "パステキスト"),
    ("ec.time.pixelmotionblur", "Pixel Motion Blur", "ピクセルモーションブラー"),
];

/// Traditional Chinese (`zh-hant`) category labels, keyed by the registry's English category.
const TRADITIONAL_CHINESE_CATEGORIES: &[(&str, &str)] = &[];

/// Japanese (`ja`) category labels, keyed by the registry's English category.
const JAPANESE_CATEGORIES: &[(&str, &str)] = &[
    ("3D Channel", "3D チャンネル"),
    ("Audio", "オーディオ"),
    ("Blur & Sharpen", "ブラー＆シャープ"),
    ("Channel", "チャンネル"),
    ("Color Correction", "カラー補正"),
    ("Distort", "ディストーション"),
    ("Expression Controls", "式制御"),
    ("Immersive Video", "イマーシブビデオ"),
    ("Keying", "キーイング"),
    ("Matte", "マット"),
    ("Noise & Grain", "ノイズ＆グレイン"),
    ("Obsolete", "旧バージョン"),
    ("Paint", "描画"),
    ("Perspective", "遠近"),
    ("Simulation", "シミュレーション"),
    ("Stylize", "スタイライズ"),
    ("Text", "テキスト"),
    ("Time", "時間"),
    ("Transition", "トランジション"),
    ("Utility", "ユーティリティ"),
];

/// Georgian (`ka`): one row per effect, ids from [`SIMPLIFIED_CHINESE`]. Written from the meaning
/// of the English names, with the terms the Georgian menus and panels use (FilmCraft's Georgian
/// for the effects the two apps share). The `CC` series and the brand names (`Cryptomatte`,
/// `EXtractoR`, `IDentifier`, `Key Light`, `Unmult`, `Lumetri Color`) stay in English, as
/// Adobe's Japanese interface keeps the `CC` series; `VR` and `OCIO` keep their prefix.
const GEORGIAN: &[Row] = &[
    ("ec.3d.channelextract", "3D Channel Extract", "3D არხის ამოღება"),
    ("ec.3d.cryptomatte", "Cryptomatte", "Cryptomatte"),
    ("ec.3d.depthmatte", "Depth Matte", "სიღრმის მატი"),
    ("ec.3d.depthoffield", "Depth of Field", "ველის სიღრმე"),
    ("ec.3d.extractor", "EXtractoR", "EXtractoR"),
    ("ec.3d.fog3d", "Fog 3D", "3D ნისლი"),
    ("ec.3d.identifier", "IDentifier", "IDentifier"),
    ("ec.3d.idmatte", "ID Matte", "ID მატი"),
    ("ec.audio.backwards", "Backwards", "უკუღმა"),
    ("ec.audio.basstreble", "Bass & Treble", "ბასი და ტრებლი"),
    ("ec.audio.compressor", "Compressor", "კომპრესორი"),
    ("ec.audio.delay", "Delay", "დაყოვნება"),
    ("ec.audio.distortion", "Distortion", "დამახინჯება"),
    ("ec.audio.flangechorus", "Flange & Chorus", "ფლენჯერი და ქორუსი"),
    ("ec.audio.gate", "Gate", "გეიტი"),
    ("ec.audio.highlowpass", "High-Low Pass", "მაღალი/დაბალი სიხშირეების ფილტრი"),
    ("ec.audio.modulator", "Modulator", "მოდულატორი"),
    ("ec.audio.parametriceq", "Parametric EQ", "პარამეტრული ეკვალაიზერი"),
    ("ec.audio.reverb", "Reverb", "რევერბი"),
    ("ec.audio.stereomixer", "Stereo Mixer", "სტერეომიქშერი"),
    ("ec.audio.tone", "Tone", "ტონი"),
    ("ec.blur.bilateral", "Bilateral Blur", "ბილატერალური გაბუნდოვნება"),
    ("ec.blur.cameralens", "Camera Lens Blur", "კამერის ობიექტივის გაბუნდოვნება"),
    ("ec.blur.camerashakedeblur", "Camera-Shake Deblur", "კამერის რყევით გაბუნდოვნების მოცილება"),
    ("ec.blur.cccross", "CC Cross Blur", "CC Cross Blur"),
    ("ec.blur.ccradial", "CC Radial Blur", "CC Radial Blur"),
    ("ec.blur.ccradialfast", "CC Radial Fast Blur", "CC Radial Fast Blur"),
    ("ec.blur.ccvector", "CC Vector Blur", "CC Vector Blur"),
    ("ec.blur.channel", "Channel Blur", "არხის გაბუნდოვნება"),
    ("ec.blur.compound", "Compound Blur", "შედგენილი გაბუნდოვნება"),
    ("ec.blur.directional", "Directional Blur", "მიმართული გაბუნდოვნება"),
    ("ec.blur.fastbox", "Fast Box Blur", "სწრაფი ბლოკური გაბუნდოვნება"),
    ("ec.blur.gaussian", "Gaussian Blur", "გაუსის გაბუნდოვნება"),
    ("ec.blur.radial", "Radial Blur", "რადიალური გაბუნდოვნება"),
    ("ec.blur.reduceflicker", "Reduce Interlace Flicker", "ნახევარკადრული ციმციმის შემცირება"),
    ("ec.blur.sharpen", "Sharpen", "სიმკვეთრე"),
    ("ec.blur.smart", "Smart Blur", "ჭკვიანი გაბუნდოვნება"),
    ("ec.blur.unsharp", "Unsharp Mask", "არამკვეთრი ნიღაბი"),
    ("ec.channel.arithmetic", "Arithmetic", "არითმეტიკა"),
    ("ec.channel.blend", "Blend", "შერევა"),
    ("ec.channel.calculations", "Calculations", "გამოთვლები"),
    ("ec.channel.cccomposite", "CC Composite", "CC Composite"),
    ("ec.channel.combiner", "Channel Combiner", "არხების გამაერთიანებელი"),
    ("ec.channel.compoundarithmetic", "Compound Arithmetic", "შედგენილი არითმეტიკა"),
    ("ec.channel.invert", "Invert", "ინვერსია"),
    ("ec.channel.minimax", "Minimax", "მინიმაქსი"),
    ("ec.channel.removecolormatting", "Remove Color Matting", "ფერადი მატინგის მოცილება"),
    ("ec.channel.setchannels", "Set Channels", "არხების დაყენება"),
    ("ec.channel.setmatte", "Set Matte", "მატის დაყენება"),
    ("ec.channel.shiftchannels", "Shift Channels", "არხების გადანაცვლება"),
    ("ec.channel.solidcomposite", "Solid Composite", "სოლიდთან კომპოზიტი"),
    ("ec.color.autocolor", "Auto Color", "ავტოფერი"),
    ("ec.color.autocontrast", "Auto Contrast", "ავტოკონტრასტი"),
    ("ec.color.autolevels", "Auto Levels", "ავტოდონეები"),
    ("ec.color.blackwhite", "Black & White", "შავ-თეთრი"),
    ("ec.color.brightnesscontrast", "Brightness & Contrast", "სიკაშკაშე და კონტრასტი"),
    ("ec.color.broadcast", "Broadcast Colors", "სამაუწყებლო ფერები"),
    ("ec.color.cccolorneutralizer", "CC Color Neutralizer", "CC Color Neutralizer"),
    ("ec.color.cccoloroffset", "CC Color Offset", "CC Color Offset"),
    ("ec.color.cckernel", "CC Kernel", "CC Kernel"),
    ("ec.color.cctoner", "CC Toner", "CC Toner"),
    ("ec.color.changecolor", "Change Color", "ფერის შეცვლა"),
    ("ec.color.changetocolor", "Change to Color", "ფერად შეცვლა"),
    ("ec.color.channelmixer", "Channel Mixer", "არხების მიქშერი"),
    ("ec.color.colorama", "Colorama", "კოლორამა"),
    ("ec.color.colorbalance", "Color Balance", "ფერთა ბალანსი"),
    ("ec.color.colorbalancehls", "Color Balance (HLS)", "ფერთა ბალანსი (HLS)"),
    ("ec.color.colorlink", "Color Link", "ფერის ბმა"),
    ("ec.color.colorstabilizer", "Color Stabilizer", "ფერის სტაბილიზატორი"),
    ("ec.color.curves", "Curves", "მრუდები"),
    ("ec.color.equalize", "Equalize", "გათანაბრება"),
    ("ec.color.exposure", "Exposure", "ექსპოზიცია"),
    ("ec.color.gammapedestalgain", "Gamma/Pedestal/Gain", "გამა/საფეხური/გაძლიერება"),
    ("ec.color.huesaturation", "Hue/Saturation", "ფერის ტონი/გაჯერება"),
    ("ec.color.leavecolor", "Leave Color", "ფერის დატოვება"),
    ("ec.color.levels", "Levels", "დონეები"),
    ("ec.color.levelsic", "Levels (Individual Controls)", "დონეები (ცალკეული მართვა)"),
    ("ec.color.lumetri", "Lumetri Color", "Lumetri Color"),
    ("ec.color.ociocdl", "OCIO CDL Transform", "OCIO CDL გარდაქმნა"),
    ("ec.color.ociocolorspace", "OCIO Color Space Transform", "OCIO ფერთა სივრცის გარდაქმნა"),
    ("ec.color.ociodisplay", "OCIO Display Transform", "OCIO ეკრანის გარდაქმნა"),
    ("ec.color.ociofile", "OCIO File Transform", "OCIO ფაილის გარდაქმნა"),
    ("ec.color.ociolook", "OCIO Look Transform", "OCIO ვიზუალური სტილის გარდაქმნა"),
    ("ec.color.photofilter", "Photo Filter", "ფოტოფილტრი"),
    ("ec.color.psarbitrarymap", "PS Arbitrary Map", "PS ნებისმიერი რუკა"),
    ("ec.color.selectivecolor", "Selective Color", "შერჩევითი ფერი"),
    ("ec.color.shadowhighlight", "Shadow/Highlight", "ჩრდილები/ღია ტონები"),
    ("ec.color.tint", "Tint", "ტონირება"),
    ("ec.color.tritone", "Tritone", "სამტონიანი"),
    ("ec.color.vibrance", "Vibrance", "სიხალასე"),
    ("ec.color.videolimiter", "Video Limiter", "ვიდეოლიმიტერი"),
    ("ec.control.angle", "Angle Control", "კუთხის მართვა"),
    ("ec.control.checkbox", "Checkbox Control", "მოსანიშნი ველის მართვა"),
    ("ec.control.color", "Color Control", "ფერის მართვა"),
    ("ec.control.dropdown", "Dropdown Menu Control", "ჩამოსაშლელი მენიუს მართვა"),
    ("ec.control.layer", "Layer Control", "შრის მართვა"),
    ("ec.control.point", "Point Control", "წერტილის მართვა"),
    ("ec.control.point3d", "3D Point Control", "3D წერტილის მართვა"),
    ("ec.control.slider", "Slider Control", "სლაიდერის მართვა"),
    ("ec.control.stereo3d", "Stereo 3D Controls", "სტერეო 3D მართვა"),
    ("ec.distort.bezierwarp", "Bezier Warp", "ბეზიეს დეფორმაცია"),
    ("ec.distort.bulge", "Bulge", "გამობერილობა"),
    ("ec.distort.ccbender", "CC Bender", "CC Bender"),
    ("ec.distort.ccbendit", "CC Bend It", "CC Bend It"),
    ("ec.distort.ccblobbylize", "CC Blobbylize", "CC Blobbylize"),
    ("ec.distort.ccflomotion", "CC Flo Motion", "CC Flo Motion"),
    ("ec.distort.ccgriddler", "CC Griddler", "CC Griddler"),
    ("ec.distort.cclens", "CC Lens", "CC Lens"),
    ("ec.distort.ccpageturn", "CC Page Turn", "CC Page Turn"),
    ("ec.distort.ccpowerpin", "CC Power Pin", "CC Power Pin"),
    ("ec.distort.ccripplepulse", "CC Ripple Pulse", "CC Ripple Pulse"),
    ("ec.distort.ccslant", "CC Slant", "CC Slant"),
    ("ec.distort.ccsmear", "CC Smear", "CC Smear"),
    ("ec.distort.ccsplit", "CC Split", "CC Split"),
    ("ec.distort.ccsplit2", "CC Split 2", "CC Split 2"),
    ("ec.distort.cctiler", "CC Tiler", "CC Tiler"),
    ("ec.distort.cornerpin", "Corner Pin", "კუთხეების მიმაგრება"),
    ("ec.distort.displacementmap", "Displacement Map", "წანაცვლების რუკა"),
    ("ec.distort.liquify", "Liquify", "გათხევადება"),
    ("ec.distort.magnify", "Magnify", "გადიდება"),
    ("ec.distort.meshwarp", "Mesh Warp", "ბადისებრი დეფორმაცია"),
    ("ec.distort.mirror", "Mirror", "სარკე"),
    ("ec.distort.offset", "Offset", "წანაცვლება"),
    ("ec.distort.opticscompensation", "Optics Compensation", "ოპტიკის კომპენსაცია"),
    ("ec.distort.polar", "Polar Coordinates", "პოლარული კოორდინატები"),
    ("ec.distort.puppet", "Puppet", "მარიონეტი"),
    ("ec.distort.reshape", "Reshape", "ფორმის შეცვლა"),
    ("ec.distort.ripple", "Ripple", "ტალღა"),
    ("ec.distort.rollingshutterrepair", "Rolling Shutter Repair", "მცოცავი ჩამკეტის გასწორება"),
    ("ec.distort.smear", "Smear", "გადღაბნა"),
    ("ec.distort.spherize", "Spherize", "სფერიზება"),
    ("ec.distort.transform", "Transform", "გარდაქმნა"),
    ("ec.distort.turbulentdisplace", "Turbulent Displace", "ტურბულენტური წანაცვლება"),
    ("ec.distort.twirl", "Twirl", "გრიგალი"),
    ("ec.distort.twirllegacy", "Twirl (Legacy)", "გრიგალი (მოძველებული)"),
    ("ec.distort.upscale", "Detail-preserving Upscale", "გადიდება დეტალების შენარჩუნებით"),
    ("ec.distort.warp", "Warp", "დეფორმაცია"),
    ("ec.distort.warpstabilizer", "Warp Stabilizer", "დეფორმაციის სტაბილიზატორი"),
    ("ec.distort.wavewarp", "Wave Warp", "ტალღური დეფორმაცია"),
    ("ec.generate.advancedlightning", "Advanced Lightning", "გაფართოებული ელვა"),
    ("ec.generate.audiospectrum", "Audio Spectrum", "აუდიოს სპექტრი"),
    ("ec.generate.audiowaveform", "Audio Waveform", "აუდიოს ტალღის ფორმა"),
    ("ec.generate.beam", "Beam", "სხივი"),
    ("ec.generate.ccgluegun", "CC Glue Gun", "CC Glue Gun"),
    ("ec.generate.cclightburst", "CC Light Burst 2.5", "CC Light Burst 2.5"),
    ("ec.generate.cclightrays", "CC Light Rays", "CC Light Rays"),
    ("ec.generate.cclightsweep", "CC Light Sweep", "CC Light Sweep"),
    ("ec.generate.ccthreads", "CC Threads", "CC Threads"),
    ("ec.generate.cellpattern", "Cell Pattern", "უჯრედების პატერნი"),
    ("ec.generate.checkerboard", "Checkerboard", "ჭადრაკის დაფა"),
    ("ec.generate.circle", "Circle", "წრე"),
    ("ec.generate.ellipse", "Ellipse", "ელიფსი"),
    ("ec.generate.eyedropperfill", "Eyedropper Fill", "პიპეტით შევსება"),
    ("ec.generate.fill", "Fill", "შევსება"),
    ("ec.generate.fourcolor", "4-Color Gradient", "4-ფერიანი გრადიენტი"),
    ("ec.generate.fractal", "Fractal", "ფრაქტალი"),
    ("ec.generate.gradientramp", "Gradient Ramp", "გრადიენტული გადასვლა"),
    ("ec.generate.grid", "Grid", "ბადე"),
    ("ec.generate.lensflare", "Lens Flare", "ობიექტივის ანარეკლი"),
    ("ec.generate.paintbucket", "Paint Bucket", "საღებავის ვედრო"),
    ("ec.generate.radiowaves", "Radio Waves", "რადიოტალღები"),
    ("ec.generate.scribble", "Scribble", "ჯღაბნა"),
    ("ec.generate.stroke", "Stroke", "კონტური"),
    ("ec.generate.vegas", "Vegas", "ვეგასი"),
    ("ec.generate.writeon", "Write-on", "თანდათანობითი წერა"),
    ("ec.key.advancedspill", "Advanced Spill Suppressor", "ფერის გადაღვრის გაფართოებული ჩამხშობი"),
    ("ec.key.ccsimplewireremoval", "CC Simple Wire Removal", "CC Simple Wire Removal"),
    ("ec.key.colordifference", "Color Difference Key", "ფერთა სხვაობის გასაღები"),
    ("ec.key.colorkey", "Color Key", "ფერის გასაღები"),
    ("ec.key.colorrange", "Color Range", "ფერების დიაპაზონი"),
    ("ec.key.differencematte", "Difference Matte", "სხვაობის მატი"),
    ("ec.key.extract", "Extract", "ამოღება"),
    ("ec.key.innerouter", "Inner/Outer Key", "შიდა/გარე გასაღები"),
    ("ec.key.keycleaner", "Key Cleaner", "გასაღების გამწმენდი"),
    ("ec.key.linearcolor", "Linear Color Key", "წრფივი ფერის გასაღები"),
    ("ec.key.luma", "Luma Key", "სიკაშკაშის გასაღები"),
    ("ec.key.screen", "Screen Key", "ეკრანის გასაღები"),
    ("ec.key.spill", "Spill Suppressor", "ფერის გადაღვრის ჩამხშობი"),
    ("ec.key.unmult", "Unmult", "Unmult"),
    ("ec.keying.keylight", "Key Light", "Key Light"),
    ("ec.matte.mattechoker", "Matte Choker", "მატის შემკუმშველი"),
    ("ec.matte.objectmatte", "Object Matte", "ობიექტის მატი"),
    ("ec.matte.refinehard", "Refine Hard Matte", "მკვეთრი მატის დახვეწა"),
    ("ec.matte.refinesoft", "Refine Soft Matte", "რბილი მატის დახვეწა"),
    ("ec.matte.rotobrush", "Roto Brush & Refine Edge", "როტო-ფუნჯი და კიდის დახვეწა"),
    ("ec.matte.simplechoker", "Simple Choker", "მარტივი შემკუმშველი"),
    ("ec.noise.addgrain", "Add Grain", "მარცვლოვნების დამატება"),
    ("ec.noise.curlnoise", "Curl Noise", "გრიგალური ხმაური"),
    ("ec.noise.dustscratches", "Dust & Scratches", "მტვერი და ნაკაწრები"),
    ("ec.noise.fractal", "Fractal Noise", "ფრაქტალური ხმაური"),
    ("ec.noise.matchgrain", "Match Grain", "მარცვლოვნების მისადაგება"),
    ("ec.noise.median", "Median", "მედიანა"),
    ("ec.noise.medianlegacy", "Median (Legacy)", "მედიანა (მოძველებული)"),
    ("ec.noise.noise", "Noise", "ხმაური"),
    ("ec.noise.noisealpha", "Noise Alpha", "ალფა-ხმაური"),
    ("ec.noise.noisehls", "Noise HLS", "HLS ხმაური"),
    ("ec.noise.noisehlsauto", "Noise HLS Auto", "HLS ხმაური (ავტომატური)"),
    ("ec.noise.removegrain", "Remove Grain", "მარცვლოვნების მოცილება"),
    ("ec.noise.turbulent", "Turbulent Noise", "ტურბულენტური ხმაური"),
    ("ec.obsolete.basic3d", "Basic 3D", "საბაზო 3D"),
    ("ec.obsolete.basictext", "Basic Text", "საბაზო ტექსტი"),
    ("ec.obsolete.gaussianlegacy", "Gaussian Blur (Legacy)", "გაუსის გაბუნდოვნება (მოძველებული)"),
    ("ec.obsolete.lightning", "Lightning", "ელვა"),
    ("ec.obsolete.mochashape", "Mocha shape", "Mocha-ს ფიგურა"),
    ("ec.obsolete.pathtext", "Path Text", "ტექსტი ტრაექტორიაზე"),
    ("ec.paint.paint", "Paint", "ხატვა"),
    ("ec.perspective.3dglasses", "3D Glasses", "3D სათვალე"),
    ("ec.perspective.bevelalpha", "Bevel Alpha", "ალფას ფასკა"),
    ("ec.perspective.beveledges", "Bevel Edges", "კიდეების ფასკა"),
    ("ec.perspective.cameratracker", "3D Camera Tracker", "3D კამერის ტრეკერი"),
    ("ec.perspective.cccylinder", "CC Cylinder", "CC Cylinder"),
    ("ec.perspective.ccenvironment", "CC Environment", "CC Environment"),
    ("ec.perspective.ccsphere", "CC Sphere", "CC Sphere"),
    ("ec.perspective.ccspotlight", "CC Spotlight", "CC Spotlight"),
    ("ec.perspective.dropshadow", "Drop Shadow", "ჩრდილი"),
    ("ec.perspective.radialshadow", "Radial Shadow", "რადიალური ჩრდილი"),
    ("ec.sim.carddance", "Card Dance", "მოცეკვავე ბარათები"),
    ("ec.sim.caustics", "Caustics", "კაუსტიკა"),
    ("ec.sim.ccballaction", "CC Ball Action", "CC Ball Action"),
    ("ec.sim.ccbubbles", "CC Bubbles", "CC Bubbles"),
    ("ec.sim.ccdrizzle", "CC Drizzle", "CC Drizzle"),
    ("ec.sim.cchair", "CC Hair", "CC Hair"),
    ("ec.sim.ccmrmercury", "CC Mr. Mercury", "CC Mr. Mercury"),
    ("ec.sim.ccparticlesystems2", "CC Particle Systems II", "CC Particle Systems II"),
    ("ec.sim.ccparticleworld", "CC Particle World", "CC Particle World"),
    ("ec.sim.ccpixelpolly", "CC Pixel Polly", "CC Pixel Polly"),
    ("ec.sim.ccrainfall", "CC Rainfall", "CC Rainfall"),
    ("ec.sim.ccscatterize", "CC Scatterize", "CC Scatterize"),
    ("ec.sim.ccsnowfall", "CC Snowfall", "CC Snowfall"),
    ("ec.sim.ccstarburst", "CC Star Burst", "CC Star Burst"),
    ("ec.sim.foam", "Foam", "ქაფი"),
    ("ec.sim.particleplayground", "Particle Playground", "ნაწილაკების სიმულაცია"),
    ("ec.sim.shatter", "Shatter", "დამსხვრევა"),
    ("ec.sim.waveworld", "Wave World", "ტალღების სამყარო"),
    ("ec.stylize.brushstrokes", "Brush Strokes", "ფუნჯის შტრიხები"),
    ("ec.stylize.cartoon", "Cartoon", "მულტფილმის სტილი"),
    ("ec.stylize.ccblockload", "CC Block Load", "CC Block Load"),
    ("ec.stylize.ccburnfilm", "CC Burn Film", "CC Burn Film"),
    ("ec.stylize.ccglass", "CC Glass", "CC Glass"),
    ("ec.stylize.cchextile", "CC HexTile", "CC HexTile"),
    ("ec.stylize.cckaleida", "CC Kaleida", "CC Kaleida"),
    ("ec.stylize.ccmrsmoothie", "CC Mr. Smoothie", "CC Mr. Smoothie"),
    ("ec.stylize.ccplastic", "CC Plastic", "CC Plastic"),
    ("ec.stylize.ccrepetile", "CC RepeTile", "CC RepeTile"),
    ("ec.stylize.ccthreshold", "CC Threshold", "CC Threshold"),
    ("ec.stylize.ccthresholdrgb", "CC Threshold RGB", "CC Threshold RGB"),
    ("ec.stylize.ccvignette", "CC Vignette", "CC Vignette"),
    ("ec.stylize.coloremboss", "Color Emboss", "ფერადი რელიეფი"),
    ("ec.stylize.emboss", "Emboss", "რელიეფი"),
    ("ec.stylize.findedges", "Find Edges", "კიდეების პოვნა"),
    ("ec.stylize.glow", "Glow", "ნათება"),
    ("ec.stylize.mosaic", "Mosaic", "მოზაიკა"),
    ("ec.stylize.motiontile", "Motion Tile", "მოძრავი ფილები"),
    ("ec.stylize.posterize", "Posterize", "პოსტერიზაცია"),
    ("ec.stylize.roughenedges", "Roughen Edges", "კიდეების დაუხეშება"),
    ("ec.stylize.scatter", "Scatter", "გაბნევა"),
    ("ec.stylize.strobe", "Strobe Light", "სტრობოსკოპი"),
    ("ec.stylize.texturize", "Texturize", "ტექსტურირება"),
    ("ec.stylize.threshold", "Threshold", "ზღვარი"),
    ("ec.text.numbers", "Numbers", "რიცხვები"),
    ("ec.text.timecode", "Timecode", "თაიმკოდი"),
    ("ec.time.ccforcemotionblur", "CC Force Motion Blur", "CC Force Motion Blur"),
    ("ec.time.ccwidetime", "CC Wide Time", "CC Wide Time"),
    ("ec.time.echo", "Echo", "ექო"),
    ("ec.time.pixelmotionblur", "Pixel Motion Blur", "პიქსელების მოძრაობის გაბუნდოვნება"),
    ("ec.time.posterizetime", "Posterize Time", "დროის პოსტერიზაცია"),
    ("ec.time.timedifference", "Time Difference", "დროითი სხვაობა"),
    ("ec.time.timedisplacement", "Time Displacement", "დროითი წანაცვლება"),
    ("ec.time.timewarp", "Timewarp", "Timewarp"),
    ("ec.transition.blockdissolve", "Block Dissolve", "ბლოკური გადადნობა"),
    ("ec.transition.cardwipe", "Card Wipe", "ბარათებით წმენდა"),
    ("ec.transition.ccglasswipe", "CC Glass Wipe", "CC Glass Wipe"),
    ("ec.transition.ccgridwipe", "CC Grid Wipe", "CC Grid Wipe"),
    ("ec.transition.ccimagewipe", "CC Image Wipe", "CC Image Wipe"),
    ("ec.transition.ccjaws", "CC Jaws", "CC Jaws"),
    ("ec.transition.cclightwipe", "CC Light Wipe", "CC Light Wipe"),
    ("ec.transition.cclinesweep", "CC Line Sweep", "CC Line Sweep"),
    ("ec.transition.ccradialscalewipe", "CC Radial ScaleWipe", "CC Radial ScaleWipe"),
    ("ec.transition.ccscalewipe", "CC Scale Wipe", "CC Scale Wipe"),
    ("ec.transition.cctwister", "CC Twister", "CC Twister"),
    ("ec.transition.ccwarpomatic", "CC WarpoMatic", "CC WarpoMatic"),
    ("ec.transition.gradientwipe", "Gradient Wipe", "გრადიენტული წმენდა"),
    ("ec.transition.iriswipe", "Iris Wipe", "დიაფრაგმული წმენდა"),
    ("ec.transition.linearwipe", "Linear Wipe", "წრფივი წმენდა"),
    ("ec.transition.radialwipe", "Radial Wipe", "რადიალური წმენდა"),
    ("ec.transition.venetian", "Venetian Blinds", "ჟალუზი"),
    ("ec.utility.applylut", "Apply Color LUT", "ფერის LUT-ის გამოყენება"),
    ("ec.utility.ccoverbrights", "CC Overbrights", "CC Overbrights"),
    ("ec.utility.cineon", "Cineon Converter", "Cineon-ის გარდამქმნელი"),
    ("ec.utility.colorprofileconverter", "Color Profile Converter", "ფერის პროფილის გარდამქმნელი"),
    ("ec.utility.facemeasurements", "Face Measurements", "სახის გაზომვები"),
    ("ec.utility.facetrackpoints", "Face Track Points", "სახის ტრეკინგის წერტილები"),
    ("ec.utility.growbounds", "Grow Bounds", "საზღვრების გაზრდა"),
    ("ec.utility.hdrcompander", "HDR Compander", "HDR კომპანდერი"),
    ("ec.utility.hdrcompression", "HDR Highlight Compression", "HDR ღია ტონების შეკუმშვა"),
    ("ec.vr.blur", "VR Blur", "VR გაბუნდოვნება"),
    ("ec.vr.chromaticaberrations", "VR Chromatic Aberrations", "VR ქრომატული აბერაციები"),
    ("ec.vr.colorgradients", "VR Color Gradients", "VR ფერადი გრადიენტები"),
    ("ec.vr.converter", "VR Converter", "VR გარდამქმნელი"),
    ("ec.vr.denoise", "VR De-Noise", "VR ხმაურის მოცილება"),
    ("ec.vr.digitalglitch", "VR Digital Glitch", "VR ციფრული ხარვეზი"),
    ("ec.vr.fractalnoise", "VR Fractal Noise", "VR ფრაქტალური ხმაური"),
    ("ec.vr.glow", "VR Glow", "VR ნათება"),
    ("ec.vr.planetosphere", "VR Plane to Sphere", "VR სიბრტყიდან სფეროზე"),
    ("ec.vr.rotatesphere", "VR Rotate Sphere", "VR სფეროს შემობრუნება"),
    ("ec.vr.sharpen", "VR Sharpen", "VR სიმკვეთრე"),
    ("ec.vr.spheretoplane", "VR Sphere to Plane", "VR სფეროდან სიბრტყეზე"),
];

/// Georgian (`ka`) category labels, keyed by the registry's English category.
const GEORGIAN_CATEGORIES: &[(&str, &str)] = &[
    ("3D Channel", "3D არხი"),
    ("Audio", "აუდიო"),
    ("Blur & Sharpen", "გაბუნდოვნება და სიმკვეთრე"),
    ("Channel", "არხი"),
    ("Color Correction", "ფერის კორექცია"),
    ("Distort", "დამახინჯება"),
    ("Expression Controls", "ექსპრესიის მართვა"),
    ("Generate", "გენერაცია"),
    ("Immersive Video", "იმერსიული ვიდეო"),
    ("Keying", "კეიინგი"),
    ("Matte", "მატი"),
    ("Noise & Grain", "ხმაური და მარცვლოვნება"),
    ("Obsolete", "მოძველებული"),
    ("Paint", "ხატვა"),
    ("Perspective", "პერსპექტივა"),
    ("Simulation", "სიმულაცია"),
    ("Stylize", "სტილიზაცია"),
    ("Text", "ტექსტი"),
    ("Time", "დრო"),
    ("Transition", "გადასვლა"),
    ("Utility", "დამხმარე"),
];

/// The 21 category folder names the generated Effect menu and Effects & Presets draw, in
/// Simplified Chinese. Read from the installed After Effects 2026 (26.5.0.89) Simplified Chinese
/// interface (the checklist's `07-类别` sheet); two of them are the interface's own wording where
/// our first suggestion differed (`Keying` is 抠像, not 键控; `Obsolete` is 过时, not 已废弃).
const SIMPLIFIED_CHINESE_CATEGORIES: &[(&str, &str)] = &[
    ("3D Channel", "3D 通道"),
    ("Audio", "音频"),
    ("Blur & Sharpen", "模糊和锐化"),
    ("Channel", "通道"),
    ("Color Correction", "颜色校正"),
    ("Distort", "扭曲"),
    ("Expression Controls", "表达式控制"),
    ("Generate", "生成"),
    ("Immersive Video", "沉浸式视频"),
    ("Keying", "抠像"),
    ("Matte", "遮罩"),
    ("Noise & Grain", "杂色和颗粒"),
    ("Obsolete", "过时"),
    ("Paint", "绘画"),
    ("Perspective", "透视"),
    ("Simulation", "模拟"),
    ("Stylize", "风格化"),
    ("Text", "文本"),
    ("Time", "时间"),
    ("Transition", "过渡"),
    ("Utility", "实用工具"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::set_current;
    use effectcraft_engine::effects;

    /// Every row names an effect in the registry and still carries its English name, so a rename
    /// or a removal fails here rather than attaching a translation to a different effect.
    #[test]
    fn every_row_names_a_registry_effect() {
        let registry = effects::registry();
        for (id, english, translated) in SIMPLIFIED_CHINESE {
            let Some(spec) = registry.iter().find(|spec| spec.id == *id) else {
                panic!("{id} ({english}) is not an effect in the registry");
            };
            assert_eq!(*english, spec.name, "{id}: the catalog says {english:?}, the registry {:?}", spec.name);
            assert!(!translated.is_empty(), "{id}: empty translation");
        }
    }

    /// Rows are keyed by id, so a repeated id would shadow a translation.
    #[test]
    fn ids_are_unique() {
        let mut ids: Vec<&str> = SIMPLIFIED_CHINESE.iter().map(|(id, _, _)| *id).collect();
        let listed = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(listed, ids.len(), "an effect id is listed twice");
    }

    /// The languages that have no rows yet, with the table a translator fills in for each. A new
    /// language is added here and to [`catalog`]; the test then guards its rows as they arrive.
    const PENDING: &[(&str, &[Row])] = &[("zh-hant", TRADITIONAL_CHINESE)];

    /// The languages whose tables are being filled in: guarded like [`PENDING`] row by row.
    const PARTIAL: &[(&str, &[Row])] = &[("ja", JAPANESE_EFFECTS), ("ka", GEORGIAN)];

    /// A pending language may lag Simplified Chinese but may not invent ids or disagree about
    /// which effect an id names: a translator fills these tables row by row. While they are
    /// empty the test is a no-op, which is why the slot ships before the data does.
    #[test]
    fn a_pending_language_only_uses_known_ids() {
        for (language, rows) in PENDING.iter().chain(PARTIAL) {
            let mut ids: Vec<&str> = vec![];
            for (id, english, translated) in *rows {
                let Some((_, hans_english, _)) = SIMPLIFIED_CHINESE.iter().find(|(hans_id, _, _)| hans_id == id) else {
                    panic!("{language} row {id} is not an effect the zh-hans catalog knows");
                };
                assert_eq!(*english, *hans_english, "{id}: {language} and zh-hans name the effect differently");
                assert!(!translated.is_empty(), "{id}: empty {language} translation (repeat the English name if it stays)");
                ids.push(id);
            }
            let listed = ids.len();
            ids.sort_unstable();
            ids.dedup();
            assert_eq!(listed, ids.len(), "{language} lists an effect id twice");
        }
    }

    /// Every category row of every language names a category that exists, else the generated
    /// Effect menu and Effects & Presets would carry a label no folder has.
    #[test]
    fn every_category_row_names_a_category() {
        let categories = effects::categories();
        for (language, rows) in
            [("zh-hans", SIMPLIFIED_CHINESE_CATEGORIES), ("zh-hant", TRADITIONAL_CHINESE_CATEGORIES), ("ja", JAPANESE_CATEGORIES), ("ka", GEORGIAN_CATEGORIES)]
        {
            for (english, translated) in rows {
                assert!(categories.contains(english), "{language} lists {english}, which is not a category");
                assert!(!translated.is_empty(), "empty {language} translation for {english}");
            }
        }
    }

    /// A repeated effect carries the index the engine appends to the group name; a name the user
    /// typed in Effect Controls stays verbatim.
    #[test]
    fn group_names_keep_the_instance_index_and_user_renames() {
        set_current("zh-hans");
        let translated = name("ec.blur.gaussian", "Gaussian Blur");
        assert_eq!(group("ec.blur.gaussian", "Gaussian Blur"), translated);
        assert_eq!(group("ec.blur.gaussian", "Gaussian Blur 2"), format!("{translated} 2"));
        assert_eq!(group("ec.blur.gaussian", "Gaussian Blur 12"), format!("{translated} 12"));
        assert_eq!(group("ec.blur.gaussian", "My Blur"), "My Blur", "a rename is not a row");
        assert_eq!(group("ec.blur.gaussian", "Gaussian Blur X"), "Gaussian Blur X");
        assert_eq!(group("ec.blur.gaussian", "Gaussian Blur 2x"), "Gaussian Blur 2x");
        assert_eq!(group("ec.blur.gaussian", &translated), translated, "an already translated name stays");
        set_current("en");
    }

    /// English, and a language with no catalog or no row for an effect, draws the registry's
    /// name rather than an empty label; a language with a row draws the row. Every table in
    /// [`PENDING`] and [`PARTIAL`] has an arm in [`catalog`]: an arm-less table's rows would be dead.
    #[test]
    fn languages_without_rows_draw_the_registry_name() {
        for (language, _) in PENDING.iter().chain(PARTIAL) {
            assert!(catalog(language).is_some(), "{language} has a table but no arm in `catalog`: its rows would be dead");
        }
        let tables = PENDING.iter().chain(PARTIAL).copied().chain([("zh-hans", SIMPLIFIED_CHINESE)]);
        for (language, rows) in [("en", &[][..]), ("uk", &[][..]), ("cs", &[][..])].into_iter().chain(tables) {
            set_current(language);
            let expected = rows.iter().find(|(id, _, _)| *id == "ec.blur.gaussian").map_or("Gaussian Blur", |(_, _, translated)| *translated);
            assert_eq!(name("ec.blur.gaussian", "Gaussian Blur"), expected, "{language}");
            assert_eq!(group("ec.blur.gaussian", "Gaussian Blur 2"), format!("{expected} 2"), "{language}");
            assert_eq!(name("ec.unknown.effect", "Mystery"), "Mystery", "{language}");
            assert_eq!(category("Not A Category"), "Not A Category", "{language}");
        }
        for language in ["en", "uk", "cs"] {
            set_current(language);
            assert_eq!(category("Blur & Sharpen"), "Blur & Sharpen", "{language}");
        }
        set_current("en");
    }

    /// The Effects & Presets search answers to either name, so a Chinese interface can be
    /// searched in Chinese while an English one still finds the effect by its registry name.
    #[test]
    fn search_matches_either_name() {
        set_current("zh-hans");
        assert!(matches("ec.blur.gaussian", "Gaussian Blur", "blur"));
        assert!(matches("ec.blur.gaussian", "Gaussian Blur", "gaussian"));
        assert!(matches("ec.blur.gaussian", "Gaussian Blur", &name("ec.blur.gaussian", "Gaussian Blur").to_lowercase()));
        assert!(!matches("ec.blur.gaussian", "Gaussian Blur", "sharpen"));
        set_current("en");
        assert!(matches("ec.blur.gaussian", "Gaussian Blur", "blur"));
        assert!(!matches("ec.blur.gaussian", "Gaussian Blur", "高斯"));
    }
}

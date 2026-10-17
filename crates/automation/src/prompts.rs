//! MCP prompts (docs/mcp.md "Prompts"): workflow recipes that spell out the tool flow for common
//! jobs, so a model doesn't have to rediscover it.

use serde_json::{Map, Value, json};

struct Arg {
    name: &'static str,
    description: &'static str,
    required: bool,
}

struct PromptDef {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    args: &'static [Arg],
    text: fn(&Args) -> String,
}

/// The given arguments (strings).
struct Args(Map<String, Value>);

impl Args {
    fn get(&self, k: &str) -> Option<&str> {
        self.0.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
    }
    fn or<'a>(&'a self, k: &str, default: &'a str) -> &'a str {
        self.get(k).unwrap_or(default)
    }
}

/// The time pitfall every animation recipe repeats.
const TIMES: &str = "Times are seconds. get_property, render_preview and comp commands use comp time; add_keyframe and set_property with `time` use layer time, which equals comp time only for a layer that starts at 0 and isn't stretched. Otherwise subtract the layer's `start` (from get_comp) from the comp time before keying.";

const CHECK: &str = "Look at the result with render_preview at the important times (comp time) before you finish, and fix what you see; one image is worth many property reads.";

static PROMPTS: &[PromptDef] = &[
    PromptDef {
        name: "title_card",
        title: "Title card",
        description: "Build an animated title card with the given text.",
        args: &[
            Arg { name: "text", description: "The title text.", required: true },
            Arg { name: "style", description: "Look and feel, e.g. `bold, warm, film grain` (default: clean and modern).", required: false },
        ],
        text: |a| {
            format!(
                "Build an animated title card in EffectCraft that reads \"{text}\". Style: {style}.\n\n\
                 1. doc_inspect to see the project. If no composition is active, command_run comp.new {{\"name\":\"Title Card\",\"width\":1920,\"height\":1080,\"frameRate\":30,\"duration\":5}}.\n\
                 2. Background: command_run layer.newSolid {{\"color\":\"#...\"}} (or keep the comp background). Title: command_run layer.newText {{\"text\":..., \"size\":..., \"fill\":...}}; it returns {{\"layer\": id}}. Pick a fitting font with list_fonts and apply it with command_run layer.setText {{\"layer\", \"font\", \"style\"}}.\n\
                 3. Animate it in over about 0.8 s: add_keyframe on transform/opacity and transform/scale or transform/position with `keys` and interpolation easyEase. Add effects that suit the style with add_effect (e.g. Glow, Drop Shadow, Gaussian Blur) and key their values with add_keyframe on the returned `effects/#n/...` paths.\n\
                 4. {TIMES}\n\
                 5. {CHECK} Check legibility, centring and the title-safe area at 0 s, mid-animation and the settled frame.\n\
                 6. Report the layer ids you created. Use undo or history to back out mistakes; save_project {{\"path\"}} only if asked.",
                text = a.or("text", ""),
                style = a.or("style", "clean and modern, centred white text on a dark background"),
            )
        },
    },
    PromptDef {
        name: "lower_third",
        title: "Lower third",
        description: "Build an animated lower third (name and title) that slides in and out.",
        args: &[
            Arg { name: "name", description: "The person's name (first line).", required: true },
            Arg { name: "title", description: "Their title or role (second line).", required: true },
        ],
        text: |a| {
            format!(
                "Build an animated lower third in EffectCraft for \"{name}\" — \"{title}\".\n\n\
                 1. doc_inspect; use the active composition (create one with command_run comp.new if there is none) and note its size, frame rate and duration with get_comp.\n\
                 2. A bar: command_run layer.newShape (or layer.newSolid sized to the bar), placed in the lower-left third inside the title-safe area. Two text layers with command_run layer.newText: the name (larger, bold) and the title (smaller), left-aligned over the bar.\n\
                 3. Parent the texts to the bar (command_run layer.setParent; see describe_command) or animate them together: slide in from the left with add_keyframe on transform/position (interpolation easyEase) over about 0.5 s, hold, then slide out before the end of the comp. Stagger the title a few frames after the name.\n\
                 4. {TIMES}\n\
                 5. {CHECK} Check the slide-in, the held frame and the slide-out.\n\
                 6. Report the layer ids and key times.",
                name = a.or("name", ""),
                title = a.or("title", ""),
            )
        },
    },
    PromptDef {
        name: "port_ae_script",
        title: "Run an After Effects script",
        description: "Port an After Effects ExtendScript to EffectCraft and run it with run_script.",
        args: &[Arg { name: "script", description: "The ExtendScript (.jsx) source.", required: true }],
        text: |a| {
            format!(
                "Run this After Effects script in EffectCraft, porting it where needed.\n\n\
                 run_script {{\"code\"}} runs JavaScript with an After Effects-style object model: app.project, items.addComp, comp.layers.addSolid/addText/addShape/addNull, layer.property(\"ADBE Transform Group\").property(\"ADBE Position\").setValueAtTime(t, v) (comp time, as in After Effects), Effects.addProperty(match name), expressions, markers and renderQueue. It returns {{ok, result, output, error: {{message, line, column}}}}. app.run(id, params) calls any engine command (see command_list / describe_command).\n\n\
                 1. Read the script. Note what can't work headless: file access outside the project folder, `$.evalFile` of files you don't have, and dialogs. ScriptUI windows it opens stay open; drive them with script_ui (list, get, click, set, close).\n\
                 2. Wrap the edits in app.beginUndoGroup(\"...\") / app.endUndoGroup() so one undo removes them.\n\
                 3. run_script. On an error, read error.line / error.column, undo any partial edits, fix the code and run it again.\n\
                 4. Verify with doc_inspect and get_comp, then render_preview the comps it built. {CHECK}\n\n\
                 Script:\n```javascript\n{script}\n```",
                script = a.or("script", ""),
            )
        },
    },
    PromptDef {
        name: "check_against_reference",
        title: "Check against a reference",
        description: "Render a frame and compare it with a reference image, then fix the differences.",
        args: &[
            Arg { name: "reference_path", description: "Path of the reference image (PNG/JPEG), e.g. a frame exported from After Effects.", required: true },
            Arg { name: "comp", description: "Composition id or name (default: the active comp).", required: false },
            Arg { name: "time", description: "Comp time in seconds (default: the current time).", required: false },
        ],
        text: |a| {
            let comp = a.get("comp").map_or_else(|| "the active composition".to_string(), |c| format!("composition {c}"));
            let time = a.get("time").map_or_else(|| "the current time".to_string(), |t| format!("{t} s (comp time)"));
            format!(
                "Compare EffectCraft's render of {comp} at {time} with the reference image {reference}, and make them match.\n\n\
                 1. doc_inspect and get_comp to find the composition, its size and frame rate.\n\
                 2. render_compare {{\"a\": {{\"path\": \"{reference}\"}}, \"b\": {{\"comp\", \"time\"}}, \"min_psnr\": 35}} renders the frame now and compares it with the reference: max/mean difference, PSNR, SSIM, changed pixels, `pass`, and a diff heat map image (red -> yellow -> white by size of difference) showing where they differ. Use render_preview {{\"comp\", \"time\"}} to look at the frame itself.\n\
                 3. To measure a fix, keep the current frame with render_snapshot {{\"name\": \"before\"}} and compare later frames with it as well as with the reference.\n\
                 4. Classify each difference: position or scale, colour or levels, blur or glow, text, missing layer, or timing. Render ±1 frame before blaming a property: a time offset is the usual cause. {TIMES}\n\
                 5. Fix with set_property, add_keyframe or add_effect (get_layer {{\"layer\", \"flat\": true}} lists the property paths), re-render and compare again until it matches.\n\
                 6. Report what differed, what you changed and the final comparison.",
                reference = a.or("reference_path", ""),
            )
        },
    },
    PromptDef {
        name: "animate_layer",
        title: "Animate a layer",
        description: "Animate one layer as described, with keyframes or expressions.",
        args: &[
            Arg { name: "layer", description: "Layer id, `#n` (1-based index from the top) or name.", required: true },
            Arg { name: "motion", description: "The motion, e.g. `bounce in from the top over 1 s, then wiggle gently`.", required: true },
        ],
        text: |a| {
            format!(
                "Animate layer {layer} in the active composition: {motion}.\n\n\
                 1. get_comp: find the layer (its id, `start`, in/out points and stretch) and the comp's frame rate and duration.\n\
                 2. get_layer {{\"layer\", \"flat\": true}} lists every property `path` (transform/position, transform/scale, transform/rotation, transform/opacity, effects/#n/...) and its current value.\n\
                 3. Key it with add_keyframe {{\"layer\", \"path\", \"keys\": [{{\"time\", \"value\"}}, ...], \"interpolation\"}} (easyEase for natural motion, hold for cuts). Use set_property {{\"expression\"}} for procedural motion (wiggle(freq, amp), loopOut(), time*n).\n\
                 4. {TIMES}\n\
                 5. {CHECK} Render the start, the middle and the end of the motion.\n\
                 6. Summarise the keys and expressions you set.",
                layer = a.or("layer", ""),
                motion = a.or("motion", ""),
            )
        },
    },
];

/// `prompts/list`; `titles` for clients that know `title` (2025-06-18+).
pub fn list(titles: bool) -> Value {
    let prompts: Vec<Value> = PROMPTS
        .iter()
        .map(|p| {
            let args: Vec<Value> = p.args.iter().map(|a| json!({"name": a.name, "description": a.description, "required": a.required})).collect();
            let mut v = json!({"name": p.name, "description": p.description, "arguments": args});
            if titles && let Some(o) = v.as_object_mut() {
                o.insert("title".into(), json!(p.title));
            }
            v
        })
        .collect();
    json!({"prompts": prompts})
}

/// `prompts/get`: the prompt's messages, or an invalid-params message.
pub fn get(name: &str, args: Option<&Value>) -> Result<Value, String> {
    let p = PROMPTS.iter().find(|p| p.name == name).ok_or_else(|| {
        let names: Vec<&str> = PROMPTS.iter().map(|p| p.name).collect();
        format!("unknown prompt `{name}`; available: {}", names.join(", "))
    })?;
    let given = match args {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(m)) => m.clone(),
        Some(_) => return Err("`arguments` must be an object of strings".into()),
    };
    if let Some(k) = given.keys().find(|k| p.args.iter().all(|a| a.name != k.as_str())) {
        let accepted: Vec<&str> = p.args.iter().map(|a| a.name).collect();
        return Err(format!("unknown argument `{k}` for prompt {name}; expected: {}", accepted.join(", ")));
    }
    if let Some((k, _)) = given.iter().find(|(_, v)| !v.is_string()) {
        return Err(format!("argument `{k}` must be a string"));
    }
    let given = Args(given);
    if let Some(a) = p.args.iter().find(|a| a.required && given.get(a.name).is_none()) {
        return Err(format!("missing required argument `{}` for prompt {name}", a.name));
    }
    Ok(json!({
        "description": p.description,
        "messages": [{"role": "user", "content": {"type": "text", "text": (p.text)(&given)}}],
    }))
}

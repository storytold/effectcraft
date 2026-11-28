//! The plug-in host in a real EffectCraft `Session`, wired the way the app wires it
//! (`PluginHost::start`: host commands, hooks, surfaces), with real guests written in WAT: a command
//! guest (craft-abi 1), the `pixels` effect guest, and the EffectCraft v1 `gain` module (`ec_`,
//! through the shim).
//!
//! The link step matters most. The plug-in host lends the session to a guest call only
//! (`Services::lend_session`). `craft.link`'s `link.back` is a dynamic command that reads the session
//! it is given, so if a dynamic body ever got an idle spare it would find no bound comp and answer
//! with an error. Nothing but a wired session shows that.

mod common;

use std::sync::Arc;

use common::gain::gain_module;
use common::guest::{MADE, command_package};
use common::{EDIT, FakeFilmCraft, NoWindow, PIXELS, TempRuntime, bind, install, pixels_package, rig};
use craft_plugin_host::lifecycle::PluginState;
use effectcraft_effects::plugin::{PluginFrame, PluginParams, plugin, plugins};
use effectcraft_link::{BACK_COMMAND, install_back_command, sync_back_enabled};
use serde_json::json;

fn command() -> String {
    format!("plugin.{EDIT}.make")
}

/// The id the host gives the `quad` contribution in the effect registry.
fn quad_id() -> String {
    format!("{PIXELS}.quad")
}

fn listed(id: &str) -> bool {
    plugins().iter().any(|s| s.id == id)
}

/// One pixel of straight colour (0.1, 0.2, 0.05) at alpha 0.25, premultiplied as the registry keeps it,
/// run through the `quad` effect. The guest multiplies every straight sample by 4.
fn quad_pixel() -> [f32; 4] {
    let p = plugin(&quad_id()).expect("the effect is registered");
    let mut px = vec![[0.025, 0.05, 0.0125, 0.25]];
    let mut frame = PluginFrame { width: 1, height: 1, pixels: &mut px, scale: 1.0, origin: [0.0; 2], layer_size: [1.0; 2] };
    p.render(&mut frame, &PluginParams { manifest: p.manifest(), flat: &[], texts: &[] }, 0.0).unwrap();
    px[0]
}

#[test]
fn plugins_install_run_unload_and_reload_and_link_back_still_answers() {
    let (mut session, host, comp) = rig();

    // Install: the prompt comes first, then the plug-in is active.
    let out = host.install_bytes(&command_package(), craft_plugin_host::store::Source::File).unwrap();
    assert!(matches!(out, craft_plugin_host::lifecycle::InstallOutcome::NeedsGrant(_)), "{out:?}");
    assert!(session.execute(&command(), json!({})).is_err(), "nothing runs before the user answers");
    install(&host, &command_package());
    assert!(matches!(host.manager.state(EDIT), Some(PluginState::Active { .. })), "{:?}", host.manager.state(EDIT));

    // The guest calls the host from inside the command, and the host reaches the session.
    session.execute(&command(), json!({})).unwrap();
    assert!(session.project.find_by_name(MADE).is_some(), "the guest's cmd.execute added a comp");

    // The effect is in the registry and renders. It needs no permission.
    assert!(!listed(&quad_id()));
    install(&host, &pixels_package());
    let spec = plugins().into_iter().find(|s| s.id == quad_id()).expect("the effect is listed");
    assert_eq!((spec.name, spec.category), ("Quad", "Test Plug-ins"));
    assert!(spec.params.is_empty());
    let out = quad_pixel();
    for (got, want) in out.iter().zip([0.4, 0.8, 0.2, 1.0]) {
        assert!((got - want).abs() < 1e-5, "straight x4 then premultiplied again: {out:?}");
    }

    // link.back answers while the plug-ins are loaded and the session is wired.
    let runtime = TempRuntime::new("life");
    let peer = FakeFilmCraft::start(&runtime.dir);
    let result = std::env::temp_dir().join(format!("ecp-life-{}", std::process::id())).join(format!("{}.mov", common::LINK));
    // The guest's command added a comp and made it the active one. link.back needs the bound comp.
    session.open_comp(comp);
    bind(&mut session, comp, &result.to_string_lossy());
    install_back_command(&mut session, Some(runtime.dir.clone()), Arc::new(NoWindow)).unwrap();
    sync_back_enabled(&session);
    let back = session.execute(BACK_COMMAND, json!({})).unwrap_or_else(|e| panic!("link.back failed in the wired session: {e}"));
    assert_eq!(back["activated"], "requested", "{back}");
    assert!(result.exists(), "link.back renders the result");
    assert_eq!(peer.methods(), ["link.focus", "app.activate"]);
    let _ = std::fs::remove_dir_all(result.parent().unwrap());

    // Disable unloads: the effect leaves the registry and the command leaves the table.
    host.manager.disable(PIXELS).unwrap();
    assert!(plugin(&quad_id()).is_none() && !listed(&quad_id()));
    host.manager.disable(EDIT).unwrap();
    assert!(session.execute(&command(), json!({})).is_err());

    // Enable loads it again into the same slot with the same spec: no leak per reload.
    host.manager.enable(PIXELS).unwrap();
    let again = plugins().into_iter().find(|s| s.id == quad_id()).expect("listed again");
    assert!(std::ptr::eq(spec, again), "the slot and its spec are reused by plug-in id");
    assert!((quad_pixel()[0] - 0.4).abs() < 1e-5);
    host.manager.enable(EDIT).unwrap();
    session.execute(&command(), json!({})).unwrap();

    // Remove drops everything.
    host.manager.remove(PIXELS).unwrap();
    assert!(plugin(&quad_id()).is_none() && !listed(&quad_id()));
    host.manager.remove(EDIT).unwrap();
    assert!(host.surfaces.is_empty());
}

#[test]
fn a_v1_ec_plugin_loads_through_the_shim_and_renders_in_the_effect_registry() {
    let (_session, host, _comp) = rig();
    let id = "org.test.gain.lifecycle";
    let wasm = wat::parse_str(gain_module(id, "")).unwrap();
    let out = install(&host, &wasm);
    assert!(matches!(out, craft_plugin_host::lifecycle::InstallOutcome::Loaded { .. }), "{out:?}");
    let info = host.manager.info(id).unwrap();
    assert!(info.legacy.is_some(), "the shim describes it as a v1 plug-in");

    let p = plugin(id).expect("the v1 effect is in the effect registry under its own id");
    let spec = plugins().into_iter().find(|s| s.id == id).unwrap();
    assert_eq!((spec.category, spec.params.len()), ("Test Plug-ins", 3));
    // Parameters are in id order: gain, on, tint.
    let mut px = vec![[0.8, 0.6, 0.2, 1.0]; 8];
    let mut frame = PluginFrame { width: 4, height: 2, pixels: &mut px, scale: 1.0, origin: [0.0; 2], layer_size: [4.0, 2.0] };
    p.render(&mut frame, &PluginParams { manifest: p.manifest(), flat: &[0.5, 0.0, 0.0, 0.0, 1.0, 1.0], texts: &[] }, 0.0).unwrap();
    assert!(px.iter().all(|q| (q[0] - 0.4).abs() < 1e-6 && (q[1] - 0.3).abs() < 1e-6 && (q[2] - 0.1).abs() < 1e-6 && q[3] == 1.0), "{:?}", px[0]);

    host.manager.remove(id).unwrap();
    assert!(plugin(id).is_none());
}

#[test]
fn the_grant_command_refuses_params_of_the_wrong_type() {
    let (mut session, host, _comp) = rig();
    let out = host.install_bytes(&command_package(), craft_plugin_host::store::Source::File).unwrap();
    assert!(matches!(out, craft_plugin_host::lifecycle::InstallOutcome::NeedsGrant(_)), "{out:?}");
    let grant = "plugin.app.effectcraft.grant";

    // A string is not a boolean. The old code read it as "accept" and granted the plug-in.
    let err = session.execute(grant, json!({"id": EDIT, "accept": "false"})).expect_err("a string `accept` must fail");
    assert!(err.to_string().contains("boolean"), "{err}");
    let err = session.execute(grant, json!({"id": EDIT, "optional": "clock"})).expect_err("a string `optional` must fail");
    assert!(err.to_string().contains("array"), "{err}");
    assert!(!matches!(host.manager.state(EDIT), Some(PluginState::Active { .. })), "a malformed answer must not grant");

    // The prompt is still open: a well-formed answer works.
    session.execute(grant, json!({"id": EDIT, "accept": true})).expect("a boolean answers the prompt");
    assert!(matches!(host.manager.state(EDIT), Some(PluginState::Active { .. })), "{:?}", host.manager.state(EDIT));
}

//! CollabCraft transport for the EffectCraft browser build.
//!
//! Same jobs and events as the desktop worker (`apps/effectcraft/src/cloud`),
//! but the client is async and the session rides the cookie the CollabCraft
//! service set when the user signed in on the page. There is no device flow
//! and no token file; cookies are managed by the service.

use craft_cloud_client::{Client, Error};
use effectcraft_ui_egui::cloud::{Account, CloudBackend, Event, Fault, Job, Post};
use wasm_bindgen_futures::spawn_local;

/// Hands cloud jobs to async tasks on the same origin. Cheap to clone.
#[derive(Clone)]
pub struct Service {
    client: Client,
    ctx: egui::Context,
}

impl Service {
    /// Always available when the app is served by CollabCraft on the same origin.
    pub fn new(ctx: egui::Context) -> Service {
        log::info!("cloud storage through {}", Client::from_page().base());
        Service { client: Client::from_page(), ctx }
    }
}

impl CloudBackend for Service {
    fn submit(&self, job: Job, post: Post) {
        let worker = self.clone();
        spawn_local(async move {
            worker.run(job, post).await;
        });
    }
}

impl Service {
    async fn run(self, job: Job, post: Post) {
        let ctx = self.ctx.clone();
        let send = move |event: Event| {
            post.send(event);
            ctx.request_repaint();
        };
        match job {
            Job::Account => {
                let answer = self.client.me().await.map_err(to_fault).map(|me| Account {
                    display: me.display,
                    capabilities: me.capabilities,
                    provider_linked: me.provider_linked,
                });
                send(Event::Account(answer));
            }
            Job::LinkStart => match self.client.link_start().await.map_err(to_fault) {
                Ok(start) => send(Event::Browser { url: start.login_url, handle: Some(start.handle) }),
                Err(fault) => send(Event::Linked(Err(fault))),
            },
            Job::LinkPoll(handle) => {
                let answer = self.client.link_poll(&handle).await.map_err(to_fault);
                send(Event::Linked(answer));
            }
            Job::SignOut => match fetch_logout().await {
                Ok(()) => {
                    let _ = web_sys::window().and_then(|w| w.location().reload().ok());
                }
                Err(e) => send(Event::Account(Err(Fault::Refused(format!("Sign out failed: {e}"))))),
            },
            Job::Conversation(id) => {
                let answer = self.client.conversation(&id).await.map_err(to_fault);
                send(Event::Conversation(answer));
            }
            Job::List(path) => {
                let entries = self.client.list(&path).await.map_err(to_fault);
                send(Event::Listed { path, entries });
            }
            Job::SharedWithMe => {
                let answer = self.client.shared_with_me().await.map_err(to_fault);
                send(Event::Shared(answer));
            }
            Job::Read { entry, into } => {
                let id = entry.id.clone();
                let bytes = self.client.read(&id).await.map_err(to_fault);
                send(Event::Read { entry, into, bytes });
            }
            Job::Write { uid, entry, bytes } => {
                let id = entry.id.clone();
                let etag = entry.etag.clone();
                let answer = self.client.write(&id, &bytes, etag.as_deref()).await.map_err(to_fault);
                send(Event::Saved { uid, entry: answer });
            }
            Job::Create { uid, folder, name, bytes } => {
                let answer = self.client.create(&folder, &name, &bytes).await.map_err(to_fault);
                send(Event::Saved { uid, entry: answer });
            }
        }
    }
}

fn to_fault(e: Error) -> Fault {
    match e {
        Error::Unauthenticated => Fault::Unauthenticated,
        Error::NotLinked => Fault::NotLinked,
        Error::Unreachable(detail) => Fault::Unreachable(detail),
        Error::Refused { code, .. } if code == "conflict" || code == "409" || code == "412" => Fault::Conflict,
        Error::Refused { code, detail } => Fault::Refused(if detail.trim().is_empty() { code } else { detail }),
        Error::Malformed(detail) => Fault::Refused(format!("the cloud service gave an answer EffectCraft could not read: {detail}")),
        Error::Unsendable(detail) => {
            log::error!("cloud request could not be encoded: {detail}");
            Fault::Refused("EffectCraft could not build that request".to_owned())
        }
    }
}

/// The CollabCraft path for ending a session. The web app POSTs here on sign-out.
///
/// Note: the old `/logout` root is not a CollabCraft route; the correct path is
/// `/auth/logout`.
const LOGOUT_PATH: &str = "/auth/logout";

async fn fetch_logout() -> Result<(), String> {
    use wasm_bindgen::JsCast as _;
    let global = js_sys::global();
    let fetch = js_sys::Reflect::get(&global, &wasm_bindgen::JsValue::from_str("fetch"))
        .ok()
        .and_then(|v| v.dyn_into::<js_sys::Function>().ok())
        .ok_or_else(|| "no fetch".to_owned())?;
    let init = web_sys::RequestInit::new();
    init.set_method("POST");
    init.set_credentials(web_sys::RequestCredentials::Include);
    let req = web_sys::Request::new_with_str_and_init(LOGOUT_PATH, &init).map_err(|e| format!("{e:?}"))?;
    let promised = fetch.call1(global.as_ref(), req.as_ref()).map_err(|e| format!("{e:?}"))?;
    let promised: js_sys::Promise = promised.dyn_into().map_err(|_| "not a promise".to_owned())?;
    let resp_val = wasm_bindgen_futures::JsFuture::from(promised).await.map_err(|e| format!("{e:?}"))?;
    let resp: web_sys::Response = resp_val.dyn_into().map_err(|_| "logout response was not an HTTP response".to_owned())?;
    if !resp.ok() {
        return Err(format!("logout failed (HTTP {})", resp.status()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::LOGOUT_PATH;

    /// Sign-out must POST to the CollabCraft path `/auth/logout`, not the bare
    /// `/logout` that is not a CollabCraft route.
    #[test]
    fn logout_path_is_collabcraft_auth_route() {
        assert_eq!(LOGOUT_PATH, "/auth/logout");
    }
}
